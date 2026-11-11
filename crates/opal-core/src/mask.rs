//! A selection's mask (design.md, *Selections*, *The mask*): how selected each pixel of the
//! plane is, from 0 (not at all) to 1 (fully), in 16-bit levels ([`FULL`] is 1).
//!
//! It is kept in tiles on the images' pixel grid ([`TILE_SIZE`]), sparsely: a tile not kept
//! has the mask's level *outside* (0, or 1 once inverted, so that a mask can select the whole
//! plane but for a hole), and a tile all of one level is kept as that level alone, so a
//! selection across a huge canvas costs its edges. Copies share their tiles until one changes,
//! as the history's states share images'.
//!
//! Every operation limited to a selection goes through one step, [`blend`]: the pixel it would
//! make, weighed against the one there by the mask's level.

use crate::geom::{FRect, IRect};
use crate::raster::RasterImage;
use crate::tile::{PixelF32, TILE_PIXELS, TILE_SIZE, Tile, TileCoord, pixel_from_f32};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

/// The level of a pixel fully selected.
pub const FULL: u16 = u16::MAX;

/// Where the marching ants run by default: where the mask crosses half.
pub const HALF: u16 = FULL / 2 + 1;

/// `level` as a fraction of 1.
pub fn fraction(level: u16) -> f32 {
    f32::from(level) / f32::from(FULL)
}

/// The level nearest the fraction `f` (taken as 0 or 1 past them).
pub fn level(f: f32) -> u16 {
    (f.clamp(0.0, 1.0) * f32::from(FULL)).round() as u16
}

/// A tile of a mask: all one level, or a level a pixel, row by row.
#[derive(Clone, Debug, PartialEq)]
pub enum MaskTile {
    Level(u16),
    Levels(Arc<[u16]>),
}

impl MaskTile {
    /// The level of the tile's pixel `(x, y)`.
    pub fn get(&self, x: u32, y: u32) -> u16 {
        match self {
            MaskTile::Level(l) => *l,
            MaskTile::Levels(l) => l[(y * TILE_SIZE + x) as usize],
        }
    }

    /// The tile `levels` (a tile's worth) make: one level alone where all are alike.
    fn from_levels(levels: Vec<u16>) -> MaskTile {
        debug_assert_eq!(levels.len(), TILE_PIXELS);
        let first = levels[0];
        if levels.iter().all(|&l| l == first) { MaskTile::Level(first) } else { MaskTile::Levels(levels.into()) }
    }

    /// Its levels each made `f` of themselves.
    fn map(&self, f: impl Fn(u16) -> u16) -> MaskTile {
        match self {
            MaskTile::Level(l) => MaskTile::Level(f(*l)),
            MaskTile::Levels(l) => MaskTile::from_levels(l.iter().map(|&v| f(v)).collect()),
        }
    }

    /// The memory its levels take (none for a tile of one level).
    pub fn bytes(&self) -> u64 {
        match self {
            MaskTile::Level(_) => 0,
            MaskTile::Levels(l) => (l.len() * std::mem::size_of::<u16>()) as u64,
        }
    }

    /// Its levels, a tile's worth.
    fn levels(&self) -> std::borrow::Cow<'_, [u16]> {
        match self {
            MaskTile::Level(l) => std::borrow::Cow::Owned(vec![*l; TILE_PIXELS]),
            MaskTile::Levels(l) => std::borrow::Cow::Borrowed(l),
        }
    }
}

/// How much of the plane a mask selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// Nothing at all.
    Nothing,
    /// Nothing outside this rectangle (the smallest that holds every pixel selected at all).
    Within(IRect),
    /// Without bound: every pixel far enough out (the mask has been inverted).
    Unbounded,
}

/// How a mask made by a step combines with the selection before it (design.md, *The
/// recipe*).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    /// It is the selection; the one before goes.
    #[default]
    Replace,
    /// Selected where either selects (the greater level).
    Add,
    /// Selected where the one before selects and it does not (the lesser of the one
    /// before's level and the opposite of its).
    Subtract,
    /// Selected where both select (the lesser level).
    Intersect,
}

impl Combine {
    /// The level a pixel gets where the selection before has `before` and the new mask `new`.
    pub fn level(self, before: u16, new: u16) -> u16 {
        match self {
            Combine::Replace => new,
            Combine::Add => before.max(new),
            Combine::Subtract => before.min(FULL - new),
            Combine::Intersect => before.min(new),
        }
    }
}

