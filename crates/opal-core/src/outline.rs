//! Outlines (design.md, *The outline tool*): closed paths of straight segments and cubic
//! curves, which select what they enclose, each pixel as much of it as they cover. A loop that
//! crosses itself selects each of its lobes, and loops inside one another going the same way
//! select all of them (the nonzero rule).
//!
//! A mask is made of one a row of tiles at a time (`Coverage::row`): the tiles its edges pass
//! through are worked out (`Coverage::tile`), and those between them, wholly in or out, are
//! told by the winding at their middles, so that the work follows the outline's length, not
//! the area it encloses. In a tile, each row of pixels is cut where edges begin, end or
//! cross; between the cuts, each stretch the outline goes round adds the area it covers along
//! the row's cells, and a running sum along the row gives each pixel's share. A sharp outline
//! selects a pixel or not by its middle, counting the edges crossing its row.

use crate::geom::{Affine, IRect};
use crate::mask::{Class, Coverage, FULL, MASK_REACH, MaskTile, world};
use crate::modify::{Bounds, Halt, UNBOUNDED};
use crate::tile::{TILE_PIXELS, TILE_SIZE, TileCoord};
use std::collections::HashMap;

/// How far a curve's flattening may stray from it, in pixels.
const TOLERANCE: f64 = 0.05;

/// The most pieces a curve is flattened to, however long it is.
const MOST_PIECES: usize = 4096;

/// The most points an outline's curves are flattened to, all together.
const MOST_FLATTENED: usize = 4_000_000;

/// One segment of an outline, from where the one before it ends.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Segment {
    Line {
        to: [f64; 2],
    },
    /// A cubic Bézier curve, pulled toward its control points `c1` and `c2`.
    Curve {
        c1: [f64; 2],
        c2: [f64; 2],
        to: [f64; 2],
    },
}

impl Segment {
    /// Where it ends.
    pub fn end(&self) -> [f64; 2] {
        match self {
            Segment::Line { to } | Segment::Curve { to, .. } => *to,
        }
    }

    /// The segment `m` carries it to (exactly: a curve's control points carry it).
    fn transformed(&self, m: &Affine) -> Segment {
        match self {
            Segment::Line { to } => Segment::Line { to: m.apply(*to) },
            Segment::Curve { c1, c2, to } => Segment::Curve { c1: m.apply(*c1), c2: m.apply(*c2), to: m.apply(*to) },
        }
    }
}

/// A closed outline: from `start` along its segments, then back to `start`; sharp if not
/// `smooth` (a pixel selected or not by its middle).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Outline {
    pub start: [f64; 2],
    pub segments: Vec<Segment>,
    pub smooth: bool,
}

impl Outline {
    /// The outline `m` carries this one to.
    pub fn transformed(&self, m: &Affine) -> Outline {
        Outline {
            start: m.apply(self.start),
            segments: self.segments.iter().map(|s| s.transformed(m)).collect(),
            smooth: self.smooth,
        }
    }

    /// The polygon it comes to (see [`flattened`]), closed (the last point joined to the first).
    pub fn polygon(&self) -> Vec<[f64; 2]> {
        flattened(self.start, &self.segments)
    }

    /// What it covers, to make a mask of (`Mask::of`).
    pub fn filled(&self) -> Filled {
        self.filled_within(UNBOUNDED).expect("unbounded")
    }

    /// [`filled`](Self::filled), unless `bounds` hold it back: the points its curves are
    /// flattened to, then its edges and their lists by row of tiles, admitted before they are
    /// made. What it then holds is [`Filled::bytes`], to give back once its mask is made.
    pub fn filled_within(&self, bounds: Bounds) -> Result<Filled, Halt> {
        let points = (flattened_len(self.start, &self.segments) * std::mem::size_of::<[f64; 2]>()) as u64;
        if !(bounds.admit)(points) {
            return Err(Halt::NoMemory);
        }
        let polygon = self.polygon();
        let made = if (bounds.admit)(Filled::need(&polygon)) {
            Ok(Filled::of(&polygon, self.smooth))
        } else {
            Err(Halt::NoMemory)
        };
        (bounds.give_back)(points);
        made
    }
}

/// How many pieces each of `segments` (from `start`) is flattened to before the share
/// [`MOST_FLATTENED`] leaves: a line one, a curve enough that each strays at most
/// [`TOLERANCE`] (a cubic's distance from its chord is at most 3/4 of its control polygon's
/// second differences), up to [`MOST_PIECES`].
fn pieces(start: [f64; 2], segments: &[Segment]) -> Vec<usize> {
    let mut at = start;
    segments
        .iter()
        .map(|s| {
            let n = match *s {
                Segment::Line { .. } => 1,
                Segment::Curve { c1, c2, to } => {
                    let d = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
                        (a[0] - 2.0 * b[0] + c[0]).hypot(a[1] - 2.0 * b[1] + c[1])
                    };
                    let bend = d(at, c1, c2).max(d(c1, c2, to));
                    ((0.75 * bend / TOLERANCE).sqrt().ceil() as usize).clamp(1, MOST_PIECES)
                }
            };
            at = s.end();
            n
        })
        .collect()
}

