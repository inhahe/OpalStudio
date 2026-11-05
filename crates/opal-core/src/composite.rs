//! Compositing: flattening layers into one image.
//!
//! [`composite_tile`] is the reference compositor. [`Projection`] caches its results, and a
//! pyramid of half-size copies for zoomed-out views, and recomputes a tile only when
//! something that contributes to it has changed.
//!
//! # How the cache knows what changed
//!
//! Each cached tile keeps a *key*: the list of everything that went into it, in compositing
//! order: each opacity, each contributing stored tile (held as an `Arc`), with the offset
//! of pixels shifted off the tile grid, and each image object's image (held as an `Arc`)
//! with its transform and sampling. A cached tile is current exactly when the key computed
//! from the document now equals the stored one, comparing tiles and images by pointer.
//! Because every change to pixels replaces the tile's `Arc`, and the image's (copy-on-write;
//! the key's own reference makes the `Arc` shared, so it is always copied), no change can
//! go unnoticed, and nothing has to report changes to the cache. Keys hold what they refer
//! to alive, so a freed address can never be reused while a key still refers to it.

use crate::document::{ImageObject, Layer, LayerContent, Object, ObjectKind, PixelObject};
use crate::geom::{FRect, IRect};
use crate::raster::RasterImage;
use crate::resample;
use crate::tile::{Pixel, PixelF32, TILE_PIXELS, TILE_SIZE, Tile, TileCoord, downsample};
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

/// What an object shows in one level-0 tile.
enum Shown<'a> {
    /// A stored tile, lying exactly on the tile.
    Stored(&'a Arc<Tile>),
    /// Pixels drawn or assembled for the tile.
    Drawn(Tile),
}

impl Shown<'_> {
    fn tile(&self) -> &Tile {
        match self {
            Shown::Stored(t) => t,
            Shown::Drawn(t) => t,
        }
    }
}

/// How far a pixel object's offset is from whole tiles, and which whole tiles.
fn tile_offset(p: &PixelObject) -> ([i64; 2], [i64; 2]) {
    let t = i64::from(TILE_SIZE);
    let [ox, oy] = p.offset;
    ([ox.div_euclid(t), oy.div_euclid(t)], [ox.rem_euclid(t), oy.rem_euclid(t)])
}

/// The stored tiles of `p` that show in document tile `c`: one when the offset is whole
/// tiles, else the two or four the tile straddles (`None` where a tile is absent).
fn pixel_sources(p: &PixelObject, c: TileCoord) -> [Option<&Arc<Tile>>; 4] {
    let ([tx, ty], [px, py]) = tile_offset(p);
    let base = (i64::from(c.x) - tx, i64::from(c.y) - ty);
    let at = |dx: i64, dy: i64| {
        let (x, y) = (base.0 + dx, base.1 + dy);
        let (x, y) = (i32::try_from(x).ok()?, i32::try_from(y).ok()?);
        p.image.tile(TileCoord::new(x, y))
    };
    // Shifting right by `px` brings in the tile to the left: document tile c starts inside
    // stored tile (c - whole) - 1 when there is a remainder.
    match (px > 0, py > 0) {
        (false, false) => [at(0, 0), None, None, None],
        (true, false) => [at(-1, 0), at(0, 0), None, None],
        (false, true) => [at(0, -1), at(0, 0), None, None],
        (true, true) => [at(-1, -1), at(0, -1), at(-1, 0), at(0, 0)],
    }
}

/// What object `o` shows in level-0 document tile `c`, if anything.
fn object_tile(o: &Object, c: TileCoord) -> Option<Shown<'_>> {
    match &o.kind {
        ObjectKind::Pixels(p) => {
            let sources = pixel_sources(p, c);
            let (_, [px, py]) = tile_offset(p);
            if px == 0 && py == 0 {
                return sources[0].map(Shown::Stored);
            }
            if sources.iter().all(Option::is_none) {
                return None;
            }
            let rect = c.rect(0).translate(-p.offset[0], -p.offset[1]);
            let tile = Tile::from_pixels(p.image.read_rect(&rect));
            (!tile.is_transparent()).then_some(Shown::Drawn(tile))
        }
        ObjectKind::Image(i) => {
            if !image_touches(i, c) {
                return None;
            }
            resample::render_tile(&i.image, &i.transform, &i.sampling, c).map(Shown::Drawn)
        }
    }
}