/// What a mask can be made of: how much of each pixel a shape covers.
pub trait Coverage: Sync {
    /// The pixels outside of which it covers nothing.
    fn bounds(&self) -> IRect;

    /// The distance from `(x, y)` to its edge, less than 0 inside it: never more than the
    /// true distance inside, nor less outside, so that a whole area can be told inside or
    /// outside from its middle (a tile's from the middle of the tile).
    fn distance(&self, x: f64, y: f64) -> f64;

    /// How much of the pixel whose middle is `(x, y)` it covers, from 0 to 1.
    fn at(&self, x: f64, y: f64) -> f32;
}

/// A selection's mask: see the module's description.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mask {
    /// The tiles of other levels than `outside` (never one of that level alone).
    tiles: HashMap<TileCoord, MaskTile>,
    outside: u16,
}

impl Mask {
    /// A mask that selects nothing.
    pub fn none() -> Mask {
        Mask::default()
    }

    /// A mask that selects the whole plane.
    pub fn all() -> Mask {
        Mask { tiles: HashMap::new(), outside: FULL }
    }

    /// The level of every pixel no tile is kept for.
    pub fn outside(&self) -> u16 {
        self.outside
    }

    /// The tiles kept (every one of another level than [`outside`](Self::outside) somewhere).
    pub fn tiles(&self) -> impl Iterator<Item = (TileCoord, &MaskTile)> {
        self.tiles.iter().map(|(c, t)| (*c, t))
    }

    /// The tile at `c`, if one is kept there.
    pub fn tile(&self, c: TileCoord) -> Option<&MaskTile> {
        self.tiles.get(&c)
    }

    /// Keeps `tile` at `c`, or none there if it is all of the level outside.
    fn put(&mut self, c: TileCoord, tile: MaskTile) {
        if tile == MaskTile::Level(self.outside) {
            self.tiles.remove(&c);
        } else {
            self.tiles.insert(c, tile);
        }
    }

    /// The level of pixel `(x, y)`.
    pub fn level_at(&self, x: i64, y: i64) -> u16 {
        let c = TileCoord::containing(x, y);
        match self.tiles.get(&c) {
            None => self.outside,
            Some(t) => {
                let r = c.rect(0);
                t.get((x - r.x0) as u32, (y - r.y0) as u32)
            }
        }
    }

    /// Whether it selects nothing at all.
    pub fn is_none(&self) -> bool {
        self.outside == 0 && self.tiles.is_empty()
    }

    /// The memory its tiles take.
    pub fn bytes(&self) -> u64 {
        self.tiles.values().map(MaskTile::bytes).sum()
    }

    /// How much of the plane it selects.
    pub fn extent(&self) -> Extent {
        if self.outside != 0 {
            return Extent::Unbounded;
        }
        let mut within: Option<IRect> = None;
        for (c, t) in &self.tiles {
            let r = c.rect(0);
            let part = match t {
                MaskTile::Level(_) => r,
                MaskTile::Levels(l) => {
                    let (mut x0, mut y0, mut x1, mut y1) = (TILE_SIZE, TILE_SIZE, 0, 0);
                    for y in 0..TILE_SIZE {
                        let row = &l[(y * TILE_SIZE) as usize..((y + 1) * TILE_SIZE) as usize];
                        let Some(first) = row.iter().position(|&v| v > 0) else { continue };
                        let last = row.iter().rposition(|&v| v > 0).expect("one is above 0");
                        (x0, x1, y0, y1) = (x0.min(first as u32), x1.max(last as u32 + 1), y0.min(y), y + 1);
                    }
                    IRect::new(r.x0 + i64::from(x0), r.y0 + i64::from(y0), r.x0 + i64::from(x1), r.y0 + i64::from(y1))
                }
            };
            within = Some(within.map_or(part, |w| w.union(&part)));
        }
        within.map_or(Extent::Nothing, Extent::Within)
    }

