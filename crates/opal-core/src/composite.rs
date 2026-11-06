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
use crate::geom::{Affine, FRect, IRect};
use crate::raster::RasterImage;
use crate::resample::{self, Sampling};
use crate::tile::{Pixel, PixelF32, TILE_PIXELS, TILE_SIZE, Tile, TileCoord, downsample};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

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
    let content = content.to_f().expand(r);
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
    touches(image, &t, &s, c).then(|| Token::Image {
        image: Arc::downgrade(image),
        transform: t.to_bits(),
        sampling: s.key(),
    })
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
        ObjectKind::Image(i) => image_rects(i, &mut |r| f(r, false)),
    });
}

/// Calls `f` with rectangles (canvas pixels) that together cover what image object `i`
/// draws, one per tile of its image.
fn image_rects(i: &ImageObject, f: &mut dyn FnMut(IRect)) {
    let r = resample::reach(&i.transform, &i.sampling);
    for (c, _) in i.image.tiles() {
        f(IRect::covering(&i.transform.bounds(&c.rect(0).to_f().expand(r))));
    }
}

/// Appends the visible pixel objects of `layers`, at any depth, to `out`.
fn visible_pixel_objects<'a>(layers: &'a [Arc<Layer>], out: &mut Vec<&'a PixelObject>) {
    for layer in layers.iter().filter(|l| l.visible && l.opacity > 0.0) {
        match &layer.content {
            LayerContent::Objects(objects) => out.extend(objects.iter().filter_map(|o| match &o.kind {
                ObjectKind::Pixels(p) => Some(p),
                ObjectKind::Image(_) => None,
            })),
            LayerContent::Group(children) => visible_pixel_objects(children, out),
        }
    }
}

/// Where the image objects of a set of layers draw, worked out once for them and asked of
/// each area shown: rectangles, one per tile of each image (which, transformed, may span
/// more tiles than are worth listing). Pixel objects are asked of their own tiles.
#[derive(Debug, Default)]
struct Occupancy {
    rects: Vec<IRect>,
}

impl Occupancy {
    fn of(layers: &[Arc<Layer>]) -> Occupancy {
        let mut rects = Vec::new();
        for_each_visible_object(layers, &mut |o| {
            if let ObjectKind::Image(i) = &o.kind {
                image_rects(i, &mut |r| rects.push(r));
            }
        });
        Occupancy { rects }
    }

