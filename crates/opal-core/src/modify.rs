//! Modifiers of a selection's mask (design.md, *Feather*): growing or shrinking it, and
//! feathering it, each worked out from every pixel's distance to its outline, where it crosses
//! half. The distance is measured from where the outline crosses between pixels (read between
//! their levels, so that a smooth edge stays smooth), and only in a band of tiles round it:
//! farther in or out, every pixel ends as selected as the band's edge says.
//!
//! Each works from the outline alone: a pixel selected less than half counts as out, at least
//! half as in, so a soft selection grown or feathered loses its own softness.

use crate::geom::IRect;
use crate::mask::{FULL, HALF, Mask, MaskTile, level};
use crate::tile::{TILE_PIXELS, TILE_SIZE, TileCoord};
use std::collections::HashSet;

/// The farthest a selection is grown, shrunk or feathered (in pixels), by a dialog, a call,
/// or a transform carrying the step: past it, the step is taken as its mask (see
/// `Selection::transformed`). The memory a tile's working needs grows with its square.
pub const MAX_DISTANCE: f64 = 1000.0;

/// The memory the windows worked out at once may take (see [`by_distance`]).
const WINDOWS_MEMORY: usize = 512 << 20;

/// What a pixel of a window takes while it is worked out: its level, its nearest point of the
/// outline (if one is found), and how far that is.
const WINDOW_PIXEL: usize = 2 + std::mem::size_of::<Option<[f32; 2]>>() + 4;

/// The most tiles a side a block worked out in one window has.
const MOST_BLOCK: i32 = 8;

named!(Reach: Round = "round", Square = "square", Diamond = "diamond");
named!(Side: Inward = "inward", Outward = "outward", Centered = "centered");
named!(Falloff: Gaussian = "gaussian", Linear = "linear");

/// How far a pixel is from the outline, so what shape growing gives a corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// Straight across: a corner grows round.
    #[default]
    Round,
    /// The farther of across and down: a corner grows square.
    Square,
    /// Across and down together: a corner grows to a point, as a diamond's.
    Diamond,
}

impl Reach {
    fn length(self, dx: f32, dy: f32) -> f32 {
        match self {
            Reach::Round => dx.hypot(dy),
            Reach::Square => dx.abs().max(dy.abs()),
            Reach::Diamond => dx.abs() + dy.abs(),
        }
    }
}

/// Where a feather's falloff lies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Inside the outline: what lies outside stays unselected.
    Inward,
    /// Outside it: what lies inside stays fully selected.
    Outward,
    /// Half each way.
    #[default]
    Centered,
}

/// The shape of a feather's falloff across its width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    /// As a Gaussian blur's edge: gentle at both ends.
    #[default]
    Gaussian,
    /// Straight.
    Linear,
}

impl Falloff {
    /// How selected a pixel `u` of the way across the falloff is (0 at its outer edge, 1 at
    /// its inner), from 0 to 1.
    fn at(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Falloff::Linear => u,
            // The normal distribution's, from two deviations out to two in, stretched to 0..1.
            Falloff::Gaussian => {
                let k = 2.0 / std::f64::consts::SQRT_2;
                (erf(k * (2.0 * u - 1.0)) / erf(k) + 1.0) / 2.0
            }
        }
    }
}

/// The error function, to within 1.5 × 10⁻⁷ (Abramowitz and Stegun, 7.1.26).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly =
        t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let y = 1.0 - poly * (-x * x).exp();
    if x < 0.0 { -y } else { y }
}

/// How much of a pixel whose middle is `d` from an edge (less than 0 inside) lies inside it.
fn coverage(d: f64) -> u16 {
    level((0.5 - d).clamp(0.0, 1.0) as f32)
}

/// What may hold a modifier's work back, asked as it goes: `stop`, between the pieces of it,
/// whether it is wanted no longer; `admit`, before it takes memory of note (the windows it works
/// out at once, then each block's tiles as it keeps them), whether it may take that many bytes
/// more, all of them in memory; `give_back`, told the bytes it no longer holds once done (its
/// windows), so that the steps after it may have them.
#[derive(Clone, Copy)]
pub struct Bounds<'a> {
    pub stop: &'a (dyn Fn() -> bool + Sync),
    pub admit: &'a (dyn Fn(u64) -> bool + Sync),
    pub give_back: &'a (dyn Fn(u64) + Sync),
}

/// Bounds that never hold the work back.
pub const UNBOUNDED: Bounds<'static> = Bounds { stop: &never, admit: &always, give_back: &forget };

fn forget(_: u64) {}

fn never() -> bool {
    false
}

fn always(_: u64) -> bool {
    true
}

/// Why a modifier's work did not finish (see [`Bounds`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// It was wanted no longer.
    Stopped,
    /// It was not let take the memory it needed.
    NoMemory,
}