/// Whether image object `i` can draw anything into document tile `c`: whether the tile,
/// carried back into the image's pixels, comes within the filter's reach of its content.
fn image_touches(i: &ImageObject, c: TileCoord) -> bool {
    let (Some(content), Some(inv)) = (i.image.content_bounds(), i.transform.inverse()) else { return false };
    let back = inv.bounds(&c.rect(0).to_f());
    let r = resample::reach(&i.transform, &i.sampling);
    let content =
        FRect::new(content.x0 as f64 - r, content.y0 as f64 - r, content.x1 as f64 + r, content.y1 as f64 + r);
    back.x0 < content.x1 && back.x1 > content.x0 && back.y0 < content.y1 && back.y1 > content.y0
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
            LayerContent::Objects(objects) => {
                let shown: Vec<Shown> = objects.iter().filter_map(|o| object_tile(o, c)).collect();
                if shown.is_empty() {
                    continue;
                }
                let buf = scratch.get_or_insert_with(|| vec![[0.0; 4]; TILE_PIXELS]);
                if let [only] = &shown[..] {
                    only.tile().to_f32(buf);
                } else {
                    // Objects composite among themselves first; the layer's opacity then
                    // applies to the combined result.
                    let mut combined = vec![[0.0f32; 4]; TILE_PIXELS];
                    for s in &shown {
                        s.tile().to_f32(buf);
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

/// How opaque object `o` is at document pixel `(x, y)` (an image object's nearest pixel).
fn alpha_at(o: &Object, x: i64, y: i64) -> f32 {
    match &o.kind {
        ObjectKind::Pixels(p) => p.image.pixel(x - p.offset[0], y - p.offset[1])[3].to_f32(),
        ObjectKind::Image(i) => match i.transform.inverse() {
            Some(inv) => {
                let [u, v] = inv.apply([x as f64 + 0.5, y as f64 + 0.5]);
                i.image.pixel(u.floor() as i64, v.floor() as i64)[3].to_f32()
            }
            None => 0.0,
        },
    }
}

/// What object `o` shows at document point `(x, y)`: the pixel there (an image object's
/// nearest pixel).
fn object_point(o: &Object, x: f64, y: f64) -> PixelF32 {
    let p = match &o.kind {
        ObjectKind::Pixels(p) => p.image.pixel(x.floor() as i64 - p.offset[0], y.floor() as i64 - p.offset[1]),
        ObjectKind::Image(i) => match i.transform.inverse() {
            Some(inv) => {
                let [u, v] = inv.apply([x, y]);
                i.image.pixel(u.floor() as i64, v.floor() as i64)
            }
            None => return [0.0; 4],
        },
    };
    crate::tile::pixel_to_f32(p)
}

/// What `layer` itself holds at document point `(x, y)`, whatever its own visibility and
/// opacity (the layers inside a group count with theirs).
pub fn layer_point(layer: &Layer, x: f64, y: f64) -> PixelF32 {
    let mut acc = [0.0f32; 4];
    match &layer.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                over(std::slice::from_mut(&mut acc), &[object_point(o, x, y)], 1.0);
            }
        }
        LayerContent::Group(children) => acc = point(children, x, y),
    }
    acc
}

/// What `layers` show at document point `(x, y)`, composited as the canvas composites but
/// from each object's nearest pixel: for thumbnails, where a few such points averaged per
/// thumbnail pixel stand in for the full composite at a tiny fraction of the cost.
pub fn point(layers: &[Arc<Layer>], x: f64, y: f64) -> PixelF32 {
    let mut acc = [0.0f32; 4];
    for layer in layers {
        if layer.visible && layer.opacity > 0.0 {
            let p = layer_point(layer, x, y);
            over(std::slice::from_mut(&mut acc), &[p], layer.opacity);
        }
    }
    acc
}

/// The layers whose content shows at document pixel `(x, y)`, topmost first, with how
/// opaque it is there (times their opacity and their groups'): for picking layers with the
/// pointer. Groups themselves are not listed; hidden layers are left out.
pub fn layers_at(layers: &[Arc<Layer>], x: i64, y: i64) -> Vec<(crate::document::LayerId, f32)> {
    fn walk(layers: &[Arc<Layer>], x: i64, y: i64, scale: f32, out: &mut Vec<(crate::document::LayerId, f32)>) {
        for layer in layers.iter().rev() {
            if !layer.visible || layer.opacity <= 0.0 {
                continue;
            }
            let k = scale * layer.opacity;
            match &layer.content {
                LayerContent::Objects(objects) => {
                    let a = objects.iter().fold(0.0f32, |a, o| a + alpha_at(o, x, y) * (1.0 - a));
                    if a > 0.0 {
                        out.push((layer.id, a * k));
                    }
                }
                LayerContent::Group(children) => walk(children, x, y, k, out),
            }
        }
    }
    let mut out = Vec::new();
    walk(layers, x, y, 1.0, &mut out);
    out
}

/// Calls `f` with every visible object of `layers`, at any depth.
fn for_each_visible_object(layers: &[Arc<Layer>], f: &mut dyn FnMut(&Object)) {
    for layer in layers {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        match &layer.content {
            LayerContent::Objects(objects) => objects.iter().for_each(&mut *f),
            LayerContent::Group(children) => for_each_visible_object(children, f),
        }
    }
}

/// Builds what drawing the visible image objects will need (their pyramids), so tiles
/// drawn in parallel find it ready instead of each building it.
fn prepare(layers: &[Arc<Layer>]) {
    for_each_visible_object(layers, &mut |o| {
        if let ObjectKind::Image(i) = &o.kind {
            resample::prepare(&i.image, &i.transform, &i.sampling);
        }
    });
}

/// Every level-0 tile coordinate that visible content of `layers` occupies within `range`
/// (all of them when `range` is `None`).
pub fn occupied_tiles(layers: &[Arc<Layer>], range: Option<&IRect>) -> HashSet<TileCoord> {
    let mut out = HashSet::new();
    let mut add = |r: IRect| {
        let r = match range {
            Some(range) => r.intersect(range),
            None => r,
        };
        if !r.is_empty() {
            out.extend(TileCoord::covering(&r, 0));
        }
    };
    for_each_visible_object(layers, &mut |o| match &o.kind {
        ObjectKind::Pixels(p) => {
            for (c, _) in p.image.tiles() {
                add(c.rect(0).translate(p.offset[0], p.offset[1]));
            }
        }
        ObjectKind::Image(i) => {
            let r = resample::reach(&i.transform, &i.sampling);
            for (c, _) in i.image.tiles() {
                let t = c.rect(0);
                let grown = FRect::new(t.x0 as f64 - r, t.y0 as f64 - r, t.x1 as f64 + r, t.y1 as f64 + r);
                add(IRect::covering(&i.transform.bounds(&grown)));
            }
        }
    });
    out
}

/// Composites every tile `layers` occupy within `range` (everywhere when `None`) into one
/// bitmap, in parallel.
fn composite_region(layers: &[Arc<Layer>], range: Option<&IRect>) -> RasterImage {
    prepare(layers);
    let coords: Vec<TileCoord> = occupied_tiles(layers, range).into_iter().collect();
    let tiles: Vec<(TileCoord, Option<Tile>)> = coords.par_iter().map(|&c| (c, composite_tile(layers, c))).collect();
    let mut img = RasterImage::new();
    for (c, t) in tiles {
        img.set_tile(c, t.map(Arc::new));
    }
    img
}

/// Composites every tile `layers` occupy into one bitmap (for merging and flattening).
pub fn composite_all(layers: &[Arc<Layer>]) -> RasterImage {
    composite_region(layers, None)
}

/// Composites what `layers` show within `range` into one bitmap.
pub fn composite_within(layers: &[Arc<Layer>], range: &IRect) -> RasterImage {
    composite_region(layers, Some(range))
}

/// The composite of `layers` over `rect`, as a row-major buffer (for exporting).
pub fn flatten(layers: &[Arc<Layer>], rect: &IRect) -> Vec<Pixel> {
    composite_region(layers, Some(rect)).read_rect(rect)
}

/// One element of a level-0 cache key; see the module documentation.
#[derive(Clone, Debug)]
enum Token {
    /// A layer begins, with this opacity (as bits, so equality is exact).
    Layer(u32),
    /// A group begins, with this opacity.
    Group(u32),
    /// A stored tile, lying exactly on the tile.
    Tile(Arc<Tile>),
    /// Pixels off the tile grid: their offset within a tile, and the stored tiles that show
    /// (in [`pixel_sources`]'s order).
    Shifted { phase: [i64; 2], tiles: [Option<Arc<Tile>>; 4] },
    /// An image object: its pixels (the image stands for all of them, since an image
    /// object's pixels are not changed in place) and how it is drawn.
    Image { image: Arc<RasterImage>, transform: [u64; 6], sampling: [u32; 4] },
    /// The innermost open layer or group ends.
    End,
}

impl PartialEq for Token {
    fn eq(&self, other: &Token) -> bool {
        let same = |a: &Option<Arc<Tile>>, b: &Option<Arc<Tile>>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        match (self, other) {
            (Token::Layer(a), Token::Layer(b)) | (Token::Group(a), Token::Group(b)) => a == b,
            (Token::Tile(a), Token::Tile(b)) => Arc::ptr_eq(a, b),
            (Token::Shifted { phase: p, tiles: a }, Token::Shifted { phase: q, tiles: b }) => {
                p == q && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (
                Token::Image { image: a, transform: m, sampling: s },
                Token::Image { image: b, transform: n, sampling: t },
            ) => Arc::ptr_eq(a, b) && m == n && s == t,
            (Token::End, Token::End) => true,
            _ => false,
        }
    }
}

/// The token for what object `o` shows in tile `c`, if it can show anything there.
fn object_token(o: &Object, c: TileCoord) -> Option<Token> {
    match &o.kind {
        ObjectKind::Pixels(p) => {
            let sources = pixel_sources(p, c);
            let (_, phase) = tile_offset(p);
            if phase == [0, 0] {
                return sources[0].map(|t| Token::Tile(t.clone()));
            }
            if sources.iter().all(Option::is_none) {
                return None;
            }
            Some(Token::Shifted { phase, tiles: sources.map(|t| t.cloned()) })
        }
        ObjectKind::Image(i) => image_touches(i, c).then(|| Token::Image {
            image: i.image.clone(),
            transform: i.transform.to_bits(),
            sampling: i.sampling.key(),
        }),
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
            LayerContent::Objects(objects) => {
                out.push(Token::Layer(layer.opacity.to_bits()));
                let before = out.len();
                out.extend(objects.iter().filter_map(|o| object_token(o, c)));
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

/// The source tile, if `tokens` describe exactly one stored tile lying on the tile, from one
/// layer at full opacity (perhaps inside groups at full opacity): the composite is then
/// that tile unchanged.
fn lone_source(tokens: &[Token]) -> Option<&Arc<Tile>> {
    let full = 1.0f32.to_bits();
    let mut tile = None;
    for t in tokens {
        match t {
            Token::Layer(o) | Token::Group(o) if *o != full => return None,
            Token::Shifted { .. } | Token::Image { .. } => return None,
            Token::Tile(_) if tile.is_some() => return None,
            Token::Tile(t) => tile = Some(t),
            _ => {}
        }
    }
    tile
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
    last: Option<LastUpdate>,
}

/// What the last [`Projection::update`] was asked and answered, so that asking the same again
/// (an unchanged document in an unchanged view) costs nothing.
#[derive(Debug)]
struct LastUpdate {
    /// The top-level layers, held so a replaced layer cannot reuse an old address.
    layers: Vec<Arc<Layer>>,
    level: u32,
    rect: IRect,
    result: Vec<(TileCoord, Arc<Tile>)>,
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
        // Any change below the top level replaces the top-level layer holding it.
        if let Some(last) = &self.last
            && last.level == level
            && last.rect == *rect
            && last.layers.len() == layers.len()
            && last.layers.iter().zip(layers).all(|(a, b)| Arc::ptr_eq(a, b))
        {
            return last.result.clone();
        }
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
        prepare(layers);
        self.update_base(layers, &coords, &outer);
        for l in 1..=level {
            coords = coords.iter().map(|c| c.parent()).collect();
            self.update_pyramid(l, &coords, &outer);
        }
        let top = &self.levels[level as usize];
        let result: Vec<(TileCoord, Arc<Tile>)> =
            coords.iter().filter_map(|c| top.get(c).and_then(|e| e.tile.clone()).map(|t| (*c, t))).collect();
        self.last = Some(LastUpdate { layers: layers.to_vec(), level, rect: *rect, result: result.clone() });
        result
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
            .map(|(c, key)| {
                let Key::Base(tokens) = &key else { unreachable!() };
                // A tile that one layer at full opacity supplies alone is shown as it is,
                // without a copy.
                let tile = match lone_source(tokens) {
                    Some(t) => Some(t.clone()),
                    None => composite_tile(layers, c).map(Arc::new),
                };
                (c, Entry { key, tile })
            })
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
        // Tiles with the same four children (a uniform image is full of them) are reduced
        // once and share the result.
        let ptr = |t: &Option<Arc<Tile>>| t.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
        let mut unique: HashMap<[usize; 4], [Option<Arc<Tile>>; 4]> = HashMap::new();
        for (_, key) in &stale {
            let Key::Pyramid(kids) = key else { unreachable!() };
            unique.entry(kids.each_ref().map(ptr)).or_insert_with(|| kids.clone());
        }
        let reduced: HashMap<[usize; 4], Option<Arc<Tile>>> = unique
            .into_par_iter()
            .map(|(k, kids)| {
                let tile = downsample(kids.each_ref().map(|k| k.as_deref()));
                (k, tile.map(Arc::new))
            })
            .collect();
        for (c, key) in stale {
            let Key::Pyramid(kids) = &key else { unreachable!() };
            let tile = reduced[&kids.each_ref().map(ptr)].clone();
            cache.insert(c, Entry { tile, key });
        }
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
        let LayerContent::Objects(objects) = &mut d.layer_mut(l).unwrap().content else { panic!() };
        let ObjectKind::Pixels(px) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut px.image).set_pixel(300, 0, pixel_from_f32([0.0, 0.0, 0.0, 1.0]));

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

    #[test]
    fn a_lone_opaque_layer_s_tiles_are_shown_without_copies() {
        let mut ids = IdGen::default();
        let mut d = Document::new(600, 300);
        let l = add(&mut d, &mut ids, solid(IRect::from_size(600, 300), [0.2, 0.4, 0.6, 1.0]));
        let mut p = Projection::new();
        let shown = p.update(&d.layers, 0, &IRect::from_size(600, 300));
        let src = d.layer(l).unwrap().objects()[0].image().clone();
        for (c, t) in shown {
            assert!(Arc::ptr_eq(&t, src.tile(c).unwrap()), "tile {c:?} was copied");
        }
    }

    /// The pixel object of layer `l`, for changing.
    fn pixels_mut(d: &mut Document, l: LayerId) -> &mut PixelObject {
        let LayerContent::Objects(objects) = &mut d.layer_mut(l).unwrap().content else { panic!() };
        let ObjectKind::Pixels(p) = &mut objects[0].kind else { panic!() };
        p
    }

    #[test]
    fn pixel_objects_move_by_whole_pixels_off_the_tile_grid() {
        let mut ids = IdGen::default();
        let mut d = Document::new(1000, 1000);
        let mut img = RasterImage::new();
        img.set_pixel(0, 0, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        img.set_pixel(255, 255, pixel_from_f32([0.0, 1.0, 0.0, 1.0]));
        let l = add(&mut d, &mut ids, img);
        let mut p = Projection::new();
        let view = IRect::new(-1000, -1000, 1000, 1000);
        for offset in [[300, 7], [-1, -1], [512, -256], [-300, 600]] {
            pixels_mut(&mut d, l).offset = offset;
            let shown: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view).into_iter().collect();
            let at = |x: i64, y: i64| {
                let c = TileCoord::containing(x, y);
                let r = c.rect(0);
                shown.get(&c).map(|t| pixel_to_f32(t.get((x - r.x0) as u32, (y - r.y0) as u32)))
            };
            let [ox, oy] = offset;
            assert_eq!(at(ox, oy), Some([1.0, 0.0, 0.0, 1.0]), "offset {offset:?}");
            assert_eq!(at(ox + 255, oy + 255), Some([0.0, 1.0, 0.0, 1.0]), "offset {offset:?}");
            assert_eq!(
                flatten(&d.layers, &IRect::new(ox, oy, ox + 1, oy + 1)),
                vec![pixel_from_f32([1.0, 0.0, 0.0, 1.0])]
            );
        }
        // On the tile grid, the stored tile itself is shown.
        pixels_mut(&mut d, l).offset = [256, 512];
        let shown = p.update(&d.layers, 0, &view);
        let stored = d.layer(l).unwrap().objects()[0].image().tile(TileCoord::new(0, 0)).unwrap().clone();
        assert_eq!(shown.len(), 1);
        assert!(Arc::ptr_eq(&shown[0].1, &stored));
    }

    #[test]
    fn image_objects_composite_through_their_transform_and_invalidate_when_it_changes() {
        use crate::document::{ImageObject, Object};
        use crate::geom::Affine;
        use crate::resample::{Method, Sampling};
        let mut ids = IdGen::default();
        let mut d = Document::new(2000, 2000);
        add(&mut d, &mut ids, solid(IRect::new(1500, 1500, 1600, 1600), [0.0, 0.0, 1.0, 1.0]));
        let img = solid(IRect::from_size(10, 10), [1.0, 0.0, 0.0, 1.0]);
        let nearest = Sampling { method: Method::Nearest, anti_ringing: false };
        let image = ImageObject {
            image: Arc::new(img),
            frame: IRect::from_size(10, 10),
            transform: Affine::scale(30.0, 30.0).then(Affine::translate(100.0, 100.0)),
            sampling: nearest,
        };
        let top = ids.layer();
        let layer =
            Layer::with_objects(top, "image", vec![Object { id: ids.object(), kind: ObjectKind::Image(image) }]);
        d.insert(Position { parent: None, index: 1 }, layer).unwrap();
        let view = IRect::from_size(2000, 2000);
        let mut p = Projection::new();
        let before: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view).into_iter().collect();
        let px = flatten(&d.layers, &IRect::new(399, 100, 401, 101));
        assert_eq!(
            px,
            vec![pixel_from_f32([1.0, 0.0, 0.0, 1.0]), [half::f16::ZERO; 4]],
            "300 pixels wide from x = 100"
        );

        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = Affine::scale(30.0, 30.0).then(Affine::translate(110.0, 100.0));
        let after: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view).into_iter().collect();
        let blue = TileCoord::containing(1500, 1500);
        assert!(Arc::ptr_eq(&before[&blue], &after[&blue]), "a tile the image does not touch is kept");
        let moved = TileCoord::containing(100, 100);
        assert!(!Arc::ptr_eq(&before[&moved], &after[&moved]), "a tile it touches is redrawn");
        let px = flatten(&d.layers, &IRect::new(409, 100, 411, 101));
        assert_eq!(px, vec![pixel_from_f32([1.0, 0.0, 0.0, 1.0]), [half::f16::ZERO; 4]]);
    }

    #[test]
    fn occupied_tiles_cover_everything_a_rotated_image_draws() {
        use crate::document::{ImageObject, Object};
        use crate::geom::Affine;
        use crate::resample::Sampling;
        let img = solid(IRect::from_size(700, 300), [0.5, 0.5, 0.5, 1.0]);
        let image = ImageObject {
            image: Arc::new(img),
            frame: IRect::from_size(700, 300),
            transform: Affine::rotate(0.6).then(Affine::scale(1.3, 1.3)).then(Affine::translate(-200.5, 40.25)),
            sampling: Sampling::default(),
        };
        let expected = crate::resample::render_image(&image.image, &image.transform, &image.sampling, None);
        let mut ids = IdGen::default();
        let layer = Layer::with_objects(
            ids.layer(),
            "image",
            vec![Object { id: ids.object(), kind: ObjectKind::Image(image) }],
        );
        let layers = vec![Arc::new(layer)];
        let got = composite_all(&layers);
        assert_eq!(got.tile_count(), expected.tile_count());
        for (c, t) in expected.tiles() {
            assert_eq!(got.tile(c).map(|g| g.pixels() == t.pixels()), Some(true), "tile {c:?}");
        }
    }

    #[test]
    fn point_samples_match_the_composite() {
        let mut ids = IdGen::default();
        let mut d = Document::new(4, 4);
        add(&mut d, &mut ids, solid(IRect::from_size(4, 4), [1.0, 0.0, 0.0, 1.0]));
        let top = add(&mut d, &mut ids, solid(IRect::from_size(2, 4), [0.0, 0.0, 0.5, 0.5]));
        d.layer_mut(top).unwrap().opacity = 0.5;
        let flat = flatten(&d.layers, &IRect::from_size(4, 1));
        for (x, p) in flat.iter().enumerate() {
            assert!(close(point(&d.layers, x as f64 + 0.5, 0.5), pixel_to_f32(*p)));
        }
    }

    #[test]
    fn layers_at_lists_what_shows_topmost_first() {
        let mut ids = IdGen::default();
        let mut d = Document::new(10, 10);
        let bottom = add(&mut d, &mut ids, solid(IRect::from_size(10, 10), [1.0; 4]));
        let top = add(&mut d, &mut ids, solid(IRect::from_size(5, 5), [0.0, 0.0, 0.5, 0.5]));
        d.layer_mut(top).unwrap().opacity = 0.5;
        let at = layers_at(&d.layers, 2, 2);
        assert_eq!(at.iter().map(|p| p.0).collect::<Vec<_>>(), vec![top, bottom]);
        assert!((at[0].1 - 0.25).abs() < 1e-3, "half alpha at half opacity: {}", at[0].1);
        assert_eq!(layers_at(&d.layers, 7, 7).iter().map(|p| p.0).collect::<Vec<_>>(), vec![bottom]);
        d.layer_mut(bottom).unwrap().visible = false;
        assert!(layers_at(&d.layers, 7, 7).is_empty());
    }

    #[test]
    fn identical_children_share_one_reduced_tile() {
        // A uniform image: every full tile is the same Arc, and so is every reduction of it.
        let mut ids = IdGen::default();
        let mut d = Document::new(2048, 2048);
        let full = Arc::new(Tile::filled(pixel_from_f32([1.0; 4])));
        let mut img = RasterImage::new();
        for c in TileCoord::covering(&IRect::from_size(2048, 2048), 0) {
            img.set_tile(c, Some(full.clone()));
        }
        add(&mut d, &mut ids, img);
        let mut p = Projection::new();
        let shown = p.update(&d.layers, 1, &IRect::from_size(2048, 2048));
        assert_eq!(shown.len(), 16);
        assert!(shown.iter().all(|(_, t)| Arc::ptr_eq(t, &shown[0].1)), "each reduced tile was computed separately");
    }
}
