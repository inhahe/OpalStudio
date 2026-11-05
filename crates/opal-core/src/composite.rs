//! Compositing: flattening layers into one image.
//!
//! [`composite_tile`] is the reference compositor. [`Projection`] caches its results, and a
//! pyramid of half-size copies for zoomed-out views, and recomputes a tile only when
//! something that contributes to it has changed.
//!
//! # How the cache knows what changed
//!
//! Each cached tile keeps a *key*: the list of everything that went into it (each
//! contributing source tile, held as an `Arc`, and each opacity, in compositing order). A
//! cached tile is current exactly when the key computed from the document now equals the
//! stored one, comparing source tiles by pointer. Because every change to pixels replaces
//! the tile's `Arc` (copy-on-write), no change can go unnoticed, and nothing has to report
//! changes to the cache. Keys hold their source tiles alive, so a freed tile's address can
//! never be reused by a new tile while a key still refers to it.

use crate::document::{Layer, LayerContent};
use crate::geom::IRect;
use crate::raster::RasterImage;
use crate::tile::{Pixel, PixelF32, TILE_PIXELS, TILE_SIZE, Tile, TileCoord};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Composites `dst = src × opacity over dst`, premultiplied.
fn over(dst: &mut [PixelF32], src: &[PixelF32], opacity: f32) {
    for (d, s) in dst.iter_mut().zip(src) {
        let k = 1.0 - s[3] * opacity;
        for i in 0..4 {
            d[i] = s[i] * opacity + d[i] * k;
        }
    }
}

/// Composites the visible layers' content at tile `c` onto `acc`. Returns whether anything
/// was drawn.
fn composite_into(layers: &[Arc<Layer>], c: TileCoord, acc: &mut [PixelF32]) -> bool {
    let mut any = false;
    let mut scratch: Option<Vec<PixelF32>> = None;
    for layer in layers {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        match &layer.content {
            LayerContent::Objects(_) => {
                let tiles: Vec<&Arc<Tile>> = layer.rasters().filter_map(|r| r.tile(c)).collect();
                if tiles.is_empty() {
                    continue;
                }
                let buf = scratch.get_or_insert_with(|| vec![[0.0; 4]; TILE_PIXELS]);
                if let [only] = tiles[..] {
                    only.to_f32(buf);
                } else {
                    // Objects composite among themselves first; the layer's opacity then
                    // applies to the combined result.
                    let mut combined = vec![[0.0f32; 4]; TILE_PIXELS];
                    for t in tiles {
                        t.to_f32(buf);
                        over(&mut combined, buf, 1.0);
                    }
                    buf.copy_from_slice(&combined);
                }
                over(acc, buf, layer.opacity);
                any = true;
            }
            LayerContent::Group(children) => {
                let mut group = vec![[0.0f32; 4]; TILE_PIXELS];
                if composite_into(children, c, &mut group) {
                    over(acc, &group, layer.opacity);
                    any = true;
                }
            }
        }
    }
    any
}

/// The composite of `layers` at level-0 tile `c`, or `None` where nothing shows.
pub fn composite_tile(layers: &[Arc<Layer>], c: TileCoord) -> Option<Tile> {
    let mut acc = vec![[0.0f32; 4]; TILE_PIXELS];
    if !composite_into(layers, c, &mut acc) {
        return None;
    }
    let t = Tile::from_f32(&acc);
    (!t.is_transparent()).then_some(t)
}

/// Every level-0 tile coordinate that visible content of `layers` occupies within `range`
/// (all of them when `range` is `None`).
pub fn occupied_tiles(layers: &[Arc<Layer>], range: Option<&IRect>) -> HashSet<TileCoord> {
    fn walk(layers: &[Arc<Layer>], range: Option<&IRect>, out: &mut HashSet<TileCoord>) {
        for layer in layers {
            if !layer.visible || layer.opacity <= 0.0 {
                continue;
            }
            match &layer.content {
                LayerContent::Objects(_) => {
                    for r in layer.rasters() {
                        for (c, _) in r.tiles() {
                            let inside = range.is_none_or(|rg| !c.rect(0).intersect(rg).is_empty());
                            if inside {
                                out.insert(c);
                            }
                        }
                    }
                }
                LayerContent::Group(children) => walk(children, range, out),
            }
        }
    }
    let mut out = HashSet::new();
    walk(layers, range, &mut out);
    out
}

/// Composites every tile `layers` occupy into one bitmap (for merging and flattening).
pub fn composite_all(layers: &[Arc<Layer>]) -> RasterImage {
    let coords: Vec<TileCoord> = occupied_tiles(layers, None).into_iter().collect();
    let tiles: Vec<(TileCoord, Option<Tile>)> = coords.par_iter().map(|&c| (c, composite_tile(layers, c))).collect();
    let mut img = RasterImage::new();
    for (c, t) in tiles {
        img.set_tile(c, t.map(Arc::new));
    }
    img
}