impl Mask {
    /// The mask grown by `by` pixels (shrunk, if less than 0), its corners growing as `reach`
    /// says.
    pub fn grown(&self, by: f64, reach: Reach) -> Mask {
        self.grown_within(by, reach, UNBOUNDED).expect("unbounded")
    }

    /// The mask feathered: its edge falling off over `width` pixels, on the `side` of its
    /// outline, along `falloff`.
    pub fn feathered(&self, width: f64, side: Side, falloff: Falloff) -> Mask {
        self.feathered_within(width, side, falloff, UNBOUNDED).expect("unbounded")
    }

    /// [`grown`](Self::grown), unless `bounds` hold it back.
    pub fn grown_within(&self, by: f64, reach: Reach, bounds: Bounds) -> Result<Mask, Halt> {
        if by == 0.0 {
            return Ok(self.clone());
        }
        by_distance(self, grow_band(by), reach, |d| coverage(d - by), bounds)
    }

    /// The mask with its outline smoothed over `radius` pixels: closed (grown, then shrunk
    /// back), so that pinholes and notches narrower than twice the radius fill and inward
    /// corners round, then opened (shrunk, then grown back), so that specks and spurs as thin
    /// go and outward corners round; unless `bounds` hold it back. Each pass's mask goes once
    /// the next is made, and what it took is given back.
    pub fn smoothed_within(&self, radius: f64, bounds: Bounds) -> Result<Mask, Halt> {
        if radius <= 0.0 {
            return Ok(self.clone());
        }
        let mut mask = self.grown_within(radius, Reach::Round, bounds)?;
        for by in [-radius, -radius, radius] {
            let next = mask.grown_within(by, Reach::Round, bounds)?;
            (bounds.give_back)(std::mem::replace(&mut mask, next).bytes());
        }
        Ok(mask)
    }

    /// The band `width` pixels wide along the mask's outline, on its `side` (inside it,
    /// outside it, or half each way), its edges as smooth as a grown outline's; unless `bounds`
    /// hold it back.
    pub fn bordered_within(&self, width: f64, side: Side, bounds: Bounds) -> Result<Mask, Halt> {
        if width <= 0.0 {
            return Ok(Mask::none());
        }
        // How far the band reaches out of the outline, and in.
        let outer = feather_outer(width, side);
        let inner = width - outer;
        // Within the band's outer edge and outside its inner one, each as much as a pixel is:
        // its share of both.
        let level_of = move |d: f64| {
            let within = u32::from(coverage(d - outer));
            let beyond = u32::from(FULL - coverage(d + inner));
            (within + beyond).saturating_sub(u32::from(FULL)) as u16
        };
        by_distance(self, Band::new(inner, outer), Reach::Round, level_of, bounds)
    }

    /// [`feathered`](Self::feathered), unless `bounds` hold it back.
    pub fn feathered_within(&self, width: f64, side: Side, falloff: Falloff, bounds: Bounds) -> Result<Mask, Halt> {
        if width <= 0.0 {
            return Ok(self.clone());
        }
        let outer = feather_outer(width, side);
        by_distance(
            self,
            feather_band(width, side),
            Reach::Round,
            move |d| level(falloff.at((outer - d) / width) as f32),
            bounds,
        )
    }
}

/// How far from the outline a modifier's levels vary, inside it and outside: past that, each
/// way, every pixel is as the band's edge says. (Two pixels more than the levels' own reach,
/// for the pixels beside the outline, whose distance is read between their levels.)
#[derive(Clone, Copy, Debug, PartialEq)]
struct Band {
    inside: f64,
    outside: f64,
}

impl Band {
    /// Out to `outside` pixels beyond the outline, and `inside` within it (each at least 0).
    fn new(inside: f64, outside: f64) -> Band {
        Band { inside: inside.max(0.0) + 2.0, outside: outside.max(0.0) + 2.0 }
    }

    /// The farther way.
    fn most(self) -> f64 {
        self.inside.max(self.outside)
    }
}

/// The band growing by `by` (shrinking, if less than 0) works out: the levels vary where a
/// pixel is `by` from the outline, inside it if `by` is less than 0, else outside.
fn grow_band(by: f64) -> Band {
    Band::new(-by, by)
}

/// The band a feather over `width` on `side` works out: from where its falloff begins
/// (outside the outline) to `width` farther in.
fn feather_band(width: f64, side: Side) -> Band {
    let outer = feather_outer(width, side);
    Band::new(width - outer, outer)
}

/// The side of the window a block of `b` tiles a side is worked out in for `band`: the block,
/// and a margin of the band round it (and a pixel, for the distances read between pixels).
fn window_side(b: i32, band: f64) -> usize {
    b as usize * TILE_SIZE as usize + 2 * (band.ceil() as usize + 1)
}

