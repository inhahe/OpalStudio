//! The native `.opal` format, version 1.
//!
//! A zip archive holding:
//!
//! - `mimetype`, stored first and uncompressed: `application/x-opal-studio`.
//! - `document.json`: the canvas (a rectangle), the layer tree, and the next free
//!   identifier. Objects are `pixels` (tiles and a whole-pixel offset) or `image` (tiles,
//!   the frame, the transform as `[a, b, c, d, e, f]`, and the sampling).
//! - `tiles/<n>.bin`: one tile each, numbered from 0. A tile shared by several layers or
//!   objects (after duplicating a layer, say) is stored once and referenced from each.
//!
//! Version 0 had only pixel objects, called `raster`, without offsets, and a canvas
//! without a position; it is still read.
//!
//! A tile file is the tile's 256×256 pixels × 4 channels of little-endian half floats,
//! **byte-shuffled**: all the low bytes first, then all the high bytes. The high bytes
//! (sign, exponent, top of the mantissa) of neighboring values are similar, so grouping them
//! lets deflate compress photos far better.

use crate::{IoError, Progress, write_all, write_atomically};
use opal_core::document::{
    Canvas, Document, IdGen, ImageObject, Layer, LayerContent, LayerId, Object, ObjectId, ObjectKind, PixelObject,
};
use opal_core::geom::{Affine, IRect};
use opal_core::raster::RasterImage;
use opal_core::resample::Sampling;
use opal_core::tile::{Pixel, TILE_PIXELS, Tile, TileCoord};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;
use zip::result::ZipResult;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const MIME_TYPE: &str = "application/x-opal-studio";
pub const FORMAT_VERSION: u32 = 1;
const TILE_BYTES: usize = TILE_PIXELS * 4 * 2;
/// How many tiles saving compresses (in parallel) before writing them: what it holds at
/// once beyond the document.
const TILES_AT_ONCE: usize = 256;
/// How many tiles one worker compresses into an archive of its own.
const TILES_PER_PART: usize = 8;
/// The largest `document.json` accepted.
const MAX_JSON: usize = 256 << 20;

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

/// In both kinds, each entry of `tiles` is `[tile x, tile y, tile file number]`.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum FileObjectKind {
    /// Version 0 called these `raster`, and had no offset.
    #[serde(alias = "raster")]
    Pixels {
        tiles: Vec<[i64; 3]>,
        #[serde(default)]
        offset: [i64; 2],
    },
    Image {
        tiles: Vec<[i64; 3]>,
        frame: IRect,
        transform: Affine,
        sampling: Sampling,
    },
}

impl FileObjectKind {
    fn tiles(&self) -> &[[i64; 3]] {
        match self {
            FileObjectKind::Pixels { tiles, .. } | FileObjectKind::Image { tiles, .. } => tiles,
        }
    }
}

use opal_core::document::MAX_COORD;

/// Saves `doc` with the identifier generator's state to `path` (atomically), telling
/// `progress` how far it has got in tiles written, and stopping if it is cancelled.
pub fn save(doc: &Document, ids: &IdGen, path: &Path, progress: &Progress) -> Result<(), IoError> {
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
    progress.begin(tiles.len() as u64);

    write_atomically(path, Some(progress), |out| {
        let zip_err = |e: zip::result::ZipError| IoError::io(path, std::io::Error::other(e));
        let mut zip = ZipWriter::new(out);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).compression_level(Some(3));
        zip.start_file("mimetype", stored).map_err(zip_err)?;
        write_all(&mut zip, MIME_TYPE.as_bytes(), path)?;
        zip.start_file("document.json", deflated).map_err(zip_err)?;
        write_all(&mut zip, &json, path)?;
        write_tiles(&mut zip, &tiles, deflated, TILES_AT_ONCE, progress).map_err(zip_err)?;
        zip.finish().map_err(zip_err)?;
        Ok(())
    })
}

