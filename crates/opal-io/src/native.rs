//! The native `.opal` format, version 0.
//!
//! A zip archive holding:
//!
//! - `mimetype`, stored first and uncompressed: `application/x-opal-studio`.
//! - `document.json`: the canvas, the layer tree, and the next free identifier.
//! - `tiles/<n>.bin`: one tile each, numbered from 0. A tile shared by several layers or
//!   objects (after duplicating a layer, say) is stored once and referenced from each.
//!
//! A tile file is the tile's 256×256 pixels × 4 channels of little-endian half floats,
//! **byte-shuffled**: all the low bytes first, then all the high bytes. The high bytes
//! (sign, exponent, top of the mantissa) of neighboring values are similar, so grouping them
//! lets deflate compress photos far better.

use crate::{IoError, write_all, write_atomically};
use opal_core::document::{Canvas, Document, IdGen, Layer, LayerContent, LayerId, Object, ObjectId, ObjectKind};
use opal_core::raster::RasterImage;
use opal_core::tile::{Pixel, TILE_PIXELS, Tile, TileCoord};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::Arc;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const MIME_TYPE: &str = "application/x-opal-studio";
pub const FORMAT_VERSION: u32 = 0;
const TILE_BYTES: usize = TILE_PIXELS * 4 * 2;

#[derive(Serialize, Deserialize)]
struct FileDoc {
    format: String,
    version: u32,
    canvas: Canvas,
    next_id: u64,
    layers: Vec<FileLayer>,
}

#[derive(Serialize, Deserialize)]
struct FileLayer {
    id: LayerId,
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    #[serde(flatten)]
    content: FileContent,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum FileContent {
    Objects { objects: Vec<FileObject> },
    Group { children: Vec<FileLayer> },
}

#[derive(Serialize, Deserialize)]
struct FileObject {
    id: ObjectId,
    #[serde(flatten)]
    kind: FileObjectKind,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum FileObjectKind {
    /// Each entry is `[tile x, tile y, tile file number]`.
    Raster { tiles: Vec<[i64; 3]> },
}

/// Saves `doc` with the identifier generator's state to `path` (atomically).
pub fn save(doc: &Document, ids: &IdGen, path: &Path) -> Result<(), IoError> {
    // Number the distinct tiles, keeping their order of first appearance.
    let mut numbers: HashMap<*const Tile, usize> = HashMap::new();
    let mut tiles: Vec<Arc<Tile>> = Vec::new();
    let layers = doc.layers.iter().map(|l| file_layer(l, &mut numbers, &mut tiles)).collect();
    let file = FileDoc {
        format: "opal-studio".into(),
        version: FORMAT_VERSION,
        canvas: doc.canvas,
        next_id: ids.peek(),
        layers,
    };
    let json = serde_json::to_vec_pretty(&file).expect("the document serializes");
    let shuffled: Vec<Vec<u8>> = tiles.par_iter().map(|t| shuffle(t.pixels())).collect();

    write_atomically(path, |out| {
        let zip_err = |e: zip::result::ZipError| IoError::io(path, std::io::Error::other(e));
        let mut zip = ZipWriter::new(out);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(3));
        zip.start_file("mimetype", stored).map_err(zip_err)?;
        write_all(&mut zip, MIME_TYPE.as_bytes(), path)?;
        zip.start_file("document.json", deflated).map_err(zip_err)?;
        write_all(&mut zip, &json, path)?;
        for (i, bytes) in shuffled.iter().enumerate() {
            zip.start_file(format!("tiles/{i}.bin"), deflated).map_err(zip_err)?;
            write_all(&mut zip, bytes, path)?;
        }
        zip.finish().map_err(zip_err)?;
        Ok(())
    })
}

fn file_layer(layer: &Layer, numbers: &mut HashMap<*const Tile, usize>, tiles: &mut Vec<Arc<Tile>>) -> FileLayer {
    let content = match &layer.content {
        LayerContent::Group(children) => {
            FileContent::Group { children: children.iter().map(|c| file_layer(c, numbers, tiles)).collect() }
        }
        LayerContent::Objects(objects) => FileContent::Objects {
            objects: objects
                .iter()
                .map(|o| {
                    let ObjectKind::Raster(img) = &o.kind;
                    let mut refs: Vec<[i64; 3]> = img
                        .tiles()
                        .map(|(c, t)| {
                            let n = *numbers.entry(Arc::as_ptr(t)).or_insert_with(|| {
                                tiles.push(t.clone());
                                tiles.len() - 1
                            });
                            [i64::from(c.x), i64::from(c.y), n as i64]
                        })
                        .collect();
                    refs.sort_unstable();
                    FileObject { id: o.id, kind: FileObjectKind::Raster { tiles: refs } }
                })
                .collect(),
        },
    };
    FileLayer {
        id: layer.id,
        name: layer.name.clone(),
        visible: layer.visible,
        locked: layer.locked,
        opacity: layer.opacity,
        content,
    }
}

/// A document loaded from a file, with the identifier generator to keep using with it.
pub struct Loaded {
    pub doc: Document,
    pub ids: IdGen,
}

/// Loads an `.opal` file.
pub fn load(path: &Path) -> Result<Loaded, IoError> {
    let file = std::fs::File::open(path).map_err(|e| IoError::io(path, e))?;
    load_from(std::io::BufReader::new(file), path)
}

/// Loads an `.opal` file from a reader; `path` is used only in messages.
pub fn load_from(reader: impl Read + Seek, path: &Path) -> Result<Loaded, IoError> {
    let bad = |message: String| IoError::Format { path: path.to_owned(), message };
    let mut zip = ZipArchive::new(reader).map_err(|e| bad(format!("not a zip archive ({e})")))?;

    let mime = read_entry(&mut zip, "mimetype", path)?;
    if mime != MIME_TYPE.as_bytes() {
        return Err(bad("wrong mimetype".into()));
    }
    let json = read_entry(&mut zip, "document.json", path)?;
    let file: FileDoc = serde_json::from_slice(&json).map_err(|e| bad(format!("document.json: {e}")))?;
    if file.format != "opal-studio" {
        return Err(bad(format!("unknown format {:?}", file.format)));
    }
    if file.version > FORMAT_VERSION {
        return Err(IoError::Unsupported {
            path: path.to_owned(),
            message: format!(
                "saved by a newer Opal Studio (format version {}; this version reads up to {FORMAT_VERSION})",
                file.version
            ),
        });
    }

    // Read every tile the document references, once each.
    let mut wanted: Vec<usize> = Vec::new();
    collect_tile_numbers(&file.layers, &mut wanted);
    wanted.sort_unstable();
    wanted.dedup();
    let mut raw: Vec<(usize, Vec<u8>)> = Vec::with_capacity(wanted.len());
    for n in wanted {
        let bytes = read_entry(&mut zip, &format!("tiles/{n}.bin"), path)?;
        if bytes.len() != TILE_BYTES {
            return Err(bad(format!("tiles/{n}.bin has {} bytes, not {TILE_BYTES}", bytes.len())));
        }
        raw.push((n, bytes));
    }
    let tiles: HashMap<usize, Arc<Tile>> =
        raw.into_par_iter().map(|(n, bytes)| (n, Arc::new(unshuffle(&bytes)))).collect();

    let mut layers = Vec::with_capacity(file.layers.len());
    for l in file.layers {
        layers.push(Arc::new(build_layer(l, &tiles, path)?));
    }
    let doc = Document { canvas: file.canvas, layers };
    let mut ids = IdGen::starting_at(file.next_id);
    ids.reserve_above(&doc);
    Ok(Loaded { doc, ids })
}

fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str, path: &Path) -> Result<Vec<u8>, IoError> {
    let mut entry = zip
        .by_name(name)
        .map_err(|_| IoError::Format { path: path.to_owned(), message: format!("{name} is missing") })?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).map_err(|e| IoError::io(path, e))?;
    Ok(bytes)
}

