//! Outlines (design.md, *The outline tool*): closed paths of straight segments and cubic
//! curves, which select what they enclose, each pixel as much of it as they cover. A loop that
//! crosses itself selects each of its lobes, and loops inside one another going the same way
//! select all of them (the nonzero rule).
//!
//! A mask is made of one a tile at a time (`Coverage::tile`): of the edges its curves flatten
//! to, those crossing a tile's rows, each clipped to the tile, add the area they cover along
//! each row's cells, and a running sum along the row gives each pixel's share. A sharp
//! outline selects a pixel or not by its middle, counting the edges crossing its row.

use crate::geom::{Affine, IRect};
use crate::mask::{Coverage, FULL, MaskTile};
use crate::tile::{TILE_PIXELS, TILE_SIZE, TileCoord};
use std::collections::HashMap;

/// How far a curve's flattening may stray from it, in pixels.
const TOLERANCE: f64 = 0.05;

/// The most points a curve is flattened to, however long it is.
const MOST_POINTS: usize = 4096;

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

    /// The polygon it comes to: its curves flattened to within [`TOLERANCE`], closed (the
    /// last point joined to the first).
    pub fn polygon(&self) -> Vec<[f64; 2]> {
        let mut points = vec![self.start];
        let mut at = self.start;
        for s in &self.segments {
            match *s {
                Segment::Line { to } => points.push(to),
                Segment::Curve { c1, c2, to } => {
                    // Enough pieces that each strays at most the tolerance: a cubic's distance from
                    // its chord is at most 3/4 of its control polygon's second differences.
                    let d = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
                        (a[0] - 2.0 * b[0] + c[0]).hypot(a[1] - 2.0 * b[1] + c[1])
                    };
                    let bend = d(at, c1, c2).max(d(c1, c2, to));
                    let n = ((0.75 * bend / TOLERANCE).sqrt().ceil() as usize).clamp(1, MOST_POINTS);
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

    /// What it covers, to make a mask of (`Mask::of`).
    pub fn filled(&self) -> Filled {
        Filled::of(&self.polygon(), self.smooth)
    }
}

/// An edge of a polygon, from `a` to `b`.
#[derive(Clone, Copy, Debug)]
struct Edge {
    a: [f64; 2],
    b: [f64; 2],
}

impl Edge {
    /// The distance from `p` to the edge.
    fn distance(&self, p: [f64; 2]) -> f64 {
        let (dx, dy) = (self.b[0] - self.a[0], self.b[1] - self.a[1]);
        let len2 = dx * dx + dy * dy;
        let t =
            if len2 > 0.0 { (((p[0] - self.a[0]) * dx + (p[1] - self.a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        (p[0] - self.a[0] - t * dx).hypot(p[1] - self.a[1] - t * dy)
    }

    /// How it winds about the point `p` seen along the ray to its right: +1 crossing it
    /// downward, -1 upward, 0 not at all (a point's row counted half-open, so an edge's ends
    /// count once).
    fn winding(&self, p: [f64; 2]) -> i32 {
        let ([x0, y0], [x1, y1]) = (self.a, self.b);
        let down = y0 <= p[1] && p[1] < y1;
        let up = y1 <= p[1] && p[1] < y0;
        if !down && !up {
            return 0;
        }
        let x = x0 + (p[1] - y0) / (y1 - y0) * (x1 - x0);
        if x > p[0] { if down { 1 } else { -1 } } else { 0 }
    }
}

/// A polygon filled by the nonzero rule, as a mask is made of it (see the module's
/// description): its edges, and which cross each row of tiles.
pub struct Filled {
    edges: Vec<Edge>,
    /// The edges crossing each row of tiles (by its y, in tiles), and each tile (to look up
    /// the edges near a point).
    rows: HashMap<i32, Vec<usize>>,
    tiles: HashMap<(i32, i32), Vec<usize>>,
    bounds: IRect,
    smooth: bool,
}

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
        let side = f64::from(TILE_SIZE);
        let tile = |v: f64| (v / side).floor() as i32;
        let mut rows: HashMap<i32, Vec<usize>> = HashMap::new();
        let mut tiles: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (i, e) in edges.iter().enumerate() {
            let (lx, hx) = (e.a[0].min(e.b[0]), e.a[0].max(e.b[0]));
            let (ly, hy) = (e.a[1].min(e.b[1]), e.a[1].max(e.b[1]));
            (x0, y0, x1, y1) = (x0.min(lx), y0.min(ly), x1.max(hx), y1.max(hy));
            for ty in tile(ly)..=tile(hy) {
                rows.entry(ty).or_default().push(i);
                for tx in tile(lx)..=tile(hx) {
                    tiles.entry((tx, ty)).or_default().push(i);
                }
            }
        }
        let bounds = if edges.is_empty() {
            IRect::new(0, 0, 0, 0)
        } else {
            IRect::new(x0.floor() as i64, y0.floor() as i64, x1.ceil() as i64, y1.ceil() as i64)
        };
        Filled { edges, rows, tiles, bounds, smooth }
    }

    /// The winding number of the point `p`: how many times the outline goes round it.
    fn winding(&self, p: [f64; 2]) -> i32 {
        let ty = (p[1] / f64::from(TILE_SIZE)).floor() as i32;
        self.rows.get(&ty).map_or(0, |row| row.iter().map(|&i| self.edges[i].winding(p)).sum())
    }

    /// The levels of tile `c`, smooth: each pixel as much as the polygon covers it.
    fn smooth_tile(&self, c: TileCoord) -> Vec<u16> {
        let r = c.rect(0);
        let (ox, oy) = (r.x0 as f64, r.y0 as f64);
        let (w, h) = (TILE_SIZE as usize, TILE_SIZE as usize);
        // Each row's cells, and one past them for what reaches the tile's right side.
        let stride = w + 2;
        let mut acc = vec![0.0f32; stride * h];
        for &i in self.rows.get(&c.y).into_iter().flatten() {
            let e = &self.edges[i];
            let a = [e.a[0] - ox, e.a[1] - oy];
            let b = [e.b[0] - ox, e.b[1] - oy];
            if a[0].min(b[0]) >= w as f64 {
                continue;
            }
            for (p, q) in split_at_sides(a, b, w as f64) {
                draw(&mut acc, stride, h, p, q);
            }
        }
        let mut levels = vec![0u16; TILE_PIXELS];
        for y in 0..h {
            let mut sum = 0.0f32;
            for x in 0..w {
                sum += acc[y * stride + x];
                levels[y * w + x] = (sum.abs().min(1.0) * f32::from(FULL)).round() as u16;
            }
        }
        levels
    }

    /// The levels of tile `c`, sharp: each pixel selected or not by whether its middle is.
    fn sharp_tile(&self, c: TileCoord) -> Vec<u16> {
        let r = c.rect(0);
        let w = TILE_SIZE as usize;
        let mut levels = vec![0u16; TILE_PIXELS];
        let edges: Vec<&Edge> = self.rows.get(&c.y).into_iter().flatten().map(|&i| &self.edges[i]).collect();
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
        }
        levels
    }
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

    fn distance(&self, x: f64, y: f64) -> f64 {
        // The edges in the tile the point is in and those round it: any farther is a tile's side
        // away at least, which a nearer one leaves aside.
        let side = f64::from(TILE_SIZE);
        let (tx, ty) = ((x / side).floor() as i32, (y / side).floor() as i32);
        let mut near = side;
        for dy in -1..=1 {
            for dx in -1..=1 {
                for &i in self.tiles.get(&(tx + dx, ty + dy)).into_iter().flatten() {
                    near = near.min(self.edges[i].distance([x, y]));
                }
            }
        }
        if self.winding([x, y]) != 0 { -near } else { near }
    }

    fn at(&self, x: f64, y: f64) -> f32 {
        // A pixel alone, as its tile would have it.
        let c = TileCoord::containing(x.floor() as i64, y.floor() as i64);
        let r = c.rect(0);
        let levels = self.tile(c).map(|t| t.get((x.floor() as i64 - r.x0) as u32, (y.floor() as i64 - r.y0) as u32));
        crate::mask::fraction(levels.unwrap_or(0))
    }

    fn tile(&self, c: TileCoord) -> Option<MaskTile> {
        let levels = if self.smooth { self.smooth_tile(c) } else { self.sharp_tile(c) };
        Some(MaskTile::from_levels(levels))
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