/// The composite of `layers` over `rect`, as a row-major buffer (for exporting).
pub fn flatten(layers: &[Arc<Layer>], rect: &IRect) -> Vec<Pixel> {
    let coords: Vec<TileCoord> = occupied_tiles(layers, Some(rect)).into_iter().collect();
    let tiles: Vec<(TileCoord, Option<Tile>)> = coords.par_iter().map(|&c| (c, composite_tile(layers, c))).collect();
    let mut img = RasterImage::new();
    for (c, t) in tiles {
        img.set_tile(c, t.map(Arc::new));
    }
    img.read_rect(rect)
}

/// Halves four tiles (top-left, top-right, bottom-left, bottom-right) into one, averaging
/// each 2×2 block of premultiplied linear pixels.
fn downsample(children: [Option<&Arc<Tile>>; 4]) -> Option<Tile> {
    if children.iter().all(Option::is_none) {
        return None;
    }
    let half = TILE_SIZE / 2;
    let mut out = vec![[0.0f32; 4]; TILE_PIXELS];
    let mut buf = vec![[0.0f32; 4]; TILE_PIXELS];
    for (q, child) in children.iter().enumerate() {
        let Some(child) = child else { continue };
        child.to_f32(&mut buf);
        let (qx, qy) = ((q as u32 % 2) * half, (q as u32 / 2) * half);
        for y in 0..half {
            for x in 0..half {
                let i = |dx: u32, dy: u32| ((2 * y + dy) * TILE_SIZE + 2 * x + dx) as usize;
                let (a, b, c, d) = (buf[i(0, 0)], buf[i(1, 0)], buf[i(0, 1)], buf[i(1, 1)]);
                let o = &mut out[((qy + y) * TILE_SIZE + qx + x) as usize];
                for k in 0..4 {
                    o[k] = (a[k] + b[k] + c[k] + d[k]) * 0.25;
                }
            }
        }
    }
    let t = Tile::from_f32(&out);
    (!t.is_transparent()).then_some(t)
}

/// One element of a level-0 cache key; see the module documentation.
#[derive(Clone, Debug)]
enum Token {
    /// A layer begins, with this opacity (as bits, so equality is exact).
    Layer(u32),
    /// A group begins, with this opacity.
    Group(u32),
    /// A source tile.
    Tile(Arc<Tile>),
    /// The innermost open layer or group ends.
    End,
}

impl PartialEq for Token {
    fn eq(&self, other: &Token) -> bool {
        match (self, other) {
            (Token::Layer(a), Token::Layer(b)) | (Token::Group(a), Token::Group(b)) => a == b,
            (Token::Tile(a), Token::Tile(b)) => Arc::ptr_eq(a, b),
            (Token::End, Token::End) => true,
            _ => false,
        }
    }
}

/// Appends the key of `layers` at `c` to `out`. Returns whether anything contributes.
fn key_into(layers: &[Arc<Layer>], c: TileCoord, out: &mut Vec<Token>) -> bool {
    let mut any = false;
    for layer in layers {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        let start = out.len();
        let contributes = match &layer.content {
            LayerContent::Objects(_) => {
                out.push(Token::Layer(layer.opacity.to_bits()));
                let before = out.len();
                out.extend(layer.rasters().filter_map(|r| r.tile(c)).map(|t| Token::Tile(t.clone())));
                out.len() > before
            }
            LayerContent::Group(children) => {
                out.push(Token::Group(layer.opacity.to_bits()));
                key_into(children, c, out)
            }
        };
        if contributes {
            out.push(Token::End);
            any = true;
        } else {
            out.truncate(start);
        }
    }
    any
}

#[derive(Clone, Debug)]
enum Key {
    Base(Vec<Token>),
    Pyramid([Option<Arc<Tile>>; 4]),
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        match (self, other) {
            (Key::Base(a), Key::Base(b)) => a == b,
            (Key::Pyramid(a), Key::Pyramid(b)) => a.iter().zip(b).all(|(x, y)| match (x, y) {
                (None, None) => true,
                (Some(x), Some(y)) => Arc::ptr_eq(x, y),
                _ => false,
            }),
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    key: Key,
    tile: Option<Arc<Tile>>,
}

/// The composited image, cached per tile, with a pyramid of reduced levels: a level-`k`
/// tile covers `256 << k` canvas pixels each way at `1 / 2^k` of full resolution.
#[derive(Default, Debug)]
pub struct Projection {
    levels: Vec<HashMap<TileCoord, Entry>>,
}

/// The highest pyramid level kept (a tile then covers 256 × 2^16 pixels).
pub const MAX_LEVEL: u32 = 16;

impl Projection {
    pub fn new() -> Projection {
        Projection::default()
    }

