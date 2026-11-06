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

use crate::document::{ImageObject, Layer, LayerContent, Object, ObjectId, ObjectKind, PixelObject};
use crate::geom::{Affine, FRect, IRect};
use crate::raster::RasterImage;
use crate::resample::{self, Drawing, Sampling};
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
            if !touches(&i.image, &i.transform, resample::reach(&i.transform, &i.sampling), c) {
                return None;
            }
            resample::render_tile(&i.image, &i.transform, &i.sampling, c).map(Shown::Drawn)
        }
    }
}

/// Whether `image`, drawn through `transform`, can draw anything into tile `c`: whether the
/// tile, carried back into the image's pixels, comes within `reach` (the filter's, in the
/// image's pixels: see [`resample::reach`]) of its content.
fn touches(image: &RasterImage, transform: &Affine, reach: f64, c: TileCoord) -> bool {
    let (Some(content), Some(inv)) = (image.content_bounds(), transform.inverse()) else { return false };
    let back = inv.bounds(&c.rect(0).to_f());
    let content = content.to_f().expand(reach);
    back.x0 < content.x1 && back.x1 > content.x0 && back.y0 < content.y1 && back.y1 > content.y0
}

/// Object `o` placed at pyramid level `level` (above 0): its pixels, carried by the
/// transform into that level's pixels (a level-`level` tile is then drawn like a level-0
/// one), and how it is drawn there: pixel objects with Bilinear (exact on whole level
/// pixels), image objects as they ask (during a drag, the faster method if the preference
/// says so), but for Nearest, which is drawn by area: as reducing full resolution drawn
/// with it averages, point sampling only where it enlarges.
fn at_level(o: &Object, level: u32) -> (&Arc<RasterImage>, Affine, Drawing) {
    debug_assert!(level > 0, "level 0 is drawn exactly, not at a level");
    let down = Affine::scale(0.5f64.powi(level as i32), 0.5f64.powi(level as i32));
    match &o.kind {
        ObjectKind::Pixels(p) => {
            let t = Affine::translate(p.offset[0] as f64, p.offset[1] as f64).then(down);
            (&p.image, t, Drawing::Sampled(Sampling::DRAFT))
        }
        ObjectKind::Image(i) => {
            let drawing = match i.sampling.method {
                resample::Method::Nearest => Drawing::ByArea,
                _ => Drawing::Sampled(i.sampling),
            };
            (&i.image, i.transform.then(down), drawing)
        }
    }
}

/// Reduced copies of pixel objects' images, made for the tiles drawn at their level (see
/// [`Quality::Preview`], [`costly_image_tiles`]) alone, so their cost and memory follow the
/// screen. Kept by image, so objects showing one image share them; when an object's image
/// changes, the copies of its new image start from those of its last, but for the tiles
/// over what changed.
#[derive(Default, Debug)]
struct Reduced {
    /// By image (its address: each holds its image, so the address cannot be reused).
    images: HashMap<usize, Copies>,
    /// For each pixel object, the image (by address) its copies were last made of.
    last: HashMap<ObjectId, usize>,
    /// For each stored tile repeated over an area (by address; the chain holds the tile, so
    /// the address cannot be reused): it halved from four copies of itself, by level, as
    /// far as asked. Kept from one update to the next while a reduced tile kept is one of
    /// its halvings ([`sweep`](Self::sweep)), so that an edit beneath a view of a large
    /// uniform area does not halve its tile all the way up again.
    halved: HashMap<usize, Vec<Option<Arc<Tile>>>>,
}

/// One image's reduced tiles.
#[derive(Debug)]
struct Copies {
    image: Arc<RasterImage>,
    /// By level, and tile at that level; `None` where nothing is.
    tiles: HashMap<(u32, TileCoord), Option<Arc<Tile>>>,
}

impl Copies {
    /// These copies, carried over to `image`: but for those over a tile in which it and
    /// the image they are of differ (an edit replaces only the tiles it changes, so these
    /// are what it changed).
    fn carried_to(&self, image: &Arc<RasterImage>) -> Copies {
        let mut tiles = self.tiles.clone();
        if !tiles.is_empty() {
            let changed = image.differences(&self.image);
            tiles.retain(|&(l, c), _| {
                let (x0, x1, y0, y1) = (c.x << l, (c.x + 1) << l, c.y << l, (c.y + 1) << l);
                let crossed = |spans: &Vec<(i32, i32)>| spans.iter().any(|s| s.0 < x1 && s.1 > x0);
                // Asked of the rows beneath it, or of those that changed, whichever are fewer.
                let changed_beneath = if ((y1 - y0) as usize) <= changed.len() {
                    (y0..y1).any(|y| changed.get(&y).is_some_and(crossed))
                } else {
                    changed.iter().any(|(&y, spans)| (y0..y1).contains(&y) && crossed(spans))
                };
                !changed_beneath
            });
        }
        Copies { image: image.clone(), tiles }
    }

    /// Tile `c` of level `level` (0: the image's own), if made and with content.
    fn get(&self, level: u32, c: TileCoord) -> Option<&Arc<Tile>> {
        if level == 0 {
            return self.image.tile(c);
        }
        self.tiles.get(&(level, c))?.as_ref()
    }
}

impl Reduced {
    /// Keeps, of what was made, what `reads` says the view reads again: for each pixel
    /// object's image, its tiles at `level`; nothing beneath, and nothing of other images.
    fn keep_only(&mut self, level: u32, reads: &[(ObjectId, Arc<RasterImage>, HashSet<TileCoord>)]) {
        let mut keep: HashMap<usize, HashSet<TileCoord>> = HashMap::new();
        for (_, image, r) in reads {
            keep.entry(Arc::as_ptr(image) as usize).or_default().extend(r);
        }
        self.images.retain(|image, copies| {
            let Some(keep) = keep.get(image) else { return false };
            copies.tiles.retain(|(l, c), _| *l == level && keep.contains(c));
            true
        });
        let objects: HashSet<ObjectId> = reads.iter().map(|r| r.0).collect();
        self.last.retain(|id, _| objects.contains(id));
    }

    /// Keeps, of what was made, the tiles at `level`: what the view at that level may read
    /// again, nothing beneath.
    fn keep_level(&mut self, level: u32) {
        for copies in self.images.values_mut() {
            copies.tiles.retain(|(l, _), _| *l == level);
        }
    }

    /// The addresses of the reduced tiles kept.
    fn kept(&self) -> HashSet<usize> {
        self.images.values().flat_map(|c| c.tiles.values().flatten()).map(|t| Arc::as_ptr(t) as usize).collect()
    }

    /// Lets go of the halvings of repeated tiles that no reduced tile kept is one of: what
    /// the view no longer reads. Once an update, after it has kept what it keeps.
    fn sweep(&mut self) {
        let kept = self.kept();
        self.halved
            .retain(|_, chain| chain.iter().skip(1).flatten().any(|t| kept.contains(&(Arc::as_ptr(t) as usize))));
    }

    /// Tile `c` of `image` reduced to `level`, if it was made and has content.
    fn get<'a>(&'a self, image: &'a Arc<RasterImage>, level: u32, c: TileCoord) -> Option<&'a Arc<Tile>> {
        if level == 0 {
            return image.tile(c);
        }
        let made = self.images.get(&(Arc::as_ptr(image) as usize)).and_then(|copies| copies.tiles.get(&(level, c)));
        debug_assert!(made.is_some(), "reduced tile {c:?} of level {level} read before it was made");
        made?.as_ref()
    }

    /// The bytes of the tiles kept, the halvings of repeated tiles included, each once
    /// however many places hold it.
    fn bytes(&self) -> u64 {
        let mut tiles = self.kept();
        tiles.extend(
            self.halved.values().flat_map(|chain| chain.iter().skip(1).flatten()).map(|t| Arc::as_ptr(t) as usize),
        );
        tiles.len() as u64 * TILE_BYTES
    }

