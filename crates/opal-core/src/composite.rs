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

use crate::document::{Layer, LayerContent, Object, ObjectKind, PixelObject};
use crate::geom::{Affine, FRect, IRect};
use crate::raster::RasterImage;
use crate::resample::{self, Sampling};
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
            if !touches(&i.image, &i.transform, &i.sampling, c) {
                return None;
            }
            resample::render_tile(&i.image, &i.transform, &i.sampling, c).map(Shown::Drawn)
        }
    }
}

/// Whether `image`, drawn through `transform` with `sampling`, can draw anything into tile
/// `c`: whether the tile, carried back into the image's pixels, comes within the filter's
/// reach of its content.
fn touches(image: &RasterImage, transform: &Affine, sampling: &Sampling, c: TileCoord) -> bool {
    let (Some(content), Some(inv)) = (image.content_bounds(), transform.inverse()) else { return false };
    let back = inv.bounds(&c.rect(0).to_f());
    let r = resample::reach(transform, sampling);
    let content =
        FRect::new(content.x0 as f64 - r, content.y0 as f64 - r, content.x1 as f64 + r, content.y1 as f64 + r);
    back.x0 < content.x1 && back.x1 > content.x0 && back.y0 < content.y1 && back.y1 > content.y0
}

/// Object `o` placed at pyramid level `level`: its pixels, carried by the transform into
/// that level's pixels (a level-`level` tile is then drawn like a level-0 one), and how it
/// is drawn there: pixel objects with Bilinear (exact on whole level pixels), image objects
/// as they ask (during a drag, the faster method if the preference says so).
fn at_level(o: &Object, level: u32) -> (&Arc<RasterImage>, Affine, Sampling) {
    let down = Affine::scale(0.5f64.powi(level as i32), 0.5f64.powi(level as i32));
    match &o.kind {
        ObjectKind::Pixels(p) => {
            (&p.image, Affine::translate(p.offset[0] as f64, p.offset[1] as f64).then(down), Sampling::DRAFT)
        }
        ObjectKind::Image(i) => (&i.image, i.transform.then(down), i.sampling),
    }
}

/// Reduced copies of pixel objects' tiles, made while previewing (see [`Quality::Preview`])
/// for the tiles being previewed alone, so their cost and memory follow the screen; cleared
/// by the next exact update. Keyed by image (its address; `held` keeps the image alive, so
/// the address cannot be reused), level, and tile at that level.
#[derive(Default, Debug)]
struct Reduced {
    tiles: HashMap<(usize, u32, TileCoord), Option<Arc<Tile>>>,
    held: HashMap<usize, Arc<RasterImage>>,
    /// For each image, by level: the tiles with some of the image under them.
    occupied: HashMap<usize, Vec<HashSet<TileCoord>>>,
}

/// Tile `c` of `image` reduced to `level`, from `tiles` (a [`Reduced`]'s), if it was made
/// and has content.
fn reduced_tile<'a>(
    tiles: &'a HashMap<(usize, u32, TileCoord), Option<Arc<Tile>>>,
    image: &'a Arc<RasterImage>,
    level: u32,
    c: TileCoord,
) -> Option<&'a Arc<Tile>> {
    if level == 0 {
        return image.tile(c);
    }
    tiles.get(&(Arc::as_ptr(image) as usize, level, c))?.as_ref()
}

impl Reduced {
    fn get<'a>(&'a self, image: &'a Arc<RasterImage>, level: u32, c: TileCoord) -> Option<&'a Arc<Tile>> {
        reduced_tile(&self.tiles, image, level, c)
    }

    fn has(&self, image: &Arc<RasterImage>, level: u32, c: TileCoord) -> bool {
        level == 0 || self.tiles.contains_key(&(Arc::as_ptr(image) as usize, level, c))
    }

    /// Makes the level-`level` tiles `need` of `image`, and the finer ones they are made
    /// from, each from the four below it. Only tiles with some of the image under them are
    /// made; the rest of `need` is recorded as empty.
    fn make(&mut self, image: &Arc<RasterImage>, level: u32, need: HashSet<TileCoord>) {
        let id = Arc::as_ptr(image) as usize;
        let Reduced { tiles, held, occupied } = self;
        held.entry(id).or_insert_with(|| image.clone());
        let occupied = occupied.entry(id).or_insert_with(|| vec![image.tiles().map(|(c, _)| c).collect()]);
        while occupied.len() <= level as usize {
            let up = occupied.last().expect("level 0").iter().map(|c| c.parent()).collect();
            occupied.push(up);
        }
        // What each level needs, from the top down.
        let (full, empty): (HashSet<TileCoord>, HashSet<TileCoord>) =
            need.into_iter().partition(|c| occupied[level as usize].contains(c));
        tiles.extend(empty.into_iter().map(|c| ((id, level, c), None)));
        let mut wanted = vec![HashSet::new(); level as usize + 1];
        wanted[level as usize] = full;
        for l in (2..=level).rev() {
            let below: HashSet<TileCoord> = wanted[l as usize]
                .iter()
                .flat_map(|c| c.children())
                .filter(|k| occupied[l as usize - 1].contains(k) && !tiles.contains_key(&(id, l - 1, *k)))
                .collect();
            wanted[l as usize - 1] = below;
        }
        // Made from the bottom up, each level in parallel.
        for l in 1..=level {
            let made: Vec<(TileCoord, Option<Arc<Tile>>)> = std::mem::take(&mut wanted[l as usize])
                .into_par_iter()
                .map(|c| {
                    let kids = c.children().map(|k| reduced_tile(tiles, image, l - 1, k).map(|t| &**t));
                    (c, downsample(kids).map(Arc::new))
                })
                .collect();
            tiles.extend(made.into_iter().map(|(c, t)| ((id, l, c), t)));
        }
    }
}