    /// Brings the tiles of pyramid level `level` that intersect `rect` (canvas pixels) up to
    /// date with `layers`, computing any that are stale in parallel, and returns those that
    /// have content.
    pub fn update(&mut self, layers: &[Arc<Layer>], level: u32, rect: &IRect) -> Vec<(TileCoord, Arc<Tile>)> {
        let level = level.min(MAX_LEVEL);
        while self.levels.len() <= level as usize {
            self.levels.push(HashMap::new());
        }
        // The level-0 area under the requested level-`level` tiles.
        let span = i64::from(TILE_SIZE) << level;
        let outer = IRect::new(
            rect.x0.div_euclid(span) * span,
            rect.y0.div_euclid(span) * span,
            (rect.x1 + span - 1).div_euclid(span) * span,
            (rect.y1 + span - 1).div_euclid(span) * span,
        );
        let mut coords: HashSet<TileCoord> = occupied_tiles(layers, Some(&outer));
        self.update_base(layers, &coords, &outer);
        for l in 1..=level {
            coords = coords.iter().map(|c| c.parent()).collect();
            self.update_pyramid(l, &coords, &outer);
        }
        let top = &self.levels[level as usize];
        coords.iter().filter_map(|c| top.get(c).and_then(|e| e.tile.clone()).map(|t| (*c, t))).collect()
    }

    /// The cached tile at `level` and `c` as of the last [`update`](Self::update) that
    /// covered it.
    pub fn tile(&self, level: u32, c: TileCoord) -> Option<&Arc<Tile>> {
        self.levels.get(level as usize)?.get(&c)?.tile.as_ref()
    }

    fn update_base(&mut self, layers: &[Arc<Layer>], coords: &HashSet<TileCoord>, outer: &IRect) {
        let cache = &mut self.levels[0];
        // Tiles inside the area that nothing occupies any more.
        cache.retain(|c, _| coords.contains(c) || c.rect(0).intersect(outer).is_empty());
        let mut stale = Vec::new();
        for &c in coords {
            let mut tokens = Vec::new();
            key_into(layers, c, &mut tokens);
            let key = Key::Base(tokens);
            if cache.get(&c).is_none_or(|e| e.key != key) {
                stale.push((c, key));
            }
        }
        let done: Vec<(TileCoord, Entry)> = stale
            .into_par_iter()
            .map(|(c, key)| (c, Entry { key, tile: composite_tile(layers, c).map(Arc::new) }))
            .collect();
        cache.extend(done);
    }

    fn update_pyramid(&mut self, level: u32, coords: &HashSet<TileCoord>, outer: &IRect) {
        let (below, rest) = self.levels.split_at_mut(level as usize);
        let below = &below[level as usize - 1];
        let cache = &mut rest[0];
        cache.retain(|c, _| coords.contains(c) || c.rect(level).intersect(outer).is_empty());
        let mut stale = Vec::new();
        for &c in coords {
            let kids = c.children().map(|k| below.get(&k).and_then(|e| e.tile.clone()));
            let key = Key::Pyramid(kids);
            if cache.get(&c).is_none_or(|e| e.key != key) {
                stale.push((c, key));
            }
        }
        let done: Vec<(TileCoord, Entry)> = stale
            .into_par_iter()
            .map(|(c, key)| {
                let Key::Pyramid(kids) = &key else { unreachable!() };
                let tile = downsample([kids[0].as_ref(), kids[1].as_ref(), kids[2].as_ref(), kids[3].as_ref()]);
                (c, Entry { tile: tile.map(Arc::new), key })
            })
            .collect();
        cache.extend(done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, IdGen, LayerId, Position};
    use crate::tile::{pixel_from_f32, pixel_to_f32};

    fn solid(rect: IRect, rgba: PixelF32) -> RasterImage {
        let mut img = RasterImage::new();
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                img.set_pixel(x, y, pixel_from_f32(rgba));
            }
        }
        img
    }

    fn add(doc: &mut Document, ids: &mut IdGen, img: RasterImage) -> LayerId {
        let id = ids.layer();
        let index = doc.layers.len();
        let layer = Layer::new_raster(id, ids.object(), "l", img);
        doc.insert(Position { parent: None, index }, layer).unwrap();
        id
    }

    fn close(a: PixelF32, b: PixelF32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
    }