    /// The mask of `shape`: each pixel's level, how much of it the shape covers.
    pub fn of(shape: &impl Coverage) -> Mask {
        let half_diagonal = f64::from(TILE_SIZE) * std::f64::consts::FRAC_1_SQRT_2;
        let tiles: Vec<(TileCoord, MaskTile)> = TileCoord::covering(&shape.bounds(), 0)
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(|c| {
                let r = c.rect(0).to_f();
                let [cx, cy] = r.center();
                // Told from its middle: every pixel's middle is nearer it than the corners.
                let d = shape.distance(cx, cy);
                if d >= half_diagonal + 1.0 {
                    return None;
                }
                if d <= -(half_diagonal + 1.0) {
                    return Some((c, MaskTile::Level(FULL)));
                }
                let mut levels = vec![0u16; TILE_PIXELS];
                for (i, l) in levels.iter_mut().enumerate() {
                    let (x, y) = ((i as u32 % TILE_SIZE) as f64, (i as u32 / TILE_SIZE) as f64);
                    *l = level(shape.at(r.x0 + x + 0.5, r.y0 + y + 0.5));
                }
                Some((c, MaskTile::from_levels(levels)))
            })
            .collect();
        let mut mask = Mask::none();
        for (c, t) in tiles {
            mask.put(c, t);
        }
        mask
    }

    /// The opposite: selected where it is not, and the other way round.
    pub fn inverted(&self) -> Mask {
        let mut out = Mask { tiles: HashMap::with_capacity(self.tiles.len()), outside: FULL - self.outside };
        for (c, t) in &self.tiles {
            out.put(*c, t.map(|l| FULL - l));
        }
        out
    }

    /// `new` combined with this one, as `how` says.
    pub fn combined(&self, new: &Mask, how: Combine) -> Mask {
        if how == Combine::Replace {
            return new.clone();
        }
        let mut out = Mask { tiles: HashMap::new(), outside: how.level(self.outside, new.outside) };
        let mut coords: Vec<TileCoord> = self.tiles.keys().chain(new.tiles.keys()).copied().collect();
        coords.sort_unstable();
        coords.dedup();
        let tiles: Vec<(TileCoord, MaskTile)> = coords
            .into_par_iter()
            .map(|c| {
                let a = self.tiles.get(&c).cloned().unwrap_or(MaskTile::Level(self.outside));
                let b = new.tiles.get(&c).cloned().unwrap_or(MaskTile::Level(new.outside));
                let t = match (&a, &b) {
                    (MaskTile::Level(x), MaskTile::Level(y)) => MaskTile::Level(how.level(*x, *y)),
                    _ => {
                        let (la, lb) = (a.levels(), b.levels());
                        MaskTile::from_levels(la.iter().zip(lb.iter()).map(|(&x, &y)| how.level(x, y)).collect())
                    }
                };
                (c, t)
            })
            .collect();
        for (c, t) in tiles {
            out.put(c, t);
        }
        out
    }

    /// The levels of the pixels of `area`, row by row.
    pub fn levels_in(&self, area: &IRect) -> Vec<u16> {
        let (w, h) = (area.width().max(0) as usize, area.height().max(0) as usize);
        let mut out = vec![self.outside; w * h];
        for c in TileCoord::covering(area, 0) {
            let Some(t) = self.tiles.get(&c) else { continue };
            let r = c.rect(0);
            let part = r.intersect(area);
            for y in part.y0..part.y1 {
                let row = &mut out[(y - area.y0) as usize * w..][..w];
                for x in part.x0..part.x1 {
                    row[(x - area.x0) as usize] = t.get((x - r.x0) as u32, (y - r.y0) as u32);
                }
            }
        }
        out
    }

    /// Where the mask crosses `threshold` (the marching ants): closed lines along the pixels'
    /// edges, between those at `threshold` or above and those below, in document coordinates,
    /// each by its corners. They go clockwise round what is selected (anticlockwise round a
    /// hole), and two that meet at a corner stay apart there.
    pub fn outline(&self, threshold: u16) -> Vec<Vec<[f64; 2]>> {
        contours(self, threshold)
    }
}

/// `before` with each pixel `p` made `p × (1 − m) + changed(p) × m`, `m` the level of `mask`
/// there: the step every operation limited to a selection goes through (design.md, *The
/// mask*). `before` is a pixel object's bitmap, its pixel `(0, 0)` at `offset` in the document.
/// `changed` must leave a transparent pixel transparent (only the tiles `before` has are
/// changed); tiles the mask leaves alone, and those it covers but `changed` would not alter,
/// stay as they were, shared.
pub fn blend(
    before: &RasterImage,
    offset: [i64; 2],
    mask: &Mask,
    changed: impl Fn(PixelF32) -> PixelF32 + Sync,
) -> RasterImage {
    blend_by(before, offset, mask, fraction, changed)
}