    /// The level-0 tiles image objects draw into within `range`.
    fn within(&self, range: &IRect) -> HashSet<TileCoord> {
        let mut out = HashSet::new();
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
/// bitmap, in parallel. Tiles made from the same stored tiles in the same way composite
/// alike, so each such set is composited once and shared; and a tile that shows one stored
/// tile as it is ([`lone_source`]) is that tile, uncopied. A uniform background thus costs
/// a few tiles however large it is.
fn composite_region(layers: &[Arc<Layer>], range: Option<&IRect>) -> RasterImage {
    prepare(layers, range);
    let mut img = RasterImage::new();
    place(layers, &tiles_to_make(layers, range), &mut img);
    img
}

/// [`composite_within`] a batch of tiles at a time, telling `go_on` before each batch how
/// many of the places the tiles go have one so far, and how many there are in all (first
/// `(0, 0)`, asked before anything is known or done), and stopping (`None`) if it says not
/// to go on.
pub fn composite_within_until(
    layers: &[Arc<Layer>],
    range: &IRect,
    go_on: &mut dyn FnMut(usize, usize) -> bool,
) -> Option<RasterImage> {
    // Enough sets of tiles to keep every thread busy, few enough to stop soon when asked.
    const BATCH: usize = 1024;
    // Before the image objects' pyramids are built, which may itself take a while.
    if !go_on(0, 0) {
        return None;
    }
    prepare(layers, Some(range));
    let work = tiles_to_make(layers, Some(range));
    let total = work.iter().map(|(at, _)| at.len()).sum();
    let mut img = RasterImage::new();
    let mut placed = 0;
    for batch in work.chunks(BATCH) {
        if !go_on(placed, total) {
            return None;
        }
        placed += place(layers, batch, &mut img);
    }
    Some(img)
}

/// Makes the tiles `work` lists ([`tiles_to_make`]), in parallel, and puts each where it
/// goes in `img`; returns how many places that is.
fn place(layers: &[Arc<Layer>], work: &[(Vec<TileCoord>, Vec<Token>)], img: &mut RasterImage) -> usize {
    let made: Vec<Option<Arc<Tile>>> = work
        .par_iter()
        .map(|(at, tokens)| match lone_source(tokens).and_then(Weak::upgrade) {
            Some(t) => Some(t),
            None => composite_tile(layers, at[0]).map(Arc::new),
        })
        .collect();
    let mut places = 0;
    for ((at, _), tile) in work.iter().zip(made) {
        places += at.len();
        if let Some(t) = tile {
            for &c in at {
                img.set_tile(c, Some(t.clone()));
            }
        }
    }
    places
}

/// The tiles compositing `layers` within `range` makes ([`composite_region`]): where each
/// goes (tiles alike made once), and its key. Those showing an image object are each their
/// own, as what they show depends on where they are.
///
/// Tiles are grouped as their keys are found, so a key is kept once for each set of tiles
/// alike, not once for each tile.
fn tiles_to_make(layers: &[Arc<Layer>], range: Option<&IRect>) -> Vec<(Vec<TileCoord>, Vec<Token>)> {
    type Alike = HashMap<Vec<u64>, (Vec<TileCoord>, Vec<Token>)>;
    type Own = Vec<(Vec<TileCoord>, Vec<Token>)>;
    let coords: Vec<TileCoord> = occupied_tiles(layers, range).into_iter().collect();
    let (alike, mut own) = coords
        .par_iter()
        .fold(
            || (Alike::new(), Own::new()),
            |(mut alike, mut own), &c| {
                let mut tokens = Vec::new();
                key_into(layers, c, &mut tokens, &object_token);
                match signature(&tokens) {
                    Some(s) => alike.entry(s).or_insert_with(|| (Vec::new(), tokens)).0.push(c),
                    None => own.push((vec![c], tokens)),
                }
                (alike, own)
            },
        )
        .reduce(
            || (Alike::new(), Own::new()),
            |(mut a, mut x), (b, y)| {
                for (s, (at, tokens)) in b {
                    a.entry(s).or_insert_with(|| (Vec::new(), tokens)).0.extend(at);
                }
                x.extend(y);
                (a, x)
            },
        );
    own.extend(alike.into_values());
    own
}

/// The memory, in bytes, the tiles compositing `layers` within `range` (everywhere when
/// `None`) would take: one tile for each it makes, nothing for one it shares (see
/// [`composite_region`]).
pub fn composite_cost(layers: &[Arc<Layer>], range: Option<&IRect>) -> u64 {
    let made = tiles_to_make(layers, range).iter().filter(|(_, tokens)| lone_source(tokens).is_none()).count();
    made as u64 * TILE_BYTES
}

/// `tokens` as numbers, alike exactly when the tiles they are the keys of composite alike
/// wherever they are; `None` if they name an image object, whose pixels in a tile depend
/// on where the tile is.
fn signature(tokens: &[Token]) -> Option<Vec<u64>> {
    let at = |w: &Weak<Tile>| w.as_ptr() as usize as u64;
    let mut out = Vec::with_capacity(tokens.len() * 2);
    // Each kind of token a tag, then as many numbers as that kind always has.
    for t in tokens {
        match t {
            Token::Layer(o) => out.extend([0, u64::from(*o)]),
            Token::Group(o) => out.extend([1, u64::from(*o)]),
            Token::Tile(w) => out.extend([2, at(w)]),
            Token::Shifted { phase, tiles } => {
                out.extend([3, phase[0] as u64, phase[1] as u64]);
                out.extend(tiles.iter().map(|t| t.as_ref().map_or(0, at)));
            }
            Token::Image { .. } => return None,
            Token::End => out.push(4),
        }
    }
    Some(out)
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
    Tile(Weak<Tile>),
    /// Pixels off the tile grid: their offset within a tile, and the stored tiles that show
    /// (in [`pixel_sources`]'s order).
    Shifted { phase: [i64; 2], tiles: [Option<Weak<Tile>>; 4] },
    /// An image object: its pixels (the image stands for all of them, since an image
    /// object's pixels are not changed in place) and how it is drawn.
    Image { image: Weak<RasterImage>, transform: [u64; 6], sampling: [u32; 4] },
    /// The innermost open layer or group ends.
    End,
}

/// Tokens name what they stand for by weak references, compared by address: a cached key
/// then keeps no pixels alive (a tile the document has let go of is freed, though the
/// cache still names it), and the address cannot be reused while it is named, so a new
/// tile never passes for an old one.
impl PartialEq for Token {
    fn eq(&self, other: &Token) -> bool {
        let same = |a: &Option<Weak<Tile>>, b: &Option<Weak<Tile>>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Weak::ptr_eq(a, b),
            _ => false,
        };
        match (self, other) {
            (Token::Layer(a), Token::Layer(b)) | (Token::Group(a), Token::Group(b)) => a == b,
            (Token::Tile(a), Token::Tile(b)) => Weak::ptr_eq(a, b),
            (Token::Shifted { phase: p, tiles: a }, Token::Shifted { phase: q, tiles: b }) => {
                p == q && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (
                Token::Image { image: a, transform: m, sampling: s },
                Token::Image { image: b, transform: n, sampling: t },
            ) => Weak::ptr_eq(a, b) && m == n && s == t,
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
                return sources[0].map(|t| Token::Tile(Arc::downgrade(t)));
            }
            if sources.iter().all(Option::is_none) {
                return None;
            }
            Some(Token::Shifted { phase, tiles: sources.map(|t| t.map(Arc::downgrade)) })
        }
        ObjectKind::Image(i) => touches(&i.image, &i.transform, &i.sampling, c).then(|| Token::Image {
            image: Arc::downgrade(&i.image),
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
fn lone_source(tokens: &[Token]) -> Option<&Weak<Tile>> {
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

#[derive(Clone, Debug, PartialEq)]
enum Key {
    Base(Vec<Token>),
    /// The versions (see [`Entry::version`]) of the four tiles below.
    Pyramid([u64; 4]),
    /// A tile drawn at its level directly (see [`Quality::Preview`]): what went into it,
    /// as for a level-0 tile. It never equals an exact key, so the next exact update
    /// replaces the tile.
    Preview(Vec<Token>),
    /// A tile over a uniform block (see [`uniform_block`]): the key every level-0 tile in
    /// it has. Made from one such tile, with nothing cached below it.
    Uniform(Vec<Token>),
}

/// Whether every level-0 tile under level-`level` tile `c` composites alike, wherever it
/// is: each visible pixel object has one stored tile repeated under it, or none, and no
/// image object (drawn differently at each place) reaches into it. Then every tile in it
/// has the same key.
fn uniform_block(layers: &[Arc<Layer>], c: TileCoord, level: u32) -> bool {
    fn alike(layers: &[Arc<Layer>], block: &IRect) -> bool {
        layers.iter().filter(|l| l.visible && l.opacity > 0.0).all(|layer| match &layer.content {
            LayerContent::Group(children) => alike(children, block),
            LayerContent::Objects(objects) => objects.iter().all(|o| match &o.kind {
                ObjectKind::Pixels(p) => p.image.alike_under(&block.translate(-p.offset[0], -p.offset[1])),
                ObjectKind::Image(i) => resample::footprint(&i.image, &i.transform, &i.sampling)
                    .is_none_or(|f| f.intersect(block).is_empty()),
            }),
        })
    }
    alike(layers, &c.rect(level))
}

/// The first level-0 tile under level-`level` tile `c`.
fn first_under(c: TileCoord, level: u32) -> TileCoord {
    let r = c.rect(level);
    TileCoord::containing(r.x0, r.y0)
}

/// The key of level-`level` tile `c` over a uniform block of `layers`: that of every
/// level-0 tile in it.
fn uniform_key(layers: &[Arc<Layer>], c: TileCoord, level: u32) -> Key {
    let Key::Base(tokens) = base_key(layers, first_under(c, level)) else { unreachable!("a level-0 key") };
    Key::Uniform(tokens)
}

/// The tiles of uniform blocks made during one update, for each key ([`Key::Uniform`]):
/// at every level from 0 up to the highest asked for, each the one below halved from four
/// copies of itself, as the pyramid makes it from such tiles. A block a level up then costs
/// one halving, not all of them again from level 0.
#[derive(Debug, Default)]
struct Chains(Vec<Chain>);

/// A key's tiles in [`Chains`], by level.
#[derive(Debug)]
struct Chain {
    key: Vec<Token>,
    tiles: Vec<Option<Arc<Tile>>>,
}

/// The tiles an exact update makes at each level up to the one shown, from the top down:
/// where `layers` cover a block uniformly ([`uniform_block`]) its tile is made at its
/// level from one level-0 tile, and nothing beneath it is needed; the other tiles are made
/// from the four below, down to level 0. Only tiles with content under them are made, as
/// the content's own tiles say, so the plan costs what it holds, not the tiles beneath.
struct Plan<'a> {
    /// By level, the tiles made there.
    need: Vec<HashSet<TileCoord>>,
    /// By level, those of `need` made as uniform blocks.
    uniform: Vec<HashSet<TileCoord>>,
    /// The visible pixel objects.
    pixels: Vec<&'a PixelObject>,
    /// By level, the tiles image objects draw into, within the area shown.
    images: Vec<HashSet<TileCoord>>,
}

impl<'a> Plan<'a> {
    /// The plan for showing the level-`level` tiles over `outer` (whole tiles of that
    /// level), where `occupancy` says the image objects of `layers` draw.
    fn of(layers: &'a [Arc<Layer>], level: u32, outer: &IRect, occupancy: &Occupancy) -> Plan<'a> {
        let top = level as usize;
        let mut pixels = Vec::new();
        visible_pixel_objects(layers, &mut pixels);
        let mut images = vec![occupancy.within(outer)];
        for l in 1..=top {
            let up = images[l - 1].iter().map(|c| c.parent()).collect();
            images.push(up);
        }
        let empty = vec![HashSet::new(); top + 1];
        let mut plan = Plan { need: empty.clone(), uniform: empty, pixels, images };
        // Each question costs a few lookups: in parallel, they would cost more.
        plan.need[top] = TileCoord::covering(outer, level).filter(|&c| plan.occupied(c, level)).collect();
        for l in (1..=top).rev() {
            let (alike, mixed): (Vec<TileCoord>, Vec<TileCoord>) =
                plan.need[l].iter().partition(|&&c| uniform_block(layers, c, l as u32));
            let below: HashSet<TileCoord> =
                mixed.into_iter().flat_map(|c| c.children()).filter(|&k| plan.occupied(k, l as u32 - 1)).collect();
            plan.uniform[l] = alike.into_iter().collect();
            plan.need[l - 1] = below;
        }
        plan
    }

    /// Whether content lies in level-`level` tile `c` (within the area shown).
    fn occupied(&self, c: TileCoord, level: u32) -> bool {
        let r = c.rect(level);
        self.images[level as usize].contains(&c)
            || self.pixels.iter().any(|p| p.image.any_under(&r.translate(-p.offset[0], -p.offset[1])))
    }
}

/// A cached tile's pixels.
#[derive(Clone, Debug)]
enum Pixels {
    /// Nothing shows in the tile.
    Empty,
    /// Pixels the cache made.
    Held(Arc<Tile>),
    /// A layer's own tile, shown as it is: it costs nothing more, and is held weakly so that
    /// it is freed with the layer's (an entry current with the layers can always reach it).
    Layer(Weak<Tile>),
    /// Let go to keep within the budget; made again from the key, which still holds, when
    /// they are needed.
    Dropped,
}

#[derive(Clone, Debug)]
struct Entry {
    key: Key,
    pixels: Pixels,
    /// Names what the tile shows: a new one each time it is computed with a new key, 0 when
    /// it shows nothing. The tile above keys on it, so its pixels can be dropped without
    /// the tile above being redrawn.
    version: u64,
    /// For a level-0 tile: the generation of layers its key was last found current for
    /// (see [`Projection::generation`]).
    checked: u64,
}

impl Entry {
    /// The tile's pixels, if it has them at hand.
    fn held(&self) -> Option<Arc<Tile>> {
        match &self.pixels {
            Pixels::Held(t) => Some(t.clone()),
            Pixels::Layer(t) => t.upgrade(),
            _ => None,
        }
    }
}

/// The level-`level` tiles over uniform blocks of `layers` at `todo`, each with its key
/// ([`Key::Uniform`]): each key's tile made once, from one level-0 tile of a block with it,
/// or from what `chains` already holds of it.
fn uniform_tiles<'k>(
    layers: &[Arc<Layer>],
    level: u32,
    todo: impl Iterator<Item = (TileCoord, &'k Key)>,
    chains: &mut Chains,
) -> HashMap<TileCoord, Option<Arc<Tile>>> {
    let mut by_key: Vec<(&'k Vec<Token>, Vec<TileCoord>)> = Vec::new();
    for (c, key) in todo {
        let Key::Uniform(tokens) = key else { unreachable!("a uniform key") };
        match by_key.iter_mut().find(|(k, _)| *k == tokens) {
            Some((_, at)) => at.push(c),
            None => by_key.push((tokens, vec![c])),
        }
    }
    // Each key's chain, taken out to be carried up to the level in parallel.
    let work: Vec<_> = by_key
        .into_iter()
        .map(|(tokens, at)| {
            let had = chains.0.iter().position(|c| c.key == *tokens);
            (tokens, at, had.map(|i| chains.0.swap_remove(i).tiles).unwrap_or_default())
        })
        .collect();
    let done: Vec<_> = work
        .into_par_iter()
        .map(|(tokens, at, mut chain)| {
            if chain.is_empty() {
                chain.push(match base_pixels(layers, first_under(at[0], level), tokens) {
                    Pixels::Layer(t) => t.upgrade(),
                    Pixels::Held(t) => Some(t),
                    Pixels::Empty | Pixels::Dropped => None,
                });
            }
            while chain.len() <= level as usize {
                let below = chain.last().expect("a tile at level 0").as_deref();
                chain.push(below.and_then(|t| downsample([Some(t); 4])).map(Arc::new));
            }
            (tokens, at, chain)
        })
        .collect();
    let mut made = HashMap::new();
    for (tokens, at, chain) in done {
        let tile = &chain[level as usize];
        made.extend(at.into_iter().map(|c| (c, tile.clone())));
        chains.0.push(Chain { key: tokens.clone(), tiles: chain });
    }
    made
}

/// What a level-0 tile with key `tokens` shows, composited at `c` from `layers` (whose tiles
/// are those the key names): a lone source tile is shown as it is, without a copy.
fn base_pixels(layers: &[Arc<Layer>], c: TileCoord, tokens: &[Token]) -> Pixels {
    match lone_source(tokens) {
        Some(t) => Pixels::Layer(t.clone()),
        None => composite_tile(layers, c).map_or(Pixels::Empty, |t| Pixels::Held(Arc::new(t))),
    }
}

/// Whether two lists of layers are the same layers (the same allocations, in order).
pub fn same_layers(a: &[Arc<Layer>], b: &[Arc<Layer>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y))
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
    /// The last version given (see [`Entry::version`]).
    versions: u64,
    /// How many level-0 keys have been built (for tests).
    keys_built: usize,
    /// How many bytes of tiles the cache may hold (see [`set_budget`](Self::set_budget)).
    budget: Option<u64>,
    /// The uniform tiles made during the update under way; empty between updates.
    chains: Chains,
}

use crate::raster::TILE_BYTES;

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
    generation: u64,
    level: u32,
    rect: IRect,
    quality: Quality,
    budget: Option<u64>,
    result: Vec<(TileCoord, Arc<Tile>)>,
}

/// The highest pyramid level kept (a tile then covers 256 × 2^16 pixels).
pub const MAX_LEVEL: u32 = 16;

impl Projection {
    pub fn new() -> Projection {
        Projection::default()
    }

    /// Limits the pixels the cache holds to `bytes` (`None` for no limit), counting each
    /// tile it made once: past it, an update lets go of the tiles farthest from the view,
    /// then of the pixels of finer levels under the view (kept as keys, and made again
    /// when needed). The tiles at the level shown in the view always stay.
    pub fn set_budget(&mut self, bytes: Option<u64>) {
        self.budget = bytes;
    }

    /// Whether the last update drew what changed as a preview (see [`Quality::Preview`]).
    pub fn shows_previews(&self) -> bool {
        self.last.as_ref().is_some_and(|l| l.quality == Quality::Preview)
    }

    fn next_version(&mut self) -> u64 {
        self.versions += 1;
        self.versions
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
        if !same_layers(&self.layers, layers) {
            self.generation += 1;
            self.layers = layers.to_vec();
            self.occupancy = Occupancy::of(layers);
        }
        if let Some(last) = &self.last
            && last.generation == self.generation
            && last.level == level
            && last.rect == *rect
            && last.quality == quality
            && last.budget == self.budget
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
        let result = match quality {
            Quality::Exact => {
                self.reduced = Reduced::default();
                prepare(layers, Some(&outer));
                let coords = self.update_exact(layers, level, &outer);
                self.restore(layers, level, coords.iter().copied());
                self.shown(level, coords.iter())
            }
            Quality::Preview => self.update_preview(layers, level, &outer),
        };
        self.trim(level, &outer);
        self.chains = Chains::default();
        self.last = Some(LastUpdate {
            generation: self.generation,
            level,
            rect: *rect,
            quality,
            budget: self.budget,
            result: result.clone(),
        });
        result
    }

    /// [`Quality::Preview`]: the level-`level` tiles over `outer` with anything out of date
    /// under them are drawn at that level; the others come from the exact pyramid, which is
    /// brought up to date for them alone (their level-0 tiles are current, so that costs
    /// little).
    fn update_preview(&mut self, layers: &[Arc<Layer>], level: u32, outer: &IRect) -> Vec<(TileCoord, Arc<Tile>)> {
        // The level-`level` tile over tile `c` of level `from`.
        let up = |mut c: TileCoord, from: u32| {
            for _ in from..level {
                c = c.parent();
            }
            c
        };
        // The level-`level` tiles with a level-0 tile under them out of date, or emptied:
        // of those an exact update would make. A tile over a uniform block is made exactly
        // whatever happened, being made from one level-0 tile: no dearer than a preview.
        let generation = self.generation;
        let plan = Plan::of(layers, level, outer, &self.occupancy);
        let mut dirty: HashSet<TileCoord> = HashSet::new();
        for c in self.levels[0].keys() {
            if !c.rect(0).intersect(outer).is_empty() && !plan.occupied(*c, 0) {
                dirty.insert(up(*c, 0));
            }
        }
        for &c in &plan.need[0] {
            if self.check_base(layers, c).is_some() {
                dirty.insert(up(c, 0));
            }
        }
        // The exact pyramid for the rest; previews already drawn at the level are kept.
        for l in 1..=level {
            let clean = |c: &&TileCoord| !dirty.contains(&up(**c, l));
            let coords: HashSet<TileCoord> = plan.need[l as usize].iter().filter(clean).copied().collect();
            let uniform: HashSet<TileCoord> = plan.uniform[l as usize].iter().filter(clean).copied().collect();
            let keep = if l == level { dirty.clone() } else { HashSet::new() };
            self.update_pyramid(layers, l, &coords, &uniform, outer, &keep);
        }
        let coords: Vec<TileCoord> = plan.need[level as usize].iter().filter(|c| !dirty.contains(c)).copied().collect();
        self.restore(layers, level, coords.iter().copied());
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
        let drawn: Vec<(TileCoord, Key, Option<Arc<Tile>>)> = stale
            .into_par_iter()
            .map(|(c, key)| {
                let tile = composite_with(layers, c, &show).map(Arc::new);
                (c, key, tile)
            })
            .collect();
        drop(show);
        for (c, key, tile) in drawn {
            let version = if tile.is_some() { self.next_version() } else { 0 };
            let pixels = tile.map_or(Pixels::Empty, Pixels::Held);
            self.levels[level as usize].insert(c, Entry { key, pixels, version, checked: generation });
        }
        self.shown(level, coords.iter().chain(&dirty))
    }

    /// Keeps the cache within its budget (see [`trim_pixels`](Self::trim_pixels) and
    /// [`trim_entries`](Self::trim_entries)); `level` and `outer` are the view just drawn.
    fn trim(&mut self, level: u32, outer: &IRect) {
        let Some(budget) = self.budget else { return };
        // Cheap to know: no more entries than tiles the budget holds, nothing to do.
        let entries: usize = self.levels.iter().map(HashMap::len).sum();
        let tiles = (budget / TILE_BYTES) as usize;
        if entries <= tiles {
            return;
        }
        self.trim_pixels(level, outer, budget);
        // Entries without pixels are far smaller than tiles: four times as many may be kept.
        self.trim_entries(level, outer, (4 * tiles).max(256));
    }

    /// How far tile rect `r` lies from `outer`, in canvas pixels.
    fn gap(r: &IRect, outer: &IRect) -> i64 {
        let dx = (outer.x0 - r.x1).max(r.x0 - outer.x1).max(0);
        let dy = (outer.y0 - r.y1).max(r.y0 - outer.y1).max(0);
        dx + dy
    }

    /// Whether the entry at `l` and `c` is one the view drawn at `level` over `outer` uses.
    fn in_view(l: usize, c: TileCoord, level: u32, outer: &IRect) -> bool {
        l as u32 <= level && !c.rect(l as u32).intersect(outer).is_empty()
    }

    /// Removes the entries farthest from the view first, of whatever kind, while there are
    /// more than `cap`; never one the view uses.
    fn trim_entries(&mut self, level: u32, outer: &IRect, cap: usize) {
        let mut entries: usize = self.levels.iter().map(HashMap::len).sum();
        if entries <= cap {
            return;
        }
        let mut away: Vec<(i64, usize, TileCoord)> = Vec::new();
        for (l, cache) in self.levels.iter().enumerate() {
            for c in cache.keys() {
                if !Self::in_view(l, *c, level, outer) {
                    away.push((Self::gap(&c.rect(l as u32), outer), l, *c));
                }
            }
        }
        away.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
        for (_, l, c) in away {
            if entries <= cap {
                return;
            }
            self.levels[l].remove(&c);
            entries -= 1;
        }
    }

    /// Lets go of cached pixels while those the cache made take more than `budget`: first
    /// the tiles outside `outer` (or above level `level`), farthest from it first, then the
    /// pixels of the finer levels within it, finest first, which keep their keys and versions
    /// (so nothing above them is redrawn) and are made again only when needed. The tiles at
    /// level `level` within `outer`, what the view shows, always stay.
    fn trim_pixels(&mut self, level: u32, outer: &IRect, budget: u64) {
        // Each tile the cache made counts once, however many entries share it.
        let ptr = |t: &Arc<Tile>| Arc::as_ptr(t) as usize;
        let mut holders: HashMap<usize, usize> = HashMap::new();
        for cache in &self.levels {
            for e in cache.values() {
                if let Pixels::Held(t) = &e.pixels {
                    *holders.entry(ptr(t)).or_default() += 1;
                }
            }
        }
        let mut bytes = holders.len() as u64 * TILE_BYTES;
        if bytes <= budget {
            return;
        }
        let mut away: Vec<(i64, usize, TileCoord)> = Vec::new();
        let mut under: Vec<(usize, TileCoord)> = Vec::new();
        for (l, cache) in self.levels.iter().enumerate() {
            let below = (l as u32) < level;
            for (c, e) in cache {
                // Only pixels the cache made cost anything; the rest (keys, a layer's own
                // tiles) are kept, so as not to be built again.
                if !matches!(e.pixels, Pixels::Held(_)) {
                    continue;
                }
                let r = c.rect(l as u32);
                // A preview off the level shown is out of date: the next exact update
                // replaces it.
                if l as u32 > level || r.intersect(outer).is_empty() || (below && matches!(e.key, Key::Preview(_))) {
                    away.push((Self::gap(&r, outer), l, *c));
                } else if below {
                    under.push((l, *c));
                }
            }
        }
        let mut release = |bytes: &mut u64, p: &Pixels| {
            if let Pixels::Held(t) = p {
                let n = holders.get_mut(&ptr(t)).expect("counted");
                *n -= 1;
                if *n == 0 {
                    *bytes -= TILE_BYTES;
                }
            }
        };
        away.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
        for (_, l, c) in away {
            if bytes <= budget {
                return;
            }
            if let Some(e) = self.levels[l].remove(&c) {
                release(&mut bytes, &e.pixels);
            }
        }
        under.sort_unstable_by_key(|u| u.0);
        for (l, c) in under {
            if bytes <= budget {
                return;
            }
            let e = self.levels[l].get_mut(&c).expect("listed");
            let pixels = std::mem::replace(&mut e.pixels, Pixels::Dropped);
            release(&mut bytes, &pixels);
        }
    }

    /// Makes again the dropped pixels of the tiles `coords` at `level`, and of the finer
    /// tiles they are made from (all current, so their keys say what they show).
    fn restore(&mut self, layers: &[Arc<Layer>], level: u32, coords: impl IntoIterator<Item = TileCoord>) {
        let dropped = |levels: &[HashMap<TileCoord, Entry>], l: u32, c: &TileCoord| {
            levels[l as usize].get(c).is_some_and(|e| matches!(e.pixels, Pixels::Dropped))
        };
        let mut need: Vec<Vec<TileCoord>> = vec![Vec::new(); level as usize + 1];
        need[level as usize] = coords.into_iter().filter(|c| dropped(&self.levels, level, c)).collect();
        if need[level as usize].is_empty() {
            return;
        }
        for l in (1..=level).rev() {
            // A uniform tile is made from the layers, not from the tiles below.
            let cache = &self.levels[l as usize];
            let below: HashSet<TileCoord> = need[l as usize]
                .iter()
                .filter(|c| !matches!(cache[c].key, Key::Uniform(..)))
                .flat_map(|c| c.children())
                .filter(|k| dropped(&self.levels, l - 1, k))
                .collect();
            need[l as usize - 1] = below.into_iter().collect();
        }
        for (l, todo) in need.into_iter().enumerate() {
            if todo.is_empty() {
                continue;
            }
            let made: Vec<(TileCoord, Pixels)> = if l == 0 {
                let cache = &self.levels[0];
                todo.into_par_iter()
                    .map(|c| {
                        let Key::Base(tokens) = &cache[&c].key else { unreachable!("a level-0 key") };
                        (c, base_pixels(layers, c, tokens))
                    })
                    .collect()
            } else {
                // Uniform tiles from one tile of their block; the others from the four below.
                let (cache, below) = (&self.levels[l], &self.levels[l - 1]);
                let (alike, rest): (Vec<TileCoord>, Vec<TileCoord>) =
                    todo.into_iter().partition(|c| matches!(cache[c].key, Key::Uniform(..)));
                let alike =
                    uniform_tiles(layers, l as u32, alike.iter().map(|c| (*c, &cache[c].key)), &mut self.chains);
                let mut made: Vec<(TileCoord, Pixels)> = rest
                    .into_par_iter()
                    .map(|c| {
                        let kids = c.children().map(|k| below.get(&k).and_then(Entry::held));
                        let tile = downsample(kids.each_ref().map(|k| k.as_deref()));
                        (c, tile.map_or(Pixels::Empty, |t| Pixels::Held(Arc::new(t))))
                    })
                    .collect();
                made.extend(alike.into_iter().map(|(c, t)| (c, t.map_or(Pixels::Empty, Pixels::Held))));
                made
            };
            for (c, pixels) in made {
                self.levels[l].get_mut(&c).expect("dropped, so cached").pixels = pixels;
            }
        }
    }

    /// The tiles of level `level` at `coords` that have content.
    fn shown<'a>(&self, level: u32, coords: impl Iterator<Item = &'a TileCoord>) -> Vec<(TileCoord, Arc<Tile>)> {
        let top = &self.levels[level as usize];
        coords.filter_map(|c| top.get(c).and_then(Entry::held).map(|t| (*c, t))).collect()
    }

    /// The cached tile at `level` and `c` as of the last [`update`](Self::update) that
    /// covered it, if its pixels are at hand.
    pub fn tile(&self, level: u32, c: TileCoord) -> Option<Arc<Tile>> {
        self.levels.get(level as usize)?.get(&c)?.held()
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

    /// Brings the level-`level` tiles over `outer` up to date, and returns those over
    /// content. As [`Plan`] says, from the top down: a tile over a uniform block is made at
    /// its level from one tile of it ([`Key::Uniform`]), with nothing cached below it; the
    /// others are made from the four below, as far down as level 0. So a large area of one
    /// colour costs a tile, not every tile beneath it.
    fn update_exact(&mut self, layers: &[Arc<Layer>], level: u32, outer: &IRect) -> HashSet<TileCoord> {
        let mut plan = Plan::of(layers, level, outer, &self.occupancy);
        self.update_base(layers, &plan.need[0], outer);
        for l in 1..=level as usize {
            self.update_pyramid(layers, l as u32, &plan.need[l], &plan.uniform[l], outer, &HashSet::new());
        }
        plan.need.pop().expect("a set a level")
    }

    /// Makes the uniform tiles at level `level` whose key changed ([`Key::Uniform`]), each
    /// key's tile once. `coords` are uniform blocks of `layers`.
    fn update_uniform(&mut self, layers: &[Arc<Layer>], level: u32, coords: &HashSet<TileCoord>) {
        if coords.is_empty() {
            return;
        }
        let cache = &self.levels[level as usize];
        let stale: Vec<(TileCoord, Key)> = coords
            .par_iter()
            .filter_map(|&c| {
                let key = uniform_key(layers, c, level);
                cache.get(&c).is_none_or(|e| e.key != key).then_some((c, key))
            })
            .collect();
        let made = uniform_tiles(layers, level, stale.iter().map(|(c, k)| (*c, k)), &mut self.chains);
        for (c, key) in stale {
            let tile = made[&c].clone();
            let version = if tile.is_some() { self.next_version() } else { 0 };
            let pixels = tile.map_or(Pixels::Empty, Pixels::Held);
            self.levels[level as usize].insert(c, Entry { key, pixels, version, checked: 0 });
        }
    }

    fn update_base(&mut self, layers: &[Arc<Layer>], coords: &HashSet<TileCoord>, outer: &IRect) {
        let generation = self.generation;
        // Tiles inside the area that nothing occupies any more.
        self.levels[0].retain(|c, _| coords.contains(c) || c.rect(0).intersect(outer).is_empty());
        let stale: Vec<(TileCoord, Key)> =
            coords.iter().filter_map(|&c| self.check_base(layers, c).map(|key| (c, key))).collect();
        let done: Vec<(TileCoord, Key, Pixels)> = stale
            .into_par_iter()
            .map(|(c, key)| {
                let Key::Base(tokens) = &key else { unreachable!() };
                let pixels = base_pixels(layers, c, tokens);
                (c, key, pixels)
            })
            .collect();
        for (c, key, pixels) in done {
            let version = if matches!(pixels, Pixels::Empty) { 0 } else { self.next_version() };
            self.levels[0].insert(c, Entry { key, pixels, version, checked: generation });
        }
    }

    /// Brings the level-`level` tiles at `coords` up to date from the level below (made
    /// again from `layers` where their pixels were dropped), dropping other cached tiles
    /// within `outer` except those in `keep`. Those of `coords` in `uniform`, over uniform
    /// blocks, are made from the layers instead ([`update_uniform`](Self::update_uniform)).
    fn update_pyramid(
        &mut self,
        layers: &[Arc<Layer>],
        level: u32,
        coords: &HashSet<TileCoord>,
        uniform: &HashSet<TileCoord>,
        outer: &IRect,
        keep: &HashSet<TileCoord>,
    ) {
        let l = level as usize;
        self.levels[l]
            .retain(|c, _| coords.contains(c) || keep.contains(c) || c.rect(level).intersect(outer).is_empty());
        self.update_uniform(layers, level, uniform);
        let (below, cache) = (&self.levels[l - 1], &self.levels[l]);
        let stale: Vec<(TileCoord, [u64; 4])> = coords
            .iter()
            .filter(|c| !uniform.contains(c))
            .filter_map(|&c| {
                let kids = c.children().map(|k| below.get(&k).map_or(0, |e| e.version));
                let current = cache.get(&c).is_some_and(|e| e.key == Key::Pyramid(kids));
                (!current).then_some((c, kids))
            })
            .collect();
        if stale.is_empty() {
            return;
        }
        self.restore(layers, level - 1, stale.iter().flat_map(|(c, _)| c.children()));
        let below = &self.levels[l - 1];
        let pixels = |k: TileCoord| below.get(&k).and_then(Entry::held);
        // Tiles with the same four children (a uniform image is full of them) are reduced
        // once and share the result.
        let ptr = |t: &Option<Arc<Tile>>| t.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
        let mut unique: HashMap<[usize; 4], [Option<Arc<Tile>>; 4]> = HashMap::new();
        let mut of: Vec<(TileCoord, [u64; 4], [usize; 4])> = Vec::with_capacity(stale.len());
        for (c, kids) in stale {
            let tiles = c.children().map(pixels);
            let id = tiles.each_ref().map(ptr);
            unique.entry(id).or_insert(tiles);
            of.push((c, kids, id));
        }
        let reduced: HashMap<[usize; 4], Option<Arc<Tile>>> = unique
            .into_par_iter()
            .map(|(id, kids)| (id, downsample(kids.each_ref().map(|k| k.as_deref())).map(Arc::new)))
            .collect();
        for (c, kids, id) in of {
            let tile = reduced[&id].clone();
            let version = if tile.is_some() { self.next_version() } else { 0 };
            let pixels = tile.map_or(Pixels::Empty, Pixels::Held);
            self.levels[l].insert(c, Entry { key: Key::Pyramid(kids), pixels, version, checked: 0 });
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

    /// An image of `rect` whose tiles are all one tile of colour `rgba` (`rect` on the tile
    /// grid), and that tile.
    fn shared(rect: IRect, rgba: PixelF32) -> (RasterImage, Arc<Tile>) {
        let t = Arc::new(Tile::filled(pixel_from_f32(rgba)));
        let mut img = RasterImage::new();
        for c in TileCoord::covering(&rect, 0) {
            img.set_tile(c, Some(t.clone()));
        }
        (img, t)
    }

    #[test]
    fn merging_composites_tiles_alike_once_and_copies_no_lone_tile() {
        let mut ids = IdGen::default();
        let mut d = Document::new(1024, 1024);
        let (bg, bg_tile) = shared(IRect::from_size(1024, 1024), [0.2, 0.4, 0.6, 1.0]);
        add(&mut d, &mut ids, bg);
        let top = add(&mut d, &mut ids, shared(IRect::new(0, 0, 512, 1024), [1.0, 0.0, 0.0, 1.0]).0);
        d.layer_mut(top).unwrap().opacity = 0.5;
        let out = composite_all(&d.layers);
        // Under the top layer, one composite for 8 tiles; beside it, the background's tile.
        let distinct: HashSet<*const Tile> = out.tiles().map(|(_, t)| Arc::as_ptr(t)).collect();
        assert_eq!((out.tile_count(), distinct.len()), (16, 2));
        assert!(Arc::ptr_eq(out.tile(TileCoord::new(3, 1)).unwrap(), &bg_tile), "a lone tile was copied");
        assert!(close(pixel_to_f32(out.pixel(10, 1000)), [0.6, 0.2, 0.3, 1.0]));
    }

    #[test]
    fn the_cost_of_merging_counts_the_tiles_made_not_those_shared() {
        let mut ids = IdGen::default();
        let mut d = Document::new(1024, 1024);
        add(&mut d, &mut ids, shared(IRect::from_size(1024, 1024), [0.2, 0.4, 0.6, 1.0]).0);
        assert_eq!(composite_cost(&d.layers, None), 0, "a lone layer is shared whole");
        // Over half of it, half opaque: one tile made for eight alike.
        let top = add(&mut d, &mut ids, shared(IRect::new(0, 0, 512, 1024), [1.0, 0.0, 0.0, 1.0]).0);
        d.layer_mut(top).unwrap().opacity = 0.5;
        assert_eq!(composite_cost(&d.layers, None), TILE_BYTES);
        // A tile of its own, in one place: one more.
        add(&mut d, &mut ids, solid(IRect::new(900, 900, 910, 910), [0.0, 1.0, 0.0, 1.0]));
        assert_eq!(composite_cost(&d.layers, None), 2 * TILE_BYTES);
        assert_eq!(composite_cost(&d.layers, Some(&IRect::from_size(512, 512))), TILE_BYTES);
    }

    #[test]
    fn merging_tiles_alike_once_gives_what_compositing_each_gives() {
        use crate::document::{ImageObject, Object};
        use crate::geom::Affine;
        use crate::resample::Sampling;
        let mut ids = IdGen::default();
        let mut d = Document::new(2048, 2048);
        add(&mut d, &mut ids, shared(IRect::from_size(2048, 2048), [0.2, 0.4, 0.6, 1.0]).0);
        // Off the tile grid: alike only where the same four tiles show at the same phase.
        let (mut pattern, _) = shared(IRect::from_size(1024, 1024), [0.0, 1.0, 0.0, 0.5]);
        pattern.set_tile(TileCoord::new(1, 1), Some(Arc::new(Tile::filled(pixel_from_f32([0.0, 0.0, 1.0, 1.0])))));
        let shifted = add(&mut d, &mut ids, pattern.clone());
        pixels_mut(&mut d, shifted).offset = [300, 77];
        // The same tiles at another phase, apart from the first.
        let again = add(&mut d, &mut ids, pattern);
        pixels_mut(&mut d, again).offset = [1300, 1100];
        // One tile, at two opacities, apart from each other (rows 6 and 7, columns 0 to 3 and 4).
        let (left, magenta) = shared(IRect::new(0, 1536, 1024, 2048), [1.0, 0.0, 1.0, 1.0]);
        let mut right = RasterImage::new();
        for c in TileCoord::covering(&IRect::new(1024, 1536, 1280, 2048), 0) {
            right.set_tile(c, Some(magenta.clone()));
        }
        add(&mut d, &mut ids, left);
        let right = add(&mut d, &mut ids, right);
        d.layer_mut(right).unwrap().opacity = 0.4;
        // In a group, partly transparent.
        let g = ids.layer();
        d.insert(Position { parent: None, index: d.layers.len() }, Layer::new_group(g, "g")).unwrap();
        let (inner, _) = shared(IRect::new(512, 0, 1536, 1536), [1.0, 1.0, 0.0, 1.0]);
        let child = Layer::new_raster(ids.layer(), ids.object(), "c", inner);
        d.insert(Position { parent: Some(g), index: 0 }, child).unwrap();
        d.layer_mut(g).unwrap().opacity = 0.3;
        // One tile in two groups of different opacities, apart (row 8, below the canvas).
        let (below, cyan) = shared(IRect::new(0, 2048, 512, 2304), [0.0, 1.0, 1.0, 1.0]);
        for (x, opacity, image) in [(0, 0.3, Some(below)), (768, 0.7, None)] {
            let image = image.unwrap_or_else(|| {
                let mut img = RasterImage::new();
                for c in TileCoord::covering(&IRect::new(x, 2048, x + 512, 2304), 0) {
                    img.set_tile(c, Some(cyan.clone()));
                }
                img
            });
            let group = ids.layer();
            d.insert(Position { parent: None, index: d.layers.len() }, Layer::new_group(group, "g")).unwrap();
            let child = Layer::new_raster(ids.layer(), ids.object(), "c", image);
            d.insert(Position { parent: Some(group), index: 0 }, child).unwrap();
            d.layer_mut(group).unwrap().opacity = opacity;
        }
        // An image object, whose tiles differ with where they are, though its pixels are alike.
        let image = ImageObject {
            image: Arc::new(shared(IRect::from_size(256, 256), [1.0, 0.0, 0.0, 1.0]).0),
            frame: IRect::from_size(256, 256),
            transform: Affine::rotate(0.3).then(Affine::scale(2.0, 2.0)).then(Affine::translate(900.0, 500.0)),
            sampling: Sampling::default(),
        };
        let object = Object { id: ids.object(), kind: ObjectKind::Image(image) };
        let index = d.layers.len();
        d.insert(Position { parent: None, index }, Layer::with_objects(ids.layer(), "image", vec![object])).unwrap();
        let out = composite_all(&d.layers);
        let coords = occupied_tiles(&d.layers, None);
        assert_eq!(out.tile_count(), coords.len());
        for c in coords {
            let each = composite_tile(&d.layers, c).expect("an occupied tile shows something");
            assert!(**out.tile(c).unwrap() == each, "tile {c:?} differs");
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
        let before0 = p.tile(0, TileCoord::containing(2600, 2600));
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
            p.tile(0, TileCoord::containing(2600, 2600)).map(|t| Arc::as_ptr(&t)) == before0.map(|t| Arc::as_ptr(&t)),
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
            assert!(Arc::ptr_eq(&shown, t), "tile {c:?} shows something else");
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
        // Translucent, so every tile is one the cache makes (not the layer's own).
        let base = add(&mut d, &mut ids, img);
        d.layer_mut(base).unwrap().opacity = 0.5;
        let mut p = Projection::new();
        p.set_budget(Some(24 * TILE_BYTES));
        let view = |x: i64| IRect::new(x, 0, x + 1024, 1024);
        // Sixteen tiles in view, then sixteen more to the right, then sixteen far beyond.
        p.update(&d.layers, 0, &view(0), Quality::Exact);
        p.update(&d.layers, 0, &view(1024), Quality::Exact);
        let cached = |p: &Projection| p.levels[0].values().filter(|e| e.held().is_some()).count();
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

    /// A 4096 × 1024 layer at half opacity, every tile its own pixels, so every composite
    /// is one the cache makes.
    fn translucent_document() -> (Document, LayerId) {
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 1024);
        let mut img = RasterImage::new();
        for c in TileCoord::covering(&IRect::from_size(4096, 1024), 0) {
            let mut t = Tile::filled(pixel_from_f32([0.2, 0.4, 0.6, 1.0]));
            t.set(3, 3, pixel_from_f32([c.x as f32 / 16.0, c.y as f32 / 4.0, 0.0, 1.0]));
            img.set_tile(c, Some(Arc::new(t)));
        }
        let l = add(&mut d, &mut ids, img);
        d.layer_mut(l).unwrap().opacity = 0.5;
        (d, l)
    }

    /// The bytes of tiles a projection made and holds, each counted once.
    fn made_bytes(p: &Projection) -> u64 {
        let mut ptrs = HashSet::new();
        for cache in &p.levels {
            for e in cache.values() {
                if let Pixels::Held(t) = &e.pixels {
                    ptrs.insert(Arc::as_ptr(t) as usize);
                }
            }
        }
        ptrs.len() as u64 * TILE_BYTES
    }

    #[test]
    fn zoomed_out_the_budget_holds_by_dropping_finer_pixels() {
        let (mut d, l) = translucent_document();
        let view = IRect::from_size(4096, 1024);
        let mut p = Projection::new();
        // At level 2 the view shows 4 tiles, over 16 at level 1 and 64 at level 0.
        p.set_budget(Some(6 * TILE_BYTES));
        p.update(&d.layers, 2, &view, Quality::Exact);
        assert!(made_bytes(&p) <= 6 * TILE_BYTES, "{} tiles held", made_bytes(&p) / TILE_BYTES);
        for c in TileCoord::covering(&view, 2) {
            assert!(p.tile(2, c).is_some(), "a tile shown was dropped");
        }
        // An edit under the view: the tile above it is drawn again, right, from dropped
        // pixels made again.
        let LayerContent::Objects(objects) = &mut d.layer_mut(l).unwrap().content else { panic!() };
        let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut o.image).set_pixel(1500, 300, pixel_from_f32([1.0, 1.0, 0.0, 1.0]));
        let got: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 2, &view, Quality::Exact).into_iter().collect();
        let want: HashMap<TileCoord, Arc<Tile>> =
            Projection::new().update(&d.layers, 2, &view, Quality::Exact).into_iter().collect();
        assert_eq!(got.len(), want.len());
        for (c, t) in &want {
            assert!(got[c].pixels() == t.pixels(), "tile {c:?} differs from one drawn afresh");
        }
        assert!(made_bytes(&p) <= 6 * TILE_BYTES);
        // Zoomed in, the dropped tiles the view now shows are made again.
        let near = IRect::new(1024, 0, 2048, 1024);
        let got: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 0, &near, Quality::Exact).into_iter().collect();
        assert_eq!(got.len(), 16);
        let want = Projection::new().update(&d.layers, 0, &near, Quality::Exact);
        for (c, t) in want {
            assert!(got[&c].pixels() == t.pixels(), "tile {c:?}");
        }
    }

    #[test]
    fn the_cache_keeps_no_tile_the_document_let_go_of() {
        // A layer's own tiles shown as they are (opaque), and composites keyed on them
        // (translucent).
        for opacity in [1.0, 0.5] {
            let mut ids = IdGen::default();
            let mut d = Document::new(2048, 256);
            let mut img = RasterImage::new();
            for x in 0..8 {
                let mut t = Tile::filled(pixel_from_f32([0.5, 0.5, 0.5, 1.0]));
                t.set(x, 0, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
                img.set_tile(TileCoord::new(x as i32, 0), Some(Arc::new(t)));
            }
            let l = add(&mut d, &mut ids, img);
            d.layer_mut(l).unwrap().opacity = opacity;
            let mut p = Projection::new();
            p.update(&d.layers, 0, &IRect::new(0, 0, 1024, 256), Quality::Exact);
            p.update(&d.layers, 0, &IRect::new(1024, 0, 2048, 256), Quality::Exact);
            // The whole layer replaced while the view is elsewhere.
            let LayerContent::Objects(objects) = &mut d.layer_mut(l).unwrap().content else { panic!() };
            let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
            let old = Arc::downgrade(o.image.tile(TileCoord::new(0, 0)).unwrap());
            o.image = Arc::new(solid(IRect::from_size(2048, 256), [0.0, 0.0, 1.0, 1.0]));
            p.update(&d.layers, 0, &IRect::new(1024, 0, 2048, 256), Quality::Exact);
            assert!(old.upgrade().is_none(), "the cache keeps the old tile alive (opacity {opacity})");
        }
    }

    #[test]
    fn trimming_keeps_what_costs_nothing() {
        // Composites beside the view (a translucent layer over the opaque one), and the
        // opaque layer's own tiles, farther away.
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 256);
        add(&mut d, &mut ids, solid(IRect::from_size(4096, 256), [0.2, 0.4, 0.6, 1.0]));
        add(&mut d, &mut ids, solid(IRect::new(2048, 0, 3072, 4), [1.0, 0.0, 0.0, 0.5]));
        let mut p = Projection::new();
        p.update(&d.layers, 0, &IRect::new(0, 0, 3072, 256), Quality::Exact);
        p.set_budget(Some(0));
        p.update(&d.layers, 0, &IRect::new(3072, 0, 4096, 256), Quality::Exact);
        for x in 8..12 {
            assert!(!p.levels[0].contains_key(&TileCoord::new(x, 0)), "a composite out of view stays");
        }
        for x in 0..8 {
            assert!(p.levels[0].contains_key(&TileCoord::new(x, 0)), "a tile that cost nothing went");
        }
    }

    #[test]
    fn entries_that_cost_little_are_bounded_too() {
        // A layer's own tiles, 600 of them in a row, panned across end to end.
        let mut ids = IdGen::default();
        let mut d = Document::new(600 * 256, 256);
        let tile = Arc::new(Tile::filled(pixel_from_f32([0.2, 0.4, 0.6, 1.0])));
        let mut img = RasterImage::new();
        for x in 0..600 {
            img.set_tile(TileCoord::new(x, 0), Some(tile.clone()));
        }
        add(&mut d, &mut ids, img);
        let mut p = Projection::new();
        p.set_budget(Some(0));
        for step in 0..150 {
            p.update(&d.layers, 0, &IRect::new(step * 1024, 0, step * 1024 + 1024, 256), Quality::Exact);
        }
        assert!(p.levels[0].len() <= 256, "{} entries kept", p.levels[0].len());
        for x in 596..600 {
            assert!(p.levels[0].contains_key(&TileCoord::new(x, 0)), "a tile in view went");
        }
    }

    #[test]
    fn a_lower_budget_trims_even_an_unchanged_view() {
        let (d, _) = translucent_document();
        let mut p = Projection::new();
        p.update(&d.layers, 0, &IRect::new(0, 0, 1024, 1024), Quality::Exact);
        p.update(&d.layers, 0, &IRect::new(3072, 0, 4096, 1024), Quality::Exact);
        assert_eq!(made_bytes(&p), 32 * TILE_BYTES);
        p.set_budget(Some(16 * TILE_BYTES));
        p.update(&d.layers, 0, &IRect::new(3072, 0, 4096, 1024), Quality::Exact);
        assert_eq!(made_bytes(&p), 16 * TILE_BYTES);
    }

    #[test]
    fn a_layers_own_tiles_shown_as_they_are_cost_nothing() {
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 1024);
        add(&mut d, &mut ids, solid(IRect::from_size(4096, 1024), [0.2, 0.4, 0.6, 1.0]));
        let mut p = Projection::new();
        p.set_budget(Some(0));
        p.update(&d.layers, 0, &IRect::new(0, 0, 1024, 1024), Quality::Exact);
        p.update(&d.layers, 0, &IRect::new(3072, 0, 4096, 1024), Quality::Exact);
        assert_eq!(made_bytes(&p), 0);
        assert!(p.tile(0, TileCoord::new(0, 0)).is_some(), "a tile that took nothing was dropped");
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

    /// Every level up to `top` as the pyramid would show it made from every level-0 tile:
    /// each composited, then reduced four at a time.
    fn from_every_tile(layers: &[Arc<Layer>], top: u32) -> Vec<HashMap<TileCoord, Tile>> {
        let base: HashMap<TileCoord, Tile> =
            occupied_tiles(layers, None).into_iter().filter_map(|c| Some((c, composite_tile(layers, c)?))).collect();
        let mut levels = vec![base];
        for _ in 0..top {
            let below = levels.last().unwrap();
            let up: HashSet<TileCoord> = below.keys().map(|c| c.parent()).collect();
            let next =
                up.into_iter().filter_map(|c| Some((c, downsample(c.children().map(|k| below.get(&k)))?))).collect();
            levels.push(next);
        }
        levels
    }

    /// The image of pixel layer `l`, for changing.
    fn image_mut(d: &mut Document, l: LayerId) -> &mut RasterImage {
        Arc::make_mut(&mut pixels_mut(d, l).image)
    }

    #[test]
    fn uniform_blocks_show_what_reducing_every_tile_beneath_shows() {
        use crate::document::{ImageObject, Object};
        let mut ids = IdGen::default();
        let size: i64 = 256 * 16;
        let mut d = Document::new(size as u32, size as u32);
        let (white, white_tile) = shared(IRect::from_size(size, size), [1.0; 4]);
        let white = add(&mut d, &mut ids, white);
        let mut dot = RasterImage::new();
        dot.set_pixel(3000, 1234, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        add(&mut d, &mut ids, dot);
        // Off the tile grid: alike where the four tiles under each tile are all its one tile.
        let shifted = add(&mut d, &mut ids, shared(IRect::from_size(2048, 2048), [0.0, 0.5, 0.0, 0.5]).0);
        pixels_mut(&mut d, shifted).offset = [700, 333];
        // Translucent, in a translucent group.
        let g = ids.layer();
        d.insert(Position { parent: None, index: d.layers.len() }, Layer::new_group(g, "g")).unwrap();
        let child = ids.layer();
        let blue = shared(IRect::new(2048, 2048, size, size), [0.0, 0.0, 1.0, 1.0]).0;
        d.insert(Position { parent: Some(g), index: 0 }, Layer::new_raster(child, ids.object(), "c", blue)).unwrap();
        d.layer_mut(child).unwrap().opacity = 0.4;
        d.layer_mut(g).unwrap().opacity = 0.5;
        // An image object, over tiles each of its own.
        let image = ImageObject {
            image: Arc::new(shared(IRect::from_size(256, 256), [1.0, 0.0, 1.0, 1.0]).0),
            frame: IRect::from_size(256, 256),
            transform: Affine::rotate(0.3).then(Affine::scale(3.0, 3.0)).then(Affine::translate(3000.0, 600.0)),
            sampling: Sampling::default(),
        };
        let object = Object { id: ids.object(), kind: ObjectKind::Image(image) };
        let picture = ids.layer();
        let index = d.layers.len();
        d.insert(Position { parent: None, index }, Layer::with_objects(picture, "image", vec![object])).unwrap();
        let mut p = Projection::new();
        let views =
            [IRect::from_size(size, size), IRect::new(1000, 1500, 3900, 3700), IRect::new(-500, -500, 300, 300)];
        let check = |p: &mut Projection, d: &Document, what: &str| {
            let want = from_every_tile(&d.layers, 4);
            for level in [3, 0, 1, 4, 2] {
                for view in &views {
                    let shown: HashMap<TileCoord, Arc<Tile>> =
                        p.update(&d.layers, level, view, Quality::Exact).into_iter().collect();
                    let want: Vec<(&TileCoord, &Tile)> = want[level as usize]
                        .iter()
                        .filter(|(c, _)| !c.rect(level).intersect(view).is_empty())
                        .collect();
                    assert_eq!(shown.len(), want.len(), "{what}: level {level}, {view:?}");
                    for (c, t) in want {
                        assert!(shown.get(c).is_some_and(|s| **s == *t), "{what}: level {level}, tile {c:?} differs");
                    }
                }
            }
        };
        check(&mut p, &d, "as made");
        // A tile of the uniform layer changed, then put back: its block is mixed, then alike.
        let at = TileCoord::containing(3500, 3500);
        image_mut(&mut d, white).set_pixel(3500, 3500, pixel_from_f32([0.0, 0.0, 0.0, 1.0]));
        check(&mut p, &d, "a pixel changed");
        image_mut(&mut d, white).set_tile(at, Some(white_tile));
        check(&mut p, &d, "put back");
        // Moved onto the tile grid, off it again, and translucent.
        pixels_mut(&mut d, shifted).offset = [512, 256];
        check(&mut p, &d, "on the grid");
        pixels_mut(&mut d, shifted).offset = [-100, 1800];
        d.layer_mut(white).unwrap().opacity = 0.7;
        check(&mut p, &d, "moved, and the white translucent");
        // The image object moved over what was alike.
        let LayerContent::Objects(objects) = &mut d.layer_mut(picture).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = Affine::rotate(-0.2).then(Affine::translate(3300.0, 3300.0));
        check(&mut p, &d, "the image moved");
        // After a preview, exact again.
        d.layer_mut(child).unwrap().opacity = 0.9;
        p.update(&d.layers, 3, &views[0], Quality::Preview);
        check(&mut p, &d, "after a preview");
    }

    #[test]
    fn zoomed_out_a_uniform_canvas_costs_a_tile_a_block_not_every_tile_beneath() {
        let mut ids = IdGen::default();
        let size: i64 = 256 * 64;
        let mut d = Document::new(size as u32, size as u32);
        let white = add(&mut d, &mut ids, shared(IRect::from_size(size, size), [1.0; 4]).0);
        let all = IRect::from_size(size, size);
        let entries = |p: &Projection| p.levels.iter().map(HashMap::len).sum::<usize>();
        let mut p = Projection::new();
        let shown = p.update(&d.layers, 6, &all, Quality::Exact);
        assert_eq!(shown.len(), 1);
        assert_eq!((entries(&p), p.keys_built), (1, 0), "tiles beneath the block were made");
        // Asked again over part of it, nothing is made again.
        let again = p.update(&d.layers, 6, &IRect::new(10, 10, 500, 500), Quality::Exact);
        assert!(Arc::ptr_eq(&again[0].1, &shown[0].1));
        // A dot: the tiles over it are made from the four below, down to level 0; the
        // others at each level are blocks.
        let red = pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        image_mut(&mut d, white).set_pixel(5000, 9000, red);
        p.update(&d.layers, 6, &all, Quality::Exact);
        assert_eq!((entries(&p), p.keys_built), (1 + 5 * 4 + 4, 4));
        // At level 4, sixteen tiles: fifteen blocks, and the one over the dot.
        let exact: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 4, &all, Quality::Exact).into_iter().collect();
        assert_eq!(exact.len(), 16);
        let over_dot = TileCoord::new(5000 >> 12, 9000 >> 12);
        // Dragged on: a preview keys the level-0 tiles an exact update would make alone,
        // and keeps the blocks, which nothing changed.
        image_mut(&mut d, white).set_pixel(5001, 9000, red);
        let built = p.keys_built;
        let preview: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 4, &all, Quality::Preview).into_iter().collect();
        assert!(p.shows_previews());
        assert_eq!(p.keys_built - built, 4, "a preview built the keys of the tiles beneath the blocks");
        for (c, t) in &exact {
            assert!(*c == over_dot || Arc::ptr_eq(t, &preview[c]), "block {c:?} was drawn again");
        }
        // A change to every block: during a preview, blocks are made exactly (from a tile
        // each), as an exact update makes them.
        d.layer_mut(white).unwrap().opacity = 0.5;
        let preview: HashMap<TileCoord, Arc<Tile>> =
            p.update(&d.layers, 4, &all, Quality::Preview).into_iter().collect();
        let want: HashMap<TileCoord, Arc<Tile>> =
            Projection::new().update(&d.layers, 4, &all, Quality::Exact).into_iter().collect();
        assert_eq!(preview.len(), 16);
        for (c, t) in &want {
            assert!(*c == over_dot || **t == *preview[c], "block {c:?} differs from one drawn exactly");
        }
    }

    #[test]
    fn dropped_uniform_tiles_are_made_again_from_the_layers() {
        let mut ids = IdGen::default();
        let size: i64 = 256 * 16;
        let mut d = Document::new(size as u32, size as u32);
        let mut white = shared(IRect::from_size(size, size), [1.0, 1.0, 1.0, 0.5]).0;
        white.set_pixel(100, 100, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        add(&mut d, &mut ids, white);
        let all = IRect::from_size(size, size);
        let mut p = Projection::new();
        // The blocks at levels 1 to 3, under the view, have their pixels dropped.
        p.set_budget(Some(TILE_BYTES));
        p.update(&d.layers, 4, &all, Quality::Exact);
        let dropped = (1..4).flat_map(|l| p.levels[l].values()).filter(|e| matches!(e.pixels, Pixels::Dropped)).count();
        assert!(dropped >= 3, "only {dropped} tiles were dropped");
        // Zoomed in, shown again: made again from the layers, for none is made below them.
        p.set_budget(None);
        for level in [2, 1] {
            let got: HashMap<TileCoord, Arc<Tile>> =
                p.update(&d.layers, level, &all, Quality::Exact).into_iter().collect();
            let want = Projection::new().update(&d.layers, level, &all, Quality::Exact);
            assert_eq!(got.len(), want.len(), "level {level}");
            for (c, t) in want {
                assert!(got.get(&c).is_some_and(|g| **g == *t), "level {level}: tile {c:?}");
            }
        }
    }
}