    #[test]
    fn layers_composite_over_each_other_with_opacity() {
        let mut ids = IdGen::default();
        let mut d = Document::new(4, 4);
        add(&mut d, &mut ids, solid(IRect::from_size(4, 4), [1.0, 0.0, 0.0, 1.0]));
        let top = add(&mut d, &mut ids, solid(IRect::from_size(2, 4), [0.0, 0.0, 0.5, 0.5]));
        d.layer_mut(top).unwrap().opacity = 0.5;
        let px = flatten(&d.layers, &IRect::from_size(4, 1));
        // Blue at half alpha, at half opacity, over opaque red.
        assert!(close(pixel_to_f32(px[0]), [0.75, 0.0, 0.25, 1.0]), "{:?}", pixel_to_f32(px[0]));
        assert!(close(pixel_to_f32(px[3]), [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn hidden_layers_and_groups_follow_their_flags() {
        let mut ids = IdGen::default();
        let mut d = Document::new(1, 1);
        let g = ids.layer();
        d.insert(Position { parent: None, index: 0 }, Layer::new_group(g, "g")).unwrap();
        let child = ids.layer();
        let layer = Layer::new_raster(child, ids.object(), "c", solid(IRect::from_size(1, 1), [0.0, 1.0, 0.0, 1.0]));
        d.insert(Position { parent: Some(g), index: 0 }, layer).unwrap();
        d.layer_mut(g).unwrap().opacity = 0.25;
        let px = flatten(&d.layers, &IRect::from_size(1, 1));
        assert!(close(pixel_to_f32(px[0]), [0.0, 0.25, 0.0, 0.25]));
        d.layer_mut(child).unwrap().visible = false;
        assert!(composite_tile(&d.layers, TileCoord::new(0, 0)).is_none());
    }

    #[test]
    fn content_far_outside_the_canvas_composites() {
        let mut ids = IdGen::default();
        let mut d = Document::new(10, 10);
        add(&mut d, &mut ids, solid(IRect::new(-3000, -3000, -2999, -2999), [1.0; 4]));
        let px = flatten(&d.layers, &IRect::new(-3000, -3000, -2999, -2999));
        assert!(close(pixel_to_f32(px[0]), [1.0; 4]));
    }

    #[test]
    fn the_projection_recomputes_exactly_the_tiles_that_changed() {
        let mut ids = IdGen::default();
        let mut d = Document::new(600, 10);
        let l = add(&mut d, &mut ids, solid(IRect::new(0, 0, 600, 1), [1.0; 4]));
        let mut p = Projection::new();
        let view = IRect::from_size(600, 10);
        let first = p.update(&d.layers, 0, &view);
        assert_eq!(first.len(), 3);
        let before: HashMap<TileCoord, Arc<Tile>> = first.into_iter().collect();

        // Change one pixel in the middle tile.
        let crate::document::LayerContent::Objects(objects) = &mut d.layer_mut(l).unwrap().content else { panic!() };
        let crate::document::ObjectKind::Raster(img) = &mut objects[0].kind;
        Arc::make_mut(img).set_pixel(300, 0, pixel_from_f32([0.0, 0.0, 0.0, 1.0]));

        let after: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view).into_iter().collect();
        for (c, t) in &after {
            let same = Arc::ptr_eq(t, &before[c]);
            assert_eq!(same, c.x != 1, "tile {c:?} recomputed: {}", !same);
        }
        assert!(close(pixel_to_f32(after[&TileCoord::new(1, 0)].get(300 - 256, 0)), [0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn opacity_changes_and_layer_removal_invalidate() {
        let mut ids = IdGen::default();
        let mut d = Document::new(10, 10);
        let l = add(&mut d, &mut ids, solid(IRect::from_size(1, 1), [1.0; 4]));
        let mut p = Projection::new();
        let view = IRect::from_size(10, 10);
        assert_eq!(p.update(&d.layers, 0, &view).len(), 1);
        d.layer_mut(l).unwrap().opacity = 0.5;
        let t = p.update(&d.layers, 0, &view);
        assert!(close(pixel_to_f32(t[0].1.get(0, 0)), [0.5; 4]));
        d.remove(l).unwrap();
        assert!(p.update(&d.layers, 0, &view).is_empty());
        assert!(p.tile(0, TileCoord::new(0, 0)).is_none());
    }

    #[test]
    fn pyramid_levels_average_their_children() {
        let mut ids = IdGen::default();
        let mut d = Document::new(512, 512);
        // A checker of opaque white and transparent pixels averages to half-transparent gray.
        let mut img = RasterImage::new();
        for y in 0..512 {
            for x in 0..512 {
                if (x + y) % 2 == 0 {
                    img.set_pixel(x, y, pixel_from_f32([1.0; 4]));
                }
            }
        }
        add(&mut d, &mut ids, img);
        let mut p = Projection::new();
        let tiles = p.update(&d.layers, 1, &IRect::from_size(512, 512));
        assert_eq!(tiles.len(), 1);
        let (c, t) = &tiles[0];
        assert_eq!(*c, TileCoord::new(0, 0));
        assert!(close(pixel_to_f32(t.get(0, 0)), [0.5; 4]));
        assert!(close(pixel_to_f32(t.get(255, 255)), [0.5; 4]));
    }
}