/// The tiles of pixel object `p`'s image, reduced to level `level`, that tile `c` of that
/// level shows (with a margin for filtering when `p` lies between whole level pixels).
fn reduced_area(p: &PixelObject, level: u32, c: TileCoord) -> IRect {
    let s = 0.5f64.powi(level as i32);
    let r = c.rect(0).to_f();
    let shifted = FRect::new(
        r.x0 - p.offset[0] as f64 * s,
        r.y0 - p.offset[1] as f64 * s,
        r.x1 - p.offset[0] as f64 * s,
        r.y1 - p.offset[1] as f64 * s,
    );
    IRect::covering(&shifted).expand(2)
}

/// What object `o` shows in tile `c` of pyramid level `level`, drawn there directly: a
/// pixel object from its reduced tiles in `reduced` (made beforehand), an image object
/// through its transform.
fn level_tile<'o>(o: &'o Object, c: TileCoord, level: u32, reduced: &Reduced) -> Option<Shown<'o>> {
    let (image, t, s) = at_level(o, level);
    if !touches(image, &t, &s, c) {
        return None;
    }
    match &o.kind {
        ObjectKind::Pixels(p) => {
            let mut local = RasterImage::new();
            for rc in TileCoord::covering(&reduced_area(p, level, c), 0) {
                if let Some(t) = reduced.get(&p.image, level, rc) {
                    local.set_tile(rc, Some(t.clone()));
                }
            }
            let k = 0.5f64.powi(level as i32);
            let shift = Affine::translate(p.offset[0] as f64 * k, p.offset[1] as f64 * k);
            resample::render_tile(&local, &shift, &s, c).map(Shown::Drawn)
        }
        ObjectKind::Image(_) => resample::render_tile(image, &t, &s, c).map(Shown::Drawn),
    }
}

/// [`level_tile`] at `level`, as a [`ShowFn`] takes it.
fn shown_at(level: u32, reduced: &Reduced) -> impl for<'o> Fn(&'o Object, TileCoord) -> Option<Shown<'o>> + Sync + '_ {
    move |o, c| level_tile(o, c, level, reduced)
}

/// The token for what object `o` shows in tile `c` of pyramid level `level`.
fn level_token(o: &Object, c: TileCoord, level: u32) -> Option<Token> {
    let (image, t, s) = at_level(o, level);
    touches(image, &t, &s, c).then(|| Token::Image { image: image.clone(), transform: t.to_bits(), sampling: s.key() })
}

/// What an object shows in a tile: at full resolution, or drawn at a pyramid level.
type ShowFn<'f> = &'f (dyn for<'o> Fn(&'o Object, TileCoord) -> Option<Shown<'o>> + Sync);
/// The key token of what an object shows in a tile.
type TokenFn<'f> = &'f (dyn Fn(&Object, TileCoord) -> Option<Token> + Sync);