    /// Makes the level-`level` tiles `need` of object `id`'s image `image`, and the finer
    /// ones they are made from, each from the four below it, but for those it has (see
    /// [`Copies::carried_to`]). From the top down, a tile over an area where the image has
    /// one stored tile repeated (or none) is that tile halved to its level, made once a
    /// level, with nothing made beneath it. So a reduced copy of a large uniform area costs
    /// a tile a level, not every tile beneath (as an exact view's uniform blocks).
    fn make(&mut self, id: ObjectId, image: &Arc<RasterImage>, level: u32, need: HashSet<TileCoord>) {
        let key = Arc::as_ptr(image) as usize;
        if !self.images.contains_key(&key) {
            let copies = match self.last.get(&id).and_then(|k| self.images.get(k)) {
                Some(last) => last.carried_to(image),
                None => Copies { image: image.clone(), tiles: HashMap::new() },
            };
            self.images.insert(key, copies);
        }
        self.last.insert(id, key);
        let Reduced { images, halved, .. } = self;
        let copies = images.get_mut(&key).expect("made above");
        let mut wanted = vec![HashSet::new(); level as usize + 1];
        wanted[level as usize] = need;
        for l in (1..=level).rev() {
            let mut mixed = HashSet::new();
            for c in std::mem::take(&mut wanted[l as usize]) {
                if copies.tiles.contains_key(&(l, c)) {
                    continue;
                }
                match image.alike_tile_under(&c.rect(l)) {
                    Some(one) => {
                        copies.tiles.insert((l, c), one.and_then(|t| halved_to(halved, t, l)));
                    }
                    None => {
                        mixed.insert(c);
                    }
                }
            }
            // Below, only where some of the image lies (the rest reads as nothing).
            if l > 1 {
                wanted[l as usize - 1] =
                    mixed.iter().flat_map(|c| c.children()).filter(|k| image.any_under(&k.rect(l - 1))).collect();
            }
            wanted[l as usize] = mixed;
        }
        // Made from the bottom up, each level in parallel; tiles of the same four below (an
        // edge of one tile repeated, say) are made once and shared.
        for l in 1..=level {
            let items: Vec<(TileCoord, [Option<Arc<Tile>>; 4])> = std::mem::take(&mut wanted[l as usize])
                .into_iter()
                .map(|c| (c, c.children().map(|k| copies.get(l - 1, k).cloned())))
                .collect();
            copies.tiles.extend(reduce_shared(items).into_iter().map(|(c, t)| ((l, c), t)));
        }
    }
}

/// What each of `items` reduces to from its four tiles below (top-left, top-right,
/// bottom-left, bottom-right), in parallel; those with the same four (a uniform area is full
/// of them) reduced once, sharing the result.
fn reduce_shared<T: Send>(items: Vec<(T, [Option<Arc<Tile>>; 4])>) -> Vec<(T, Option<Arc<Tile>>)> {
    let address = |t: &Option<Arc<Tile>>| t.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
    let mut unique: HashMap<[usize; 4], [Option<Arc<Tile>>; 4]> = HashMap::new();
    let mut of = Vec::with_capacity(items.len());
    for (item, kids) in items {
        let key = kids.each_ref().map(address);
        unique.entry(key).or_insert(kids);
        of.push((item, key));
    }
    let made: HashMap<[usize; 4], Option<Arc<Tile>>> = unique
        .into_par_iter()
        .map(|(key, kids)| (key, downsample(kids.each_ref().map(|k| k.as_deref())).map(Arc::new)))
        .collect();
    of.into_iter().map(|(item, key)| (item, made[&key].clone())).collect()
}

/// Carries `chain`, a tile (or none) and each next one the one before halved from four
/// copies of itself, as reducing an area of that tile repeated makes it, up to `level`.
fn extend_chain(chain: &mut Vec<Option<Arc<Tile>>>, level: u32) {
    while chain.len() <= level as usize {
        let below = chain.last().expect("the tile at level 0").as_deref();
        chain.push(below.and_then(|t| downsample([Some(t); 4])).map(Arc::new));
    }
}

/// `tile` halved from four copies of itself `level` times, as reducing an area of it
/// repeated makes it; each level's made once, kept in `halved`.
fn halved_to(halved: &mut HashMap<usize, Vec<Option<Arc<Tile>>>>, tile: &Arc<Tile>, level: u32) -> Option<Arc<Tile>> {
    let chain = halved.entry(Arc::as_ptr(tile) as usize).or_insert_with(|| vec![Some(tile.clone())]);
    extend_chain(chain, level);
    chain[level as usize].clone()
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

/// For each visible pixel object of `layers`, the tiles of its image reduced to `level`
/// that drawing the level-`level` tiles `tiles` at that level reads ([`level_tile`]).
fn reduced_reads(
    layers: &[Arc<Layer>],
    level: u32,
    tiles: &HashSet<TileCoord>,
) -> Vec<(ObjectId, Arc<RasterImage>, HashSet<TileCoord>)> {
    let mut out = Vec::new();
    if tiles.is_empty() {
        return out;
    }
    for_each_visible_object(layers, &mut |o| {
        let ObjectKind::Pixels(p) = &o.kind else { return };
        let (image, t, d) = at_level(o, level);
        let reads: HashSet<TileCoord> = tiles
            .iter()
            .filter(|&&c| touches(image, &t, d.reach(&t), c))
            .flat_map(|&c| TileCoord::covering(&reduced_area(p, level, c), 0))
            .collect();
        if !reads.is_empty() {
            out.push((o.id, p.image.clone(), reads));
        }
    });
    out
}

/// What object `o` shows in tile `c` of pyramid level `level`, drawn there directly: a
/// pixel object from its reduced tiles in `reduced` (made beforehand), an image object
/// through its transform.
fn level_tile<'o>(o: &'o Object, c: TileCoord, level: u32, reduced: &Reduced) -> Option<Shown<'o>> {
    let (image, t, d) = at_level(o, level);
    if !touches(image, &t, d.reach(&t), c) {
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
            d.render_tile(&local, &shift, c).map(Shown::Drawn)
        }
        ObjectKind::Image(_) => d.render_tile(image, &t, c).map(Shown::Drawn),
    }
}

/// [`level_tile`] at `level`, as a [`ShowFn`] takes it.
fn shown_at(level: u32, reduced: &Reduced) -> impl for<'o> Fn(&'o Object, TileCoord) -> Option<Shown<'o>> + Sync + '_ {
    move |o, c| level_tile(o, c, level, reduced)
}