/// `before` (a pixel object's bitmap at `offset`, as for [`blend`]) with only what `mask`
/// selects kept, as much of each pixel as it selects it: what copying within a selection
/// copies.
pub fn keep_selected(before: &RasterImage, offset: [i64; 2], mask: &Mask) -> RasterImage {
    blend_by(before, offset, mask, |l| 1.0 - fraction(l), |_| [0.0; 4])
}

/// [`blend`], with `weight` giving `m` from each level (tiles where it is 0 throughout left
/// alone).
fn blend_by(
    before: &RasterImage,
    offset: [i64; 2],
    mask: &Mask,
    weight: impl Fn(u16) -> f32 + Sync,
    changed: impl Fn(PixelF32) -> PixelF32 + Sync,
) -> RasterImage {
    debug_assert_eq!(changed([0.0; 4]), [0.0; 4], "a transparent pixel changed");
    let coords: Vec<TileCoord> = before.tiles().map(|(c, _)| c).collect();
    let done: Vec<(TileCoord, Option<Arc<Tile>>)> = coords
        .into_par_iter()
        .filter_map(|c| {
            let tile = before.tile(c).expect("listed");
            let area = c.rect(0).translate(offset[0], offset[1]);
            let weights: Vec<f32> = mask.levels_in(&area).into_iter().map(&weight).collect();
            if weights.iter().all(|&m| m == 0.0) {
                return None;
            }
            let mut px = vec![[0.0f32; 4]; TILE_PIXELS];
            tile.to_f32(&mut px);
            let mut same = true;
            for (p, &m) in px.iter_mut().zip(&weights) {
                if m == 0.0 {
                    continue;
                }
                let c = changed(*p);
                let new = [0, 1, 2, 3].map(|i| p[i] * (1.0 - m) + c[i] * m);
                if new != *p {
                    same = false;
                    *p = new;
                }
            }
            if same {
                return None;
            }
            let t = Tile::from_pixels(px.iter().map(|&p| pixel_from_f32(p)).collect());
            Some((c, (!t.is_transparent()).then(|| Arc::new(t))))
        })
        .collect();
    let mut out = before.clone();
    for (c, t) in done {
        out.set_tile(c, t);
    }
    out
}

/// What [`Mask::outline`] gives: the edges between a pixel in (at `threshold` or above) and a
/// neighbour out, joined into closed lines.
fn contours(mask: &Mask, threshold: u16) -> Vec<Vec<[f64; 2]>> {
    // An edge between two pixels side by side is looked at from the tile of the right (or
    // lower) one, or, where that tile is not kept, from the left (or upper) one's: only edges
    // with a pixel in a kept tile can part in from out (every other pixel is of the level
    // outside).
    let kept = |x: i64, y: i64| mask.tiles.contains_key(&TileCoord::containing(x, y));
    let mut coords: Vec<TileCoord> = mask.tiles.keys().copied().collect();
    coords.sort_unstable();
    let edges: Vec<Step> = coords
        .into_par_iter()
        .flat_map_iter(|c| {
            let r = c.rect(0);
            let mut out = Vec::new();
            let mut look = |x: i64, y: i64, across: bool, at: &dyn Fn(i64, i64) -> u16| {
                // `across`: the edge on the left of pixel (x, y); else the one above it.
                let (px, py) = if across { (x - 1, y) } else { (x, y - 1) };
                let ours = if r.contains(x, y) { true } else { r.contains(px, py) && !kept(x, y) };
                if !ours {
                    return;
                }
                let (before, after) = (at(px, py) >= threshold, at(x, y) >= threshold);
                if before == after {
                    return;
                }
                // Clockwise round what is in (in on the right, y downward).
                out.push(match (across, after) {
                    (true, true) => Step { from: (x, y + 1), to: (x, y) },
                    (true, false) => Step { from: (x, y), to: (x, y + 1) },
                    (false, true) => Step { from: (x, y), to: (x + 1, y) },
                    (false, false) => Step { from: (x + 1, y), to: (x, y) },
                });
            };
            match &mask.tiles[&c] {
                // All one level: only the edges on its border can part in from out.
                MaskTile::Level(_) => {
                    let at = |x: i64, y: i64| mask.level_at(x, y);
                    for y in r.y0..r.y1 {
                        look(r.x0, y, true, &at);
                        look(r.x1, y, true, &at);
                    }
                    for x in r.x0..r.x1 {
                        look(x, r.y0, false, &at);
                        look(x, r.y1, false, &at);
                    }
                }
                MaskTile::Levels(_) => {
                    // Its pixels and a margin of one round it, read once.
                    let area = r.expand(1);
                    let w = area.width() as usize;
                    let levels = mask.levels_in(&area);
                    let at = |x: i64, y: i64| levels[(y - area.y0) as usize * w + (x - area.x0) as usize];
                    for y in r.y0..=r.y1 {
                        for x in r.x0..=r.x1 {
                            if y < r.y1 {
                                look(x, y, true, &at);
                            }
                            if x < r.x1 {
                                look(x, y, false, &at);
                            }
                        }
                    }
                }
            }
            out
        })
        .collect();
    join(edges)
}