/// How many points [`flattened`] makes of `segments` from `start`, at most.
fn flattened_len(start: [f64; 2], segments: &[Segment]) -> usize {
    pieces(start, segments).iter().sum::<usize>().min(MOST_FLATTENED + segments.len()) + 1
}

/// The points of the line from `start` along `segments`: their ends, and their curves
/// flattened to within [`TOLERANCE`]; to fewer points, each curve in proportion, if together
/// they would come to more than [`MOST_FLATTENED`].
pub fn flattened(start: [f64; 2], segments: &[Segment]) -> Vec<[f64; 2]> {
    let pieces = pieces(start, segments);
    let all: usize = pieces.iter().sum();
    let share = if all > MOST_FLATTENED { MOST_FLATTENED as f64 / all as f64 } else { 1.0 };
    let mut points = Vec::with_capacity(all.min(MOST_FLATTENED + segments.len()) + 1);
    points.push(start);
    let mut at = start;
    for (s, &n) in segments.iter().zip(&pieces) {
        match *s {
            Segment::Line { to } => points.push(to),
            Segment::Curve { c1, c2, to } => {
                let n = ((n as f64 * share).ceil() as usize).max(1);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    points.push([
                        a * at[0] + b * c1[0] + c * c2[0] + e * to[0],
                        a * at[1] + b * c1[1] + c * c2[1] + e * to[1],
                    ]);
                }
            }
        }
        at = s.end();
    }
    points
}

/// An edge of a polygon, from `a` to `b`.
#[derive(Clone, Copy, Debug)]
struct Edge {
    a: [f64; 2],
    b: [f64; 2],
}

/// A polygon filled by the nonzero rule, as a mask is made of it (see the module's
/// description): its edges, and which reach into each row of tiles.
pub struct Filled {
    edges: Vec<Edge>,
    /// The edges reaching into each row of tiles (by its y, in tiles), the rows within the
    /// mask's reach only, so that how many entries it keeps follows the outline's length.
    rows: HashMap<i32, Vec<usize>>,
    /// Within the mask's reach.
    bounds: IRect,
    smooth: bool,
}

/// The tile `v` (pixels) lies in, along either axis, held to one past the mask's reach.
fn tile_of(v: f64) -> i32 {
    (v / f64::from(TILE_SIZE)).floor().clamp(f64::from(-MASK_REACH - 1), f64::from(MASK_REACH)) as i32
}

/// How many rows of tiles within the mask's reach an edge from height `y0` to `y1` is listed in.
fn rows_spanned(y0: f64, y1: f64) -> u64 {
    let (first, last) = (tile_of(y0.min(y1)).max(-MASK_REACH), tile_of(y0.max(y1)).min(MASK_REACH - 1));
    u64::try_from(i64::from(last) - i64::from(first) + 1).unwrap_or(0)
}

/// The bytes `edges` edges take, listed `entries` times by row of tiles (twice what a list's
/// entry takes, for the room lists keep spare).
fn held(edges: u64, entries: u64) -> u64 {
    edges * std::mem::size_of::<Edge>() as u64 + entries * 2 * std::mem::size_of::<usize>() as u64
}

/// An edge in a tile's coordinates, from its top to its bottom, with which way it winds (+1
/// where the outline runs down it, -1 where up).
type Down = ([f64; 2], [f64; 2], i32);

impl Filled {
    fn of(points: &[[f64; 2]], smooth: bool) -> Filled {
        let finite = points.iter().all(|p| p[0].is_finite() && p[1].is_finite());
        let edges: Vec<Edge> = if finite && points.len() >= 3 {
            (0..points.len())
                .map(|i| Edge { a: points[i], b: points[(i + 1) % points.len()] })
                .filter(|e| e.a != e.b)
                .collect()
        } else {
            Vec::new()
        };
        let mut rows: HashMap<i32, Vec<usize>> = HashMap::new();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (i, e) in edges.iter().enumerate() {
            let (ly, hy) = (e.a[1].min(e.b[1]), e.a[1].max(e.b[1]));
            (x0, y0, x1, y1) = (x0.min(e.a[0].min(e.b[0])), y0.min(ly), x1.max(e.a[0].max(e.b[0])), y1.max(hy));
            for ty in tile_of(ly).max(-MASK_REACH)..=tile_of(hy).min(MASK_REACH - 1) {
                rows.entry(ty).or_default().push(i);
            }
        }
        let bounds = if edges.is_empty() {
            IRect::new(0, 0, 0, 0)
        } else {
            IRect::new(x0.floor() as i64, y0.floor() as i64, x1.ceil() as i64, y1.ceil() as i64).intersect(&world())
        };
        Filled { edges, rows, bounds, smooth }
    }