/// Composites the visible layers' content at tile `c` onto `acc`, each object as `draw`
/// draws it. Returns whether anything was drawn.
fn composite_into(layers: &[Arc<Layer>], c: TileCoord, acc: &mut [PixelF32], draw: ShowFn) -> bool {
    let mut any = false;
    let mut scratch: Option<Vec<PixelF32>> = None;
    for layer in layers {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        match &layer.content {
            LayerContent::Objects(objects) => {
                let shown: Vec<Shown> = objects.iter().filter_map(|o| draw(o, c)).collect();
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
                if composite_into(children, c, &mut group, draw) {
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
    composite_with(layers, c, &object_tile)
}

/// The composite of `layers` at tile `c`, each object as `shown` draws it.
fn composite_with(layers: &[Arc<Layer>], c: TileCoord, shown: ShowFn) -> Option<Tile> {
    let mut acc = vec![[0.0f32; 4]; TILE_PIXELS];
    if !composite_into(layers, c, &mut acc, shown) {
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

/// Builds what drawing the visible image objects that reach into `range` (anywhere, for
/// `None`) will need (their pyramids), so tiles drawn in parallel find it ready instead of
/// each building it.
fn prepare(layers: &[Arc<Layer>], range: Option<&IRect>) {
    for_each_visible_object(layers, &mut |o| {
        if let ObjectKind::Image(i) = &o.kind {
            let reaches = match range {
                None => true,
                Some(r) => o.extent().is_some_and(|e| !e.intersect(r).is_empty()),
            };
            if reaches {
                resample::prepare(&i.image, &i.transform, &i.sampling);
            }
        }
    });
}

/// Every level-0 tile coordinate that visible content of `layers` occupies within `range`
/// (all of them when `range` is `None`).
pub fn occupied_tiles(layers: &[Arc<Layer>], range: Option<&IRect>) -> HashSet<TileCoord> {
    let mut out = HashSet::new();
    each_occupied_rect(layers, &mut |r, _| {
        let r = match range {
            Some(range) => r.intersect(range),
            None => r,
        };
        if !r.is_empty() {
            out.extend(TileCoord::covering(&r, 0));
        }
    });
    out
}

/// Calls `f` with rectangles (canvas pixels) that together cover what visible content of
/// `layers` draws, one per tile of each object, and whether it is a pixel object's.
fn each_occupied_rect(layers: &[Arc<Layer>], f: &mut dyn FnMut(IRect, bool)) {
    for_each_visible_object(layers, &mut |o| match &o.kind {
        ObjectKind::Pixels(p) => {
            for (c, _) in p.image.tiles() {
                f(c.rect(0).translate(p.offset[0], p.offset[1]), true);
            }
        }
        ObjectKind::Image(i) => {
            let r = resample::reach(&i.transform, &i.sampling);
            for (c, _) in i.image.tiles() {
                let t = c.rect(0);
                let grown = FRect::new(t.x0 as f64 - r, t.y0 as f64 - r, t.x1 as f64 + r, t.y1 as f64 + r);
                f(IRect::covering(&i.transform.bounds(&grown)), false);
            }
        }
    });
}

/// Where the content of a set of layers lies, worked out once for them and asked of each
/// area shown: the tiles pixel objects occupy, and the rectangles image objects cover
/// (which, transformed, may span more tiles than are worth listing).
#[derive(Debug, Default)]
struct Occupancy {
    tiles: HashSet<TileCoord>,
    rects: Vec<IRect>,
}

impl Occupancy {
    fn of(layers: &[Arc<Layer>]) -> Occupancy {
        let mut o = Occupancy::default();
        each_occupied_rect(layers, &mut |r, pixels| {
            if pixels {
                o.tiles.extend(TileCoord::covering(&r, 0));
            } else {
                o.rects.push(r);
            }
        });
        o
    }

    /// The level-0 tiles occupied within `range`.
    fn within(&self, range: &IRect) -> HashSet<TileCoord> {
        let mut out: HashSet<TileCoord> =
            self.tiles.iter().filter(|c| !c.rect(0).intersect(range).is_empty()).copied().collect();
        for r in &self.rects {
            let r = r.intersect(range);
            if !r.is_empty() {
                out.extend(TileCoord::covering(&r, 0));
            }
        }
        out
    }
}

/// Composites every tile `layers` occupy within `range` (everywhere when `None`) into one
/// bitmap, in parallel.
fn composite_region(layers: &[Arc<Layer>], range: Option<&IRect>) -> RasterImage {
    prepare(layers, range);
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
        ObjectKind::Image(i) => touches(&i.image, &i.transform, &i.sampling, c).then(|| Token::Image {
            image: i.image.clone(),
            transform: i.transform.to_bits(),
            sampling: i.sampling.key(),
        }),
    }
}

/// The exact key of level-0 tile `c`.
fn base_key(layers: &[Arc<Layer>], c: TileCoord) -> Key {
    let mut tokens = Vec::new();
    key_into(layers, c, &mut tokens, &object_token);
    Key::Base(tokens)
}

/// Appends the key of `layers` at `c` to `out`, each object's token from `token`. Returns
/// whether anything contributes.
fn key_into(layers: &[Arc<Layer>], c: TileCoord, out: &mut Vec<Token>, token: TokenFn) -> bool {
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
                out.extend(objects.iter().filter_map(|o| token(o, c)));
                out.len() > before
            }
            LayerContent::Group(children) => {
                out.push(Token::Group(layer.opacity.to_bits()));
                key_into(children, c, out, token)
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
    /// A tile drawn at its level directly (see [`Quality::Preview`]): what went into it,
    /// as for a level-0 tile. It never equals an exact key, so the next exact update
    /// replaces the tile.
    Preview(Vec<Token>),
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        match (self, other) {
            (Key::Base(a), Key::Base(b)) | (Key::Preview(a), Key::Preview(b)) => a == b,
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
    /// For a level-0 tile: the generation of layers its key was last found current for
    /// (see [`Projection::generation`]).
    checked: u64,
}

/// The composited image, cached per tile, with a pyramid of reduced levels: a level-`k`
/// tile covers `256 << k` canvas pixels each way at `1 / 2^k` of full resolution.
#[derive(Default, Debug)]
pub struct Projection {
    levels: Vec<HashMap<TileCoord, Entry>>,
    reduced: Reduced,
    last: Option<LastUpdate>,
    /// Counts the sets of layers updated against: it moves on whenever they differ from
    /// the last ones, so a level-0 entry checked in this generation is current without
    /// building its key again (panning then costs only the tiles newly in view).
    generation: u64,
    /// The layers of this generation, held so that a replaced layer cannot reuse an
    /// address, and where their content lies.
    layers: Vec<Arc<Layer>>,
    occupancy: Occupancy,
    /// How many level-0 keys have been built (for tests).
    keys_built: usize,
    /// How many bytes of tiles the cache may hold (see [`set_budget`](Self::set_budget)).
    budget: Option<u64>,
}

/// The bytes a cached tile's pixels take.
const TILE_BYTES: u64 = (TILE_PIXELS * std::mem::size_of::<Pixel>()) as u64;

/// How [`Projection::update`] brings tiles up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// At full resolution, then reduced: exactly what the image is.
    Exact,
    /// While something is dragged: tiles out of date at the level shown (above 0) are drawn
    /// at that level directly, from each object's reduced copy, so their cost follows the
    /// screen rather than the image; tiles still up to date stay exact. The next exact
    /// update recomputes what was previewed.
    Preview,
}

/// What the last [`Projection::update`] was asked and answered, so that asking the same again
/// (an unchanged document in an unchanged view) costs nothing.
#[derive(Debug)]
struct LastUpdate {
    /// The top-level layers, held so a replaced layer cannot reuse an old address.
    layers: Vec<Arc<Layer>>,
    level: u32,
    rect: IRect,
    quality: Quality,
    result: Vec<(TileCoord, Arc<Tile>)>,
}

/// The highest pyramid level kept (a tile then covers 256 × 2^16 pixels).
pub const MAX_LEVEL: u32 = 16;

impl Projection {
    pub fn new() -> Projection {
        Projection::default()
    }

    /// Limits the tiles the cache holds to `bytes`, beyond those the view shown needs: past
    /// it, an update drops the tiles farthest from the view first. `None` for no limit.
    pub fn set_budget(&mut self, bytes: Option<u64>) {
        self.budget = bytes;
    }

    /// Whether the last update drew what changed as a preview (see [`Quality::Preview`]).
    pub fn shows_previews(&self) -> bool {
        self.last.as_ref().is_some_and(|l| l.quality == Quality::Preview)
    }

    /// Brings the tiles of pyramid level `level` that intersect `rect` (canvas pixels) up to
    /// date with `layers`, computing any that are stale in parallel, and returns those that
    /// have content.
    pub fn update(
        &mut self,
        layers: &[Arc<Layer>],
        level: u32,
        rect: &IRect,
        quality: Quality,
    ) -> Vec<(TileCoord, Arc<Tile>)> {
        let level = level.min(MAX_LEVEL);
        let quality = if level == 0 { Quality::Exact } else { quality };
        // Any change below the top level replaces the top-level layer holding it.
        if let Some(last) = &self.last
            && last.level == level
            && last.rect == *rect
            && last.quality == quality
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
        if self.layers.len() != layers.len() || !self.layers.iter().zip(layers).all(|(a, b)| Arc::ptr_eq(a, b)) {
            self.generation += 1;
            self.layers = layers.to_vec();
            self.occupancy = Occupancy::of(layers);
        }
        let base = self.occupancy.within(&outer);
        let result = match quality {
            Quality::Exact => {
                self.reduced = Reduced::default();
                prepare(layers, Some(&outer));
                self.update_base(layers, &base, &outer);
                let mut coords = base;
                for l in 1..=level {
                    coords = coords.iter().map(|c| c.parent()).collect();
                    self.update_pyramid(l, &coords, &outer, &HashSet::new());
                }
                self.shown(level, coords.iter())
            }
            Quality::Preview => self.update_preview(layers, level, &outer, base),
        };
        self.trim(level, &outer);
        self.last = Some(LastUpdate { layers: layers.to_vec(), level, rect: *rect, quality, result: result.clone() });
        result
    }

    /// [`Quality::Preview`]: the level-`level` tiles over `outer` with anything out of date
    /// under them are drawn at that level; the others come from the exact pyramid, which is
    /// brought up to date for them alone (their level-0 tiles are current, so that costs
    /// little). `base` holds the level-0 tiles content occupies.
    fn update_preview(
        &mut self,
        layers: &[Arc<Layer>],
        level: u32,
        outer: &IRect,
        base: HashSet<TileCoord>,
    ) -> Vec<(TileCoord, Arc<Tile>)> {
        let up = |mut c: TileCoord| {
            for _ in 0..level {
                c = c.parent();
            }
            c
        };
        // The level-`level` tiles with a level-0 tile under them out of date, or emptied.
        let generation = self.generation;
        let mut dirty: HashSet<TileCoord> = HashSet::new();
        for &c in &base {
            if self.check_base(layers, c).is_some() {
                dirty.insert(up(c));
            }
        }
        let cache = &self.levels[0];
        for c in cache.keys() {
            if !base.contains(c) && !c.rect(0).intersect(outer).is_empty() {
                dirty.insert(up(*c));
            }
        }
        // The exact pyramid for the rest; previews already drawn at the level are kept.
        let mut coords: HashSet<TileCoord> = base.into_iter().filter(|&c| !dirty.contains(&up(c))).collect();
        for l in 1..=level {
            coords = coords.iter().map(|c| c.parent()).collect();
            let keep = if l == level { dirty.clone() } else { HashSet::new() };
            self.update_pyramid(l, &coords, outer, &keep);
        }
        // The dirty ones whose key changed since they were last drawn, drawn at the level.
        let token = |o: &Object, c: TileCoord| level_token(o, c, level);
        let top = &self.levels[level as usize];
        let mut stale = Vec::new();
        for &c in &dirty {
            let mut tokens = Vec::new();
            key_into(layers, c, &mut tokens, &token);
            let key = Key::Preview(tokens);
            if top.get(&c).is_none_or(|e| e.key != key) {
                stale.push((c, key));
            }
        }
        // What drawing them needs, made first: image objects' pyramids, and the reduced
        // tiles of pixel objects under them alone.
        let mut need: Vec<(Arc<RasterImage>, HashSet<TileCoord>)> = Vec::new();
        for_each_visible_object(layers, &mut |o| {
            if o.extent().is_none_or(|e| e.intersect(outer).is_empty()) {
                return;
            }
            match &o.kind {
                ObjectKind::Image(_) => {
                    let (image, t, s) = at_level(o, level);
                    resample::prepare(image, &t, &s);
                }
                ObjectKind::Pixels(p) => {
                    let (image, t, s) = at_level(o, level);
                    let tiles: HashSet<TileCoord> = stale
                        .iter()
                        .filter(|(c, _)| touches(image, &t, &s, *c))
                        .flat_map(|(c, _)| TileCoord::covering(&reduced_area(p, level, *c), 0))
                        .filter(|&rc| !self.reduced.has(&p.image, level, rc))
                        .collect();
                    if !tiles.is_empty() {
                        need.push((p.image.clone(), tiles));
                    }
                }
            }
        });
        for (image, tiles) in need {
            self.reduced.make(&image, level, tiles);
        }
        let show = shown_at(level, &self.reduced);
        let drawn: Vec<(TileCoord, Entry)> = stale
            .into_par_iter()
            .map(|(c, key)| {
                let tile = composite_with(layers, c, &show).map(Arc::new);
                (c, Entry { key, tile, checked: generation })
            })
            .collect();
        self.levels[level as usize].extend(drawn);
        self.shown(level, coords.iter().chain(&dirty))
    }

    /// Drops cached tiles while they take more than the budget, farthest from `outer` first,
    /// keeping every tile up to level `level` within it (what the view drawn needs).
    fn trim(&mut self, level: u32, outer: &IRect) {
        let Some(budget) = self.budget else { return };
        // Cheap to know, and at least what the tiles take.
        let entries: usize = self.levels.iter().map(HashMap::len).sum();
        if entries as u64 * TILE_BYTES <= budget {
            return;
        }
        let gap = |r: &IRect| {
            let dx = (outer.x0 - r.x1).max(r.x0 - outer.x1).max(0);
            let dy = (outer.y0 - r.y1).max(r.y0 - outer.y1).max(0);
            dx + dy
        };
        let mut bytes = 0;
        let mut away: Vec<(i64, usize, TileCoord)> = Vec::new();
        for (l, cache) in self.levels.iter().enumerate() {
            for (c, e) in cache {
                if e.tile.is_some() {
                    bytes += TILE_BYTES;
                }
                let r = c.rect(l as u32);
                if l as u32 > level || r.intersect(outer).is_empty() {
                    away.push((gap(&r), l, *c));
                }
            }
        }
        away.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
        for (_, l, c) in away {
            if bytes <= budget {
                break;
            }
            if self.levels[l].remove(&c).is_some_and(|e| e.tile.is_some()) {
                bytes -= TILE_BYTES;
            }
        }
    }

    /// The tiles of level `level` at `coords` that have content.
    fn shown<'a>(&self, level: u32, coords: impl Iterator<Item = &'a TileCoord>) -> Vec<(TileCoord, Arc<Tile>)> {
        let top = &self.levels[level as usize];
        coords.filter_map(|c| top.get(c).and_then(|e| e.tile.clone()).map(|t| (*c, t))).collect()
    }

    /// The cached tile at `level` and `c` as of the last [`update`](Self::update) that
    /// covered it.
    pub fn tile(&self, level: u32, c: TileCoord) -> Option<&Arc<Tile>> {
        self.levels.get(level as usize)?.get(&c)?.tile.as_ref()
    }

    /// Checks the cached level-0 tile `c` against `layers` (this generation's): `None` if it
    /// is current (marked so, if its key had to be built), or the key it needs now.
    fn check_base(&mut self, layers: &[Arc<Layer>], c: TileCoord) -> Option<Key> {
        let generation = self.generation;
        let entry = self.levels[0].get_mut(&c);
        if entry.as_ref().is_some_and(|e| e.checked == generation) {
            return None;
        }
        self.keys_built += 1;
        let key = base_key(layers, c);
        match entry {
            Some(e) if e.key == key => {
                e.checked = generation;
                None
            }
            _ => Some(key),
        }
    }

    fn update_base(&mut self, layers: &[Arc<Layer>], coords: &HashSet<TileCoord>, outer: &IRect) {
        let generation = self.generation;
        // Tiles inside the area that nothing occupies any more.
        self.levels[0].retain(|c, _| coords.contains(c) || c.rect(0).intersect(outer).is_empty());
        let stale: Vec<(TileCoord, Key)> =
            coords.iter().filter_map(|&c| self.check_base(layers, c).map(|key| (c, key))).collect();
        let cache = &mut self.levels[0];
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
                (c, Entry { key, tile, checked: generation })
            })
            .collect();
        cache.extend(done);
    }

    /// Brings the level-`level` tiles at `coords` up to date from the level below, dropping
    /// other cached tiles within `outer` except those in `keep`.
    fn update_pyramid(&mut self, level: u32, coords: &HashSet<TileCoord>, outer: &IRect, keep: &HashSet<TileCoord>) {
        let (below, rest) = self.levels.split_at_mut(level as usize);
        let below = &below[level as usize - 1];
        let cache = &mut rest[0];
        cache.retain(|c, _| coords.contains(c) || keep.contains(c) || c.rect(level).intersect(outer).is_empty());
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
            cache.insert(c, Entry { tile, key, checked: 0 });
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
        let first = p.update(&d.layers, 0, &view, Quality::Exact);
        assert_eq!(first.len(), 3);
        let before: HashMap<TileCoord, Arc<Tile>> = first.into_iter().collect();

        // Change one pixel in the middle tile.
        let LayerContent::Objects(objects) = &mut d.layer_mut(l).unwrap().content else { panic!() };
        let ObjectKind::Pixels(px) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut px.image).set_pixel(300, 0, pixel_from_f32([0.0, 0.0, 0.0, 1.0]));

        let after: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view, Quality::Exact).into_iter().collect();
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
        assert_eq!(p.update(&d.layers, 0, &view, Quality::Exact).len(), 1);
        d.layer_mut(l).unwrap().opacity = 0.5;
        let t = p.update(&d.layers, 0, &view, Quality::Exact);
        assert!(close(pixel_to_f32(t[0].1.get(0, 0)), [0.5; 4]));
        d.remove(l).unwrap();
        assert!(p.update(&d.layers, 0, &view, Quality::Exact).is_empty());
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
        let tiles = p.update(&d.layers, 1, &IRect::from_size(512, 512), Quality::Exact);
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
        let shown = p.update(&d.layers, 0, &IRect::from_size(600, 300), Quality::Exact);
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
            let shown: HashMap<TileCoord, Arc<Tile>> =
                p.update(&d.layers, 0, &view, Quality::Exact).into_iter().collect();
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
        let shown = p.update(&d.layers, 0, &view, Quality::Exact);
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
        let before: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view, Quality::Exact).into_iter().collect();
        let px = flatten(&d.layers, &IRect::new(399, 100, 401, 101));
        assert_eq!(
            px,
            vec![pixel_from_f32([1.0, 0.0, 0.0, 1.0]), [half::f16::ZERO; 4]],
            "300 pixels wide from x = 100"
        );

        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = Affine::scale(30.0, 30.0).then(Affine::translate(110.0, 100.0));
        let after: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &view, Quality::Exact).into_iter().collect();
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
    fn a_preview_draws_only_what_changed_at_the_level_shown() {
        use crate::document::{ImageObject, Object};
        use crate::geom::Affine;
        use crate::resample::Sampling;
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 4096);
        add(&mut d, &mut ids, solid(IRect::from_size(4096, 4096), [0.2, 0.4, 0.6, 1.0]));
        let image = ImageObject {
            image: Arc::new(solid(IRect::from_size(300, 300), [1.0, 0.0, 0.0, 1.0])),
            frame: IRect::from_size(300, 300),
            transform: Affine::translate(100.0, 100.0),
            sampling: Sampling::default(),
        };
        let top = ids.layer();
        let layer =
            Layer::with_objects(top, "pasted", vec![Object { id: ids.object(), kind: ObjectKind::Image(image) }]);
        d.insert(Position { parent: None, index: 1 }, layer).unwrap();
        let view = IRect::from_size(4096, 4096);
        let mut p = Projection::new();
        let exact: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 2, &view, Quality::Exact).into_iter().collect();

        // Drag the image far across: only the level-2 tiles it left or reached are redrawn.
        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = Affine::translate(2500.5, 2500.25);
        let before0 = p.tile(0, TileCoord::containing(2600, 2600)).cloned();
        let preview: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 2, &view, Quality::Preview).into_iter().collect();
        let far = TileCoord::new(3, 0);
        assert!(Arc::ptr_eq(&preview[&far], &exact[&far]), "an untouched tile stays exact");
        shows_what_it_returned(&p, 2, &preview);
        // The next step of the drag redraws the tile the image is in, and keeps the one it
        // left on the last step, previewed already.
        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = Affine::translate(2510.5, 2500.25);
        let changed = TileCoord::new(2, 2);
        let again: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 2, &view, Quality::Preview).into_iter().collect();
        assert_eq!(again.len(), preview.len());
        for (c, t) in &again {
            assert_eq!(Arc::ptr_eq(t, &preview[c]), *c != changed, "tile {c:?}");
        }
        shows_what_it_returned(&p, 2, &again);
        let preview = again;
        assert!(
            p.tile(0, TileCoord::containing(2600, 2600)).cloned().map(|t| Arc::as_ptr(&t))
                == before0.map(|t| Arc::as_ptr(&t)),
            "no full-resolution tile was drawn"
        );
        // The previewed tile shows the image where it now is, as the exact update will.
        let at = |tiles: &HashMap<TileCoord, Arc<Tile>>, x: i64, y: i64| {
            let c = TileCoord::new((x >> 2).div_euclid(256) as i32, (y >> 2).div_euclid(256) as i32);
            pixel_to_f32(tiles[&c].get(((x >> 2) % 256) as u32, ((y >> 2) % 256) as u32))
        };
        let exact_now: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 2, &view, Quality::Exact).into_iter().collect();
        for (x, y) in [(2650, 2650), (2400, 2650), (150, 150)] {
            assert!(
                close(at(&preview, x, y), at(&exact_now, x, y)),
                "at ({x}, {y}): {:?} vs {:?}",
                at(&preview, x, y),
                at(&exact_now, x, y)
            );
        }
    }

    /// Asserts that the tiles an update returned are the ones [`Projection::tile`] gives.
    fn shows_what_it_returned(p: &Projection, level: u32, returned: &HashMap<TileCoord, Arc<Tile>>) {
        for (c, t) in returned {
            let shown = p.tile(level, *c).unwrap_or_else(|| panic!("tile {c:?} is not shown"));
            assert!(Arc::ptr_eq(shown, t), "tile {c:?} shows something else");
        }
    }

    #[test]
    fn a_previewed_pixel_layer_reduces_only_the_tiles_shown() {
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 4096);
        add(&mut d, &mut ids, solid(IRect::from_size(4096, 4096), [0.2, 0.4, 0.6, 1.0]));
        // 64-pixel blocks, each its own colour, so a block drawn in the wrong place shows.
        let colour = |bx: i64, by: i64| [bx as f32 / 16.0, by as f32 / 16.0, 0.5, 1.0];
        let px: Vec<Pixel> =
            (0..1024 * 1024).map(|i| pixel_from_f32(colour((i % 1024) / 64, (i / 1024) / 64))).collect();
        let pattern = Arc::new(RasterImage::from_pixels(1024, 1024, &px));
        let top = ids.layer();
        let object = Object {
            id: ids.object(),
            kind: ObjectKind::Pixels(PixelObject { image: pattern.clone(), offset: [0, 0] }),
        };
        d.insert(Position { parent: None, index: 1 }, Layer::with_objects(top, "top", vec![object])).unwrap();
        let view = IRect::from_size(4096, 4096);
        let mut p = Projection::new();
        p.update(&d.layers, 2, &view, Quality::Exact);

        // Moved by a distance that is not a whole number of level-2 pixels.
        let offset = [2501i64, 2499];
        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
        o.offset = offset;
        let preview: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 2, &view, Quality::Preview).into_iter().collect();
        shows_what_it_returned(&p, 2, &preview);
        assert!(!pattern.has_reduced(1) && !pattern.has_reduced(2), "the whole layer was reduced");
        assert!(!p.reduced.tiles.is_empty());

        let at = |tiles: &HashMap<TileCoord, Arc<Tile>>, x: i64, y: i64| {
            let c = TileCoord::new((x >> 2).div_euclid(256) as i32, (y >> 2).div_euclid(256) as i32);
            pixel_to_f32(tiles[&c].get(((x >> 2) % 256) as u32, ((y >> 2) % 256) as u32))
        };
        let exact: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 2, &view, Quality::Exact).into_iter().collect();
        assert!(p.reduced.tiles.is_empty() && p.reduced.held.is_empty(), "the exact update keeps no reduced tiles");
        for (bx, by) in [(0, 0), (5, 9), (15, 15), (15, 0), (8, 3)] {
            let (x, y) = (offset[0] + bx * 64 + 32, offset[1] + by * 64 + 32);
            assert!(close(at(&preview, x, y), colour(bx, by)), "block ({bx}, {by}): {:?}", at(&preview, x, y));
            assert!(close(at(&preview, x, y), at(&exact, x, y)));
        }
        assert!(close(at(&preview, 500, 500), [0.2, 0.4, 0.6, 1.0]), "the old place shows what is under it");
    }

    #[test]
    fn a_preview_reduces_only_tiles_with_content_under_them() {
        let mut ids = IdGen::default();
        let mut d = Document::new(8192, 8192);
        // A small sticker far from a pixel layer dragged at level 3.
        let sticker = Arc::new(solid(IRect::new(7000, 7000, 7010, 7010), [1.0, 0.0, 0.0, 1.0]));
        let big = Arc::new(solid(IRect::from_size(600, 600), [0.0, 1.0, 0.0, 1.0]));
        for (name, image) in [("sticker", &sticker), ("big", &big)] {
            let object = Object {
                id: ids.object(),
                kind: ObjectKind::Pixels(PixelObject { image: image.clone(), offset: [0, 0] }),
            };
            let index = d.layers.len();
            d.insert(Position { parent: None, index }, Layer::with_objects(ids.layer(), name, vec![object])).unwrap();
        }
        let top = d.layers[1].id;
        let view = IRect::from_size(8192, 8192);
        let mut p = Projection::new();
        p.update(&d.layers, 3, &view, Quality::Exact);
        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
        o.offset = [1001, 999];
        p.update(&d.layers, 3, &view, Quality::Preview);
        let (sticker_id, big_id) = (Arc::as_ptr(&sticker) as usize, Arc::as_ptr(&big) as usize);
        assert!(p.reduced.tiles.keys().all(|k| k.0 != sticker_id), "the sticker was reduced for a drag far from it");
        assert!(p.reduced.tiles.keys().any(|k| k.0 == big_id));
        // Below the level shown, only tiles with some of the image under them were made.
        for &(id, l, c) in p.reduced.tiles.keys() {
            assert!(
                id != big_id || l == 3 || !c.rect(l).intersect(&IRect::from_size(600, 600)).is_empty(),
                "{l} {c:?}"
            );
        }
    }

    #[test]
    fn panning_builds_keys_for_the_tiles_newly_in_view_alone() {
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 1024);
        let bg = add(&mut d, &mut ids, solid(IRect::from_size(4096, 1024), [0.2, 0.4, 0.6, 1.0]));
        let mut p = Projection::new();
        // Four tiles by four, then one tile to the right: four new tiles.
        p.update(&d.layers, 0, &IRect::new(0, 0, 1024, 1024), Quality::Exact);
        assert_eq!(p.keys_built, 16);
        p.update(&d.layers, 0, &IRect::new(256, 0, 1280, 1024), Quality::Exact);
        assert_eq!(p.keys_built, 20, "panning rebuilt the keys of tiles already checked");
        // Away, a change to a tile left behind, and back: the change shows.
        p.update(&d.layers, 0, &IRect::new(3072, 0, 4096, 1024), Quality::Exact);
        let LayerContent::Objects(objects) = &mut d.layer_mut(bg).unwrap().content else { panic!() };
        let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut o.image).set_pixel(300, 10, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        p.update(&d.layers, 0, &IRect::new(256, 0, 1280, 1024), Quality::Exact);
        let t = p.tile(0, TileCoord::new(1, 0)).expect("the tile");
        assert_eq!(pixel_to_f32(t.get(44, 10)), [1.0, 0.0, 0.0, 1.0], "the change made out of view does not show");
    }

    #[test]
    fn a_previewed_image_object_keeps_its_own_sampling() {
        use crate::document::{ImageObject, Object};
        use crate::geom::Affine;
        use crate::resample::{Method, Sampling};
        let mut ids = IdGen::default();
        let mut d = Document::new(2048, 2048);
        // Stripes, which bilinear and cubic filters draw differently when shrunk.
        let px: Vec<Pixel> = (0..512 * 512)
            .map(|i| pixel_from_f32(if (i % 512) % 3 == 0 { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] }))
            .collect();
        let sampling = Sampling { method: Method::Lanczos { lobes: 3 }, anti_ringing: false };
        let image = ImageObject {
            image: Arc::new(RasterImage::from_pixels(512, 512, &px)),
            frame: IRect::from_size(512, 512),
            transform: Affine::translate(0.0, 0.0),
            sampling,
        };
        let top = ids.layer();
        let layer =
            Layer::with_objects(top, "image", vec![Object { id: ids.object(), kind: ObjectKind::Image(image) }]);
        d.insert(Position { parent: None, index: 0 }, layer).unwrap();
        let view = IRect::from_size(2048, 2048);
        let mut p = Projection::new();
        p.update(&d.layers, 2, &view, Quality::Exact);
        let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = Affine::scale(1.3, 1.3).then(Affine::translate(37.5, 11.25));
        let (image, transform) = (i.image.clone(), i.transform);
        let preview: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 2, &view, Quality::Preview).into_iter().collect();
        let c = TileCoord::new(0, 0);
        let direct = resample::render_tile(&image, &transform.then(Affine::scale(0.25, 0.25)), &sampling, c).unwrap();
        assert!(preview[&c].pixels() == direct.pixels(), "the preview did not draw with the object's sampling");
    }

    /// Times dragging a 24-megapixel image object over a 24-megapixel layer at 25%, exactly
    /// and previewed (run with `--release --ignored --nocapture`).
    #[test]
    #[ignore]
    fn time_a_large_drag() {
        use crate::document::{ImageObject, Object};
        use crate::geom::Affine;
        use crate::resample::Sampling;
        let (w, h) = (6000u32, 4000u32);
        let px: Vec<Pixel> = (0..w * h).map(|i| pixel_from_f32([(i % 251) as f32 / 251.0, 0.5, 0.25, 1.0])).collect();
        let photo = RasterImage::from_pixels(w, h, &px);
        let mut ids = IdGen::default();
        let mut d = Document::new(w, h);
        add(&mut d, &mut ids, photo.clone());
        let image = ImageObject {
            image: Arc::new(photo),
            frame: IRect::from_size(6000, 4000),
            transform: Affine::IDENTITY,
            sampling: Sampling::DRAFT,
        };
        let top = ids.layer();
        let layer =
            Layer::with_objects(top, "pasted", vec![Object { id: ids.object(), kind: ObjectKind::Image(image) }]);
        d.insert(Position { parent: None, index: 1 }, layer).unwrap();
        let view = IRect::from_size(6000, 4000);
        let mut p = Projection::new();
        p.update(&d.layers, 2, &view, Quality::Exact);
        for quality in [Quality::Exact, Quality::Preview] {
            let mut total = std::time::Duration::ZERO;
            for step in 1..=5 {
                let LayerContent::Objects(objects) = &mut d.layer_mut(top).unwrap().content else { panic!() };
                let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
                i.transform = Affine::scale(0.9, 0.9).then(Affine::translate(step as f64 * 37.3, step as f64 * 11.1));
                let t = std::time::Instant::now();
                p.update(&d.layers, 2, &view, quality);
                total += t.elapsed();
            }
            eprintln!("{quality:?}: {:?} a frame", total / 5);
        }
    }

    #[test]
    fn past_its_budget_the_cache_drops_what_is_farthest_from_the_view() {
        let mut ids = IdGen::default();
        let mut d = Document::new(8192, 1024);
        let mut img = RasterImage::new();
        for c in TileCoord::covering(&IRect::from_size(8192, 1024), 0) {
            let mut t = Tile::transparent();
            t.set(0, 0, pixel_from_f32([c.x as f32 / 32.0, c.y as f32 / 4.0, 0.0, 1.0]));
            img.set_tile(c, Some(Arc::new(t)));
        }
        add(&mut d, &mut ids, img);
        add(&mut d, &mut ids, solid(IRect::new(0, 0, 8192, 8), [0.0, 0.0, 1.0, 0.5]));
        let mut p = Projection::new();
        p.set_budget(Some(24 * TILE_BYTES));
        let view = |x: i64| IRect::new(x, 0, x + 1024, 1024);
        // Sixteen tiles in view, then sixteen more to the right, then sixteen far beyond.
        p.update(&d.layers, 0, &view(0), Quality::Exact);
        p.update(&d.layers, 0, &view(1024), Quality::Exact);
        let cached = |p: &Projection| p.levels[0].values().filter(|e| e.tile.is_some()).count();
        assert_eq!(cached(&p), 24, "over budget, the farthest tiles go");
        assert!(p.tile(0, TileCoord::new(1, 0)).is_none() && p.tile(0, TileCoord::new(3, 0)).is_some());
        p.update(&d.layers, 0, &view(6144), Quality::Exact);
        assert_eq!(cached(&p), 24);
        for c in TileCoord::covering(&view(6144), 0) {
            assert!(p.tile(0, c).is_some(), "a tile in view was dropped");
        }
        // A budget smaller than the view itself keeps the view.
        p.set_budget(Some(4 * TILE_BYTES));
        p.update(&d.layers, 0, &view(2048), Quality::Exact);
        for c in TileCoord::covering(&view(2048), 0) {
            assert!(p.tile(0, c).is_some(), "a tile in view was dropped");
        }
        // Without a budget, nothing goes.
        p.set_budget(None);
        p.update(&d.layers, 0, &view(0), Quality::Exact);
        assert_eq!(cached(&p), 32);
    }

    /// Times panning a view of four 24-megapixel layers at 25% (run with `--release
    /// --ignored --nocapture`).
    #[test]
    #[ignore]
    fn time_panning() {
        let (w, h) = (6000u32, 4000u32);
        let mut ids = IdGen::default();
        let mut d = Document::new(w, h);
        for k in 0..4 {
            let px: Vec<Pixel> =
                (0..w * h).map(|i| pixel_from_f32([((i + k) % 251) as f32 / 251.0, 0.5, 0.25, 0.5])).collect();
            add(&mut d, &mut ids, RasterImage::from_pixels(w, h, &px));
        }
        let mut p = Projection::new();
        let view = |x: i64| IRect::new(x, 0, x + 4000, 4000);
        p.update(&d.layers, 2, &view(0), Quality::Exact);
        // Every position once first, so the frames timed composite nothing new.
        for step in 0..50 {
            p.update(&d.layers, 2, &view(step * 37), Quality::Exact);
        }
        let t = std::time::Instant::now();
        for step in (0..50).rev() {
            p.update(&d.layers, 2, &view(step * 37), Quality::Exact);
        }
        eprintln!("panning: {:?} a frame", t.elapsed() / 50);
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
        let shown = p.update(&d.layers, 1, &IRect::from_size(2048, 2048), Quality::Exact);
        assert_eq!(shown.len(), 16);
        assert!(shown.iter().all(|(_, t)| Arc::ptr_eq(t, &shown[0].1)), "each reduced tile was computed separately");
    }
}