/// A pixel's edge, from one of its corners to the next, clockwise round what is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    from: (i64, i64),
    to: (i64, i64),
}

impl Step {
    fn direction(self) -> (i64, i64) {
        (self.to.0 - self.from.0, self.to.1 - self.from.1)
    }
}

/// Edges joined end to end into closed lines, each made of its corners alone. Where two
/// lines touch at a corner (pixels in meeting only diagonally), each turns right there, so
/// that it keeps to its own pixels.
fn join(steps: Vec<Step>) -> Vec<Vec<[f64; 2]>> {
    let mut from: HashMap<(i64, i64), Vec<usize>> = HashMap::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        from.entry(s.from).or_default().push(i);
    }
    let mut used = vec![false; steps.len()];
    let mut lines = Vec::new();
    for start in 0..steps.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut line = vec![steps[start].from];
        let mut at = steps[start];
        loop {
            line.push(at.to);
            let (dx, dy) = at.direction();
            // Right, ahead, then left (in screen coordinates, y downward).
            let order = [(-dy, dx), (dx, dy), (dy, -dx)];
            let next = order.iter().find_map(|&d| {
                from.get(&at.to)?.iter().copied().find(|&j| steps[j].direction() == d && (!used[j] || j == start))
            });
            let Some(j) = next else { break };
            if j == start {
                break;
            }
            used[j] = true;
            at = steps[j];
        }
        // Closed: its last point is its first again.
        if line.len() > 2 && line.first() == line.last() {
            line.pop();
        }
        lines.push(corners(line.into_iter().map(|(x, y)| [x as f64, y as f64]).collect()));
    }
    lines
}

/// A closed line made of its corners alone: points between two others in line with them go.
fn corners(line: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    let n = line.len();
    if n < 3 {
        return line;
    }
    let straight = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        cross.abs() < 1e-9
    };
    let kept: Vec<[f64; 2]> =
        (0..n).filter(|&i| !straight(line[(i + n - 1) % n], line[i], line[(i + 1) % n])).map(|i| line[i]).collect();
    if kept.len() < 3 { line } else { kept }
}

/// A rectangle or an ellipse, turned about its middle, as a selection's shape.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Its middle, in document pixels.
    pub center: [f64; 2],
    /// Its width and height (an ellipse's diameters), in pixels.
    pub size: [f64; 2],
    /// How far it is turned clockwise, in degrees.
    pub angle: f64,
    /// Whether its edge is smoothed (each pixel as selected as it is covered), or sharp
    /// (selected or not by its middle).
    pub smooth: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
}

impl Shape {
    /// `(x, y)` in the shape's own frame: from its middle, along its sides.
    fn local(&self, x: f64, y: f64) -> [f64; 2] {
        let (s, c) = self.angle.to_radians().sin_cos();
        let (dx, dy) = (x - self.center[0], y - self.center[1]);
        [dx * c + dy * s, -dx * s + dy * c]
    }
}

impl Coverage for Shape {
    fn bounds(&self) -> IRect {
        let [w, h] = self.size.map(|v| v.abs() / 2.0);
        let (s, c) = self.angle.to_radians().sin_cos();
        let (ex, ey) = (w * c.abs() + h * s.abs(), w * s.abs() + h * c.abs());
        let [x, y] = self.center;
        IRect::covering(&FRect::new(x - ex, y - ey, x + ex, y + ey)).expand(1)
    }