    /// The bytes a `Filled` of the polygon `points` holds (its edges, and their lists by row of
    /// tiles, with room to spare), worked out without making it.
    fn need(points: &[[f64; 2]]) -> u64 {
        let n = points.len();
        let entries: u64 = (0..n).map(|i| rows_spanned(points[i][1], points[(i + 1) % n][1])).sum();
        held(n as u64, entries)
    }

    /// The bytes it holds (as [`need`](Self::need) counts them).
    pub fn bytes(&self) -> u64 {
        held(self.edges.len() as u64, self.rows.values().map(|r| r.len() as u64).sum())
    }

    /// The edges reaching into row `y` of tiles, but those wholly right of `right` (pixels):
    /// what lies left of `right` does not depend on them.
    fn row_edges(&self, y: i32, right: f64) -> impl Iterator<Item = &Edge> {
        self.rows.get(&y).into_iter().flatten().map(|&i| &self.edges[i]).filter(move |e| e.a[0].min(e.b[0]) < right)
    }

    /// The levels of tile `c`, smooth: each pixel as much of it as the polygon covers, by the
    /// nonzero rule. Each row of pixels is cut where an edge begins, ends or crosses another
    /// (the edges wholly left of the tile counted only as the winding at its left side), so
    /// that between the cuts the edges lie side by side; there each stretch the outline goes
    /// round (from the edge where the winding leaves 0 to the one where it comes back to it)
    /// adds what it covers of each pixel. The stretches never overlap, so what they add is
    /// exact however the outline crosses itself or nests.
    fn smooth_tile(&self, c: TileCoord) -> Vec<u16> {
        let r = c.rect(0);
        let (ox, oy) = (r.x0 as f64, r.y0 as f64);
        let w = TILE_SIZE as usize;
        // Each row's cells, and one past them for what reaches the tile's right side.
        let stride = w + 2;
        let mut acc = vec![0.0f32; stride * w];
        let all: Vec<Down> = self
            .row_edges(c.y, r.x1 as f64)
            .filter_map(|e| {
                let (a, b) = ([e.a[0] - ox, e.a[1] - oy], [e.b[0] - ox, e.b[1] - oy]);
                match a[1].total_cmp(&b[1]) {
                    std::cmp::Ordering::Less => Some((a, b, 1)),
                    std::cmp::Ordering::Greater => Some((b, a, -1)),
                    // Level, it bounds nothing.
                    std::cmp::Ordering::Equal => None,
                }
            })
            .collect();
        // Edges wholly left of the tile only wind: each row of pixels takes them as the winding
        // at its left side, which changes at their ends (and does not where one ends as
        // another begins, along a line of them).
        let (beside, edges): (Vec<Down>, Vec<Down>) = all.into_iter().partition(|(a, b, _)| a[0].max(b[0]) <= 0.0);
        let rows_of = |(a, b, _): &Down| {
            let first = a[1].floor().max(0.0) as usize;
            let past = (b[1].ceil().min(w as f64).max(0.0)) as usize;
            first.min(past)..past
        };
        let mut by_row: Vec<Vec<usize>> = vec![Vec::new(); w];
        for (i, e) in edges.iter().enumerate() {
            for row in &mut by_row[rows_of(e)] {
                row.push(i);
            }
        }
        let mut sides: Vec<Vec<(f64, i32)>> = vec![Vec::new(); w];
        for e in &beside {
            let (a, b, winds) = *e;
            for y in rows_of(e) {
                sides[y].extend([(a[1].max(y as f64), winds), (b[1].min(y as f64 + 1.0), -winds)]);
            }
        }
        let mut cuts: Vec<f64> = Vec::new();
        let mut across: Vec<(f64, f64, usize)> = Vec::new();
        let mut side: Vec<(f64, i32)> = Vec::new();
        for (y, here) in by_row.iter().enumerate() {
            // The left side's winding: from each height on, where it changes.
            let steps = &mut sides[y];
            steps.sort_by(|p, q| p.0.total_cmp(&q.0));
            side.clear();
            let mut sum = 0;
            for &(at, winds) in steps.iter() {
                sum += winds;
                match side.last_mut() {
                    Some(last) if last.0 == at => last.1 = sum,
                    _ => side.push((at, sum)),
                }
            }
            side.dedup_by(|later, earlier| later.1 == earlier.1);
            if here.is_empty() && side.iter().all(|s| s.1 == 0) {
                continue;
            }
            let (top, bottom) = (y as f64, y as f64 + 1.0);
            cuts.clear();
            cuts.extend([top, bottom]);
            for &i in here {
                let (a, b, _) = edges[i];
                cuts.extend([a[1], b[1]].into_iter().filter(|&v| v > top && v < bottom));
            }
            cuts.extend(side.iter().map(|s| s.0).filter(|&v| v > top && v < bottom));
            cuts.sort_by(f64::total_cmp);
            cuts.dedup();
            // As many as the edges' ends and their crossings can make: past that, only a
            // rounding error would be cutting further.
            let most = 2 + 2 * here.len() + here.len() * here.len() + side.len();
            let mut k = 0;
            while k + 1 < cuts.len() {
                let (ya, yb) = (cuts[k], cuts[k + 1]);
                across.clear();
                for &i in here {
                    let (a, b, _) = edges[i];
                    if a[1] <= ya && b[1] >= yb {
                        let x = |v: f64| a[0] + (v - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                        across.push((x(ya), x(yb), i));
                    }
                }
                if let Some(yc) = first_crossing(&mut across, ya, yb)
                    && cuts.len() < most
                {
                    cuts.insert(k + 1, yc);
                    continue;
                }
                // Left to right as they lie in the middle between the cuts (at a cut just
                // made where two cross, rounding may leave them either way round), from the
                // left side's winding, each stretch gone round.
                across.sort_by(|p, q| (p.0 + p.1).total_cmp(&(q.0 + q.1)));
                let middle = (ya + yb) / 2.0;
                let start = side.partition_point(|s| s.0 <= middle).checked_sub(1).map_or(0, |i| side[i].1);
                let mut stretch = |from: (f64, f64), sign: f64| {
                    let (p, q) = ([from.0, ya], [from.1, yb]);
                    let (p, q) = if sign > 0.0 { (p, q) } else { (q, p) };
                    for (p, q) in split_at_sides(p, q, w as f64) {
                        draw(&mut acc, stride, w, p, q);
                    }
                };
                let mut winding = start;
                let mut left: Option<(f64, f64)> = (start != 0).then_some((0.0, 0.0));
                for &(xa, xb, i) in &across {
                    let was = winding;
                    winding += edges[i].2;
                    if was == 0 && winding != 0 {
                        left = Some((xa, xb));
                    } else if was != 0 && winding == 0 {
                        stretch(left.take().expect("a stretch begun"), 1.0);
                        stretch((xa, xb), -1.0);
                    }
                }
                // One going on past the tile's right side, whose end was left out.
                if let Some(l) = left {
                    stretch(l, 1.0);
                }
                k += 1;
            }
        }
        let mut levels = vec![0u16; TILE_PIXELS];
        for y in 0..w {
            let mut sum = 0.0f32;
            for x in 0..w {
                sum += acc[y * stride + x];
                levels[y * w + x] = (sum.clamp(0.0, 1.0) * f32::from(FULL)).round() as u16;
            }
        }
        levels
    }

    /// The levels of tile `c`, sharp: each pixel selected or not by whether its middle is.
    fn sharp_tile(&self, c: TileCoord) -> Vec<u16> {
        let r = c.rect(0);
        let w = TILE_SIZE as usize;
        let mut levels = vec![0u16; TILE_PIXELS];
        let edges: Vec<&Edge> = self.row_edges(c.y, r.x1 as f64).collect();
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for y in 0..w {
            let py = r.y0 as f64 + y as f64 + 0.5;
            crossings.clear();
            for e in &edges {
                let ([x0, y0], [x1, y1]) = (e.a, e.b);
                let dir = if y0 <= py && py < y1 {
                    1
                } else if y1 <= py && py < y0 {
                    -1
                } else {
                    continue;
                };
                crossings.push((x0 + (py - y0) / (y1 - y0) * (x1 - x0), dir));
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            // Left to right: the winding past each crossing; a pixel is in where it is not 0.
            let mut winding = 0;
            let mut from = f64::NEG_INFINITY;
            for &(x, dir) in &crossings {
                if winding != 0 {
                    fill_middles(&mut levels[y * w..][..w], r.x0 as f64, from, x);
                }
                winding += dir;
                from = x;
            }
            // One going on past the tile's right side, whose end was left out.
            if winding != 0 {
                fill_middles(&mut levels[y * w..][..w], r.x0 as f64, from, f64::INFINITY);
            }
        }
        levels
    }
}

/// Where the first two of the edges `across` (each `(x at ya, x at yb, which)`) cross between
/// `ya` and `yb`, if any do (by more than a hair); sorts them left to right as they lie at
/// `ya`, then at `yb`.
fn first_crossing(across: &mut [(f64, f64, usize)], ya: f64, yb: f64) -> Option<f64> {
    across.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
    // In the same order at both cuts, straight edges cannot cross between them.
    if across.windows(2).all(|w| w[0].1 <= w[1].1) {
        return None;
    }
    let mut first: Option<f64> = None;
    for (i, p) in across.iter().enumerate() {
        for q in &across[i + 1..] {
            let (top, bottom) = (q.0 - p.0, q.1 - p.1);
            if top * bottom < 0.0 {
                let y = ya + top / (top - bottom) * (yb - ya);
                if y - ya > 1e-9 && y < yb {
                    first = Some(first.map_or(y, |f: f64| f.min(y)));
                }
            }
        }
    }
    first
}

/// Selects the pixels of `row` (its first at `x0`) whose middles lie in `[from, to)`.
fn fill_middles(row: &mut [u16], x0: f64, from: f64, to: f64) {
    let first = (from - x0 - 0.5).ceil().max(0.0) as usize;
    let last = ((to - x0 - 0.5).ceil().max(0.0) as usize).min(row.len());
    if first < last {
        row[first..last].fill(FULL);
    }
}

/// The edge `a`–`b` (in a tile's coordinates) in pieces each wholly left of the tile, within
/// it, or right of it, those left of it made vertical at its left side (what lies left of a
/// pixel covers it wholly across), and those right of it at its right (where it covers
/// nothing of the tile).
fn split_at_sides(a: [f64; 2], b: [f64; 2], w: f64) -> Vec<([f64; 2], [f64; 2])> {
    let mut cuts = vec![0.0, 1.0];
    for side in [0.0, w] {
        if (a[0] - side) * (b[0] - side) < 0.0 {
            cuts.push((side - a[0]) / (b[0] - a[0]));
        }
    }
    cuts.sort_by(f64::total_cmp);
    let at = |t: f64| [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
    cuts.windows(2)
        .map(|t| {
            let (p, q) = (at(t[0]), at(t[1]));
            let clamp = |p: [f64; 2]| [p[0].clamp(0.0, w), p[1]];
            (clamp(p), clamp(q))
        })
        .collect()
}

/// Adds what the edge `p`–`q` (within the tile across, x from 0 to the row's width) covers of
/// each cell of the rows it crosses to `acc` (`stride` cells a row, `h` rows): the signed area
/// of the cell to its right it accounts for, whose running sum along a row is each pixel's
/// coverage.
fn draw(acc: &mut [f32], stride: usize, h: usize, p: [f64; 2], q: [f64; 2]) {
    if p[1] == q[1] {
        return;
    }
    let (dir, p, q) = if p[1] < q[1] { (1.0f64, p, q) } else { (-1.0, q, p) };
    let (top, bottom) = (p[1].max(0.0), q[1].min(h as f64));
    if top >= bottom {
        return;
    }
    let dxdy = (q[0] - p[0]) / (q[1] - p[1]);
    let mut x = p[0] + (top - p[1]) * dxdy;
    let mut y = top;
    while y < bottom {
        let row = y.floor() as usize;
        let next = (row as f64 + 1.0).min(bottom);
        let dy = next - y;
        let xnext = x + dxdy * dy;
        let d = (dy * dir) as f32;
        let line = &mut acc[row * stride..][..stride];
        let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let (c0, c1) = (x0.floor() as usize, (x1.ceil() as usize).max(x0.floor() as usize + 1));
        if c1 <= c0 + 1 {
            // Within one cell: what lies right of its middle there, and the rest after it.
            let mid = (0.5 * (x + xnext) - c0 as f64) as f32;
            line[c0] += d * (1.0 - mid);
            line[c0 + 1] += d * mid;
        } else {
            // Across cells: each the share of its width the edge leaves to its right.
            let s = (1.0 / (x1 - x0)) as f32;
            let f0 = (x0 - c0 as f64) as f32;
            let a0 = 0.5 * s * (1.0 - f0) * (1.0 - f0);
            let f1 = (x1 - c1 as f64 + 1.0) as f32;
            let am = 0.5 * s * f1 * f1;
            line[c0] += d * a0;
            if c1 == c0 + 2 {
                line[c0 + 1] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - f0);
                line[c0 + 1] += d * (a1 - a0);
                for cell in &mut line[c0 + 2..c1 - 1] {
                    *cell += d * s;
                }
                let a2 = a1 + (c1 - c0 - 3) as f32 * s;
                line[c1 - 1] += d * (1.0 - a2 - am);
            }
            line[c1] += d * am;
        }
        x = xnext;
        y = next;
    }
}

impl Coverage for Filled {
    fn bounds(&self) -> IRect {
        self.bounds
    }

    fn tile(&self, c: TileCoord) -> MaskTile {
        MaskTile::from_levels(if self.smooth { self.smooth_tile(c) } else { self.sharp_tile(c) })
    }

    fn row(&self, y: i32, x0: i32, x1: i32) -> Vec<(i32, i32, Class)> {
        // The tiles an edge passes through (each edge's stretch within the row), and, between
        // them, tiles wholly in or out, told by the winding at their middles, which no edge
        // between them changes: the work follows the outline's length, not its area.
        let side = f64::from(TILE_SIZE);
        let (top, bottom) = (f64::from(y) * side, f64::from(y + 1) * side);
        let middle = top + side / 2.0;
        let mut crossed: Vec<(i32, i32)> = Vec::new();
        // Where the edges cross the row's middle, and which way.
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for &i in self.rows.get(&y).into_iter().flatten() {
            let ([ax, ay], [bx, by]) = (self.edges[i].a, self.edges[i].b);
            let (lx, hx) = if ay == by {
                (ax.min(bx), ax.max(bx))
            } else {
                let x = |v: f64| ax + (v.clamp(ay.min(by), ay.max(by)) - ay) / (by - ay) * (bx - ax);
                let (p, q) = (x(top), x(bottom));
                (p.min(q), p.max(q))
            };
            let (a, b) = (tile_of(lx).max(x0), tile_of(hx).min(x1 - 1));
            if a <= b {
                crossed.push((a, b));
            }
            let winds = if ay <= middle && middle < by {
                1
            } else if by <= middle && middle < ay {
                -1
            } else {
                0
            };
            if winds != 0 {
                crossings.push((ax + (middle - ay) / (by - ay) * (bx - ax), winds));
            }
        }
        crossed.sort_unstable();
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        // The winding at a point counts the crossings right of it.
        let all: i32 = crossings.iter().map(|c| c.1).sum();
        let (mut passed, mut left_of) = (0, 0);
        let mut told: Vec<(i32, i32, Class)> = Vec::new();
        let mut gap = |from: i32, to: i32, told: &mut Vec<(i32, i32, Class)>| {
            let cx = (f64::from(from) + 0.5) * side;
            while passed < crossings.len() && crossings[passed].0 <= cx {
                left_of += crossings[passed].1;
                passed += 1;
            }
            if all - left_of != 0 {
                told.push((from, to, Class::Inside));
            } else {
                told.push((from, to, Class::Outside));
            }
        };
        let mut x = x0;
        let mut k = 0;
        while k < crossed.len() {
            let (a, mut b) = crossed[k];
            k += 1;
            while k < crossed.len() && crossed[k].0 <= b + 1 {
                b = b.max(crossed[k].1);
                k += 1;
            }
            if x < a {
                gap(x, a, &mut told);
            }
            told.extend((a.max(x)..=b).map(|t| (t, t + 1, Class::Edge)));
            x = x.max(b + 1);
        }
        if x < x1 {
            gap(x, x1, &mut told);
        }
        told
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::{Extent, Mask, Shape, ShapeKind};

    fn polygon(points: &[[f64; 2]], smooth: bool) -> Outline {
        Outline { start: points[0], segments: points[1..].iter().map(|&to| Segment::Line { to }).collect(), smooth }
    }

    #[test]
    fn a_rectangle_outlined_covers_each_pixel_as_much_as_it_overlaps_it() {
        // On whole pixels, and between them, across tiles.
        for (x0, y0, x1, y1) in [(10.0, 20.0, 300.0, 280.0), (10.3, 20.6, 300.25, 280.9)] {
            let smooth = Mask::of(&polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], true).filled());
            let overlap = |a: f64, b: f64, p: i64| (b.min(p as f64 + 1.0) - a.max(p as f64)).max(0.0);
            let worst = (0..320)
                .flat_map(|y| (0..320).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let exact = crate::mask::level((overlap(x0, x1, x) * overlap(y0, y1, y)) as f32);
                    smooth.level_at(x, y).abs_diff(exact)
                })
                .max()
                .unwrap();
            assert!(worst <= 2, "{worst} apart at worst ({x0}, {y0})");
            // Sharp: as the sharp rectangle selects, by each pixel's middle.
            let sharp = Mask::of(&polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], false).filled());
            let shape = Shape {
                kind: ShapeKind::Rectangle,
                center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
                size: [x1 - x0, y1 - y0],
                angle: 0.0,
                smooth: false,
            };
            assert_eq!(sharp, Mask::of(&shape));
        }
    }

    #[test]
    fn a_curve_flattened_follows_a_circle_and_its_inside_is_selected() {
        // Four curves round a circle of radius 100 (the usual 0.5523 handles), across tiles.
        let (cx, cy, r, k) = (250.0, 260.0, 100.0, 0.552_284_75 * 100.0);
        let start = [cx + r, cy];
        let segments = vec![
            Segment::Curve { c1: [cx + r, cy + k], c2: [cx + k, cy + r], to: [cx, cy + r] },
            Segment::Curve { c1: [cx - k, cy + r], c2: [cx - r, cy + k], to: [cx - r, cy] },
            Segment::Curve { c1: [cx - r, cy - k], c2: [cx - k, cy - r], to: [cx, cy - r] },
            Segment::Curve { c1: [cx + k, cy - r], c2: [cx + r, cy - k], to: start },
        ];
        let m = Mask::of(&Outline { start, segments, smooth: true }.filled());
        assert_eq!(m.extent(), Extent::Within(IRect::new(150, 160, 350, 360)));
        assert_eq!([m.level_at(250, 260), m.level_at(160, 260), m.level_at(140, 260)], [FULL, FULL, 0]);
        // All it covers is the circle's area, as near as four curves come to a circle.
        let area: f64 = (160..360)
            .flat_map(|y| (150..350).map(move |x| (x, y)))
            .map(|(x, y)| f64::from(crate::mask::fraction(m.level_at(x, y))))
            .sum();
        let circle = std::f64::consts::PI * r * r;
        assert!((area - circle).abs() < circle * 0.001, "{area} for {circle}");
    }

    #[test]
    fn a_loop_crossing_itself_selects_both_lobes_and_one_inside_another_both() {
        // A figure eight: each lobe selected.
        let eight = polygon(&[[0.0, 0.0], [200.0, 200.0], [200.0, 0.0], [0.0, 200.0]], false);
        let m = Mask::of(&eight.filled());
        assert_eq!([m.level_at(20, 100), m.level_at(180, 100), m.level_at(100, 20)], [FULL, FULL, 0]);
        // A loop inside another going the same way: all selected, the inner too.
        let mut twice = polygon(&[[0.0, 0.0], [300.0, 0.0], [300.0, 300.0], [0.0, 300.0]], true);
        twice.segments.push(Segment::Line { to: [0.0, 0.0] });
        for to in [[100.0, 100.0], [200.0, 100.0], [200.0, 200.0], [100.0, 200.0], [100.0, 100.0]] {
            twice.segments.push(Segment::Line { to });
        }
        let m = Mask::of(&twice.filled());
        assert_eq!([m.level_at(150, 150), m.level_at(50, 50)], [FULL, FULL]);
    }

    #[test]
    fn where_lobes_or_loops_meet_within_a_pixel_it_is_covered_as_they_cover_it_together() {
        let half = f64::from(FULL) / 2.0;
        // A figure eight crossing in the middle of pixel (5, 5): each lobe covers a quarter of
        // it, half of it together, though they go round opposite ways.
        let eight = polygon(&[[0.0, 0.0], [11.0, 11.0], [11.0, 0.0], [0.0, 11.0]], true);
        let m = Mask::of(&eight.filled());
        assert!((f64::from(m.level_at(5, 5)) - half).abs() <= 1.0, "{}", m.level_at(5, 5));
        // Two loops going the same way, their left sides half a pixel and seven tenths of one
        // into the pixels of column 0: those covered half (as the outer one covers them), not
        // eight tenths (the two loops' shares summed).
        let mut nested = polygon(&[[0.5, 0.0], [20.0, 0.0], [20.0, 20.0], [0.5, 20.0]], true);
        nested.segments.push(Segment::Line { to: [0.5, 0.0] });
        for to in [[0.7, 2.0], [18.0, 2.0], [18.0, 18.0], [0.7, 18.0], [0.7, 2.0]] {
            nested.segments.push(Segment::Line { to });
        }
        let m = Mask::of(&nested.filled());
        assert!((f64::from(m.level_at(0, 10)) - half).abs() <= 1.0, "{}", m.level_at(0, 10));
    }

    #[test]
    fn figure_eights_crossing_anywhere_cover_exactly_their_lobes() {
        // Their crossings at heights a binary fraction does not hold: each covers its two
        // lobes, a quarter of its box each, whichever way rounding leaves the sides at the cut
        // through the crossing.
        let mut worst: f64 = 0.0;
        for i in 0..12 {
            for j in 0..12 {
                let (a, b) = (7.3 + f64::from(i) * 0.37, 5.1 + f64::from(j) * 0.53);
                let eight = polygon(&[[0.2, 0.1], [0.2 + a, 0.1 + b], [0.2 + a, 0.1], [0.2, 0.1 + b]], true);
                let m = Mask::of(&eight.filled());
                let area: f64 = (0..16)
                    .flat_map(|y| (0..16).map(move |x| (x, y)))
                    .map(|(x, y)| f64::from(crate::mask::fraction(m.level_at(x, y))))
                    .sum();
                worst = worst.max((area - a * b / 2.0).abs());
            }
        }
        assert!(worst < 0.01, "{worst} of a pixel off at worst");
    }

    #[test]
    fn an_outline_s_mask_is_worked_out_along_its_length_within_the_mask_s_reach() {
        // A square ten million pixels across: told a row of tiles at a time (39,000 rows, where
        // its area is 1.5 billion tiles), wholly selected inside.
        let huge = polygon(&[[-5e6, -5e6], [5e6, -5e6], [5e6, 5e6], [-5e6, 5e6]], true);
        let m = Mask::of(&huge.filled());
        assert_eq!(m.extent(), Extent::Within(IRect::new(-5_000_000, -5_000_000, 5_000_000, 5_000_000)));
        assert_eq!(m.level_at(1_234_567, -2_345_678), FULL);
        // A row of a triangle with a long slanting side: the tiles its sides pass through, and
        // the runs between them.
        let f = polygon(&[[0.0, 0.0], [1e6, 0.0], [0.0, 1e6]], true).filled();
        assert_eq!(
            f.row(1000, -10, 4000),
            vec![
                (-10, 0, Class::Outside),
                (0, 1, Class::Edge),
                (1, 2905, Class::Inside),
                (2905, 2906, Class::Edge),
                (2906, 2907, Class::Edge),
                (2907, 4000, Class::Outside)
            ]
        );
    }

    #[test]
    fn an_outline_s_points_and_their_lists_are_admitted_before_they_are_made() {
        use crate::modify::{Bounds, Halt, UNBOUNDED};
        // A thousand sides zigzagging across a million pixels, each listed in 3,900 rows of
        // tiles: some 4 million entries, refused within a megabyte, before any is made.
        let mut zigzag = polygon(&[[0.0, 0.0]], true);
        for i in 1..=1000 {
            zigzag.segments.push(Segment::Line { to: [f64::from(i), if i % 2 == 1 { 1e6 } else { 0.0 }] });
        }
        let megabyte = |n: u64| n <= 1 << 20;
        let refused = zigzag.filled_within(Bounds { admit: &megabyte, ..UNBOUNDED });
        assert_eq!(refused.err(), Some(Halt::NoMemory));
        // What it holds once made is what was counted.
        let f = zigzag.filled();
        assert_eq!(f.bytes(), Filled::need(&zigzag.polygon()));
        assert!(f.bytes() > 32 << 20, "{}", f.bytes());
    }

    #[test]
    fn an_outline_s_curves_are_flattened_to_four_million_points_at_most() {
        // Two thousand curves, each bending enough to be flattened to its most points alone.
        let mut o = polygon(&[[0.0, 0.0]], true);
        for i in 0..2000 {
            let x = f64::from(i) * 10.0;
            o.segments.push(Segment::Curve { c1: [x, 1e7], c2: [x + 5.0, -1e7], to: [x + 10.0, 0.0] });
        }
        let points = o.polygon().len();
        assert!(points <= MOST_FLATTENED + o.segments.len() + 1, "{points} points");
        assert!(points > MOST_FLATTENED / 2, "flattened to too few: {points}");
    }

    #[test]
    fn an_outline_turned_is_the_outline_of_its_points_turned() {
        let mut o = polygon(&[[0.0, 0.0], [100.0, 0.0], [100.0, 50.0]], true);
        o.segments.push(Segment::Curve { c1: [80.0, 80.0], c2: [20.0, 80.0], to: [0.0, 50.0] });
        let m = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let t = o.transformed(&m);
        assert_eq!(t.start, m.apply([0.0, 0.0]));
        assert_eq!(t.segments[1], Segment::Line { to: m.apply([100.0, 50.0]) });
        let curve = Segment::Curve { c1: m.apply([80.0, 80.0]), c2: m.apply([20.0, 80.0]), to: m.apply([0.0, 50.0]) };
        assert_eq!(t.segments[2], curve);
        // Too few points to enclose anything: nothing selected.
        assert!(Mask::of(&polygon(&[[0.0, 0.0], [10.0, 0.0]], true).filled()).is_none());
    }
}
