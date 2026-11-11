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
use rayon::prelude::*;
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

/// The names of `ALL`'s values, as the recipe (serde), the API and the dialogs name them.
macro_rules! named {
    ($t:ident: $($v:ident = $n:literal),+) => {
        impl $t {
            pub const ALL: [$t; [$($n),+].len()] = [$($t::$v),+];
            pub const NAMES: [&'static str; [$($n),+].len()] = [$($n),+];

            /// The one named `name` (see `NAMES`).
            pub fn named(name: &str) -> Option<$t> {
                $t::NAMES.iter().position(|n| *n == name).map(|i| $t::ALL[i])
            }
        }
    };
}

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
    let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let y = 1.0 - poly * (-x * x).exp();
    if x < 0.0 { -y } else { y }
}

/// How much of a pixel whose middle is `d` from an edge (less than 0 inside) lies inside it.
fn coverage(d: f64) -> u16 {
    level((0.5 - d).clamp(0.0, 1.0) as f32)
}

impl Mask {
    /// The mask grown by `by` pixels (shrunk, if less than 0), its corners growing as `reach`
    /// says.
    pub fn grown(&self, by: f64, reach: Reach) -> Mask {
        self.grown_unless(by, reach, &|| false).expect("never stopped")
    }

    /// The mask feathered: its edge falling off over `width` pixels, on the `side` of its
    /// outline, along `falloff`.
    pub fn feathered(&self, width: f64, side: Side, falloff: Falloff) -> Mask {
        self.feathered_unless(width, side, falloff, &|| false).expect("never stopped")
    }

    /// [`grown`](Self::grown), unless `stop` says to stop, which it is asked between the pieces
    /// of the work: then none.
    pub fn grown_unless(&self, by: f64, reach: Reach, stop: &(dyn Fn() -> bool + Sync)) -> Option<Mask> {
        if by == 0.0 {
            return Some(self.clone());
        }
        by_distance(self, by.abs() + 2.0, reach, |d| coverage(d - by), stop)
    }

    /// [`feathered`](Self::feathered), unless `stop` says to stop (see
    /// [`grown_unless`](Self::grown_unless)).
    pub fn feathered_unless(&self, width: f64, side: Side, falloff: Falloff, stop: &(dyn Fn() -> bool + Sync)) -> Option<Mask> {
        if width <= 0.0 {
            return Some(self.clone());
        }
        // Where the falloff begins, from the outline, out (more than 0) or in.
        let outer = match side {
            Side::Inward => 0.0,
            Side::Outward => width,
            Side::Centered => width / 2.0,
        };
        // Past the falloff's far side, each way, every pixel is as the falloff ends.
        let band = outer.max(width - outer) + 2.0;
        by_distance(self, band, Reach::Round, move |d| level(falloff.at((outer - d) / width) as f32), stop)
    }
}

/// A count of what may be held at once: taking one waits until one is free.
struct Permits {
    free: std::sync::Mutex<usize>,
    freed: std::sync::Condvar,
}

impl Permits {
    fn new(n: usize) -> Permits {
        Permits { free: std::sync::Mutex::new(n), freed: std::sync::Condvar::new() }
    }

    /// Runs `f` holding one.
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        let mut free = self.free.lock().expect("not poisoned");
        while *free == 0 {
            free = self.freed.wait(free).expect("not poisoned");
        }
        *free -= 1;
        drop(free);
        let r = f();
        *self.free.lock().expect("not poisoned") += 1;
        self.freed.notify_one();
        r
    }
}

/// How many tiles a side the blocks worked out in one window have for `band`: those that do
/// the most work a thread for the memory (a larger block repeats less of its margin, but
/// fewer fit in [`WINDOWS_MEMORY`] at once to keep the threads busy).
fn block_tiles(band: f64) -> i32 {
    let threads = rayon::current_num_threads().max(1) as f64;
    let margin = 2.0 * (band.ceil() + 1.0);
    let side = f64::from(TILE_SIZE);
    let pace = |b: i32| {
        let window = (f64::from(b) * side + margin).powi(2);
        let at_once = (WINDOWS_MEMORY as f64 / (window * WINDOW_PIXEL as f64)).floor().max(1.0);
        // Tiles done a thread's pixel worked, by the threads kept busy.
        at_once.min(threads) * f64::from(b * b) / window
    };
    (1..=MOST_BLOCK).max_by(|&a, &b| pace(a).total_cmp(&pace(b))).expect("one at least")
}