    fn distance(&self, x: f64, y: f64) -> f64 {
        let [lx, ly] = self.local(x, y);
        let [a, b] = self.size.map(|v| v.abs() / 2.0);
        match self.kind {
            ShapeKind::Rectangle => {
                let (dx, dy) = (lx.abs() - a, ly.abs() - b);
                dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0)
            }
            ShapeKind::Ellipse => ellipse_distance(lx, ly, a, b),
        }
    }

    fn at(&self, x: f64, y: f64) -> f32 {
        let d = self.distance(x, y);
        if self.smooth { (0.5 - d).clamp(0.0, 1.0) as f32 } else { f32::from(u8::from(d <= 0.0)) }
    }
}

/// The signed distance from `(x, y)` to the ellipse about the origin with semi-axes `a` along
/// x and `b` along y (less than 0 inside): exact, by bisection (Eberly, *Distance from a Point
/// to an Ellipse*).
fn ellipse_distance(x: f64, y: f64, a: f64, b: f64) -> f64 {
    // In the first quadrant, the longer axis first.
    let (mut p0, mut p1, mut e0, mut e1) = (x.abs(), y.abs(), a, b);
    if e0 < e1 {
        (p0, p1, e0, e1) = (p1, p0, e1, e0);
    }
    if e1 <= 0.0 {
        // Flat: a segment along the longer axis.
        return if p0 <= e0 { p1 } else { (p0 - e0).hypot(p1) };
    }
    let inside = (p0 / e0).powi(2) + (p1 / e1).powi(2) < 1.0;
    let d = if p1 > 0.0 {
        if p0 > 0.0 {
            let (z0, z1) = (p0 / e0, p1 / e1);
            let g = z0 * z0 + z1 * z1 - 1.0;
            if g == 0.0 {
                0.0
            } else {
                let r0 = (e0 / e1).powi(2);
                let n0 = r0 * z0;
                let (mut s0, mut s1) = (z1 - 1.0, if g < 0.0 { 0.0 } else { n0.hypot(z1) - 1.0 });
                let mut s = 0.0;
                for _ in 0..200 {
                    s = (s0 + s1) / 2.0;
                    if s == s0 || s == s1 {
                        break;
                    }
                    let g = (n0 / (s + r0)).powi(2) + (z1 / (s + 1.0)).powi(2) - 1.0;
                    if g > 0.0 {
                        s0 = s;
                    } else if g < 0.0 {
                        s1 = s;
                    } else {
                        break;
                    }
                }
                let (x0, x1) = (r0 * p0 / (s + r0), p1 / (s + 1.0));
                (x0 - p0).hypot(x1 - p1)
            }
        } else {
            (p1 - e1).abs()
        }
    } else {
        let (numer, denom) = (e0 * p0, e0 * e0 - e1 * e1);
        if numer < denom {
            let xde0 = numer / denom;
            let (x0, x1) = (e0 * xde0, e1 * (1.0 - xde0 * xde0).max(0.0).sqrt());
            (x0 - p0).hypot(x1)
        } else {
            (p0 - e0).abs()
        }
    };
    if inside { -d } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Shape {
        Shape {
            kind: ShapeKind::Rectangle,
            center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
            size: [x1 - x0, y1 - y0],
            angle: 0.0,
            smooth: true,
        }
    }

    #[test]
    fn a_rectangle_on_the_pixel_grid_is_sharp_and_kept_by_its_edges() {
        // 1000 × 600 at (10, 20): its edges' tiles kept, the tiles within it one level each.
        let m = Mask::of(&rect(10.0, 20.0, 1010.0, 620.0));
        assert_eq!(m.level_at(10, 20), FULL);
        assert_eq!(m.level_at(1009, 619), FULL);
        assert_eq!(m.level_at(9, 20), 0);
        assert_eq!(m.level_at(1010, 300), 0);
        assert_eq!(m.extent(), Extent::Within(IRect::new(10, 20, 1010, 620)));
        // Tiles 0..=3 across, 0..=2 down; those wholly inside it, one level.
        assert!(m.tiles().all(|(_, t)| matches!(t, MaskTile::Level(FULL)) || t.bytes() > 0));
        assert_eq!(m.tile(TileCoord::new(1, 1)), Some(&MaskTile::Level(FULL)));
        assert_eq!(m.bytes(), 10 * TILE_PIXELS as u64 * 2, "only the 10 tiles its edges cross keep levels");
    }

    #[test]
    fn an_edge_between_pixels_is_shared_by_them() {
        // From 10.5 to 20: pixel 10 half selected.
        let m = Mask::of(&rect(10.5, 0.0, 20.0, 5.0));
        assert_eq!(m.level_at(10, 2), level(0.5));
        assert_eq!(m.level_at(11, 2), FULL);
        // Sharp, the pixel whose middle is on the edge is in.
        let sharp = Mask::of(&Shape { smooth: false, ..rect(10.5, 0.0, 20.0, 5.0) });
        assert_eq!(sharp.level_at(10, 2), FULL);
        assert_eq!(sharp.level_at(9, 2), 0);
    }

    #[test]
    fn an_ellipse_is_as_far_from_its_edge_as_it_says() {
        for (x, y, a, b) in [(3.0, 4.0, 5.0, 5.0), (0.0, 0.0, 10.0, 2.0), (12.0, 0.0, 10.0, 2.0), (1.0, 7.0, 3.0, 9.0)]
        {
            let d = ellipse_distance(x, y, a, b);
            // Against the nearest of many points on it.
            let nearest = (0..20000)
                .map(|i| {
                    let t = i as f64 / 20000.0 * std::f64::consts::TAU;
                    (a * t.cos() - x).hypot(b * t.sin() - y)
                })
                .fold(f64::INFINITY, f64::min);
            assert!((d.abs() - nearest).abs() < 1e-3, "({x}, {y}) on ({a}, {b}): {d} against {nearest}");
            assert_eq!(d < 0.0, (x / a).powi(2) + (y / b).powi(2) < 1.0);
        }
        // A circle of radius 100 about pixel (0, 0)'s middle: the middle fully, pixel 100,
        // whose middle is on the edge, half.
        let c = Shape { kind: ShapeKind::Ellipse, center: [0.5, 0.5], size: [200.0, 200.0], angle: 0.0, smooth: true };
        let m = Mask::of(&c);
        assert_eq!(m.level_at(0, 0), FULL);
        assert_eq!(m.level_at(99, 0), FULL);
        assert_eq!(m.level_at(150, 0), 0);
        let edge = fraction(m.level_at(100, 0));
        assert!((edge - 0.5).abs() < 0.01, "{edge}");
    }

    #[test]
    fn a_turned_rectangle_covers_its_turned_corners() {
        // A square of 100 turned 45°: its corners on the axes, 70.7 from its middle.
        let s = Shape { angle: 45.0, ..rect(-50.0, -50.0, 50.0, 50.0) };
        let m = Mask::of(&s);
        assert_eq!(m.level_at(0, 0), FULL);
        assert_eq!(m.level_at(65, 0), FULL);
        assert_eq!(m.level_at(75, 0), 0);
        assert_eq!(m.level_at(45, 45), 0, "the square's corner was not turned away");
    }

    #[test]
    fn masks_combine_add_subtract_and_intersect() {
        let a = Mask::of(&rect(0.0, 0.0, 100.0, 100.0));
        let b = Mask::of(&rect(50.0, 0.0, 150.0, 100.0));
        let at = |m: &Mask| [m.level_at(25, 50), m.level_at(75, 50), m.level_at(125, 50)];
        assert_eq!(at(&a.combined(&b, Combine::Add)), [FULL, FULL, FULL]);
        assert_eq!(at(&a.combined(&b, Combine::Subtract)), [FULL, 0, 0]);
        assert_eq!(at(&a.combined(&b, Combine::Intersect)), [0, FULL, 0]);
        assert_eq!(at(&a.combined(&b, Combine::Replace)), [0, FULL, FULL]);
        // Subtracting all of it leaves nothing, kept as no tiles.
        assert!(a.combined(&a, Combine::Subtract).is_none());
    }

    #[test]
    fn an_inverted_mask_selects_the_plane_but_for_a_hole() {
        let a = Mask::of(&rect(0.0, 0.0, 100.0, 100.0)).inverted();
        assert_eq!(a.level_at(50, 50), 0);
        assert_eq!(a.level_at(-1_000_000, 7), FULL);
        assert_eq!(a.extent(), Extent::Unbounded);
        // Inverted again, as it was.
        assert_eq!(a.inverted(), Mask::of(&rect(0.0, 0.0, 100.0, 100.0)));
        assert!(Mask::all().inverted().is_none());
    }

    #[test]
    fn the_outline_of_a_rectangle_is_its_four_corners() {
        let m = Mask::of(&rect(10.0, 20.0, 600.0, 300.0));
        let lines = m.outline(HALF);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let mut corners = lines[0].clone();
        corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(corners, [[10.0, 20.0], [10.0, 300.0], [600.0, 20.0], [600.0, 300.0]]);
    }

    #[test]
    fn the_outline_of_a_hole_in_the_plane_and_of_two_shapes_are_closed_lines() {
        // A hole: one line, round it.
        let hole = Mask::of(&rect(0.0, 0.0, 300.0, 300.0)).inverted();
        let lines = hole.outline(HALF);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 4);
        // Two apart: two lines; an ellipse's, round it, each point on its edge.
        let e = Shape {
            kind: ShapeKind::Ellipse,
            center: [1000.0, 1000.0],
            size: [400.0, 200.0],
            angle: 30.0,
            smooth: true,
        };
        let two = Mask::of(&rect(0.0, 0.0, 100.0, 100.0)).combined(&Mask::of(&e), Combine::Add);
        let lines = two.outline(HALF);
        assert_eq!(lines.len(), 2, "{}", lines.len());
        let round = lines.iter().find(|l| l.len() > 4).expect("the ellipse's");
        // Along the pixels' edges: each corner within a pixel's diagonal of the true edge.
        for p in round {
            assert!(e.distance(p[0], p[1]).abs() < 1.0, "{p:?} is {} from the edge", e.distance(p[0], p[1]));
        }
    }

    #[test]
    fn pixels_meeting_only_at_a_corner_have_lines_of_their_own() {
        let a = Mask::of(&rect(0.0, 0.0, 1.0, 1.0));
        let b = Mask::of(&rect(1.0, 1.0, 2.0, 2.0));
        let mut lines = a.combined(&b, Combine::Add).outline(HALF);
        lines.iter_mut().for_each(|l| l.sort_by(|a, b| a.partial_cmp(b).unwrap()));
        lines.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            lines,
            [
                vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]],
                vec![[1.0, 1.0], [1.0, 2.0], [2.0, 1.0], [2.0, 2.0]]
            ]
        );
    }

    #[test]
    fn blending_through_a_mask_changes_what_it_selects_as_much_as_it_selects_it() {
        let red = crate::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let image = RasterImage::filled(&IRect::new(0, 0, 600, 100), red);
        // The left half selected, pixel 299 half; the image 1000 to the right of the origin.
        let mask = Mask::of(&rect(1000.0, 0.0, 1299.5, 100.0));
        let cleared = blend(&image, [1000, 0], &mask, |_| [0.0; 4]);
        assert_eq!(cleared.pixel(10, 50), crate::tile::pixel_from_f32([0.0; 4]));
        assert_eq!(cleared.pixel(400, 50), red);
        let half = crate::tile::pixel_to_f32(cleared.pixel(299, 50));
        assert!((half[3] - 0.5).abs() < 0.01, "{half:?}");
        // The tiles it leaves alone are the same tiles.
        let shared = |a: &RasterImage, b: &RasterImage, c| Arc::ptr_eq(a.tile(c).unwrap(), b.tile(c).unwrap());
        assert!(shared(&image, &cleared, TileCoord::new(2, 0)));
        // Wholly cleared tiles go.
        assert!(cleared.tile(TileCoord::new(0, 0)).is_none());
    }

    #[test]
    fn keeping_what_is_selected_keeps_as_much_of_each_pixel_as_is_selected() {
        let red = crate::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let image = RasterImage::filled(&IRect::new(0, 0, 600, 100), red);
        let mask = Mask::of(&rect(0.0, 0.0, 299.5, 100.0));
        let kept = keep_selected(&image, [0, 0], &mask);
        assert_eq!(kept.pixel(10, 50), red);
        assert_eq!(kept.pixel(400, 50), crate::tile::pixel_from_f32([0.0; 4]));
        let half = crate::tile::pixel_to_f32(kept.pixel(299, 50));
        assert!((half[3] - 0.5).abs() < 0.01, "{half:?}");
        // Wholly selected tiles are the same tiles; wholly unselected ones go.
        assert!(Arc::ptr_eq(image.tile(TileCoord::new(0, 0)).unwrap(), kept.tile(TileCoord::new(0, 0)).unwrap()));
        assert!(kept.tile(TileCoord::new(2, 0)).is_none());
    }
}