fn collect_tile_numbers(layers: &[FileLayer], out: &mut Vec<usize>) {
    for l in layers {
        match &l.content {
            FileContent::Group { children } => collect_tile_numbers(children, out),
            FileContent::Objects { objects } => {
                for o in objects {
                    let FileObjectKind::Raster { tiles } = &o.kind;
                    out.extend(tiles.iter().map(|t| t[2] as usize));
                }
            }
        }
    }
}

fn build_layer(l: FileLayer, tiles: &HashMap<usize, Arc<Tile>>, path: &Path) -> Result<Layer, IoError> {
    let bad = |message: String| IoError::Format { path: path.to_owned(), message };
    let content = match l.content {
        FileContent::Group { children } => {
            let mut c = Vec::with_capacity(children.len());
            for child in children {
                c.push(Arc::new(build_layer(child, tiles, path)?));
            }
            LayerContent::Group(c)
        }
        FileContent::Objects { objects } => {
            let mut out = Vec::with_capacity(objects.len());
            for o in objects {
                let FileObjectKind::Raster { tiles: refs } = o.kind;
                let mut img = RasterImage::new();
                for [x, y, n] in refs {
                    let coord = TileCoord::new(
                        i32::try_from(x).map_err(|_| bad(format!("tile x {x} out of range")))?,
                        i32::try_from(y).map_err(|_| bad(format!("tile y {y} out of range")))?,
                    );
                    let tile = tiles.get(&(n as usize)).ok_or_else(|| bad(format!("tile {n} missing")))?;
                    img.set_tile(coord, Some(tile.clone()));
                }
                out.push(Object { id: o.id, kind: ObjectKind::Raster(Arc::new(img)) });
            }
            LayerContent::Objects(out)
        }
    };
    if !(0.0..=1.0).contains(&l.opacity) {
        return Err(bad(format!("layer {:?} has opacity {}", l.name, l.opacity)));
    }
    Ok(Layer { id: l.id, name: l.name, visible: l.visible, locked: l.locked, opacity: l.opacity, content })
}