/// `mask` with each pixel's level what `level_of` makes of its distance to the outline, as
/// `reach` measures it (less than 0 inside), worked out within `band` pixels of it: farther,
/// as `level_of` says at the band's edge. The tiles within the band are worked out in blocks,
/// a window each (the block and a margin of the band round it), in parallel, as many at once
/// as fit in [`WINDOWS_MEMORY`]; `stop` is asked before each, and if it says so, none.
fn by_distance(
    mask: &Mask,
    band: f64,
    reach: Reach,
    level_of: impl Fn(f64) -> u16 + Sync,
    stop: &(dyn Fn() -> bool + Sync),
) -> Option<Mask> {
    let far_in = level_of(-band);
    let far_out = level_of(band);
    let far = |l: u16| if l >= HALF { far_in } else { far_out };
    let outside = far(mask.outside());
    // The tiles within the band of those the outline may cross: a pixel within it of the
    // outline lies at most that many tiles across or down from the outline's tile.
    let k = (band / f64::from(TILE_SIZE)).ceil() as i32;
    let mut near: HashSet<TileCoord> = HashSet::new();
    for c in mask.edge_tiles() {
        for dy in -k..=k {
            for dx in -k..=k {
                near.insert(TileCoord::new(c.x + dx, c.y + dy));
            }
        }
    }
    let b = block_tiles(band);
    let mut blocks: std::collections::HashMap<(i32, i32), Vec<TileCoord>> = std::collections::HashMap::new();
    for c in near {
        blocks.entry((c.x.div_euclid(b), c.y.div_euclid(b))).or_default().push(c);
    }
    let window = |(bx, by): (i32, i32)| {
        let (a, z) = (TileCoord::new(bx * b, by * b).rect(0), TileCoord::new(bx * b + b - 1, by * b + b - 1).rect(0));
        IRect::new(a.x0, a.y0, z.x1, z.y1).expand(band.ceil() as i64 + 1)
    };
    let bytes = (window((0, 0)).width() as usize).pow(2) * WINDOW_PIXEL;
    let permits = Permits::new((WINDOWS_MEMORY / bytes).max(1));
    let blocks: Vec<((i32, i32), Vec<TileCoord>)> = blocks.into_iter().collect();
    let worked: Vec<Vec<(TileCoord, MaskTile)>> = blocks
        .par_iter()
        .map(|(at, tiles)| {
            let w = window(*at);
            // No outline within reach of the block: each tile as its side ends.
            if let Some(inside) = mask.side_in(&w, HALF) {
                return Some(tiles.iter().map(|&c| (c, MaskTile::Level(far(if inside { FULL } else { 0 })))).collect());
            }
            if stop() {
                return None;
            }
            permits.with(|| (!stop()).then(|| block_by_distance(mask, &w, tiles, band, reach, &level_of)))
        })
        .collect::<Option<Vec<_>>>()?;
    // Farther, each tile is as its side ends.
    Some(mask.rebuilt(worked.into_iter().flatten().collect(), far, outside))
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
    let at = |x: i64, y: i64| (x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h).then(|| y as usize * w + x as usize);
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

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Mask {
        Mask::of(&Shape::rectangle(&crate::geom::FRect::new(x0, y0, x1, y1), true))
    }

    #[test]
    fn a_rectangle_grows_and_shrinks_by_whole_pixels_its_corners_as_the_reach_says() {
        let m = rect(100.0, 100.0, 200.0, 150.0);
        assert_eq!(m.grown(10.0, Reach::Square).extent(), Extent::Within(IRect::new(90, 90, 210, 160)));
        assert_eq!(m.grown(-10.0, Reach::Round).extent(), Extent::Within(IRect::new(110, 110, 190, 140)));
        // Round, the corner 10 out along the diagonal is not reached, square it is.
        let (round, square, diamond) = (m.grown(10.0, Reach::Round), m.grown(10.0, Reach::Square), m.grown(10.0, Reach::Diamond));
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
            Mask::of(&Shape { kind: ShapeKind::Ellipse, center: [300.3, 299.6], size: [2.0 * r, 2.0 * r], angle: 0.0, smooth: true })
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
        fn check<T: serde::Serialize + PartialEq + std::fmt::Debug + Copy>(all: &[T], names: &[&str], named: fn(&str) -> Option<T>) {
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
        // A small band: blocks of several tiles a side, which repeat less of their margin than
        // a tile alone would (how many depends on the threads there are to keep busy).
        assert!(block_tiles(7.0) > 1);
        assert!((1..=MOST_BLOCK).contains(&block_tiles(1002.0)));
        let m = rect(0.0, 0.0, 3000.0, 3000.0);
        assert!(m.grown_unless(40.0, Reach::Round, &|| true).is_none(), "not stopped");
        // Worked out in blocks, the same as tile by tile would give: a corner and a side.
        let grown = m.grown(40.0, Reach::Square);
        assert_eq!([grown.level_at(-40, -40), grown.level_at(-41, 1500), grown.level_at(1500, 3039)], [FULL, 0, FULL]);
    }

    #[test]
    fn a_soft_tile_wholly_on_one_side_of_half_is_no_part_of_the_outline() {
        // Inside a selected rectangle, a tile of levels from 60% to 90%: all above half, so the
        // outline does not cross it, and growing leaves it fully selected (it is in).
        let soft: std::sync::Arc<[u16]> = (0..TILE_PIXELS).map(|i| level(0.6 + 0.3 * (i % 256) as f32 / 255.0)).collect();
        let m = rect(0.0, 0.0, 4096.0, 4096.0).rebuilt(vec![(TileCoord::new(8, 8), MaskTile::Levels(soft))], |l| l, 0);
        assert!(!m.edge_tiles().contains(&TileCoord::new(8, 8)), "a tile wholly above half taken for the outline's");
        assert_eq!(m.grown(4.0, Reach::Round).level_at(2100, 2100), FULL);
    }

    #[test]
    fn an_inverted_mask_grows_into_its_hole() {
        let hole = rect(0.0, 0.0, 300.0, 300.0).inverted();
        let grown = hole.grown(20.0, Reach::Square);
        assert_eq!([grown.level_at(19, 150), grown.level_at(20, 150), grown.level_at(-5000, 7)], [FULL, 0, FULL]);
    }
}