/// Writes `tiles` to `zip` as `tiles/<n>.bin`, numbered and stored in order, compressing
/// `at_once` of them at a time in parallel: each worker compresses a few into an archive of
/// its own, which is then copied in whole, without compressing again. Each batch written
/// advances `progress` by its tiles.
fn write_tiles<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    tiles: &[Arc<Tile>],
    options: SimpleFileOptions,
    at_once: usize,
    progress: &Progress,
) -> ZipResult<()> {
    for (b, batch) in tiles.chunks(at_once).enumerate() {
        let parts: Vec<Vec<u8>> = batch
            .par_chunks(TILES_PER_PART)
            .enumerate()
            .map(|(k, part)| {
                let mut w = ZipWriter::new(Cursor::new(Vec::new()));
                for (n, t) in part.iter().enumerate() {
                    w.start_file(format!("tiles/{}.bin", b * at_once + k * TILES_PER_PART + n), options)?;
                    w.write_all(&shuffle(t.pixels()))?;
                }
                Ok(w.finish()?.into_inner())
            })
            .collect::<ZipResult<_>>()?;
        for part in parts {
            zip.merge_archive(ZipArchive::new(Cursor::new(part))?)?;
        }
        progress.advance(batch.len() as u64);
    }
    Ok(())
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
                    let img = o.image();
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
                    let kind = match &o.kind {
                        ObjectKind::Pixels(p) => FileObjectKind::Pixels { tiles: refs, offset: p.offset },
                        ObjectKind::Image(i) => FileObjectKind::Image {
                            tiles: refs,
                            frame: i.frame,
                            transform: i.transform,
                            sampling: i.sampling,
                        },
                    };
                    FileObject { id: o.id, kind }
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
pub fn load(path: &Path, free: Option<u64>) -> Result<Loaded, IoError> {
    let file = std::fs::File::open(path).map_err(|e| IoError::io(path, e))?;
    load_from(std::io::BufReader::new(file), path, free)
}

/// Loads an `.opal` file from a reader; `path` is used only in messages.
pub fn load_from(reader: impl Read + Seek, path: &Path, free: Option<u64>) -> Result<Loaded, IoError> {
    let bad = |message: String| IoError::Format { path: path.to_owned(), message };
    let mut zip = ZipArchive::new(reader).map_err(|e| bad(format!("not a zip archive ({e})")))?;

    let mime = read_entry(&mut zip, "mimetype", 256, path)?;
    if mime != MIME_TYPE.as_bytes() {
        return Err(bad("wrong mimetype".into()));
    }
    let json = read_entry(&mut zip, "document.json", MAX_JSON, path)?;
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

    check_ids(&file.layers, &mut HashSet::new(), path)?;
    let c = &file.canvas;
    if c.x.abs() > MAX_COORD || c.y.abs() > MAX_COORD || c.width == 0 || c.height == 0 {
        return Err(bad(format!("the canvas is {}×{} at ({}, {}), out of range", c.width, c.height, c.x, c.y)));
    }
    if file.next_id >= MAX_ID {
        return Err(bad(format!("next identifier {} is out of range", file.next_id)));
    }

    // Read every tile the document references, once each.
    let mut wanted: Vec<usize> = Vec::new();
    collect_tile_numbers(&file.layers, &mut wanted);
    let places = wanted.len() as u64;
    wanted.sort_unstable();
    wanted.dedup();
    // Refused before any is read if they would need more than is free: each tile is held
    // twice for a moment (as read, and as pixels), and each place one goes is an entry in
    // its image.
    let need = (wanted.len() as u64).saturating_mul(2 * TILE_BYTES as u64).saturating_add(places.saturating_mul(64));
    if let Some(free) = free.filter(|&f| need > f) {
        return Err(IoError::TooLarge { path: path.to_owned(), need, free });
    }
    let mut raw: Vec<(usize, Vec<u8>)> = Vec::with_capacity(wanted.len());
    for n in wanted {
        let bytes = read_entry(&mut zip, &format!("tiles/{n}.bin"), TILE_BYTES, path)?;
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

/// Reads an entry of at most `limit` bytes. A larger one is refused after reading one byte
/// past the limit, so a damaged or hostile file cannot make the loader inflate gigabytes.
fn read_entry<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
    limit: usize,
    path: &Path,
) -> Result<Vec<u8>, IoError> {
    let entry = zip
        .by_name(name)
        .map_err(|_| IoError::Format { path: path.to_owned(), message: format!("{name} is missing") })?;
    let mut bytes = Vec::new();
    entry.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| IoError::io(path, e))?;
    if bytes.len() > limit {
        return Err(IoError::Format { path: path.to_owned(), message: format!("{name} is larger than {limit} bytes") });
    }
    Ok(bytes)
}

/// Identifiers must stay below this, so the next free one can always be counted up to.
const MAX_ID: u64 = 1 << 53;

/// Checks that every layer and object identifier is in range and used once.
fn check_ids(layers: &[FileLayer], seen: &mut HashSet<u64>, path: &Path) -> Result<(), IoError> {
    fn take(id: u64, seen: &mut HashSet<u64>, path: &Path) -> Result<(), IoError> {
        let bad = |message: String| IoError::Format { path: path.to_owned(), message };
        if id == 0 || id >= MAX_ID {
            Err(bad(format!("identifier {id} is out of range")))
        } else if !seen.insert(id) {
            Err(bad(format!("identifier {id} is used more than once")))
        } else {
            Ok(())
        }
    }
    for l in layers {
        take(l.id.0, seen, path)?;
        match &l.content {
            FileContent::Group { children } => check_ids(children, seen, path)?,
            FileContent::Objects { objects } => {
                for o in objects {
                    take(o.id.0, seen, path)?;
                }
            }
        }
    }
    Ok(())
}

fn collect_tile_numbers(layers: &[FileLayer], out: &mut Vec<usize>) {
    for l in layers {
        match &l.content {
            FileContent::Group { children } => collect_tile_numbers(children, out),
            FileContent::Objects { objects } => {
                for o in objects {
                    out.extend(o.kind.tiles().iter().map(|t| t[2] as usize));
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
                let mut img = RasterImage::new();
                for &[x, y, n] in o.kind.tiles() {
                    let coord = TileCoord::new(
                        i32::try_from(x).map_err(|_| bad(format!("tile x {x} out of range")))?,
                        i32::try_from(y).map_err(|_| bad(format!("tile y {y} out of range")))?,
                    );
                    let tile = tiles.get(&(n as usize)).ok_or_else(|| bad(format!("tile {n} missing")))?;
                    img.set_tile(coord, Some(tile.clone()));
                }
                let image = Arc::new(img);
                let kind = match o.kind {
                    FileObjectKind::Pixels { offset, .. } => {
                        if offset.iter().any(|v| v.abs() > MAX_COORD) {
                            return Err(bad(format!("object {} has offset {offset:?}, out of range", o.id.0)));
                        }
                        ObjectKind::Pixels(PixelObject { image, offset })
                    }
                    FileObjectKind::Image { frame, transform, sampling, .. } => {
                        check_image(&frame, &transform, &sampling)
                            .map_err(|m| bad(format!("object {}: {m}", o.id.0)))?;
                        ObjectKind::Image(ImageObject { image, frame, transform, sampling })
                    }
                };
                out.push(Object { id: o.id, kind });
            }
            LayerContent::Objects(out)
        }
    };
    if !(0.0..=1.0).contains(&l.opacity) {
        return Err(bad(format!("layer {:?} has opacity {}", l.name, l.opacity)));
    }
    Ok(Layer { id: l.id, name: l.name, visible: l.visible, locked: l.locked, opacity: l.opacity, content })
}

/// Why an image object's placement is unusable, if it is.
fn check_image(frame: &IRect, m: &Affine, sampling: &Sampling) -> Result<(), String> {
    let coords = [frame.x0, frame.y0, frame.x1, frame.y1];
    if frame.is_empty() || coords.iter().any(|v| v.abs() > MAX_COORD) {
        return Err(format!("frame {frame:?} is empty or out of range"));
    }
    if !ImageObject::transform_ok(m) {
        return Err(format!("transform {:?} is not usable", m.to_array()));
    }
    sampling.check()
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

/// The tile from its shuffled bytes. Values that are not numbers, or infinite, which only a
/// damaged file holds, are read as 0.
fn unshuffle(bytes: &[u8]) -> Tile {
    let n = bytes.len() / 2;
    let (lo, hi) = bytes.split_at(n);
    let mut t = Tile::transparent();
    let values: &mut [u16] = bytemuck::cast_slice_mut(t.pixels_mut());
    for i in 0..n {
        let v = u16::from_le_bytes([lo[i], hi[i]]);
        // Exponent all ones: infinity or not a number.
        values[i] = if v & 0x7c00 == 0x7c00 { 0 } else { v };
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
    fn a_save_counts_its_tiles_and_stops_once_cancelled() {
        use std::sync::atomic::Ordering::Relaxed;
        let (doc, ids) = sample();
        let path = std::env::temp_dir().join(format!("opal-native-progress-{}.opal", std::process::id()));
        let p = Progress::default();
        save(&doc, &ids, &path, &p).unwrap();
        let (done, total) = (p.done.load(Relaxed), p.total.load(Relaxed));
        let saved = std::fs::read(&path).unwrap();
        let cancelled = Progress::default();
        cancelled.cancel();
        let r = save(&doc, &ids, &path, &cancelled);
        let back = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(total > 0 && done == total, "{done} of {total}");
        assert!(matches!(r, Err(IoError::Cancelled { .. })), "{r:?}");
        assert!(back == saved, "the file saved before was changed");
    }

    #[test]
    fn a_document_needing_more_memory_than_is_free_is_refused_before_its_tiles_are_read() {
        let (doc, ids) = sample();
        let path = std::env::temp_dir().join(format!("opal-native-free-{}.opal", std::process::id()));
        save(&doc, &ids, &path, &Progress::default()).unwrap();
        let refused = load(&path, Some(1_000_000));
        let opened = load(&path, Some(1 << 30));
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(refused, Err(IoError::TooLarge { free: 1_000_000, .. })), "{:?}", refused.err());
        assert!(opened.is_ok());
    }

    #[test]
    fn save_and_load_round_trip() {
        let (doc, ids) = sample();
        let path = std::env::temp_dir().join(format!("opal-native-{}.opal", std::process::id()));
        save(&doc, &ids, &path, &Progress::default()).unwrap();
        let loaded = load(&path, None).unwrap();
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
        let img = base.objects()[0].image();
        assert_eq!(img.pixel(5, 5), pixel_from_f32([0.25, 0.5, 0.75, 1.0]));
        assert_eq!(img.pixel(-700, 40), pixel_from_f32([-0.125, 1.5, 0.0, 0.5]));
        // Shared tiles stay shared after loading.
        let copy_img = copy.objects()[0].image();
        let c = TileCoord::containing(5, 5);
        assert!(Arc::ptr_eq(img.tile(c).unwrap(), copy_img.tile(c).unwrap()));
        // New identifiers do not collide with loaded ones.
        let mut ids2 = loaded.ids;
        assert!(!d.layer_ids().contains(&ids2.layer()));
    }

    #[test]
    fn image_objects_offsets_and_the_canvas_position_round_trip() {
        use opal_core::resample::Method;
        let (mut doc, mut ids) = sample();
        doc.canvas.x = -12;
        doc.canvas.y = 34;
        let LayerContent::Objects(objects) = &mut doc.layer_mut(doc.layers[0].id).unwrap().content else { panic!() };
        let ObjectKind::Pixels(p) = &mut objects[0].kind else { panic!() };
        p.offset = [7, -300];
        let mut pixels = RasterImage::new();
        pixels.set_pixel(1, 1, pixel_from_f32([1.0; 4]));
        let image = ImageObject {
            image: Arc::new(pixels),
            frame: IRect::from_size(3, 2),
            transform: Affine::rotate(0.25).then(Affine::scale(2.5, 0.5)).then(Affine::translate(-1.5, 9.0)),
            sampling: Sampling { method: Method::Lanczos { lobes: 4 }, anti_ringing: false },
        };
        let layer = Layer::with_objects(
            ids.layer(),
            "Pasted",
            vec![Object { id: ids.object(), kind: ObjectKind::Image(image.clone()) }],
        );
        doc.insert(Position { parent: None, index: 2 }, layer).unwrap();

        let path = std::env::temp_dir().join(format!("opal-native-v1-{}.opal", std::process::id()));
        save(&doc, &ids, &path, &Progress::default()).unwrap();
        let loaded = load(&path, None).unwrap();
        std::fs::remove_file(&path).unwrap();
        let d = loaded.doc;
        assert_eq!(d.canvas, doc.canvas);
        let ObjectKind::Pixels(p) = &d.layers[0].objects()[0].kind else { panic!("pixels") };
        assert_eq!(p.offset, [7, -300]);
        let ObjectKind::Image(i) = &d.layers[2].objects()[0].kind else { panic!("an image") };
        assert_eq!((i.frame, i.transform, i.sampling), (image.frame, image.transform, image.sampling));
        assert_eq!(i.image.pixel(1, 1), pixel_from_f32([1.0; 4]));
    }

    #[test]
    fn unusable_image_placements_are_refused() {
        let object = |transform: &str| {
            format!(
                r#"{{"id":1,"name":"l","visible":true,"locked":false,"opacity":1.0,"kind":"objects","objects":[{{"id":2,"kind":"image","tiles":[],"frame":{{"x0":0,"y0":0,"x1":4,"y1":4}},"transform":{transform},"sampling":{{"method":{{"kind":"bilinear"}},"anti_ringing":false}}}}]}}"#
            )
        };
        let doc = |layer: String| {
            format!(
                r#"{{"format":"opal-studio","version":1,"canvas":{{"width":1,"height":1}},"next_id":10,"layers":[{layer}]}}"#
            )
        };
        let ok = doc(object("[1,0,0,1,5,5]"));
        assert!(load_from(crafted(&ok, &[]), Path::new("ok.opal"), None).is_ok());
        // The last stretches by a million one way and squashes by far more the other.
        for bad in ["[0,0,0,0,0,0]", "[1e300,0,0,1,0,0]", "[1,0,0,1,1e30,0]", "[1,2,2,4,0,0]", "[1e6,0,0,2e-18,0,0]"] {
            let err = load_from(crafted(&doc(object(bad)), &[]), Path::new("bad.opal"), None).err();
            assert!(err.is_some_and(|e| e.to_string().contains("transform")), "{bad} accepted");
        }
    }

    #[test]
    fn shuffling_round_trips() {
        let mut t = Tile::transparent();
        t.set(1, 2, pixel_from_f32([1.0, -2.0, 0.001, 0.5]));
        assert_eq!(unshuffle(&shuffle(t.pixels())), t);
        // A value that is not a number comes back as 0.
        t.set(3, 3, [half::f16::NAN, half::f16::INFINITY, half::f16::ONE, half::f16::ONE]);
        let back = unshuffle(&shuffle(t.pixels()));
        assert_eq!(back.get(3, 3), [half::f16::ZERO, half::f16::ZERO, half::f16::ONE, half::f16::ONE]);
    }

    #[test]
    fn damaged_files_are_refused_with_a_reason() {
        let path = Path::new("broken.opal");
        let err = load_from(std::io::Cursor::new(b"not a zip".to_vec()), path, None).err().unwrap();
        assert!(err.to_string().contains("not a zip"), "{err}");

        // A newer format version is refused rather than misread.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let o = SimpleFileOptions::default();
            zip.start_file("mimetype", o).unwrap();
            std::io::Write::write_all(&mut zip, MIME_TYPE.as_bytes()).unwrap();
            zip.start_file("document.json", o).unwrap();
            let json =
                r#"{"format":"opal-studio","version":99,"canvas":{"width":1,"height":1},"next_id":1,"layers":[]}"#;
            std::io::Write::write_all(&mut zip, json.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buf.set_position(0);
        let err = load_from(buf, path, None).err().unwrap();
        assert!(err.to_string().contains("newer Opal Studio"), "{err}");
    }

    /// A zip with the given `document.json` and tile files.
    fn crafted(json: &str, tiles: &[(&str, &[u8])]) -> std::io::Cursor<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let o = SimpleFileOptions::default();
            zip.start_file("mimetype", o).unwrap();
            std::io::Write::write_all(&mut zip, MIME_TYPE.as_bytes()).unwrap();
            zip.start_file("document.json", o).unwrap();
            std::io::Write::write_all(&mut zip, json.as_bytes()).unwrap();
            for (name, bytes) in tiles {
                zip.start_file(*name, o).unwrap();
                std::io::Write::write_all(&mut zip, bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.set_position(0);
        buf
    }

    fn layer_json(id: u64, object: u64) -> String {
        format!(
            r#"{{"id":{id},"name":"l","visible":true,"locked":false,"opacity":1.0,"kind":"objects","objects":[{{"id":{object},"kind":"raster","tiles":[]}}]}}"#
        )
    }

    fn doc_json(layers: &[String]) -> String {
        format!(
            r#"{{"format":"opal-studio","version":0,"canvas":{{"width":1,"height":1}},"next_id":10,"layers":[{}]}}"#,
            layers.join(",")
        )
    }

    #[test]
    fn duplicate_identifiers_are_refused() {
        let json = doc_json(&[layer_json(3, 4), layer_json(3, 5)]);
        let err = load_from(crafted(&json, &[]), Path::new("dup.opal"), None).err().expect("refused");
        assert!(err.to_string().contains("more than once"), "{err}");
        let json = doc_json(&[layer_json(3, 4), layer_json(5, 4)]);
        assert!(load_from(crafted(&json, &[]), Path::new("dup.opal"), None).is_err(), "a shared object id");
    }

    #[test]
    fn identifiers_too_large_to_continue_from_are_refused() {
        let json = doc_json(&[layer_json(u64::MAX, 4)]);
        let err = load_from(crafted(&json, &[]), Path::new("big.opal"), None).err().expect("refused");
        assert!(err.to_string().contains("identifier"), "{err}");
    }

    #[test]
    fn an_oversized_tile_entry_is_refused() {
        let json = r#"{"format":"opal-studio","version":0,"canvas":{"width":1,"height":1},"next_id":10,"layers":[{"id":1,"name":"l","visible":true,"locked":false,"opacity":1.0,"kind":"objects","objects":[{"id":2,"kind":"raster","tiles":[[0,0,0]]}]}]}"#;
        let big = vec![0u8; TILE_BYTES * 3];
        let err =
            load_from(crafted(json, &[("tiles/0.bin", &big)]), Path::new("bomb.opal"), None).err().expect("refused");
        assert!(err.to_string().contains("tiles/0.bin"), "{err}");
    }

    #[test]
    fn tiles_compressed_in_batches_are_stored_in_order() {
        let tiles: Vec<Arc<Tile>> = (0..23)
            .map(|i| {
                let mut t = Tile::transparent();
                t.set(i, 7, opal_core::tile::pixel_from_f32([i as f32 / 23.0, 0.5, 0.25, 1.0]));
                Arc::new(t)
            })
            .collect();
        // Batches of one part, and batches of several (parts of 8, 8 and 4).
        for at_once in [5, 20] {
            let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
            let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            write_tiles(&mut zip, &tiles, options, at_once, &Progress::default()).unwrap();
            let mut back = ZipArchive::new(Cursor::new(zip.finish().unwrap().into_inner())).unwrap();
            assert_eq!(back.len(), 23);
            for (i, t) in tiles.iter().enumerate() {
                let mut entry = back.by_index(i).unwrap();
                assert_eq!(entry.name().unwrap(), format!("tiles/{i}.bin"), "batches of {at_once}");
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                assert!(unshuffle(&bytes) == **t, "tile {i}, batches of {at_once}");
            }
        }
    }
}