/// How the tiles `near`, worked out for `band`, are worked out with `threads`: in blocks of
/// how many tiles a side, by how many workers at once (as many as their windows fit in
/// [`WINDOWS_MEMORY`], of the threads, and one at least). The size is the one that should take
/// the least time: a larger block repeats less of its margin, but fewer fit in the memory at
/// once to keep the threads busy, and it works out the whole block's tiles however few of them
/// are near the outline. Of two alike, the smaller.
fn layout(band: f64, threads: usize, near: &[TileCoord]) -> (i32, usize) {
    let time = |b: i32| {
        let window = window_side(b, band).pow(2);
        let workers = (WINDOWS_MEMORY / (window * WINDOW_PIXEL)).clamp(1, threads.max(1));
        let blocks: HashSet<(i32, i32)> = near.iter().map(|c| (c.x.div_euclid(b), c.y.div_euclid(b))).collect();
        (blocks.len().div_ceil(workers) * window, b, workers)
    };
    // Each size's time once; of two alike, the first (the smaller).
    let (_, b, workers) = (1..=MOST_BLOCK).map(time).min_by_key(|&(t, b, _)| (t, b)).expect("one size at least");
    (b, workers)
}

/// The tiles of `mask` with pixels within `band` of its outline, each on its own side: a
/// pixel that near it lies at most that many tiles across or down from a tile the outline may
/// cross, and a tile it does not cross is all on one side.
fn near_tiles(mask: &Mask, band: Band) -> Vec<TileCoord> {
    let tiles = |pixels: f64| (pixels / f64::from(TILE_SIZE)).ceil() as i32;
    let (inside, outside) = (tiles(band.inside), tiles(band.outside));
    let k = inside.max(outside);
    // How many tiles across or down each is from the nearest the outline may cross.
    let mut apart: std::collections::HashMap<TileCoord, i32> = std::collections::HashMap::new();
    for c in mask.edge_tiles() {
        for dy in -k..=k {
            for dx in -k..=k {
                let d = dx.abs().max(dy.abs());
                let e = apart.entry(TileCoord::new(c.x + dx, c.y + dy)).or_insert(d);
                *e = (*e).min(d);
            }
        }
    }
    apart
        .into_iter()
        .filter(|&(c, d)| {
            d <= inside.min(outside)
                || match mask.side_in(&c.rect(0), HALF) {
                    Some(true) => d <= inside,
                    Some(false) => d <= outside,
                    None => true,
                }
        })
        .map(|(c, _)| c)
        .collect()
}

/// Where a feather's falloff begins, from the outline, out (more than 0) or in.
fn feather_outer(width: f64, side: Side) -> f64 {
    match side {
        Side::Inward => 0.0,
        Side::Outward => width,
        Side::Centered => width / 2.0,
    }
}

/// `mask` with each pixel's level what `level_of` makes of its distance to the outline, as
/// `reach` measures it (less than 0 inside), worked out within `band` of it: farther, as
/// `level_of` says at the band's edge. The tiles within the band are worked out in blocks, a
/// window each (the block and a margin round it as wide as the band's farther way), by as many
/// workers as [`layout`] says (no more of the threads taken). `bounds` stop it before each
/// block, and are asked to admit the windows worked out at once, and each block's tiles as
/// they are kept (at the size they take, which for a straight edge is a row of levels).
fn by_distance(
    mask: &Mask,
    band: Band,
    reach: Reach,
    level_of: impl Fn(f64) -> u16 + Sync,
    bounds: Bounds,
) -> Result<Mask, Halt> {
    let far_in = level_of(-band.inside);
    let far_out = level_of(band.outside);
    let far = |l: u16| if l >= HALF { far_in } else { far_out };
    let outside = far(mask.outside());
    let near = near_tiles(mask, band);
    if (bounds.stop)() {
        return Err(Halt::Stopped);
    }
    let (b, workers) = layout(band.most(), rayon::current_num_threads(), &near);
    let band = band.most();
    let mut blocks: std::collections::HashMap<(i32, i32), Vec<TileCoord>> = std::collections::HashMap::new();
    for c in near {
        blocks.entry((c.x.div_euclid(b), c.y.div_euclid(b))).or_default().push(c);
    }
    // No more workers than blocks: each works out a window only as it takes a block.
    let workers = workers.min(blocks.len());
    let windows = (workers * window_side(b, band).pow(2) * WINDOW_PIXEL) as u64;
    if !(bounds.admit)(windows) {
        return Err(Halt::NoMemory);
    }
    let window = |(bx, by): (i32, i32)| {
        let (a, z) = (TileCoord::new(bx * b, by * b).rect(0), TileCoord::new(bx * b + b - 1, by * b + b - 1).rect(0));
        IRect::new(a.x0, a.y0, z.x1, z.y1).expand(band.ceil() as i64 + 1)
    };
    debug_assert_eq!(window((0, 0)).width() as usize, window_side(b, band), "the window layout() reckons with");
    let blocks: Vec<((i32, i32), Vec<TileCoord>)> = blocks.into_iter().collect();
    // Each worker takes the next block not taken, until none is left or `bounds` hold it back
    // (what holds it back the first, and the last, word).
    let next = std::sync::atomic::AtomicUsize::new(0);
    let halted: std::sync::Mutex<Option<Halt>> = std::sync::Mutex::new(None);
    let worked = std::sync::Mutex::new(Vec::with_capacity(blocks.len()));
    rayon::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|_| {
                let halt = |why| {
                    halted.lock().expect("not poisoned").get_or_insert(why);
                };
                while let Some((at, tiles)) = blocks.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    if halted.lock().expect("not poisoned").is_some() {
                        return;
                    }
                    if (bounds.stop)() {
                        return halt(Halt::Stopped);
                    }
                    let w = window(*at);
                    // No outline within reach of the block: each tile as its side ends.
                    let made: Vec<(TileCoord, MaskTile)> = match mask.side_in(&w, HALF) {
                        Some(inside) => {
                            tiles.iter().map(|&c| (c, MaskTile::Level(far(if inside { FULL } else { 0 })))).collect()
                        }
                        None => block_by_distance(mask, &w, tiles, band, reach, &level_of),
                    };
                    if !(bounds.admit)(made.iter().map(|(_, t)| t.bytes()).sum()) {
                        return halt(Halt::NoMemory);
                    }
                    worked.lock().expect("not poisoned").extend(made);
                }
            });
        }
    });
    // The windows are gone, however it ended.
    (bounds.give_back)(windows);
    if let Some(why) = halted.into_inner().expect("not poisoned") {
        return Err(why);
    }
    // Farther, each tile is as its side ends. Alike tiles side by side are kept as one: what
    // that saves is given back, so that what it holds is what stays taken.
    let worked = worked.into_inner().expect("not poisoned");
    let admitted: u64 = worked.iter().map(|(_, t)| t.bytes()).sum();
    let made = mask.rebuilt(worked, far, outside);
    (bounds.give_back)(admitted.saturating_sub(made.bytes()));
    Ok(made)
}