/// The token for what object `o` shows in tile `c` of pyramid level `level`.
fn level_token(o: &Object, c: TileCoord, level: u32) -> Option<Token> {
    let (image, t, d) = at_level(o, level);
    touches(image, &t, d.reach(&t), c).then(|| Token::Image {
        image: Arc::downgrade(image),
        transform: t.to_bits(),
        sampling: d.key(),
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
    prepare_except(layers, range, &HashSet::new());
}

/// [`prepare`], but for the image objects in `skip`.
fn prepare_except(layers: &[Arc<Layer>], range: Option<&IRect>, skip: &HashSet<ObjectId>) {
    for_each_visible_object(layers, &mut |o| {
        if let ObjectKind::Image(i) = &o.kind
            && !skip.contains(&o.id)
        {
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

/// What a view zoomed out draws at the level shown rather than exactly (see
/// [`costly_image_tiles`]).
#[derive(Debug, Default)]
struct AtLevel {
    /// The image objects too costly to draw exactly.
    objects: HashSet<ObjectId>,
    /// The tiles they draw into, at the level.
    tiles: HashSet<TileCoord>,
}

/// What the view of the level-`level` tiles over `outer` draws at the level rather than
/// exactly: the visible image objects whose full-resolution tiles there (the level-0 tiles
/// each can draw into, [`tiles_reached`]) would cost too much, and the tiles at the level
/// they draw into. Those of `before` (what the view chose last) stay chosen; others are
/// chosen too, largest first, while the rest would together take more than `limit` bytes;
/// then the chosen go again, smallest first, while the rest would still fit in half that.
/// So panning or zooming near the limit does not switch objects back and forth, making all
/// of one in view at full resolution each time it switches. At level 0 nothing is: that is
/// the image itself.
fn costly_image_tiles(
    layers: &[Arc<Layer>],
    level: u32,
    outer: &IRect,
    limit: u64,
    before: &HashSet<ObjectId>,
) -> AtLevel {
    let mut at = AtLevel::default();
    if level == 0 {
        return at;
    }
    let mut costs: Vec<(u64, ObjectId)> = Vec::new();
    for_each_visible_object(layers, &mut |o| {
        if let ObjectKind::Image(i) = &o.kind {
            costs.push((tiles_reached(i, outer).saturating_mul(TILE_BYTES), o.id));
        }
    });
    // Largest first, then by id, so that the same costs choose the same objects.
    costs.sort_unstable_by(|a, b| b.cmp(a));
    let sum = |costs: &mut dyn Iterator<Item = u64>| costs.fold(0u64, u64::saturating_add);
    at.objects = costs.iter().map(|c| c.1).filter(|id| before.contains(id)).collect();
    let mut rest = sum(&mut costs.iter().filter(|c| !at.objects.contains(&c.1)).map(|c| c.0));
    for &(cost, id) in &costs {
        if rest <= limit {
            break;
        }
        if at.objects.insert(id) {
            rest = rest.saturating_sub(cost);
        }
    }
    for &(cost, id) in costs.iter().rev() {
        if at.objects.contains(&id) && rest.saturating_add(cost) <= limit / 2 {
            at.objects.remove(&id);
            rest = rest.saturating_add(cost);
        }
    }
    for_each_visible_object(layers, &mut |o| {
        if let ObjectKind::Image(i) = &o.kind
            && at.objects.contains(&o.id)
            && let Some(f) = resample::footprint(&i.image, &i.transform, &i.sampling)
        {
            let (image, t, d) = at_level(o, level);
            let reach = d.reach(&t);
            at.tiles.extend(TileCoord::covering(&f.intersect(outer), level).filter(|&c| touches(image, &t, reach, c)));
        }
    });
    at
}

/// How many level-0 tiles within `outer` image object `i` can draw into: those the content
/// of its image crosses, carried by its transform, with its filter's reach; or, if fewer,
/// those each unbroken run of its tiles along a row crosses, together (where its tiles
/// leave much of its bounds empty), counting a tile two runs cross twice.
fn tiles_reached(i: &ImageObject, outer: &IRect) -> u64 {
    let Some(content) = i.image.content_bounds() else { return 0 };
    let reach = resample::reach(&i.transform, &i.sampling);
    let crossed = |r: FRect| tiles_crossed(&r.expand(reach).corners().map(|p| i.transform.apply(p)), outer);
    let whole = crossed(content.to_f());
    // Holding at least half the tiles of its bounds, its runs could only cross more: then
    // they are not walked.
    let size = i64::from(TILE_SIZE);
    let span = |a: i64, b: i64| ((b - 1).div_euclid(size) - a.div_euclid(size) + 1).max(0) as u64;
    let bounds = span(content.x0, content.x1).saturating_mul(span(content.y0, content.y1));
    if i.image.tile_count() as u64 * 2 >= bounds {
        return whole;
    }
    let t = size as f64;
    let runs = i.image.spans().into_iter().fold(0u64, |n, (y, x0, x1)| {
        let r = FRect::new(f64::from(x0) * t, f64::from(y) * t, f64::from(x1) * t, f64::from(y + 1) * t);
        n.saturating_add(crossed(r))
    });
    whole.min(runs)
}

/// How many level-0 tiles within `outer` the convex quadrilateral `quad` crosses. (Being
/// convex, it crosses each row of tiles in one run.)
fn tiles_crossed(quad: &[[f64; 2]; 4], outer: &IRect) -> u64 {
    let size = i64::from(TILE_SIZE);
    let t = size as f64;
    let (top, bottom) = quad.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let rows = (top / t).floor().max(outer.y0.div_euclid(size) as f64) as i64
        ..(bottom / t).floor().min((outer.y1 - 1).div_euclid(size) as f64) as i64 + 1;
    let (first, last) = (outer.x0.div_euclid(size), (outer.x1 - 1).div_euclid(size));
    let mut n = 0u64;
    for row in rows {
        let (y0, y1) = (row as f64 * t, (row + 1) as f64 * t);
        // Where the edges cross the row, and the corners within it, lie between these.
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for k in 0..4 {
            let (p, q) = (quad[k], quad[(k + 1) % 4]);
            let (p, q) = if p[1] <= q[1] { (p, q) } else { (q, p) };
            let (a, b) = (p[1].max(y0), q[1].min(y1));
            if a > b {
                continue;
            }
            let xs = if q[1] > p[1] {
                let at = |y: f64| p[0] + (q[0] - p[0]) * (y - p[1]) / (q[1] - p[1]);
                [at(a), at(b)]
            } else {
                [p[0], q[0]]
            };
            for x in xs {
                (lo, hi) = (lo.min(x), hi.max(x));
            }
        }
        if lo <= hi {
            let from = ((lo / t).floor() as i64).max(first);
            let to = ((hi / t).floor() as i64).min(last);
            n += (to - from + 1).max(0) as u64;
        }
    }
    n
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
/// each area shown. Pixel objects are asked of their own tiles.
#[derive(Debug, Default)]
struct Occupancy {
    images: Vec<Placed>,
}

/// An image object as [`Occupancy`] asks of it.
#[derive(Debug)]
struct Placed {
    /// The rectangle all it draws lies in.
    bounds: IRect,
    image: Arc<RasterImage>,
    /// From document to image pixels.
    back: Affine,
    /// How far its filter reaches, in image pixels.
    reach: f64,
}

impl Occupancy {
    fn of(layers: &[Arc<Layer>]) -> Occupancy {
        let mut images = Vec::new();
        for_each_visible_object(layers, &mut |o| {
            if let ObjectKind::Image(i) = &o.kind
                && let (Some(bounds), Some(back)) =
                    (resample::footprint(&i.image, &i.transform, &i.sampling), i.transform.inverse())
            {
                let reach = resample::reach(&i.transform, &i.sampling);
                images.push(Placed { bounds, image: i.image.clone(), back, reach });
            }
        });
        Occupancy { images }
    }

    /// Whether an image object may draw into `r`: whether `r`, carried back into its
    /// image's pixels, comes within its filter's reach of one of its tiles (as the image's
    /// own index of them says, at a lookup a row).
    fn holds(&self, r: &IRect) -> bool {
        self.images.iter().any(|p| {
            !p.bounds.intersect(r).is_empty()
                && p.image.any_under(&IRect::covering(&p.back.bounds(&r.to_f()).expand(p.reach)))
        })
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

impl Eq for Token {}

/// Hashed as compared: the kind, its numbers, and the addresses it names.
impl std::hash::Hash for Token {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        std::mem::discriminant(self).hash(h);
        match self {
            Token::Layer(o) | Token::Group(o) => o.hash(h),
            Token::Tile(t) => t.as_ptr().hash(h),
            Token::Shifted { phase, tiles } => {
                phase.hash(h);
                for t in tiles {
                    t.as_ref().map(Weak::as_ptr).hash(h);
                }
            }
            Token::Image { image, transform, sampling } => {
                image.as_ptr().hash(h);
                transform.hash(h);
                sampling.hash(h);
            }
            Token::End => {}
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
        ObjectKind::Image(i) => {
            touches(&i.image, &i.transform, resample::reach(&i.transform, &i.sampling), c).then(|| Token::Image {
                image: Arc::downgrade(&i.image),
                transform: i.transform.to_bits(),
                sampling: i.sampling.key(),
            })
        }
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
    /// replaces the tile, unless that too draws it at the level (see
    /// [`costly_image_tiles`]), which it then keeps while the key holds.
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
struct Chains(HashMap<Vec<Token>, Vec<Option<Arc<Tile>>>>);

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
    /// Where image objects draw.
    images: Arc<Occupancy>,
}

impl<'a> Plan<'a> {
    /// The plan for showing the level-`level` tiles over `outer` (whole tiles of that
    /// level) but those in `drawn` (drawn at the level instead), where `occupancy` says the
    /// image objects of `layers` draw.
    fn of(
        layers: &'a [Arc<Layer>],
        level: u32,
        outer: &IRect,
        occupancy: &Arc<Occupancy>,
        drawn: &HashSet<TileCoord>,
    ) -> Plan<'a> {
        let top = level as usize;
        let mut pixels = Vec::new();
        visible_pixel_objects(layers, &mut pixels);
        let empty = vec![HashSet::new(); top + 1];
        let mut plan = Plan { need: empty.clone(), uniform: empty, pixels, images: occupancy.clone() };
        // Each question costs a few lookups: in parallel, they would cost more.
        plan.need[top] =
            TileCoord::covering(outer, level).filter(|&c| !drawn.contains(&c) && plan.occupied(c, level)).collect();
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
        self.images.holds(&r) || self.pixels.iter().any(|p| p.image.any_under(&r.translate(-p.offset[0], -p.offset[1])))
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
    let mut by_key: HashMap<&'k [Token], Vec<TileCoord>> = HashMap::new();
    for (c, key) in todo {
        let Key::Uniform(tokens) = key else { unreachable!("a uniform key") };
        by_key.entry(tokens.as_slice()).or_default().push(c);
    }
    // Each key's chain, taken out to be carried up to the level in parallel.
    let work: Vec<_> =
        by_key.into_iter().map(|(tokens, at)| (tokens, at, chains.0.remove(tokens).unwrap_or_default())).collect();
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
            extend_chain(&mut chain, level);
            (tokens, at, chain)
        })
        .collect();
    let mut made = HashMap::new();
    for (tokens, at, chain) in done {
        let tile = &chain[level as usize];
        made.extend(at.into_iter().map(|c| (c, tile.clone())));
        chains.0.insert(tokens.to_vec(), chain);
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
    occupancy: Arc<Occupancy>,
    /// The last version given (see [`Entry::version`]).
    versions: u64,
    /// How many level-0 keys have been built (for tests).
    keys_built: usize,
    /// How many bytes of tiles the cache may hold (see [`set_budget`](Self::set_budget)).
    budget: Option<u64>,
    /// The uniform tiles made during the update under way; empty between updates.
    chains: Chains,
    /// The image objects the last update drew at the level (see [`costly_image_tiles`]).
    costly: HashSet<ObjectId>,
}

use crate::raster::TILE_BYTES;

/// How [`Projection::update`] brings tiles up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// At full resolution, then reduced: exactly what the image is. Zoomed out, but for the
    /// tiles an image object reaches whose full-resolution tiles in view would cost too much
    /// ([`costly_image_tiles`]), which are drawn at the level shown, as previews are.
    Exact,
    /// While something is dragged: tiles out of date at the level shown (above 0) are drawn
    /// at that level directly, from each object's reduced copy, so their cost follows the
    /// screen rather than the image; tiles still up to date stay exact. The next exact
    /// update recomputes what was previewed (those a costly image object reaches as it
    /// draws them itself, at the level).
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

    /// What the full-resolution tiles of an image object in view may take before its tiles
    /// are drawn at the level shown instead (see [`costly_image_tiles`]): half the cache's
    /// budget, or 1 GB without one.
    fn drawn_limit(&self) -> u64 {
        self.budget.map_or(1 << 30, |b| b / 2)
    }

    /// [`costly_image_tiles`] for the view of the level-`level` tiles over `outer`, which
    /// it remembers for the next update.
    fn costly_at(&mut self, layers: &[Arc<Layer>], level: u32, outer: &IRect) -> AtLevel {
        let at = costly_image_tiles(layers, level, outer, self.drawn_limit(), &self.costly);
        self.costly.clone_from(&at.objects);
        at
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
            self.occupancy = Arc::new(Occupancy::of(layers));
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
                // Zoomed out, the tiles image objects too costly to draw exactly draw into
                // are drawn at the level shown, as previews are; nothing of those objects is
                // drawn at full resolution, so nothing of them is prepared for it.
                let at = self.costly_at(layers, level, &outer);
                prepare_except(layers, Some(&outer), &at.objects);
                let coords = self.update_exact(layers, level, &outer, &at.tiles);
                self.restore(layers, level, coords.iter().copied());
                self.draw_at_level(layers, level, &at.tiles);
                // Of the reduced tiles, those the tiles drawn at the level read stay.
                self.reduced.keep_only(level, &reduced_reads(layers, level, &at.tiles));
                self.shown(level, coords.iter().chain(&at.tiles))
            }
            Quality::Preview => self.update_preview(layers, level, &outer),
        };
        self.trim(level, &outer);
        self.chains = Chains::default();
        self.reduced.sweep();
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
        // Drawn at the level as an exact update draws them: the tiles costly image objects
        // draw into.
        let drawn = self.costly_at(layers, level, outer).tiles;
        let plan = Plan::of(layers, level, outer, &self.occupancy, &drawn);
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
        dirty.retain(|c| !drawn.contains(c));
        // The exact pyramid for the rest; previews already drawn at the level are kept.
        let at_level: HashSet<TileCoord> = dirty.union(&drawn).copied().collect();
        let none = HashSet::new();
        for l in 1..=level {
            let clean = |c: &&TileCoord| !dirty.contains(&up(**c, l));
            let coords: HashSet<TileCoord> = plan.need[l as usize].iter().filter(clean).copied().collect();
            let uniform: HashSet<TileCoord> = plan.uniform[l as usize].iter().filter(clean).copied().collect();
            let keep = if l == level { &at_level } else { &none };
            self.update_pyramid(layers, l, &coords, &uniform, outer, keep);
        }
        let coords: Vec<TileCoord> = plan.need[level as usize].iter().filter(|c| !dirty.contains(c)).copied().collect();
        self.restore(layers, level, coords.iter().copied());
        // The dirty ones, drawn for the drag, and those costly image objects draw into, as
        // an exact update draws them: each where its key changed since it last was.
        self.draw_at_level(layers, level, &at_level);
        // The reduced tiles at the level stay for the drag's length (it comes back over the
        // same ones), those beneath, made to make them, going; but only while they fit the
        // budget with the cache's own tiles: past it, only those this frame reads stay.
        self.reduced.keep_level(level);
        if self
            .budget
            .is_some_and(|b| (self.holders().len() as u64 * TILE_BYTES).saturating_add(self.reduced.bytes()) > b)
        {
            self.reduced.keep_only(level, &reduced_reads(layers, level, &at_level));
        }
        self.shown(level, coords.iter().chain(&at_level))
    }

    /// Draws the level-`level` tiles `tiles` at that level, each object from its own copy
    /// at the level (a pixel object's reduced tiles, an image object's pyramid with its
    /// own sampling: see [`at_level`]), those whose key changed since they last were.
    fn draw_at_level(&mut self, layers: &[Arc<Layer>], level: u32, tiles: &HashSet<TileCoord>) {
        let token = |o: &Object, c: TileCoord| level_token(o, c, level);
        let top = &self.levels[level as usize];
        let mut stale = HashMap::new();
        for &c in tiles {
            let mut tokens = Vec::new();
            key_into(layers, c, &mut tokens, &token);
            let key = Key::Preview(tokens);
            if top.get(&c).is_none_or(|e| e.key != key) {
                stale.insert(c, key);
            }
        }
        if !stale.is_empty() {
            self.draw_stale(layers, level, stale);
        }
    }

    /// [`draw_at_level`](Self::draw_at_level) for the tiles `stale`, each with its key.
    fn draw_stale(&mut self, layers: &[Arc<Layer>], level: u32, stale: HashMap<TileCoord, Key>) {
        let generation = self.generation;
        // What drawing them needs, made first: image objects' pyramids, and the reduced
        // tiles of pixel objects under them alone.
        let coords: HashSet<TileCoord> = stale.keys().copied().collect();
        let extent = coords.iter().map(|c| c.rect(level)).reduce(|a, b| a.union(&b)).expect("some are stale");
        for_each_visible_object(layers, &mut |o| {
            if let ObjectKind::Image(_) = &o.kind
                && o.extent().is_some_and(|e| !e.intersect(&extent).is_empty())
            {
                let (image, t, d) = at_level(o, level);
                d.prepare(image, &t);
            }
        });
        for (id, image, tiles) in reduced_reads(layers, level, &coords) {
            self.reduced.make(id, &image, level, tiles);
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

    /// How many entries hold each tile the cache made (by address).
    fn holders(&self) -> HashMap<usize, usize> {
        let mut holders: HashMap<usize, usize> = HashMap::new();
        for cache in &self.levels {
            for e in cache.values() {
                if let Pixels::Held(t) = &e.pixels {
                    *holders.entry(Arc::as_ptr(t) as usize).or_default() += 1;
                }
            }
        }
        holders
    }

    /// Lets go of cached pixels while those the cache made take more than `budget`: first
    /// the tiles outside `outer` (or above level `level`), farthest from it first, then the
    /// pixels of the finer levels within it, finest first, which keep their keys and versions
    /// (so nothing above them is redrawn) and are made again only when needed. The tiles at
    /// level `level` within `outer`, what the view shows, always stay.
    fn trim_pixels(&mut self, level: u32, outer: &IRect, budget: u64) {
        // Each tile the cache made counts once, however many entries share it.
        let ptr = |t: &Arc<Tile>| Arc::as_ptr(t) as usize;
        let mut holders = self.holders();
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
    fn update_exact(
        &mut self,
        layers: &[Arc<Layer>],
        level: u32,
        outer: &IRect,
        drawn: &HashSet<TileCoord>,
    ) -> HashSet<TileCoord> {
        let mut plan = Plan::of(layers, level, outer, &self.occupancy, drawn);
        self.update_base(layers, &plan.need[0], outer);
        let none = HashSet::new();
        for l in 1..=level {
            let keep = if l == level { drawn } else { &none };
            self.update_pyramid(layers, l, &plan.need[l as usize], &plan.uniform[l as usize], outer, keep);
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
        let items: Vec<_> = stale.into_iter().map(|(c, kids)| ((c, kids), c.children().map(pixels))).collect();
        for ((c, kids), tile) in reduce_shared(items) {
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

    /// Every reduced tile `p` keeps: its image (by address), level and place, and the tile.
    fn reduced_tiles(p: &Projection) -> Vec<(usize, u32, TileCoord, Option<Arc<Tile>>)> {
        let each = |(image, copies): (&usize, &Copies)| {
            copies.tiles.iter().map(move |((l, c), t)| (*image, *l, *c, t.clone())).collect::<Vec<_>>()
        };
        p.reduced.images.iter().flat_map(each).collect()
    }

    /// An image's address, as [`reduced_tiles`] names it.
    fn address(image: &Arc<RasterImage>) -> usize {
        Arc::as_ptr(image) as usize
    }

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
        assert!(!reduced_tiles(&p).is_empty());

        let at = |tiles: &HashMap<TileCoord, Arc<Tile>>, x: i64, y: i64| {
            let c = TileCoord::new((x >> 2).div_euclid(256) as i32, (y >> 2).div_euclid(256) as i32);
            pixel_to_f32(tiles[&c].get(((x >> 2) % 256) as u32, ((y >> 2) % 256) as u32))
        };
        let exact: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 2, &view, Quality::Exact).into_iter().collect();
        assert!(p.reduced.images.is_empty(), "the exact update keeps reduced tiles");
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
        let (sticker_id, big_id) = (ids.object(), ids.object());
        for (name, image, id) in [("sticker", &sticker, sticker_id), ("big", &big, big_id)] {
            let object = Object { id, kind: ObjectKind::Pixels(PixelObject { image: image.clone(), offset: [0, 0] }) };
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
        let kept = reduced_tiles(&p);
        assert!(kept.iter().all(|k| k.0 != address(&sticker)), "the sticker was reduced for a drag far from it");
        assert!(kept.iter().any(|k| k.0 == address(&big)));
        // Below the level shown, only tiles with some of the image under them are made.
        let mut r = Reduced::default();
        r.make(big_id, &big, 3, TileCoord::covering(&view, 3).collect());
        for &(l, c) in r.images[&address(&big)].tiles.keys() {
            assert!(l == 3 || !c.rect(l).intersect(&IRect::from_size(600, 600)).is_empty(), "{l} {c:?}");
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
        let blocks: HashSet<*const Tile> =
            exact.iter().filter(|(c, _)| **c != over_dot).map(|(_, t)| Arc::as_ptr(t)).collect();
        // Each key's tile is made once an update: the three blocks beside the dot's were made
        // by the update before, the twelve others by this one.
        assert_eq!(blocks.len(), 2, "a tile made for each block alike, not one for them all");
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
    fn a_preview_over_a_uniform_layer_reduces_it_without_the_tiles_beneath() {
        use crate::document::{ImageObject, Object};
        // A white canvas of one tile shared, or of copies of it: the second reduces four
        // tiles at a time, as a layer of different tiles would; the first halves the one.
        let scene = |shared_tile: bool| {
            let mut ids = IdGen::default();
            let size: i64 = 256 * 8;
            let mut d = Document::new(size as u32, size as u32);
            let (mut white, tile) = shared(IRect::from_size(size, size), [1.0, 0.9, 0.8, 1.0]);
            if !shared_tile {
                for c in TileCoord::covering(&IRect::from_size(size, size), 0) {
                    white.set_tile(c, Some(Arc::new((*tile).clone())));
                }
            }
            add(&mut d, &mut ids, white);
            // An image object, so that the tile over it is drawn at the level.
            let image = ImageObject {
                image: Arc::new(shared(IRect::from_size(256, 256), [1.0, 0.0, 0.0, 1.0]).0),
                frame: IRect::from_size(256, 256),
                transform: Affine::scale(0.5, 0.5).then(Affine::translate(100.0, 100.0)),
                sampling: Sampling::default(),
            };
            let object = Object { id: ids.object(), kind: ObjectKind::Image(image) };
            let index = d.layers.len();
            d.insert(Position { parent: None, index }, Layer::with_objects(ids.layer(), "image", vec![object]))
                .unwrap();
            let mut p = Projection::new();
            let shown = p.update(&d.layers, 3, &IRect::from_size(size, size), Quality::Preview);
            let mut r = Reduced::default();
            for (id, image, reads) in reduced_reads(&d.layers, 3, &shown.iter().map(|s| s.0).collect()) {
                r.make(id, &image, 3, reads);
            }
            (shown, r.images.values().map(|c| c.tiles.len()).sum::<usize>())
        };
        let (one, made) = scene(true);
        let (copies, made_from_copies) = scene(false);
        assert_eq!(one.len(), 1);
        assert!(*one[0].1 == *copies[0].1, "the halved tile shows otherwise than reducing its copies");
        // The tiles asked for (the level-3 tile and those around it the filter reaches),
        // and nothing beneath; reducing copies makes every tile beneath.
        assert_eq!((made, made_from_copies), (9, 9 + 4 + 16));
    }

    #[test]
    fn a_preview_reduces_tiles_of_the_same_four_below_once() {
        use crate::document::{ImageObject, Object};
        // A white canvas whose last column and row are part tiles, all alike: the blocks over
        // them are mixed, reduced four at a time, and alike along the edge.
        let mut ids = IdGen::default();
        let size: i64 = 256 * 16 + 100;
        let mut d = Document::new(size as u32, size as u32);
        add(&mut d, &mut ids, RasterImage::filled(&IRect::from_size(size, size), pixel_from_f32([1.0; 4])));
        let image = ImageObject {
            image: Arc::new(shared(IRect::from_size(256, 256), [1.0, 0.0, 0.0, 1.0]).0),
            frame: IRect::from_size(256, 256),
            transform: Affine::scale(size as f64 / 256.0, size as f64 / 256.0),
            sampling: Sampling::default(),
        };
        let object = Object { id: ids.object(), kind: ObjectKind::Image(image) };
        let index = d.layers.len();
        d.insert(Position { parent: None, index }, Layer::with_objects(ids.layer(), "image", vec![object])).unwrap();
        let tiles: HashSet<TileCoord> = TileCoord::covering(&IRect::from_size(size, size), 4).collect();
        let mut r = Reduced::default();
        for (id, image, reads) in reduced_reads(&d.layers, 4, &tiles) {
            r.make(id, &image, 4, reads);
        }
        let all: Vec<&Option<Arc<Tile>>> = r.images.values().flat_map(|c| c.tiles.values()).collect();
        let made: HashSet<usize> = all.iter().filter_map(|t| t.as_ref()).map(|t| Arc::as_ptr(t) as usize).collect();
        assert!(all.len() > 40, "{} reduced", all.len());
        assert!(made.len() <= 16, "{} reduced tiles made, not shared", made.len());
    }

    #[test]
    fn zoomed_out_an_image_object_over_a_huge_canvas_costs_the_tiles_shown() {
        // A photograph's worth of distinct pixels, scaled to cover the canvas.
        let size: i64 = 256 * 1024;
        let mut d = picture_over_white(size, picture(512), 0.0, 512.0, Sampling::default());
        let mut p = Projection::new();
        let all = IRect::from_size(size, size);
        let shown = p.update(&d.layers, 10, &all, Quality::Exact);
        assert_eq!(shown.len(), 1);
        let entries: usize = p.levels.iter().map(HashMap::len).sum();
        assert_eq!((entries, p.keys_built), (1, 0), "tiles beneath the image object were made");
        // An edit, and a drag: still only what is shown.
        d.layer_mut(d.layers[1].id).unwrap().opacity = 0.5;
        p.update(&d.layers, 10, &all, Quality::Preview);
        p.update(&d.layers, 10, &all, Quality::Exact);
        let entries: usize = p.levels.iter().map(HashMap::len).sum();
        assert_eq!((entries, p.keys_built), (1, 0));
    }

    /// A picture `side` pixels square, each pixel different from its neighbours.
    fn picture(side: u32) -> RasterImage {
        let n = side as f32;
        let px: Vec<Pixel> = (0..side * side)
            .map(|i| {
                let (x, y) = ((i % side) as f32 / n, (i / side) as f32 / n);
                pixel_from_f32([x, y, 0.5 + 0.5 * (x * 40.0).sin() * (y * 30.0).cos(), 1.0])
            })
            .collect();
        RasterImage::from_pixels(side, side, &px)
    }

    /// The transform that turns a picture `side` pixels square by `turn` and scales it by
    /// `scale` about its centre, which it puts at `at`.
    fn about_centre(side: u32, turn: f64, scale: f64, at: [f64; 2]) -> Affine {
        let half = f64::from(side) / 2.0;
        Affine::translate(-half, -half)
            .then(Affine::rotate(turn))
            .then(Affine::scale(scale, scale))
            .then(Affine::translate(at[0], at[1]))
    }

    /// A layer of one image object: `picture` placed by `transform`, drawn with `sampling`.
    fn picture_layer(ids: &mut IdGen, picture: Arc<RasterImage>, transform: Affine, sampling: Sampling) -> Layer {
        let frame = picture.content_bounds().expect("a picture");
        let image = ImageObject { frame, image: picture, transform, sampling };
        Layer::with_objects(ids.layer(), "picture", vec![Object { id: ids.object(), kind: ObjectKind::Image(image) }])
    }

    /// A document of a white canvas `size` pixels square and an image object of `picture`,
    /// turned by `turn` and scaled by `scale` about the canvas's centre, drawn with
    /// `sampling`.
    fn picture_over_white(size: i64, picture: RasterImage, turn: f64, scale: f64, sampling: Sampling) -> Document {
        let mut ids = IdGen::default();
        let mut d = Document::new(size as u32, size as u32);
        add(&mut d, &mut ids, shared(IRect::from_size(size, size), [1.0; 4]).0);
        let side = picture.content_bounds().unwrap().width() as u32;
        let centre = size as f64 / 2.0;
        let place = about_centre(side, turn, scale, [centre, centre]);
        d.insert(Position { parent: None, index: 1 }, picture_layer(&mut ids, Arc::new(picture), place, sampling))
            .unwrap();
        d
    }

    /// The image object of layer `index` of `d`, and its id.
    fn image_of(d: &Document, index: usize) -> (&ImageObject, ObjectId) {
        let o = &d.layers[index].objects()[0];
        let ObjectKind::Image(i) = &o.kind else { panic!("not an image object") };
        (i, o.id)
    }

    #[test]
    fn an_image_object_too_costly_to_reduce_is_drawn_at_the_level_close_to_the_reduction() {
        // Over 8 × 8 level-3 tiles; the cache's budget so small that the picture's tiles
        // beneath are too costly, so the tiles it draws into are drawn at the level.
        let size: i64 = 8192;
        let d = picture_over_white(size, picture(1024), 0.6, 3.0, Sampling::default());
        let mut p = Projection::new();
        p.set_budget(Some(4 * TILE_BYTES));
        let all = IRect::from_size(size, size);
        let shown: HashMap<TileCoord, Arc<Tile>> = p.update(&d.layers, 3, &all, Quality::Exact).into_iter().collect();
        let drawn = costly_image_tiles(&d.layers, 3, &all, p.drawn_limit(), &HashSet::new()).tiles;
        // The picture's bounds reach more tiles than it draws into, turned: only those are drawn.
        let (i, _) = image_of(&d, 1);
        let bounds = resample::footprint(&i.image, &i.transform, &i.sampling).unwrap();
        let boxed = TileCoord::covering(&bounds.intersect(&all), 3).count();
        assert!(!drawn.is_empty() && drawn.len() < boxed, "{} of the {boxed} its bounds reach drawn", drawn.len());
        let want = from_every_tile(&d.layers, 3);
        assert_eq!(shown.len(), want[3].len());
        for (c, t) in &want[3] {
            let got = &shown[c];
            if !drawn.contains(c) {
                // Where the picture draws nothing (its turned corners' tiles too), exact.
                assert!(**got == *t, "tile {c:?}, which the picture does not reach, differs");
                continue;
            }
            let (mut sum, mut worst) = (0.0f32, 0.0f32);
            for (a, b) in got.pixels().iter().zip(t.pixels()) {
                for i in 0..4 {
                    let e = (a[i].to_f32() - b[i].to_f32()).abs();
                    sum += e;
                    worst = worst.max(e);
                }
            }
            let mean = sum / (4 * TILE_PIXELS) as f32;
            assert!(mean < 0.01 && worst < 0.35, "tile {c:?} drawn at the level: mean {mean}, worst {worst}");
        }
    }

    #[test]
    fn an_image_object_drawn_at_the_level_is_drawn_exactly_again_once_it_costs_little() {
        let size: i64 = 8192;
        let d = picture_over_white(size, picture(1024), 0.6, 3.0, Sampling::default());
        let all = IRect::from_size(size, size);
        let cost = tiles_reached(image_of(&d, 1).0, &all) * TILE_BYTES;
        let view = |p: &mut Projection, budget: u64| -> HashMap<TileCoord, Arc<Tile>> {
            p.set_budget(Some(budget));
            p.update(&d.layers, 3, &all, Quality::Exact).into_iter().collect()
        };
        // Over the limit (half the budget): drawn at the level.
        let mut p = Projection::new();
        view(&mut p, 2 * cost - 2);
        assert!(!p.costly.is_empty());
        // Within it, but over half of it: still so, as it was, so as not to switch back and
        // forth near the limit; a view that never drew it at the level draws it exactly.
        view(&mut p, 3 * cost);
        assert!(!p.costly.is_empty(), "drawn exactly again as soon as it fit");
        let mut fresh = Projection::new();
        view(&mut fresh, 3 * cost);
        assert!(fresh.costly.is_empty());
        // Within half of it: every tile exact again.
        let shown = view(&mut p, 4 * cost);
        assert!(p.costly.is_empty(), "still drawn at the level");
        let want = from_every_tile(&d.layers, 3);
        assert_eq!(shown.len(), want[3].len());
        for (c, t) in &want[3] {
            assert!(*shown[c] == *t, "tile {c:?} is not exact");
        }
    }

    #[test]
    fn image_objects_within_the_limit_one_by_one_but_not_together_are_drawn_at_the_level() {
        let mut ids = IdGen::default();
        let mut d = Document::new(8192, 8192);
        add(&mut d, &mut ids, shared(IRect::from_size(8192, 8192), [1.0; 4]).0);
        // A larger and a smaller.
        let pic = Arc::new(picture(512));
        for (x, scale) in [(2048.0, 6.0), (6144.0, 5.0)] {
            let layer =
                picture_layer(&mut ids, pic.clone(), about_centre(512, 0.0, scale, [x, 4096.0]), Sampling::default());
            let index = d.layers.len();
            d.insert(Position { parent: None, index }, layer).unwrap();
        }
        let all = IRect::from_size(8192, 8192);
        let ([a, b], [big, small]) =
            ([1, 2].map(|k| tiles_reached(image_of(&d, k).0, &all) * TILE_BYTES), [1, 2].map(|k| image_of(&d, k).1));
        assert!(a > b);
        let chosen = |limit: u64, before: &[ObjectId]| {
            costly_image_tiles(&d.layers, 3, &all, limit, &before.iter().copied().collect()).objects
        };
        assert_eq!(chosen(a + b, &[]), HashSet::new());
        assert_eq!(
            chosen(a + b - 1, &[]),
            HashSet::from([big]),
            "together over the limit: the larger drawn at the level"
        );
        assert_eq!(chosen(b - 1, &[]), HashSet::from([big, small]));
        // The smaller chosen last time is enough: it stays, and the larger is not switched.
        assert_eq!(chosen(a + b - 1, &[small]), HashSet::from([small]));
    }

    #[test]
    fn an_image_object_costs_the_tiles_it_crosses_not_those_of_its_bounds() {
        // Whether convex `quad` and rectangle `r` overlap: separated along no axis of either.
        fn overlap(quad: &[[f64; 2]; 4], r: &FRect) -> bool {
            let rect = r.corners();
            let edge = |k: usize| [quad[(k + 1) % 4][0] - quad[k][0], quad[(k + 1) % 4][1] - quad[k][1]];
            let axes = [[1.0, 0.0], [0.0, 1.0], [-edge(0)[1], edge(0)[0]], [-edge(1)[1], edge(1)[0]]];
            axes.iter().all(|a| {
                let span = |ps: &[[f64; 2]; 4]| {
                    ps.iter()
                        .map(|p| p[0] * a[0] + p[1] * a[1])
                        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
                };
                let ((a0, a1), (b0, b1)) = (span(quad), span(&rect));
                a0 <= b1 && b0 <= a1
            })
        }
        // Cut off on the right.
        let outer = IRect::new(-1024, -512, 6400, 7168);
        let image = Arc::new(picture(512));
        for turn in [0.0, 0.3, std::f64::consts::FRAC_PI_4, 2.0] {
            let transform = about_centre(512, turn, 9.0, [4100.5, 3900.25]);
            let i = ImageObject {
                frame: IRect::from_size(512, 512),
                image: image.clone(),
                transform,
                sampling: Sampling::default(),
            };
            let reach = resample::reach(&i.transform, &i.sampling);
            let quad = IRect::from_size(512, 512).to_f().expand(reach).corners().map(|p| i.transform.apply(p));
            let crossed = TileCoord::covering(&outer, 0).filter(|c| overlap(&quad, &c.rect(0).to_f())).count() as u64;
            assert_eq!(tiles_reached(&i, &outer), crossed, "turned {turn}");
            if turn == std::f64::consts::FRAC_PI_4 {
                let bounds = resample::footprint(&i.image, &i.transform, &i.sampling).unwrap();
                let boxed = TileCoord::covering(&bounds.intersect(&outer), 0).count() as u64;
                assert!(crossed * 10 < boxed * 7, "{crossed} of the {boxed} its bounds hold");
            }
        }

        // An image of two tiles far apart, enlarged: about what the two cross, never less,
        // not what lies between them.
        let mut sparse = RasterImage::new();
        for c in [TileCoord::new(0, 0), TileCoord::new(5, 3)] {
            sparse.set_tile(c, Some(Arc::new(Tile::filled(pixel_from_f32([0.2, 0.4, 0.6, 1.0])))));
        }
        let frame = sparse.content_bounds().unwrap();
        let transform = Affine::rotate(0.3).then(Affine::scale(4.0, 4.0)).then(Affine::translate(300.0, -200.0));
        let i = ImageObject { frame, image: Arc::new(sparse), transform, sampling: Sampling::default() };
        let reach = resample::reach(&i.transform, &i.sampling);
        let quads: Vec<[[f64; 2]; 4]> = i
            .image
            .tiles()
            .map(|(c, _)| c.rect(0).to_f().expand(reach).corners().map(|p| i.transform.apply(p)))
            .collect();
        let crossed = TileCoord::covering(&outer, 0)
            .filter(|c| quads.iter().any(|q| overlap(q, &c.rect(0).to_f())))
            .count() as u64;
        let bounds = resample::footprint(&i.image, &i.transform, &i.sampling).unwrap();
        let boxed = TileCoord::covering(&bounds.intersect(&outer), 0).count() as u64;
        let reached = tiles_reached(&i, &outer);
        assert!(reached >= crossed && reached <= 2 * crossed, "{reached} counted, {crossed} crossed");
        assert!(reached * 3 < boxed, "{reached} counted, of the {boxed} its bounds hold");
    }

    #[test]
    fn an_image_object_shown_exactly_is_prepared_even_where_its_image_is_shown_at_the_level() {
        // One image twice: enlarged across the canvas (too costly, drawn at the level), and
        // shrunk to a quarter (drawn exactly, from its image's pyramid).
        let mut ids = IdGen::default();
        let mut d = Document::new(8192, 8192);
        let pic = Arc::new(picture(512));
        for (scale, at) in [(16.0, 4096.0), (0.25, 300.0)] {
            let layer =
                picture_layer(&mut ids, pic.clone(), about_centre(512, 0.0, scale, [at, at]), Sampling::default());
            let index = d.layers.len();
            d.insert(Position { parent: None, index }, layer).unwrap();
        }
        let all = IRect::from_size(8192, 8192);
        let at = costly_image_tiles(&d.layers, 3, &all, 4 * TILE_BYTES, &HashSet::new());
        assert_eq!(at.objects, HashSet::from([image_of(&d, 0).1]));
        prepare_except(&d.layers, Some(&all), &at.objects);
        assert!((1..=4).any(|k| pic.has_reduced(k)), "the shrunk copy was not prepared");
    }

    #[test]
    fn where_an_image_object_may_draw_covers_all_it_draws() {
        // An image of two tiles far apart, turned and enlarged; and as it is, ending 1.5
        // pixels short of the canvas's tiles, so that only its filter (Lanczos, whose
        // third lobe is positive) reaches the next.
        let mut img = RasterImage::new();
        for c in [TileCoord::new(0, 0), TileCoord::new(5, 3)] {
            img.set_tile(c, Some(Arc::new(Tile::filled(pixel_from_f32([0.2, 0.4, 0.6, 1.0])))));
        }
        let img = Arc::new(img);
        let turned = Affine::rotate(0.5).then(Affine::scale(1.7, 1.7)).then(Affine::translate(2000.0, 100.0));
        let lanczos = Sampling { method: resample::Method::Lanczos { lobes: 3 }, anti_ringing: false };
        for (transform, sampling) in [(turned, Sampling::default()), (Affine::translate(-1.5, -1.5), lanczos)] {
            let mut ids = IdGen::default();
            let mut d = Document::new(8192, 8192);
            let layer = picture_layer(&mut ids, img.clone(), transform, sampling);
            d.insert(Position { parent: None, index: 0 }, layer).unwrap();
            let occupancy = Occupancy::of(&d.layers);
            let (i, _) = image_of(&d, 0);
            let (mut drawn, mut held) = (0, 0);
            for c in TileCoord::covering(&IRect::new(-512, -512, 8192, 8192), 0) {
                let draws = resample::render_tile(&i.image, &i.transform, &i.sampling, c).is_some();
                let holds = occupancy.holds(&c.rect(0));
                assert!(holds || !draws, "tile {c:?}, which it draws into, is not held");
                drawn += usize::from(draws);
                held += usize::from(holds);
            }
            // Not everything its bounds hold, the space between its two tiles above all.
            let bounds = resample::footprint(&i.image, &i.transform, &i.sampling).unwrap();
            assert!(drawn > 0 && held < TileCoord::covering(&bounds, 0).count() / 2, "{held} held, {drawn} drawn into");
        }
    }

    #[test]
    fn beneath_an_image_object_drawn_at_the_level_an_edit_reduces_only_what_it_changed() {
        fn view(p: &mut Projection, d: &Document, rect: &IRect) -> HashMap<TileCoord, Arc<Tile>> {
            p.set_budget(Some(4 * TILE_BYTES));
            p.update(&d.layers, 2, rect, Quality::Exact).into_iter().collect()
        }
        // A photograph of 2 × 2 tiles at level 2, under a picture drawn at the level.
        let (mut d, photo_image) = photo_under_picture(1);
        let photo = d.layers[0].id;
        let all = IRect::from_size(4096, 4096);
        let mut p = Projection::new();
        view(&mut p, &d, &all);
        assert!(!p.costly.is_empty());
        let before = reduced_tiles(&p);
        assert!(before.iter().all(|t| t.0 == address(&photo_image) && t.1 == 2), "kept tiles beneath the level shown");

        // One pixel of the photograph's first tile changed.
        let LayerContent::Objects(objects) = &mut d.layer_mut(photo).unwrap().content else { panic!() };
        let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut o.image).set_pixel(10, 10, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        let shown = view(&mut p, &d, &all);
        let after = reduced_tiles(&p);
        let at = |tiles: &[(usize, u32, TileCoord, Option<Arc<Tile>>)], x: i32, y: i32| {
            tiles.iter().find(|t| t.2 == TileCoord::new(x, y)).and_then(|t| t.3.clone()).expect("a reduced tile")
        };
        assert!(!Arc::ptr_eq(&at(&before, 0, 0), &at(&after, 0, 0)), "the tile over the edit was not made again");
        for (x, y) in [(1, 0), (0, 1), (1, 1)] {
            assert!(
                Arc::ptr_eq(&at(&before, x, y), &at(&after, x, y)),
                "({x}, {y}), which the edit left alone, was made again"
            );
        }
        let fresh = view(&mut Projection::new(), &d, &all);
        assert_eq!(shown.len(), fresh.len());
        for (c, t) in &fresh {
            assert!(**t == *shown[c], "tile {c:?} differs from a view made afresh");
        }

        // Shown in part, only what that part reads is kept.
        view(&mut p, &d, &IRect::from_size(1024, 1024));
        let part = reduced_tiles(&p);
        assert!(!part.is_empty() && part.len() < after.len());
        assert!(part.iter().all(|t| t.2.x <= 1 && t.2.y <= 1), "{:?}", part.iter().map(|t| t.2).collect::<Vec<_>>());
    }

    /// A document 4096 pixels square of a photograph 2048 pixels square at its top-left,
    /// shown by `copies` pixel objects of one image, and a picture across it all, seen
    /// through, turned; and the photograph's image.
    fn photo_under_picture(copies: usize) -> (Document, Arc<RasterImage>) {
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 4096);
        let photo = Arc::new(picture(2048));
        for k in 0..copies {
            let pixels = PixelObject { image: photo.clone(), offset: [0, 0] };
            let object = Object { id: ids.object(), kind: ObjectKind::Pixels(pixels) };
            d.insert(Position { parent: None, index: k }, Layer::with_objects(ids.layer(), "photo", vec![object]))
                .unwrap();
        }
        let place = about_centre(512, 0.2, 10.0, [2048.0, 2048.0]);
        let over = picture_layer(&mut ids, Arc::new(picture(512)), place, Sampling::default());
        let over_layer = over.id;
        d.insert(Position { parent: None, index: copies }, over).unwrap();
        d.layer_mut(over_layer).unwrap().opacity = 0.5;
        (d, photo)
    }

    #[test]
    fn an_edit_beneath_keeps_the_tile_a_uniform_layer_repeats_halved() {
        // A white canvas, one tile repeated, under a picture drawn at level 3.
        let size: i64 = 8192;
        let mut d = picture_over_white(size, picture(512), 0.3, 12.0, Sampling::default());
        let mut p = Projection::new();
        p.set_budget(Some(4 * TILE_BYTES));
        let all = IRect::from_size(size, size);
        p.update(&d.layers, 3, &all, Quality::Exact);
        assert!(!p.costly.is_empty());
        let ObjectKind::Pixels(white) = &d.layers[0].objects()[0].kind else { panic!() };
        let white = Arc::as_ptr(white.image.tile(TileCoord::new(0, 0)).unwrap()) as usize;
        let halved = |p: &Projection| p.reduced.halved.get(&white).and_then(|chain| chain.get(3).cloned().flatten());
        let before = halved(&p).expect("the white tile halved to level 3");
        let same = |p: &Projection| halved(p).is_some_and(|now| Arc::ptr_eq(&before, &now));
        // The picture's opacity changed, which halves nothing anew; then the picture
        // dragged, a frame over the budget: the halving stays, read by the view all along.
        let picture = d.layers[1].id;
        d.layer_mut(picture).unwrap().opacity = 0.5;
        p.update(&d.layers, 3, &all, Quality::Exact);
        assert!(same(&p), "let go by an update that halved nothing anew");
        let LayerContent::Objects(objects) = &mut d.layer_mut(picture).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = i.transform.then(Affine::translate(40.0, 0.0));
        p.update(&d.layers, 3, &all, Quality::Preview);
        assert!(p.shows_previews());
        assert!(same(&p), "let go by a drag frame over the budget");
        p.update(&d.layers, 3, &all, Quality::Exact);
        // A pixel far from the first tile changed: the white tile is halved no more.
        let canvas = d.layers[0].id;
        let LayerContent::Objects(objects) = &mut d.layer_mut(canvas).unwrap().content else { panic!() };
        let ObjectKind::Pixels(o) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut o.image).set_pixel(5000, 5000, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        p.update(&d.layers, 3, &all, Quality::Exact);
        assert!(same(&p), "halved again");
        // Counted against the budget, the halvings beneath the level too, each once.
        let mut counted = p.reduced.kept();
        let beneath = p.reduced.halved[&white].iter().skip(1).flatten().map(|t| Arc::as_ptr(t) as usize);
        counted.extend(beneath);
        assert!(counted.len() > p.reduced.kept().len(), "no halving beneath the level to count");
        assert_eq!(p.reduced.bytes(), counted.len() as u64 * TILE_BYTES);
    }

    #[test]
    fn objects_showing_one_image_share_its_reduced_tiles() {
        let (d, photo) = photo_under_picture(2);
        let mut p = Projection::new();
        p.set_budget(Some(4 * TILE_BYTES));
        p.update(&d.layers, 2, &IRect::from_size(4096, 4096), Quality::Exact);
        assert!(!p.costly.is_empty());
        let images: HashSet<usize> = reduced_tiles(&p).iter().map(|t| t.0).collect();
        assert_eq!(images, HashSet::from([address(&photo)]), "each object reduced it apart");
    }

    #[test]
    fn a_drag_keeps_the_reduced_tiles_at_the_level_for_its_length() {
        // A small picture dragged across a layer of 4 × 4 tiles at level 2, from its
        // top-left, through the middle, to the far corner.
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 4096);
        let photo = Arc::new(shared(IRect::from_size(4096, 4096), [0.3, 0.5, 0.7, 1.0]).0);
        let pixels = PixelObject { image: photo.clone(), offset: [0, 0] };
        let object = Object { id: ids.object(), kind: ObjectKind::Pixels(pixels) };
        d.insert(Position { parent: None, index: 0 }, Layer::with_objects(ids.layer(), "photo", vec![object])).unwrap();
        let sticker =
            picture_layer(&mut ids, Arc::new(picture(64)), Affine::translate(100.0, 100.0), Sampling::default());
        let sticker_layer = sticker.id;
        d.insert(Position { parent: None, index: 1 }, sticker).unwrap();
        let all = IRect::from_size(4096, 4096);
        let mut p = Projection::new();
        p.update(&d.layers, 2, &all, Quality::Exact);
        let place = |d: &mut Document, x: f64| {
            let LayerContent::Objects(objects) = &mut d.layer_mut(sticker_layer).unwrap().content else { panic!() };
            let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
            i.transform = Affine::translate(x, x);
        };
        // Through the middle, which reads the layer's reduced tile (2, 1); then to the far
        // corner, where neither its old place nor its new one reads that.
        place(&mut d, 1100.0);
        p.update(&d.layers, 2, &all, Quality::Preview);
        let first = reduced_tiles(&p);
        place(&mut d, 3100.0);
        p.update(&d.layers, 2, &all, Quality::Preview);
        let later = reduced_tiles(&p);
        let tile = |v: &[(usize, u32, TileCoord, Option<Arc<Tile>>)]| {
            v.iter().find(|t| t.2 == TileCoord::new(2, 1)).and_then(|t| t.3.clone())
        };
        let (a, b) = (tile(&first).expect("read in the middle"), tile(&later));
        assert!(b.is_some_and(|b| Arc::ptr_eq(&a, &b)), "the tiles it left were let go during the drag");
        assert!(later.iter().all(|t| t.1 == 2), "tiles beneath the level kept");
        // Let go of: exact again, and nothing at the level drawn, nothing kept.
        p.update(&d.layers, 2, &all, Quality::Exact);
        assert!(reduced_tiles(&p).is_empty());

        // On a budget they do not fit beside the cache's own tiles (each tile counted once,
        // and this layer's are all one): only what a frame reads.
        let mut tight = Projection::new();
        tight.set_budget(Some(TILE_BYTES));
        tight.update(&d.layers, 2, &all, Quality::Exact);
        for x in [1100.0, 3100.0] {
            place(&mut d, x);
            tight.update(&d.layers, 2, &all, Quality::Preview);
        }
        assert!(tile(&reduced_tiles(&tight)).is_none(), "kept past the budget");
    }

    /// Stripes a pixel wide (each row its own value) drawn with Nearest, placed by
    /// `transform` on a canvas 4096 pixels square, too costly to draw exactly: the level-2
    /// tiles a view shows, and those reducing full resolution makes.
    fn nearest_stripes(transform: Affine) -> (HashMap<TileCoord, Arc<Tile>>, HashMap<TileCoord, Tile>) {
        let (w, h) = (256u32, 1024u32);
        let px: Vec<Pixel> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                pixel_from_f32([(x % 2) as f32, (y % 7) as f32 / 7.0, 0.5, 1.0])
            })
            .collect();
        let nearest = Sampling { method: resample::Method::Nearest, anti_ringing: false };
        let mut ids = IdGen::default();
        let mut d = Document::new(4096, 4096);
        let stripes = Arc::new(RasterImage::from_pixels(w, h, &px));
        d.insert(Position { parent: None, index: 0 }, picture_layer(&mut ids, stripes, transform, nearest)).unwrap();
        let mut p = Projection::new();
        p.set_budget(Some(4 * TILE_BYTES));
        let shown = p.update(&d.layers, 2, &IRect::from_size(4096, 4096), Quality::Exact).into_iter().collect();
        assert!(!p.costly.is_empty());
        let mut want = from_every_tile(&d.layers, 2);
        (shown, want.swap_remove(2))
    }

    #[test]
    fn nearest_drawn_at_the_level_is_what_reducing_it_from_full_resolution_gives() {
        // Stretched 8 times across and kept as tall: at level 2, enlarged one way and shrunk
        // the other.
        let (shown, want) = nearest_stripes(Affine::scale(8.0, 1.0));
        for (c, t) in &want {
            let worst = shown[c]
                .pixels()
                .iter()
                .zip(t.pixels())
                .flat_map(|(a, b)| (0..4).map(move |i| (a[i].to_f32() - b[i].to_f32()).abs()))
                .fold(0.0f32, f32::max);
            assert!(worst < 0.01, "tile {c:?} is {worst} from the reduction");
        }
        assert_eq!(shown.len(), want.len(), "drawn past the stripes' edge");
    }

    #[test]
    fn nearest_turned_and_drawn_at_the_level_keeps_its_enlarged_axis_sharp() {
        // Stretched across, squashed down 16 times at level 2, and turned: each stripe two
        // level pixels wide, as reducing full resolution keeps it, not averaged with the next.
        let place =
            Affine::scale(8.0, 0.5).then(Affine::rotate(10f64.to_radians())).then(Affine::translate(800.0, 100.0));
        let (shown, want) = nearest_stripes(place);
        let (mut sum, mut n) = (0.0f32, 0usize);
        for (c, t) in &want {
            for (a, b) in shown[c].pixels().iter().zip(t.pixels()) {
                if b[3].to_f32() > 0.99 {
                    sum += (a[0].to_f32() - b[0].to_f32()).abs();
                    n += 1;
                }
            }
        }
        let mean = sum / n as f32;
        assert!(mean < 0.05, "{mean} from the reduction, on average");
    }

    #[test]
    fn a_view_drawn_at_the_level_never_point_samples_an_image_object_it_shrinks() {
        // A one-pixel checkerboard drawn with Nearest, shrunk to a quarter: averaged, grey,
        // whether drawn at the level for its cost, or for a drag.
        let px: Vec<Pixel> = (0..512 * 512)
            .map(|i| {
                if (i % 512 + i / 512) % 2 == 0 {
                    pixel_from_f32([1.0; 4])
                } else {
                    pixel_from_f32([0.0, 0.0, 0.0, 1.0])
                }
            })
            .collect();
        let nearest = Sampling { method: resample::Method::Nearest, anti_ringing: false };
        let mut d = picture_over_white(4096, RasterImage::from_pixels(512, 512, &px), 0.0, 4.0, nearest);
        let all = IRect::from_size(4096, 4096);
        let grey = |t: &Tile| {
            for (x, y) in [(128, 128), (100, 140), (150, 90)] {
                let v = pixel_to_f32(t.get(x, y))[0];
                assert!((v - 0.5).abs() < 0.1, "({x}, {y}) is {v}: point-sampled, not averaged");
            }
        };
        // Moved by a level-4 pixel while dragged: the tile it moved in drawn at the level.
        let mut p = Projection::new();
        p.update(&d.layers, 4, &all, Quality::Exact);
        let LayerContent::Objects(objects) = &mut d.layer_mut(d.layers[1].id).unwrap().content else { panic!() };
        let ObjectKind::Image(i) = &mut objects[0].kind else { panic!() };
        i.transform = i.transform.then(Affine::translate(16.0, 16.0));
        grey(&p.update(&d.layers, 4, &all, Quality::Preview)[0].1);
        assert!(p.shows_previews());
        let mut costly = Projection::new();
        costly.set_budget(Some(4 * TILE_BYTES));
        grey(&costly.update(&d.layers, 4, &all, Quality::Exact)[0].1);
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