/// The tile's half floats as bytes, low bytes first, then high bytes.
fn shuffle(px: &[Pixel]) -> Vec<u8> {
    let values: &[u16] = bytemuck::cast_slice(px);
    let mut out = vec![0u8; values.len() * 2];
    let (lo, hi) = out.split_at_mut(values.len());
    for (i, v) in values.iter().enumerate() {
        let [l, h] = v.to_le_bytes();
        lo[i] = l;
        hi[i] = h;
    }
    out
}

fn unshuffle(bytes: &[u8]) -> Tile {
    let n = bytes.len() / 2;
    let (lo, hi) = bytes.split_at(n);
    let mut t = Tile::transparent();
    let values: &mut [u16] = bytemuck::cast_slice_mut(t.pixels_mut());
    for i in 0..n {
        values[i] = u16::from_le_bytes([lo[i], hi[i]]);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::document::Position;
    use opal_core::tile::pixel_from_f32;

    fn sample() -> (Document, IdGen) {
        let mut ids = IdGen::default();
        let mut d = Document::new(300, 200);
        let mut img = RasterImage::new();
        img.set_pixel(5, 5, pixel_from_f32([0.25, 0.5, 0.75, 1.0]));
        img.set_pixel(-700, 40, pixel_from_f32([-0.125, 1.5, 0.0, 0.5]));
        let a = ids.layer();
        let mut base = Layer::new_raster(a, ids.object(), "Base", img.clone());
        base.opacity = 0.75;
        d.insert(Position { parent: None, index: 0 }, base).unwrap();
        let g = ids.layer();
        let mut group = Layer::new_group(g, "Group");
        group.visible = false;
        d.insert(Position { parent: None, index: 1 }, group).unwrap();
        // A copy sharing every tile with the base layer.
        let mut copy = Layer::new_raster(ids.layer(), ids.object(), "Copy", img);
        copy.locked = true;
        d.insert(Position { parent: Some(g), index: 0 }, copy).unwrap();
        (d, ids)
    }

    #[test]
    fn save_and_load_round_trip() {
        let (doc, ids) = sample();
        let path = std::env::temp_dir().join(format!("opal-native-{}.opal", std::process::id()));
        save(&doc, &ids, &path).unwrap();
        let loaded = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let d = loaded.doc;
        assert_eq!(d.canvas, doc.canvas);
        assert_eq!(d.layer_ids(), doc.layer_ids());
        let base = d.layers[0].clone();
        assert_eq!((base.name.as_str(), base.opacity), ("Base", 0.75));
        let group = &d.layers[1];
        assert!(!group.visible);
        let copy = &group.children().unwrap()[0];
        assert!(copy.locked);
        let img = base.rasters().next().unwrap();
        assert_eq!(img.pixel(5, 5), pixel_from_f32([0.25, 0.5, 0.75, 1.0]));
        assert_eq!(img.pixel(-700, 40), pixel_from_f32([-0.125, 1.5, 0.0, 0.5]));
        // Shared tiles stay shared after loading.
        let copy_img = copy.rasters().next().unwrap();
        let c = TileCoord::containing(5, 5);
        assert!(Arc::ptr_eq(img.tile(c).unwrap(), copy_img.tile(c).unwrap()));
        // New identifiers do not collide with loaded ones.
        let mut ids2 = loaded.ids;
        assert!(!d.layer_ids().contains(&ids2.layer()));
    }

    #[test]
    fn shuffling_round_trips() {
        let mut t = Tile::transparent();
        t.set(1, 2, pixel_from_f32([1.0, -2.0, 0.001, 0.5]));
        assert_eq!(unshuffle(&shuffle(t.pixels())), t);
    }

    #[test]
    fn damaged_files_are_refused_with_a_reason() {
        let path = Path::new("broken.opal");
        let err = load_from(std::io::Cursor::new(b"not a zip".to_vec()), path).err().unwrap();
        assert!(err.to_string().contains("not a zip"), "{err}");

        // A newer format version is refused rather than misread.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let o = SimpleFileOptions::default();
            zip.start_file("mimetype", o).unwrap();
            std::io::Write::write_all(&mut zip, MIME_TYPE.as_bytes()).unwrap();
            zip.start_file("document.json", o).unwrap();
            let json = r#"{"format":"opal-studio","version":99,"canvas":{"width":1,"height":1},"next_id":1,"layers":[]}"#;
            std::io::Write::write_all(&mut zip, json.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buf.set_position(0);
        let err = load_from(buf, path).err().unwrap();
        assert!(err.to_string().contains("newer Opal Studio"), "{err}");
    }
}