/// The tiles `tiles` of [`by_distance`]'s mask, worked out over `window` (theirs, and the
/// band round them).
fn block_by_distance(
    mask: &Mask,
    window: &IRect,
    tiles: &[TileCoord],
    band: f64,
    reach: Reach,
    level_of: &(impl Fn(f64) -> u16 + Sync),
) -> Vec<(TileCoord, MaskTile)> {
    let window = *window;
    let (w, h) = (window.width() as usize, window.height() as usize);
    let levels = mask.levels_in(&window);
    let inside = |i: usize| levels[i] >= HALF;
    // Each pixel's nearest point of the outline found so far (window coordinates, from the
    // window's corner, a pixel's middle at +0.5), and how far it is.
    let mut nearest: Vec<Option<[f32; 2]>> = vec![None; w * h];
    let mut dist = vec![f32::INFINITY; w * h];
    let middle = |i: usize| [(i % w) as f32 + 0.5, (i / w) as f32 + 0.5];
    let offer = |nearest: &mut Vec<Option<[f32; 2]>>, dist: &mut Vec<f32>, i: usize, p: [f32; 2]| {
        let [x, y] = middle(i);
        let d = reach.length(p[0] - x, p[1] - y);
        if d < dist[i] {
            dist[i] = d;
            nearest[i] = Some(p);
        }
    };
    // Where the outline crosses between two pixels side by side, read between their levels.
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            for j in [(x + 1 < w).then(|| i + 1), (y + 1 < h).then(|| i + w)].into_iter().flatten() {
                if inside(i) == inside(j) {
                    continue;
                }
                let (a, b) = (f32::from(levels[i]), f32::from(levels[j]));
                let t = ((a - f32::from(HALF)) / (a - b)).clamp(0.0, 1.0);
                let ([xi, yi], [xj, yj]) = (middle(i), middle(j));
                let p = [xi + (xj - xi) * t, yi + (yj - yi) * t];
                offer(&mut nearest, &mut dist, i, p);
                offer(&mut nearest, &mut dist, j, p);
            }
        }
    }
    // Where a sharp edge turns (a pixel of four, all fully in or out, other than the rest),
    // the outline goes through their shared corner: there too, or a rectangle's corner would
    // be reached only through the points half a pixel along its sides.
    let sharp = |i: usize| levels[i] == 0 || levels[i] == FULL;
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let cell = [y * w + x, y * w + x + 1, (y + 1) * w + x, (y + 1) * w + x + 1];
            let ins = cell.iter().filter(|&&i| inside(i)).count();
            if (ins == 1 || ins == 3) && cell.iter().all(|&i| sharp(i)) {
                let corner = [(x + 1) as f32, (y + 1) as f32];
                for i in cell {
                    offer(&mut nearest, &mut dist, i, corner);
                }
            }
        }
    }
    // Spread to every pixel from its neighbours' (Danielsson's two sweeps of eight).
    let take = |nearest: &mut Vec<Option<[f32; 2]>>, dist: &mut Vec<f32>, i: usize, from: Option<usize>| {
        if let Some(p) = from.and_then(|j| nearest[j]) {
            offer(nearest, dist, i, p);
        }
    };
    let at = |x: i64, y: i64| {
        (x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h).then(|| y as usize * w + x as usize)
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let i = at(x, y).expect("within");
            for (dx, dy) in [(-1, 0), (-1, -1), (0, -1), (1, -1)] {
                take(&mut nearest, &mut dist, i, at(x + dx, y + dy));
            }
        }
        for x in (0..w as i64).rev() {
            take(&mut nearest, &mut dist, at(x, y).expect("within"), at(x + 1, y));
        }
    }
    for y in (0..h as i64).rev() {
        for x in (0..w as i64).rev() {
            let i = at(x, y).expect("within");
            for (dx, dy) in [(1, 0), (1, 1), (0, 1), (-1, 1)] {
                take(&mut nearest, &mut dist, i, at(x + dx, y + dy));
            }
        }
        for x in 0..w as i64 {
            take(&mut nearest, &mut dist, at(x, y).expect("within"), at(x - 1, y));
        }
    }
    let side = TILE_SIZE as usize;
    tiles
        .iter()
        .map(|&c| {
            let r = c.rect(0);
            let (ox, oy) = ((r.x0 - window.x0) as usize, (r.y0 - window.y0) as usize);
            let mut out = vec![0u16; TILE_PIXELS];
            for y in 0..side {
                for x in 0..side {
                    let i = (y + oy) * w + x + ox;
                    let d = f64::from(dist[i]).min(band);
                    out[y * side + x] = level_of(if inside(i) { -d } else { d });
                }
            }
            (c, MaskTile::from_levels(out))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::{Extent, Shape, ShapeKind};
    use std::collections::HashSet;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Mask {
        Mask::of(&Shape::rectangle(&crate::geom::FRect::new(x0, y0, x1, y1), true))
    }

    #[test]
    fn a_rectangle_grows_and_shrinks_by_whole_pixels_its_corners_as_the_reach_says() {
        let m = rect(100.0, 100.0, 200.0, 150.0);
        assert_eq!(m.grown(10.0, Reach::Square).extent(), Extent::Within(IRect::new(90, 90, 210, 160)));
        assert_eq!(m.grown(-10.0, Reach::Round).extent(), Extent::Within(IRect::new(110, 110, 190, 140)));
        // Round, the corner 10 out along the diagonal is not reached, square it is.
        let (round, square, diamond) =
            (m.grown(10.0, Reach::Round), m.grown(10.0, Reach::Square), m.grown(10.0, Reach::Diamond));
        assert_eq!(square.level_at(91, 91), FULL);
        assert_eq!(round.level_at(91, 91), 0);
        assert_eq!(round.level_at(94, 94), FULL, "within 10 of the corner (100, 100)");
        assert_eq!(diamond.level_at(94, 94), 0, "6 + 6 from the corner is not within 10 by a diamond's reach");
        assert_eq!(diamond.level_at(97, 96), FULL);
        // The sides stay straight and sharp, on whole pixels.
        assert_eq!([round.level_at(150, 89), round.level_at(150, 90)], [0, FULL]);
        // On the tiles' edges, eight tiles wide: its top grown along the middle too, far from
        // the tiles where its runs begin and end.
        let aligned = rect(0.0, 256.0, 2048.0, 768.0).grown(10.0, Reach::Square);
        assert_eq!([aligned.level_at(1024, 245), aligned.level_at(1024, 246)], [0, FULL]);
    }

    #[test]
    fn a_smooth_circle_grown_is_the_larger_smooth_circle() {
        let circle = |r: f64| {
            Mask::of(&Shape {
                kind: ShapeKind::Ellipse,
                center: [300.3, 299.6],
                size: [2.0 * r, 2.0 * r],
                angle: 0.0,
                smooth: true,
            })
        };
        let grown = circle(60.0).grown(25.0, Reach::Round);
        let expected = circle(85.0);
        let mut worst = 0u16;
        for y in 180..420 {
            for x in 180..420 {
                worst = worst.max(grown.level_at(x, y).abs_diff(expected.level_at(x, y)));
            }
        }
        // Its edge within a tenth of a pixel or so of the larger circle's, all round.
        assert!(f32::from(worst) / f32::from(FULL) < 0.12, "off by {} at most", f32::from(worst) / f32::from(FULL));
    }

    #[test]
    fn a_feather_falls_off_inward_outward_or_centered_over_its_width() {
        let m = rect(100.0, 0.0, 400.0, 100.0);
        let at = |m: &Mask, x: i64| f64::from(m.level_at(x, 50)) / f64::from(FULL);
        // Centered on the left side (x = 100), 20 wide: half at the side, nothing 10 out, all 10 in.
        let centered = m.feathered(20.0, Side::Centered, Falloff::Linear);
        assert!((at(&centered, 100) - 0.525).abs() < 0.05, "{}", at(&centered, 100));
        assert_eq!([at(&centered, 89), at(&centered, 111)], [0.0, 1.0]);
        // Inward: nothing outside, all 20 in; outward: all inside, nothing 20 out.
        let inward = m.feathered(20.0, Side::Inward, Falloff::Linear);
        assert_eq!([at(&inward, 99), at(&inward, 121)], [0.0, 1.0]);
        let outward = m.feathered(20.0, Side::Outward, Falloff::Gaussian);
        assert_eq!([at(&outward, 100), at(&outward, 79)], [1.0, 0.0]);
        // Gaussian: gentler near the ends than straight.
        let g = m.feathered(20.0, Side::Centered, Falloff::Gaussian);
        assert!(at(&g, 92) < at(&centered, 92), "{} against {}", at(&g, 92), at(&centered, 92));
        // Far from its outline, as it was.
        assert_eq!([centered.level_at(250, 50), centered.level_at(1000, 1000)], [FULL, 0]);
    }

    #[test]
    fn the_modifiers_ways_are_named_as_the_recipe_names_them() {
        fn check<T: serde::Serialize + PartialEq + std::fmt::Debug + Copy>(
            all: &[T],
            names: &[&str],
            named: fn(&str) -> Option<T>,
        ) {
            for (v, n) in all.iter().zip(names) {
                assert_eq!(serde_json::to_value(v).unwrap(), serde_json::json!(n));
                assert_eq!(named(n), Some(*v));
            }
        }
        check(&Reach::ALL, &Reach::NAMES, Reach::named);
        check(&Side::ALL, &Side::NAMES, Side::named);
        check(&Falloff::ALL, &Falloff::NAMES, Falloff::named);
    }

    #[test]
    fn a_selection_as_large_as_a_canvas_may_be_is_grown_in_time_with_its_edges() {
        // 300,000 pixels a side: its inside, 1.37 million tiles, is never looked at tile by tile.
        let m = rect(0.0, 0.0, 300_000.0, 300_000.0);
        let grown = m.grown(3.0, Reach::Square);
        assert_eq!(grown.extent(), Extent::Within(IRect::new(-3, -3, 300_003, 300_003)));
        assert_eq!(grown.level_at(150_000, 150_000), FULL);
    }

    #[test]
    fn blocks_are_as_large_as_keep_the_threads_busy_in_the_memory_and_the_work_stops_when_asked() {
        // Near tiles covering an area, a small band, threads to keep busy: blocks of several
        // tiles a side, which repeat less of their margin than a tile alone would.
        let area: Vec<TileCoord> = (0..100).flat_map(|y| (0..100).map(move |x| TileCoord::new(x, y))).collect();
        assert!(layout(7.0, 16, &area).0 > 1);
        // A line of tiles, or one alone: a tile at a time, rather than blocks mostly far from
        // the outline.
        let line: Vec<TileCoord> = (0..2000).map(|x| TileCoord::new(x, 0)).collect();
        assert_eq!(layout(7.0, 16, &line).0, 1);
        assert_eq!(layout(7.0, 16, &[TileCoord::new(3, 3)]).0, 1);
        let (b, workers) = layout(1002.0, 16, &area);
        assert!((1..=MOST_BLOCK).contains(&b));
        // As many workers as their windows fit in the memory, if not as many as the threads.
        let window = window_side(b, 1002.0).pow(2) * WINDOW_PIXEL;
        assert!(workers == 16 || ((workers + 1) * window > WINDOWS_MEMORY && workers * window <= WINDOWS_MEMORY));
        assert_eq!(layout(7.0, 16, &area).1, 16, "small windows, every thread");
        let m = rect(0.0, 0.0, 3000.0, 3000.0);
        let stop = Bounds { stop: &|| true, ..UNBOUNDED };
        assert_eq!(m.grown_within(40.0, Reach::Round, stop).unwrap_err(), Halt::Stopped, "not stopped");
        // Worked out in blocks, the same as tile by tile would give: a corner and a side.
        let grown = m.grown(40.0, Reach::Square);
        assert_eq!([grown.level_at(-40, -40), grown.level_at(-41, 1500), grown.level_at(1500, 3039)], [FULL, 0, FULL]);
    }

    #[test]
    fn a_soft_tile_wholly_on_one_side_of_half_is_no_part_of_the_outline() {
        // Inside a selected rectangle, a tile of levels from 60% to 90%: all above half, so the
        // outline does not cross it, and growing leaves it fully selected (it is in).
        let soft: std::sync::Arc<[u16]> =
            (0..TILE_PIXELS).map(|i| level(0.6 + 0.3 * (i % 256) as f32 / 255.0)).collect();
        let m = rect(0.0, 0.0, 4096.0, 4096.0).rebuilt(vec![(TileCoord::new(8, 8), MaskTile::Levels(soft))], |l| l, 0);
        assert!(!m.edge_tiles().contains(&TileCoord::new(8, 8)), "a tile wholly above half taken for the outline's");
        assert_eq!(m.grown(4.0, Reach::Round).level_at(2100, 2100), FULL);
    }

    #[test]
    fn a_modifier_asks_for_the_memory_it_takes_as_it_goes_and_stops_without_it() {
        use std::sync::Mutex;
        // What it asks for: the windows first, then the tiles it keeps, at the size they take.
        let asked = |m: &Mask, grow: f64, refuse_after: usize| {
            let asks = Mutex::new(Vec::new());
            let admit = |n: u64| {
                let mut a = asks.lock().unwrap();
                a.push(n);
                a.len() <= refuse_after
            };
            let r = m.grown_within(grow, Reach::Square, Bounds { admit: &admit, ..UNBOUNDED });
            (r.err(), asks.into_inner().unwrap())
        };
        let tile = (TILE_PIXELS * 2) as u64;
        let large = rect(0.0, 0.0, 10_000.0, 10_000.0);
        let (halt, asks) = asked(&large, 5.0, usize::MAX);
        assert_eq!(halt, None);
        assert!(asks[0] > 0 && asks[0] <= WINDOWS_MEMORY as u64, "the windows: {}", asks[0]);
        // Straight edges and square corners keep a row of levels a tile, or none: far less than
        // a whole tile's levels for each tile near the outline.
        let near = near_tiles(&large, grow_band(5.0)).len() as u64;
        let kept: u64 = asks[1..].iter().sum();
        assert!(kept < near * tile / 20, "{kept} of {}", near * tile);
        // Refused memory at its second ask, it stops, saying so.
        assert_eq!(asked(&large, 5.0, 1).0, Some(Halt::NoMemory));
        assert_eq!(asked(&large, 5.0, 0).0, Some(Halt::NoMemory), "the windows were not asked for");
        // A step that changes nothing asks for nothing.
        assert_eq!(asked(&large, 0.0, 0), (None, Vec::new()));
    }

    #[test]
    fn a_modifier_gives_its_windows_back_so_that_the_next_step_may_have_them() {
        use crate::selection::{Selection, Step};
        use std::sync::Mutex;
        // Held now, and the most held at once, as the steps of a recipe are worked out.
        let held = Mutex::new((0i64, 0i64));
        let admit = |n: u64| {
            let mut h = held.lock().unwrap();
            h.0 += n as i64;
            h.1 = h.1.max(h.0);
            true
        };
        let give_back = |n: u64| held.lock().unwrap().0 -= n as i64;
        let bounds = Bounds { admit: &admit, give_back: &give_back, ..UNBOUNDED };
        let steps = vec![
            Step::Shape {
                shape: Shape::rectangle(&crate::geom::FRect::new(0.0, 0.0, 3000.0, 3000.0), true),
                combine: crate::mask::Combine::Replace,
            },
            Step::Grow { by: 40.0, corners: Reach::Round },
            Step::Feather { width: 30.0, side: Side::Centered, falloff: Falloff::Gaussian },
        ];
        Selection::of_within(steps, bounds).unwrap();
        let (now, most) = *held.lock().unwrap();
        // Each step's windows given back: held at the end, only the tiles kept, far less than
        // the most held at once (a step's windows and the tiles so far).
        assert!(now > 0 && now * 4 < most, "held {now} of {most} at the end");
    }

    #[test]
    fn each_side_of_the_outline_is_worked_out_only_as_far_as_its_levels_vary() {
        // The left edge of a large square at x = 0: tile -1 outside it, tile 0 inside.
        let large = rect(0.0, 0.0, 10_000.0, 10_000.0);
        let near = |by: f64| near_tiles(&large, grow_band(by)).into_iter().collect::<HashSet<TileCoord>>();
        let reaches = |near: &HashSet<TileCoord>, x: i32| near.contains(&TileCoord::new(x, 20));
        // Grown by 900: four tiles out, but inside, where every pixel stays selected, only the
        // tile beside the outline's.
        let grown = near(900.0);
        assert!(reaches(&grown, -5) && !reaches(&grown, -6), "outside, as far as the band");
        assert!(reaches(&grown, 1) && !reaches(&grown, 2), "inside, past where its levels vary");
        // Shrunk, the other way round.
        let shrunk = near(-900.0);
        assert!(reaches(&shrunk, 4) && !reaches(&shrunk, 5));
        assert!(reaches(&shrunk, -2) && !reaches(&shrunk, -3));
        // And what it makes is what it was: selected all the way in, to 300 out.
        let m = rect(0.0, 0.0, 1000.0, 1000.0).grown(300.0, Reach::Square);
        assert_eq!([m.level_at(500, 500), m.level_at(-300, 500), m.level_at(-301, 500)], [FULL, FULL, 0]);
    }

    #[test]
    fn smoothing_fills_pinholes_takes_specks_away_and_rounds_corners() {
        let unbounded = |m: &Mask, r: f64| m.smoothed_within(r, UNBOUNDED).unwrap();
        // A square with a pinhole in it, and a speck beside it.
        let square = rect(0.0, 0.0, 200.0, 200.0);
        let holed = square.combined(&rect(100.0, 100.0, 103.0, 103.0), crate::mask::Combine::Subtract);
        let specked = holed.combined(&rect(300.0, 0.0, 303.0, 3.0), crate::mask::Combine::Add);
        let smooth = unbounded(&specked, 5.0);
        assert_eq!([smooth.level_at(101, 101), smooth.level_at(301, 1), smooth.level_at(150, 50)], [FULL, 0, FULL]);
        // The square's corner rounded: its very corner left out, the edge's middle kept.
        assert_eq!([smooth.level_at(0, 0), smooth.level_at(0, 100)], [0, FULL]);
        // Nothing by no radius.
        assert_eq!(unbounded(&specked, 0.0), specked);
    }

    #[test]
    fn a_border_is_a_band_along_the_outline_on_the_side_asked() {
        let square = rect(100.0, 100.0, 300.0, 300.0);
        let border = |side| square.bordered_within(10.0, side, UNBOUNDED).unwrap();
        // Pixels whose middles are 6.5 and 2.5 outside the left edge, 2.5 and 7.5 inside it, and
        // the middle: inward, 10 inside to the outline; outward, the outline to 10 outside;
        // centered, 5 each way.
        let at = |m: &Mask| [93, 97, 102, 107, 200].map(|x| m.level_at(x, 200));
        assert_eq!(at(&border(Side::Inward)), [0, 0, FULL, FULL, 0]);
        assert_eq!(at(&border(Side::Outward)), [FULL, FULL, 0, 0, 0]);
        assert_eq!(at(&border(Side::Centered)), [0, FULL, FULL, 0, 0]);
        // No width, nothing.
        assert!(square.bordered_within(0.0, Side::Centered, UNBOUNDED).unwrap().is_none());
    }

    #[test]
    fn a_band_asks_for_the_windows_of_no_more_workers_than_it_has_blocks() {
        // A small square grown far: a few blocks, on a pool of more threads.
        let m = rect(100.0, 100.0, 140.0, 140.0);
        let band = grow_band(300.0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(64).build().unwrap();
        let near = near_tiles(&m, band);
        let (b, workers) = layout(band.most(), 64, &near);
        let blocks: HashSet<(i32, i32)> = near.iter().map(|c| (c.x.div_euclid(b), c.y.div_euclid(b))).collect();
        assert!(blocks.len() < workers, "as many blocks as workers: nothing to tell");
        let first = std::sync::Mutex::new(None);
        let admit = |n: u64| {
            first.lock().unwrap().get_or_insert(n);
            true
        };
        let bounds = Bounds { admit: &admit, ..UNBOUNDED };
        pool.install(|| m.grown_within(300.0, Reach::Round, bounds)).unwrap();
        let window = (window_side(b, band.most()).pow(2) * WINDOW_PIXEL) as u64;
        assert_eq!(first.into_inner().unwrap(), Some(blocks.len() as u64 * window));
    }

    /// Bounds that count what is taken and given back, into `held`.
    fn counting(held: &std::sync::atomic::AtomicI64) -> (impl Fn(u64) -> bool + Sync + '_, impl Fn(u64) + Sync + '_) {
        use std::sync::atomic::Ordering::SeqCst;
        (
            move |n: u64| {
                held.fetch_add(n as i64, SeqCst);
                true
            },
            move |n: u64| {
                held.fetch_sub(n as i64, SeqCst);
            },
        )
    }

    #[test]
    fn what_each_smoothing_pass_took_is_given_back_once_the_next_is_made() {
        let held = std::sync::atomic::AtomicI64::new(0);
        let (admit, give_back) = counting(&held);
        let bounds = Bounds { admit: &admit, give_back: &give_back, ..UNBOUNDED };
        let circle = Mask::of(&Shape {
            kind: ShapeKind::Ellipse,
            center: [600.0, 600.0],
            size: [1000.0, 1000.0],
            angle: 0.0,
            smooth: true,
        });
        let smoothed = circle.smoothed_within(8.0, bounds).unwrap();
        // Four passes made, the last kept: what is still taken is what it holds.
        let held = held.load(std::sync::atomic::Ordering::SeqCst);
        assert!(held > 0 && held as u64 <= smoothed.bytes(), "{held} taken for {}", smoothed.bytes());
    }

    #[test]
    fn an_inverted_mask_grows_into_its_hole() {
        let hole = rect(0.0, 0.0, 300.0, 300.0).inverted();
        let grown = hole.grown(20.0, Reach::Square);
        assert_eq!([grown.level_at(19, 150), grown.level_at(20, 150), grown.level_at(-5000, 7)], [FULL, 0, FULL]);
    }
}
