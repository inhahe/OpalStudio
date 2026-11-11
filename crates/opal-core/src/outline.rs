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
use crate::symmetry::Symmetry;
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
    /// How its line goes through each of its points (`points`), in their order; told from
    /// their handles where it keeps none (an outline made of points alone, or one from before
    /// they were kept).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joins: Vec<Join>,
    /// Its symmetry, if it has one: then `start` and `segments` are one slice of it, not
    /// closed, and the outline is the slice with its copies (see [`whole`](Self::whole)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symmetry: Option<Symmetry>,
}

impl Outline {
    /// The outline `m` carries this one to.
    pub fn transformed(&self, m: &Affine) -> Outline {
        // A symmetry a map does not keep (a stretch one way more than the other): the whole
        // outline carried, as an ordinary one.
        let symmetry = match self.symmetric() {
            Some(s) => match s.transformed(m) {
                Some(s) => Some(s),
                None => return self.whole().transformed(m),
            },
            None => None,
        };
        // (A map keeps lines straight and the ratios along them: a smooth join stays smooth,
        // a symmetric one symmetric.)
        Outline {
            start: m.apply(self.start),
            segments: self.segments.iter().map(|s| s.transformed(m)).collect(),
            smooth: self.smooth,
            joins: self.joins.clone(),
            symmetry,
        }
    }

    /// The outline it is, whole: itself, or, with a symmetry, its slice and the slice's copies.
    /// An open slice's copies come one after another, each joined to the next by a straight
    /// side where it does not end where the next begins (a slice drawn from seam to seam joins
    /// exactly); with a mirror, the slice and its mirror image run back are joined first, at
    /// whichever end of the slice is nearer its own image (the end drawn on the axis); the copies
    /// are turned the way round that puts the next one's start nearest where the last ends. A
    /// closed slice's copies stay apart: each is reached from the first and left back along the
    /// same straight side, which, gone both ways, encloses nothing.
    pub fn whole(&self) -> std::borrow::Cow<'_, Outline> {
        let Some(s) = self.symmetric() else { return std::borrow::Cow::Borrowed(self) };
        let end = self.segments.last().map_or(self.start, Segment::end);
        let closed = self.segments.len() > 1 && touching(end, self.start);
        let m = s.reflection();
        let mirrored = || {
            let image: Vec<Segment> = self.segments.iter().map(|seg| seg.transformed(&m)).collect();
            reversed(m.apply(self.start), &image)
        };
        let slice = (self.start, self.segments.clone());
        // What each turn makes a copy of: the slice, and its mirror image run back.
        let unit: Vec<Path> = match (s.mirror, closed) {
            (false, _) => vec![slice],
            (true, true) => vec![slice, mirrored()],
            (true, false) => {
                let image = mirrored();
                vec![if far(m.apply(self.start), self.start) <= far(m.apply(end), end) {
                    chain(&[image, slice])
                } else {
                    chain(&[slice, image])
                }]
            }
        };
        let order = i64::from(s.order.max(1));
        let (first, last) = (unit[0].0, unit.last().map_or(unit[0].0, |p| p.1.last().map_or(p.0, Segment::end)));
        let way =
            if order > 2 && far(s.turn(-1).apply(first), last) < far(s.turn(1).apply(first), last) { -1 } else { 1 };
        let copies: Vec<Path> = (0..order)
            .flat_map(|k| {
                let t = s.turn(way * k);
                unit.iter().map(move |(start, segments)| {
                    (t.apply(*start), segments.iter().map(|seg| seg.transformed(&t)).collect())
                })
            })
            .collect();
        let (start, segments) = if closed { bridged(&copies) } else { chain(&copies) };
        std::borrow::Cow::Owned(Outline { start, segments, smooth: self.smooth, joins: Vec::new(), symmetry: None })
    }

    /// The polygon it comes to, whole (see [`whole`](Self::whole) and [`flattened`]), closed
    /// (the last point joined to the first).
    pub fn polygon(&self) -> Vec<[f64; 2]> {
        let whole = self.whole();
        flattened(whole.start, &whole.segments)
    }

    /// What it covers, to make a mask of (`Mask::of`).
    pub fn filled(&self) -> Filled {
        self.filled_within(UNBOUNDED).expect("unbounded")
    }

    /// [`filled`](Self::filled), unless `bounds` hold it back: the points its curves are
    /// flattened to, then its edges and their lists by row of tiles, admitted before they are
    /// made. What it then holds is [`Filled::bytes`], to give back once its mask is made.
    pub fn filled_within(&self, bounds: Bounds) -> Result<Filled, Halt> {
        let whole = self.whole();
        let points = (flattened_len(whole.start, &whole.segments) * std::mem::size_of::<[f64; 2]>()) as u64;
        if !(bounds.admit)(points) {
            return Err(Halt::NoMemory);
        }
        let polygon = flattened(whole.start, &whole.segments);
        let made = if (bounds.admit)(Filled::need(&polygon)) {
            Ok(Filled::of(&polygon, self.smooth))
        } else {
            Err(Halt::NoMemory)
        };
        (bounds.give_back)(points);
        made
    }
}

/// How an outline's line goes through one of its points: what moving one of the point's
/// handles does to the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    /// A corner: each handle moves alone, if it has any.
    #[default]
    Sharp,
    /// Smooth: its handles stay in line, each as long as it is.
    Smooth,
    /// Smooth, its handles as long as each other.
    Symmetric,
}

/// One of an outline's points as it goes round: where it is, its handles into it and out of
/// it (`None` where that side is straight), and how its line goes through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub at: [f64; 2],
    pub into: Option<[f64; 2]>,
    pub out: Option<[f64; 2]>,
    pub join: Join,
}

/// Which of a point's handles: the one the side into it bends by, or the one the side out of
/// it bends by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    In,
    Out,
}

/// A side of an outline as it goes round: straight, or a curve pulled toward its handles.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Line,
    Curve { c1: [f64; 2], c2: [f64; 2] },
}

impl Side {
    /// The segment it is, ending at `to`.
    fn to(self, to: [f64; 2]) -> Segment {
        match self {
            Side::Line => Segment::Line { to },
            Side::Curve { c1, c2 } => Segment::Curve { c1, c2, to },
        }
    }
}

/// An outline as its points go round (see [`Outline::points`]): each point, the side from it
/// to the next (the last's back to the first), and how the line goes through it. Every edit
/// of an outline is made on one.
#[derive(Clone, Debug)]
struct Ring {
    points: Vec<[f64; 2]>,
    sides: Vec<Side>,
    joins: Vec<Join>,
    smooth: bool,
    /// A symmetric outline's symmetry: then the points are its slice's, not closed (no side
    /// from the last back to the first).
    symmetry: Option<Symmetry>,
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn mul(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

fn len(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    add(a, mul(sub(b, a), t))
}

/// `a` made a unit's length, if it has any length to speak of.
fn unit(a: [f64; 2]) -> Option<[f64; 2]> {
    let l = len(a);
    (l > 1e-12).then(|| mul(a, 1.0 / l))
}

/// The cubic curve from `a` toward `c1` and `c2` to `b`, at `t`.
fn cubic(a: [f64; 2], c1: [f64; 2], c2: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    let u = 1.0 - t;
    let (wa, w1, w2, wb) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [wa * a[0] + w1 * c1[0] + w2 * c2[0] + wb * b[0], wa * a[1] + w1 * c1[1] + w2 * c2[1] + wb * b[1]]
}

impl Ring {
    fn of(o: &Outline) -> Ring {
        let mut points = vec![o.start];
        let mut sides = Vec::with_capacity(o.segments.len() + 1);
        for s in &o.segments {
            sides.push(match *s {
                Segment::Line { .. } => Side::Line,
                Segment::Curve { c1, c2, .. } => Side::Curve { c1, c2 },
            });
            points.push(s.end());
        }
        // Closed by a side of its own back to its start: that point is its first, once. (A
        // symmetric outline's slice is not closed.)
        if o.symmetric().is_none() {
            if points.len() > 1 && points.last() == Some(&o.start) {
                points.pop();
            } else {
                sides.push(Side::Line);
            }
        }
        let mut ring = Ring { points, sides, joins: Vec::new(), smooth: o.smooth, symmetry: o.symmetric() };
        ring.joins = if o.joins.len() == ring.points.len() {
            o.joins.clone()
        } else {
            (0..ring.points.len()).map(|i| ring.told_join(i)).collect()
        };
        ring
    }

    fn outline(self) -> Outline {
        let n = self.points.len();
        let mut segments: Vec<Segment> = (1..n).map(|i| self.sides[i - 1].to(self.points[i])).collect();
        // The side back to the first point is kept only if it is a curve (a straight one is
        // implied), or if the last point lies on the first: its last segment would then seem
        // the side back, and the point be lost. (A slice has none.)
        if self.closed()
            && (matches!(self.sides[n - 1], Side::Curve { .. }) || (n > 1 && self.points[n - 1] == self.points[0]))
        {
            segments.push(self.sides[n - 1].to(self.points[0]));
        }
        Outline { start: self.points[0], segments, smooth: self.smooth, joins: self.joins, symmetry: self.symmetry }
    }

    /// Whether it goes round, from its last point back to its first (not a symmetric
    /// outline's slice).
    fn closed(&self) -> bool {
        self.symmetry.is_none()
    }

    /// The point after point `i`, round to the first after the last.
    fn next(&self, i: usize) -> usize {
        (i + 1) % self.points.len()
    }

    /// The side into point `i` (from the point before it), if it has one: not the first of a
    /// slice.
    fn side_into(&self, i: usize) -> Option<usize> {
        let n = self.points.len();
        if self.closed() { Some((i + n - 1) % n) } else { i.checked_sub(1) }
    }

    /// The side out of point `i` (to the point after it), if it has one: not the last of a
    /// slice.
    fn side_out(&self, i: usize) -> Option<usize> {
        (i < self.sides.len()).then_some(i)
    }

    /// The point before point `i` and the point after it, each itself where it has none (at
    /// a slice's ends).
    fn neighbors(&self, i: usize) -> [[f64; 2]; 2] {
        let before = self.side_into(i).map_or(self.points[i], |s| self.points[s]);
        let after = self.side_out(i).map_or(self.points[i], |s| self.points[self.next(s)]);
        [before, after]
    }

    /// Point `i`'s handles: into it (the side before it bends by it) and out of it (the side
    /// after), each `None` where that side is straight, or there is none.
    fn handles(&self, i: usize) -> [Option<[f64; 2]>; 2] {
        let into = self.side_into(i).and_then(|s| match self.sides[s] {
            Side::Curve { c2, .. } => Some(c2),
            Side::Line => None,
        });
        let out = self.side_out(i).and_then(|s| match self.sides[s] {
            Side::Curve { c1, .. } => Some(c1),
            Side::Line => None,
        });
        [into, out]
    }

    /// Puts point `i`'s handle at `end` at `to`, a straight side becoming a curve (its other
    /// end's handle at that end); nothing, where it has no side there (a slice's end).
    fn set_handle(&mut self, i: usize, end: End, to: [f64; 2]) {
        match end {
            End::In => {
                let Some(p) = self.side_into(i) else { return };
                self.sides[p] = match self.sides[p] {
                    Side::Curve { c1, .. } => Side::Curve { c1, c2: to },
                    Side::Line => Side::Curve { c1: self.points[p], c2: to },
                };
            }
            End::Out => {
                let Some(s) = self.side_out(i) else { return };
                let at = self.points[self.next(s)];
                self.sides[s] = match self.sides[s] {
                    Side::Curve { c2, .. } => Side::Curve { c1: to, c2 },
                    Side::Line => Side::Curve { c1: to, c2: at },
                };
            }
        }
    }

    /// The join point `i`'s handles show: smooth where it has both and they are in line
    /// (symmetric, if as long as each other), else sharp. (What an outline kept no joins for
    /// is taken to have.)
    fn told_join(&self, i: usize) -> Join {
        let p = self.points[i];
        let [Some(a), Some(b)] = self.handles(i) else { return Join::Sharp };
        let (u, v) = (sub(a, p), sub(b, p));
        let (lu, lv) = (len(u), len(v));
        if lu < 1e-9 || lv < 1e-9 {
            return Join::Sharp;
        }
        // In line: pointing opposite ways, within a hair.
        let cross = (u[0] * v[1] - u[1] * v[0]) / (lu * lv);
        let dot = (u[0] * v[0] + u[1] * v[1]) / (lu * lv);
        if cross.abs() > 1e-6 || dot > 0.0 {
            Join::Sharp
        } else if (lu - lv).abs() <= 1e-6 * lu.max(lv) {
            Join::Symmetric
        } else {
            Join::Smooth
        }
    }

    /// Point `i`'s other handle, once its handle at `end` has moved, put as its join says: in
    /// line with it, as long as it was (smooth) or as the one moved (symmetric).
    fn follow(&mut self, i: usize, end: End) {
        let p = self.points[i];
        let [into, out] = self.handles(i);
        let (moved, other, other_end) = match end {
            End::In => (into, out, End::Out),
            End::Out => (out, into, End::In),
        };
        let (Some(moved), Some(other)) = (moved, other) else { return };
        let Some(away) = unit(sub(p, moved)) else { return };
        let length = match self.joins[i] {
            Join::Sharp => return,
            Join::Smooth => len(sub(other, p)),
            Join::Symmetric => len(sub(moved, p)),
        };
        self.set_handle(i, other_end, add(p, mul(away, length)));
    }

    /// Which way the handle at point `i` goes (pointing into the slice) for the whole outline
    /// to pass smoothly through it, if it is an end of a symmetric outline's slice: at right
    /// angles to the mirror line it lies on, or, turned without a mirror, in line with the
    /// handle of the copy it meets there (the next copy's first, for its last point; the one
    /// before's last, for its first).
    fn seam(&self, i: usize) -> Option<[f64; 2]> {
        let s = self.symmetry?;
        let n = self.points.len();
        if n < 2 || (i != 0 && i != n - 1) {
            return None;
        }
        let p = self.points[i];
        let inward = sub(self.points[if i == 0 { 1 } else { n - 2 }], p);
        let dir = if s.mirror {
            // The mirror line nearest it: through the center, every half step round.
            let rel = sub(p, s.center);
            let off = |u: &[f64; 2]| (rel[0] * u[1] - rel[1] * u[0]).abs();
            let lines = (0..s.copies()).map(|k| {
                let a = (s.angle + 360.0 * k as f64 / s.copies() as f64).to_radians();
                [a.sin(), -a.cos()]
            });
            let line = lines.min_by(|u, v| off(u).total_cmp(&off(v)))?;
            [-line[1], line[0]]
        } else {
            // The copies turned as the whole outline turns them (see `Outline::whole`).
            let (first, last) = (self.points[0], self.points[n - 1]);
            let way = if s.order > 2 && far(s.turn(-1).apply(first), last) < far(s.turn(1).apply(first), last) {
                -1
            } else {
                1
            };
            let (t, from, h) = if i == n - 1 {
                (s.turn(way), first, self.handles(0)[1].unwrap_or(self.points[1]))
            } else {
                (s.turn(-way), last, self.handles(n - 1)[0].unwrap_or(self.points[n - 2]))
            };
            sub(t.apply(from), t.apply(h))
        };
        let dir = unit(dir)?;
        Some(if dir[0] * inward[0] + dir[1] * inward[1] < 0.0 { mul(dir, -1.0) } else { dir })
    }

    /// The point `t` (0 to 1) along the side from point `i`.
    fn along(&self, i: usize, t: f64) -> [f64; 2] {
        let (a, b) = (self.points[i], self.points[self.next(i)]);
        match self.sides[i] {
            Side::Line => lerp(a, b, t),
            Side::Curve { c1, c2 } => cubic(a, c1, c2, b, t),
        }
    }

    /// How far along the side from point `i` (0 to 1) its point nearest `p` is.
    fn nearest_on(&self, i: usize, p: [f64; 2]) -> f64 {
        let (a, b) = (self.points[i], self.points[self.next(i)]);
        match self.sides[i] {
            Side::Line => {
                let ab = sub(b, a);
                let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                if l2 > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 }
            }
            Side::Curve { .. } => {
                // Sampled, then narrowed about the nearest sample.
                const SAMPLES: usize = 32;
                let far = |t: f64| len(sub(self.along(i, t), p));
                let best = (0..=SAMPLES).map(|k| k as f64 / SAMPLES as f64).min_by(|s, t| far(*s).total_cmp(&far(*t)));
                let best = best.expect("samples");
                let (mut lo, mut hi) = ((best - 1.0 / SAMPLES as f64).max(0.0), (best + 1.0 / SAMPLES as f64).min(1.0));
                for _ in 0..40 {
                    let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
                    if far(m1) < far(m2) {
                        hi = m2;
                    } else {
                        lo = m1;
                    }
                }
                (lo + hi) / 2.0
            }
        }
    }
}

impl Outline {
    /// An outline from `start` along `segments` and back, sharp if not `smooth`, the joins at
    /// its points told from their handles.
    pub fn new(start: [f64; 2], segments: Vec<Segment>, smooth: bool) -> Outline {
        Outline { start, segments, smooth, joins: Vec::new(), symmetry: None }
    }

    /// Its symmetry, if it has one that makes copies: then it is a slice (one making none is
    /// as none).
    pub fn symmetric(&self) -> Option<Symmetry> {
        self.symmetry.filter(Symmetry::is_on)
    }

    /// The fewest points it may have: three, or, as a symmetric outline's slice, two.
    pub fn fewest_points(&self) -> usize {
        if self.symmetric().is_some() { 2 } else { 3 }
    }

    /// Why it could not have been made in the editor, if it could not (as one read from a file
    /// or given by a script may be): a place not a number; a symmetry the editor does not make
    /// (of no order it makes, or making no copies); or a whole outline of more segments than
    /// an outline may have ([`MOST_POINTS`](crate::selection::MOST_POINTS), the sides joining
    /// its copies counted). What the editor makes keeps within it.
    pub fn refused(&self) -> Option<&'static str> {
        if !self.coordinates().flatten().all(f64::is_finite) {
            return Some("an outline is not a number");
        }
        if let Some(s) = self.symmetry {
            if let Some(why) = s.refused() {
                return Some(why);
            }
            if !s.is_on() {
                return Some("an outline's symmetry makes no copies");
            }
        }
        (self.whole_size(self.segments.len()) >= crate::selection::MOST_POINTS)
            .then_some("an outline has more points than the editor makes")
    }

    /// How many segments its whole outline has at most with a slice of `segments`: each copy
    /// its own, and a side joining it to the next.
    pub fn whole_size(&self, segments: usize) -> usize {
        let copies = self.symmetric().map_or(1, |s| s.copies());
        if copies == 1 { segments } else { (segments + 1).saturating_mul(copies) }
    }

    /// Its points as they go round, the first its start (once, if its last segment comes back
    /// to it): what the indices its edits take count.
    pub fn points(&self) -> Vec<[f64; 2]> {
        Ring::of(self).points
    }

    /// How many points it has (as [`points`](Self::points) counts them), worked out at once.
    pub fn point_count(&self) -> usize {
        // (A symmetric outline's slice is not closed: its points are its start and its ends.)
        let closing = self.symmetric().is_none() && self.segments.last().is_some_and(|s| s.end() == self.start);
        self.segments.len() + 1 - usize::from(closing)
    }

    /// Point `i`'s handles: into it and out of it, each `None` where that side is straight.
    pub fn handles(&self, i: usize) -> [Option<[f64; 2]>; 2] {
        Ring::of(self).handles(i)
    }

    /// How its line goes through point `i`.
    pub fn join(&self, i: usize) -> Join {
        Ring::of(self).joins[i]
    }

    /// Its points as they go round (as [`points`](Self::points) counts them), each with its
    /// handles and its join: what drawing it and pressing on it read, worked out at once.
    pub fn nodes(&self) -> Vec<Node> {
        let ring = Ring::of(self);
        (0..ring.points.len())
            .map(|i| {
                let [into, out] = ring.handles(i);
                Node { at: ring.points[i], into, out, join: ring.joins[i] }
            })
            .collect()
    }

    /// Every place it names: its start, and each segment's end and handles.
    pub fn coordinates(&self) -> impl Iterator<Item = [f64; 2]> + '_ {
        std::iter::once(self.start).chain(self.segments.iter().flat_map(|s| match *s {
            Segment::Line { to } => vec![to],
            Segment::Curve { c1, c2, to } => vec![c1, c2, to],
        }))
    }

    /// The point `t` (0 to 1) along its side from point `i` to the next.
    pub fn along(&self, i: usize, t: f64) -> [f64; 2] {
        Ring::of(self).along(i, t)
    }

    /// Its side nearest `p`, how far along it (0 to 1) the nearest point is, and how far that
    /// point is from `p`; none, if it has no side (a slice of one point).
    pub fn nearest(&self, p: [f64; 2]) -> Option<(usize, f64, f64)> {
        let ring = Ring::of(self);
        (0..ring.sides.len())
            .map(|i| {
                let t = ring.nearest_on(i, p);
                (i, t, len(sub(ring.along(i, t), p)))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// The outline with points `which` moved by `by`, their handles with them.
    pub fn moved(&self, which: &[usize], by: [f64; 2]) -> Outline {
        let mut ring = Ring::of(self);
        let mut which = which.to_vec();
        which.sort_unstable();
        which.dedup();
        for &i in &which {
            let [into, out] = ring.handles(i);
            ring.points[i] = add(ring.points[i], by);
            if let Some(h) = into {
                ring.set_handle(i, End::In, add(h, by));
            }
            if let Some(h) = out {
                ring.set_handle(i, End::Out, add(h, by));
            }
        }
        ring.outline()
    }

    /// The outline with point `i`'s handle at `end` put at `to`, the other following as its
    /// join says; or, if `broken`, the point made sharp first, so that it moves alone.
    pub fn with_handle(&self, i: usize, end: End, to: [f64; 2], broken: bool) -> Outline {
        let mut ring = Ring::of(self);
        if broken {
            ring.joins[i] = Join::Sharp;
        }
        ring.set_handle(i, end, to);
        ring.follow(i, end);
        ring.outline()
    }

    /// The outline with a point added `t` (0 to 1) along the side from point `i`, which it
    /// splits exactly: a curve into two (the point smooth), a straight side into two (sharp).
    /// The point added is point `i + 1`.
    pub fn split(&self, i: usize, t: f64) -> Outline {
        let mut ring = Ring::of(self);
        let (a, b) = (ring.points[i], ring.points[ring.next(i)]);
        let (q, first, second, join) = match ring.sides[i] {
            Side::Line => (lerp(a, b, t), Side::Line, Side::Line, Join::Sharp),
            Side::Curve { c1, c2 } => {
                let (e1, e2, e3) = (lerp(a, c1, t), lerp(c1, c2, t), lerp(c2, b, t));
                let (f1, f2) = (lerp(e1, e2, t), lerp(e2, e3, t));
                let q = lerp(f1, f2, t);
                (q, Side::Curve { c1: e1, c2: f1 }, Side::Curve { c1: f2, c2: e3 }, Join::Smooth)
            }
        };
        ring.sides[i] = first;
        ring.points.insert(i + 1, q);
        ring.sides.insert(i + 1, second);
        ring.joins.insert(i + 1, join);
        ring.outline()
    }

    /// The outline without points `which`, the sides either side of each made one (a curve, if
    /// either was; at a slice's end, its one side going with it); `None` if fewer than three
    /// points would be left (two, of a slice).
    pub fn without(&self, which: &[usize]) -> Option<Outline> {
        let mut ring = Ring::of(self);
        let mut which = which.to_vec();
        which.sort_unstable();
        which.dedup();
        let least = if ring.closed() { 3 } else { 2 };
        if ring.points.len() < which.len() + least {
            return None;
        }
        for &i in which.iter().rev() {
            let gone = match (ring.side_into(i), ring.side_out(i)) {
                (Some(p), Some(o)) => {
                    let (from, to) = (ring.points[p], ring.points[ring.next(o)]);
                    ring.sides[p] = match (ring.sides[p], ring.sides[o]) {
                        (Side::Line, Side::Line) => Side::Line,
                        (into, out) => Side::Curve {
                            c1: if let Side::Curve { c1, .. } = into { c1 } else { from },
                            c2: if let Side::Curve { c2, .. } = out { c2 } else { to },
                        },
                    };
                    o
                }
                (None, Some(o)) => o,
                (Some(p), None) => p,
                (None, None) => unreachable!("two points are left at least"),
            };
            ring.points.remove(i);
            ring.sides.remove(gone);
            ring.joins.remove(i);
        }
        Some(ring.outline())
    }

    /// The outline with the line through points `which` joined as `join` says: sharp, its
    /// handles taken back into the point (a side left with none straight again); smooth or
    /// symmetric, its handles put in line (pulled out along the line between its neighbors,
    /// a third of the way to each, where it had none).
    pub fn joined(&self, which: &[usize], join: Join) -> Outline {
        let mut ring = Ring::of(self);
        for &i in which {
            let (p, [a, b]) = (ring.points[i], ring.neighbors(i));
            let [into, out] = ring.handles(i);
            match join {
                Join::Sharp => {
                    for (h, end) in [(into, End::In), (out, End::Out)] {
                        if h.is_some() {
                            ring.set_handle(i, end, p);
                        }
                    }
                    for s in [ring.side_into(i), ring.side_out(i)].into_iter().flatten() {
                        let (from, to) = (ring.points[s], ring.points[ring.next(s)]);
                        if ring.sides[s] == (Side::Curve { c1: from, c2: to }) {
                            ring.sides[s] = Side::Line;
                        }
                    }
                }
                Join::Smooth | Join::Symmetric => {
                    let across = unit(sub(b, a)).unwrap_or([1.0, 0.0]);
                    let u_out = out.and_then(|h| unit(sub(h, p))).unwrap_or(across);
                    let u_in = into.and_then(|h| unit(sub(p, h))).unwrap_or(across);
                    // At a slice's end, the way that passes smoothly through its seam (the
                    // handle there pointing into the slice: out of its first point, back
                    // into its last).
                    let dir = match ring.seam(i) {
                        Some(into_slice) if i == 0 => into_slice,
                        Some(into_slice) => mul(into_slice, -1.0),
                        None => unit(add(u_out, u_in)).unwrap_or(across),
                    };
                    let held =
                        |h: Option<[f64; 2]>, or: f64| h.map(|h| len(sub(h, p))).filter(|&l| l > 1e-9).unwrap_or(or);
                    let (l_in, l_out) = (held(into, len(sub(p, a)) / 3.0), held(out, len(sub(b, p)) / 3.0));
                    let (l_in, l_out) = if join == Join::Symmetric {
                        ((l_in + l_out) / 2.0, (l_in + l_out) / 2.0)
                    } else {
                        (l_in, l_out)
                    };
                    ring.set_handle(i, End::In, sub(p, mul(dir, l_in)));
                    ring.set_handle(i, End::Out, add(p, mul(dir, l_out)));
                }
            }
            ring.joins[i] = join;
        }
        ring.outline()
    }

    /// The outline with its side from point `i`, held `t` (0 to 1) along, moved by `by`: a
    /// curve bent so that its point there moves so (its ends staying, their other handles
    /// following as their joins say), a straight side moved with its two ends.
    pub fn bent(&self, i: usize, t: f64, by: [f64; 2]) -> Outline {
        let ring = Ring::of(self);
        let Side::Curve { c1, c2 } = ring.sides[i] else { return self.moved(&[i, ring.next(i)], by) };
        let mut ring = ring;
        // The handles share the move as much as each pulls the point there (near an end, where
        // they pull it hardly at all, as near it as they still do).
        let t = t.clamp(0.05, 0.95);
        let u = 1.0 - t;
        let k = 3.0 * u * t * (u * u + t * t);
        ring.sides[i] = Side::Curve { c1: add(c1, mul(by, u / k)), c2: add(c2, mul(by, t / k)) };
        let next = ring.next(i);
        ring.follow(i, End::Out);
        ring.follow(next, End::In);
        ring.outline()
    }
}

/// A line: where it begins, and its segments.
type Path = ([f64; 2], Vec<Segment>);

/// How far apart `a` and `b` are.
fn far(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Whether `a` and `b` are one place, within a millionth of a pixel (what turning a point
/// round and back leaves).
fn touching(a: [f64; 2], b: [f64; 2]) -> bool {
    far(a, b) <= 1e-6
}

/// `segments`, their last end put at `at` exactly.
fn ending_at(mut segments: Vec<Segment>, at: [f64; 2]) -> Vec<Segment> {
    if let Some(Segment::Line { to } | Segment::Curve { to, .. }) = segments.last_mut() {
        *to = at;
    }
    segments
}

/// The lines `paths` one after another, each joined to the next by a straight side where it
/// does not end where the next begins (touching), and, if the last ends where the first
/// begins, ending there exactly: as an outline goes round.
fn chain(paths: &[Path]) -> Path {
    let start = paths[0].0;
    let mut segments: Vec<Segment> = Vec::new();
    let mut at = start;
    for (i, (from, segs)) in paths.iter().enumerate() {
        if i > 0 && !touching(at, *from) {
            segments.push(Segment::Line { to: *from });
        }
        segments.extend(segs.iter().copied());
        at = segments.last().map_or(at, Segment::end);
    }
    if segments.len() > 1 && touching(at, start) {
        segments = ending_at(segments, start);
    }
    (start, segments)
}

/// The closed lines `loops` as one outline: the first, then each other reached from its
/// start and left back to it along one straight side, which, gone both ways, encloses nothing.
fn bridged(loops: &[Path]) -> Path {
    let start = loops[0].0;
    let mut segments = ending_at(loops[0].1.clone(), start);
    for (from, segs) in &loops[1..] {
        segments.push(Segment::Line { to: *from });
        segments.extend(ending_at(segs.clone(), *from));
        segments.push(Segment::Line { to: start });
    }
    (start, segments)
}

/// The line from `start` along `segments`, run back: where it begins, and its segments.
fn reversed(start: [f64; 2], segments: &[Segment]) -> ([f64; 2], Vec<Segment>) {
    let ends: Vec<[f64; 2]> = std::iter::once(start).chain(segments.iter().map(Segment::end)).collect();
    let back = segments
        .iter()
        .enumerate()
        .rev()
        .map(|(i, s)| match *s {
            Segment::Line { .. } => Segment::Line { to: ends[i] },
            Segment::Curve { c1, c2, .. } => Segment::Curve { c1: c2, c2: c1, to: ends[i] },
        })
        .collect();
    (*ends.last().expect("its start at least"), back)
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
                    points.push(cubic(at, c1, c2, to, i as f64 / n as f64));
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
        Outline::new(points[0], points[1..].iter().map(|&to| Segment::Line { to }).collect(), smooth)
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
        let m = Mask::of(&Outline::new(start, segments, true).filled());
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

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    /// A square 100 across, its top side a curve bulging upward.
    fn bulging() -> Outline {
        Outline::new(
            [0.0, 0.0],
            vec![
                Segment::Curve { c1: [30.0, -20.0], c2: [70.0, -20.0], to: [100.0, 0.0] },
                Segment::Line { to: [100.0, 100.0] },
                Segment::Line { to: [0.0, 100.0] },
            ],
            true,
        )
    }

    #[test]
    fn an_outline_s_points_handles_and_joins_are_read_as_they_go_round() {
        let o = bulging();
        assert_eq!(o.points(), vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]]);
        assert_eq!(o.handles(0), [None, Some([30.0, -20.0])]);
        assert_eq!(o.handles(1), [Some([70.0, -20.0]), None]);
        assert_eq!(o.join(0), Join::Sharp);
        // Closed by a side of its own back to its start: the start once.
        let mut closed = o.clone();
        closed.segments.push(Segment::Line { to: [0.0, 0.0] });
        assert_eq!(closed.points().len(), 4);
        // Handles in line: smooth; as long as each other, symmetric; else sharp.
        let through = |c2: [f64; 2], c1: [f64; 2]| {
            Outline::new(
                [0.0, 0.0],
                vec![
                    Segment::Curve { c1: [10.0, 0.0], c2, to: [50.0, 0.0] },
                    Segment::Curve { c1, c2: [90.0, 10.0], to: [100.0, 50.0] },
                ],
                true,
            )
            .join(1)
        };
        assert_eq!(through([40.0, -10.0], [70.0, 20.0]), Join::Smooth);
        assert_eq!(through([40.0, -10.0], [60.0, 10.0]), Join::Symmetric);
        assert_eq!(through([40.0, -10.0], [60.0, -10.0]), Join::Sharp);
        // Joins kept go with it, through a file's words and a transform: a corner whose handles
        // happen to lie in line stays a corner (told from its handles, it would be smooth).
        let mut kept = o.joined(&[0], Join::Smooth);
        kept.joins[0] = Join::Sharp;
        let read: Outline = serde_json::from_str(&serde_json::to_string(&kept).unwrap()).unwrap();
        assert_eq!(read, kept);
        assert_eq!(kept.transformed(&Affine::translate(5.0, 5.0)).join(0), Join::Sharp);
    }

    #[test]
    fn points_are_moved_split_off_and_taken_away_exactly() {
        let o = bulging();
        // Moved, their handles with them.
        let m = o.moved(&[1, 2], [10.0, 5.0]);
        assert_eq!(m.points(), vec![[0.0, 0.0], [110.0, 5.0], [110.0, 105.0], [0.0, 100.0]]);
        assert_eq!(m.handles(1)[0], Some([80.0, -15.0]));
        // A curve split at a third: the same curve, in two, through a smooth point.
        let t = 1.0 / 3.0;
        let split = o.split(0, t);
        assert_eq!(split.points().len(), 5);
        assert!(close(split.points()[1], o.along(0, t)));
        assert_eq!(split.join(1), Join::Smooth);
        for k in 0..=10 {
            let s = f64::from(k) / 10.0;
            assert!(close(split.along(0, s), o.along(0, s * t)), "first part at {s}");
            assert!(close(split.along(1, s), o.along(0, t + s * (1.0 - t))), "second part at {s}");
        }
        // A straight side split: two straight sides, a sharp point between.
        let line = o.split(2, 0.5);
        assert_eq!(line.points()[3], [50.0, 100.0]);
        assert_eq!((line.join(3), line.handles(3)), (Join::Sharp, [None, None]));
        // The last point moved onto the first: still a point of its own, as many as before.
        let onto = o.moved(&[3], [0.0, -100.0]);
        assert_eq!((onto.points().len(), onto.point_count()), (4, 4));
        assert_eq!(onto.points()[3], onto.points()[0]);
        let mut joined = onto.joined(&[3], Join::Smooth);
        joined = joined.moved(&[2], [1.0, 1.0]);
        assert_eq!((joined.point_count(), joined.join(3)), (4, Join::Smooth), "{:?}", joined.joins);
        // Its last side a curve back to its start: as many points as sides.
        let curved = o.joined(&[0], Join::Smooth);
        assert_eq!((curved.segments.len(), curved.point_count(), curved.points().len()), (4, 4, 4));
        // Taken away: the sides either side made one; three points at least are left.
        let gone = split.without(&[1]).expect("four left");
        assert_eq!(gone.points(), o.points());
        assert_eq!(gone.handles(0)[1], split.handles(0)[1]);
        assert!(o.without(&[0]).is_some());
        assert!(o.without(&[0, 1]).is_none(), "an outline of two points");
        // The start taken away: the next point is its start.
        assert_eq!(o.without(&[0]).unwrap().start, [100.0, 0.0]);
    }

    #[test]
    fn handles_move_as_their_points_join_and_sides_bend() {
        let o = bulging().joined(&[0], Join::Smooth);
        // Made smooth from a corner: handles in line, a third of the way to each neighbor.
        let [into, out] = o.handles(0);
        let (into, out) = (into.expect("pulled out"), out.expect("kept"));
        let (u, v) = ([into[0], into[1]], [out[0], out[1]]);
        assert!((u[0] * v[1] - u[1] * v[0]).abs() < 1e-6, "not in line: {u:?} {v:?}");
        assert!((u[0].hypot(u[1]) - 100.0 / 3.0).abs() < 1e-9, "{u:?}");
        // A smooth point's handle moved: the other turns with it, keeping its length.
        let length = |h: [f64; 2]| h[0].hypot(h[1]);
        let moved = o.with_handle(0, End::Out, [0.0, -40.0], false);
        let [into2, out2] = moved.handles(0);
        assert_eq!(out2, Some([0.0, -40.0]));
        assert!(close(into2.unwrap(), [0.0, length(into)]), "{into2:?}");
        // Symmetric: mirrored. Broken (Alt): alone, the point made sharp.
        let sym = o.joined(&[0], Join::Symmetric).with_handle(0, End::Out, [0.0, -40.0], false);
        assert!(close(sym.handles(0)[0].unwrap(), [0.0, 40.0]));
        let broken = o.with_handle(0, End::Out, [0.0, -40.0], true);
        assert_eq!((broken.handles(0)[0], broken.join(0)), (Some(into), Join::Sharp));
        // Made sharp: handles taken back, the bottom-left side straight again.
        let sharp = o.joined(&[0], Join::Sharp);
        assert_eq!(sharp.handles(0)[0], None);
        assert_eq!(sharp.join(0), Join::Sharp);
        // A curve bent: its point where it is held moves as the pointer does.
        let b = bulging();
        for t in [0.3, 0.5, 0.7] {
            let bent = b.bent(0, t, [5.0, -12.0]);
            let was = b.along(0, t);
            assert!(close(bent.along(0, t), [was[0] + 5.0, was[1] - 12.0]), "at {t}");
            assert_eq!(bent.points(), b.points());
        }
        // A straight side dragged: moved with its two ends.
        assert_eq!(b.bent(1, 0.5, [10.0, 0.0]).points()[1..3], [[110.0, 0.0], [110.0, 100.0]]);
        // The side nearest a point, how far along, how far away.
        let (side, t, far) = b.nearest([103.0, 40.0]).expect("it has sides");
        assert_eq!(side, 1);
        assert!((t - 0.4).abs() < 1e-9 && (far - 3.0).abs() < 1e-9, "{t} {far}");
    }

    #[test]
    fn a_symmetric_outline_is_its_slice_and_the_slice_s_copies() {
        use crate::selection::Step;
        // A diamond round (100, 100): drawn whole, and as a slice of each symmetry that makes it.
        let diamond = polygon(&[[100.0, 50.0], [150.0, 100.0], [100.0, 150.0], [50.0, 100.0]], true);
        let sym = |order, mirror| Symmetry { order, mirror, center: [100.0, 100.0], angle: 0.0 };
        let slice = |points: &[[f64; 2]], s: Symmetry| Outline { symmetry: Some(s), ..polygon(points, true) };
        let made = [
            // A quarter, turned four times.
            slice(&[[100.0, 50.0], [150.0, 100.0]], sym(4, false)),
            // A half, from the axis back to it, mirrored.
            slice(&[[100.0, 50.0], [150.0, 100.0], [100.0, 150.0]], sym(1, true)),
            // A quarter, from the axis to the next seam, mirrored and turned twice.
            slice(&[[100.0, 50.0], [150.0, 100.0]], sym(2, true)),
            // The quarter drawn the other way round: turned the other way.
            slice(&[[150.0, 100.0], [100.0, 50.0]], sym(4, false)),
        ];
        let want = Mask::of(&diamond.filled());
        // (The copies joined end to end, no side across the middle: as long round as the
        // diamond, 4 × 50√2. A side across would still fill it, by the nonzero rule.)
        let round = |o: &Outline| {
            let p = o.polygon();
            (0..p.len()).map(|i| len(sub(p[(i + 1) % p.len()], p[i]))).sum::<f64>()
        };
        for o in &made {
            assert_eq!(Mask::of(&o.filled()), want, "{:?}", o.symmetry);
            assert!((round(o) - 200.0 * std::f64::consts::SQRT_2).abs() < 1e-6, "{:?}: {}", o.symmetry, round(o));
        }
        // Its points: the slice's, not closed.
        assert_eq!((made[1].point_count(), made[1].whole().symmetry), (3, None));
        // Carried by a turn: still symmetric; by a stretch, the whole outline carried.
        let turned = made[0].transformed(&Affine::rotate(std::f64::consts::FRAC_PI_2).about(100.0, 100.0));
        assert!(turned.symmetry.is_some());
        assert_eq!(Mask::of(&turned.filled()), want, "a diamond turned a quarter is itself");
        let stretched = made[0].transformed(&Affine::scale(2.0, 1.0));
        assert_eq!(stretched.symmetry, None);
        assert_eq!(Mask::of(&stretched.filled()), Mask::of(&diamond.transformed(&Affine::scale(2.0, 1.0)).filled()));
        // Kept through a file's words; one past what the editor makes refused.
        let read: Outline = serde_json::from_str(&serde_json::to_string(&made[2]).unwrap()).unwrap();
        assert_eq!(read, made[2]);
        let too_many = slice(&[[100.0, 50.0], [150.0, 100.0]], sym(crate::symmetry::MOST_ORDER + 1, false));
        assert!(Step::Outline { outline: too_many, combine: crate::mask::Combine::Replace }.refused().is_some());
    }

    #[test]
    fn a_symmetric_outline_s_slice_is_edited_as_a_line_not_closed() {
        // A half of a diamond, from the axis back to it, mirrored.
        let sym = Symmetry { order: 1, mirror: true, center: [100.0, 100.0], angle: 0.0 };
        let half = Outline { symmetry: Some(sym), ..polygon(&[[100.0, 50.0], [150.0, 100.0], [100.0, 150.0]], true) };
        // Its ends have a side on one hand only; its points are its own three.
        assert_eq!(half.points().len(), 3);
        assert_eq!(half.nodes()[0].into, None);
        // No side from the last back to the first: the nearest side to a point between them is
        // another.
        let (side, _, far) = half.nearest([100.0, 100.0]).expect("it has sides");
        assert!(side < 2 && far > 30.0, "{side} {far}");
        // A point added, moved, made smooth: still a slice, its symmetry kept.
        let split = half.split(1, 0.5);
        assert_eq!((split.point_count(), split.symmetry), (4, Some(sym)));
        let moved = split.moved(&[2], [10.0, 0.0]);
        assert_eq!(moved.points()[2], [135.0, 125.0]);
        let smooth = moved.joined(&[0, 2], Join::Smooth);
        assert_eq!(smooth.join(2), Join::Smooth);
        assert!(smooth.handles(0)[0].is_none(), "a handle into the slice's first point");
        // Its ends taken away: their sides go with them; two points at least are left.
        let cut = smooth.without(&[0]).expect("three left");
        assert_eq!((cut.start, cut.point_count()), ([150.0, 100.0], 3));
        assert_eq!(smooth.without(&[3]).unwrap().points().last(), Some(&[135.0, 125.0]));
        assert!(half.without(&[0, 1]).is_none(), "a slice of one point");
        // Every edit kept its whole outline symmetric: its mask is its own mirror image (to a
        // level, where reflecting a curve rounds).
        for (k, o) in [&split, &moved, &smooth, &cut].into_iter().enumerate() {
            let (m, flipped) = (Mask::of(&o.filled()), Mask::of(&o.transformed(&sym.reflection()).filled()));
            let worst = (0..200)
                .flat_map(|y| (0..200).map(move |x| (x, y)))
                .map(|(x, y)| m.level_at(x, y).abs_diff(flipped.level_at(x, y)))
                .max()
                .unwrap();
            assert!(worst <= 1, "edit {k}: {worst} apart");
            assert!(m.level_at(100, 100) == FULL, "edit {k}: its middle not selected");
        }
    }

    #[test]
    fn symmetric_outlines_join_their_copies_whichever_way_drawn_and_keep_closed_ones_apart() {
        let sym = |order, mirror| Symmetry { order, mirror, center: [100.0, 100.0], angle: 0.0 };
        let slice = |points: &[[f64; 2]], s: Symmetry| Outline { symmetry: Some(s), ..polygon(points, true) };
        // A symmetry making no copies is as none: not a slice, not refused at a stretch (no
        // endless carrying), but refused where it is kept.
        let square = polygon(&[[60.0, 60.0], [140.0, 60.0], [140.0, 140.0], [60.0, 140.0]], true);
        let none = Outline { symmetry: Some(sym(1, false)), ..square.clone() };
        assert_eq!((none.point_count(), none.symmetric()), (4, None));
        assert_eq!(
            Mask::of(&none.transformed(&Affine::scale(2.0, 1.0)).filled()),
            Mask::of(&square.transformed(&Affine::scale(2.0, 1.0)).filled())
        );
        assert!(none.refused().is_some());
        // A five-pointed star's slice, from its tip (on the axis) to its valley (on the seam),
        // or the other way: the same star, its valleys not filled.
        let tip = [100.0, 20.0];
        let valley = Symmetry { angle: 36.0, ..sym(5, true) }.axis();
        let valley = [100.0 + 30.0 * valley[0], 100.0 + 30.0 * valley[1]];
        let out = slice(&[tip, valley], sym(5, true));
        let back = slice(&[valley, tip], sym(5, true));
        let star = Mask::of(&out.filled());
        assert_eq!(Mask::of(&back.filled()), star);
        assert_eq!(star.level_at(100, 100), FULL);
        let between = Symmetry { angle: 36.0, ..sym(5, true) }.axis();
        let (x, y) = (100.0 + 60.0 * between[0], 100.0 + 60.0 * between[1]);
        assert_eq!(star.level_at(x as i64, y as i64), 0, "a valley filled");
        // Stretched, it is an ordinary outline of as many points as it has corners: none doubled.
        let diamond = slice(&[[100.0, 50.0], [150.0, 100.0]], sym(4, false));
        assert_eq!(diamond.transformed(&Affine::scale(2.0, 1.0)).point_count(), 4);
        // A closed slice (a small square away from the center), turned four times: four squares
        // apart, nothing between them selected.
        let dot = slice(&[[95.0, 40.0], [105.0, 40.0], [105.0, 50.0], [95.0, 50.0], [95.0, 40.0]], sym(4, false));
        let m = Mask::of(&dot.filled());
        assert_eq!([m.level_at(100, 45), m.level_at(155, 100), m.level_at(100, 155), m.level_at(45, 100)], [FULL; 4]);
        assert_eq!((m.level_at(100, 100), m.level_at(100, 70)), (0, 0), "the copies of a closed slice joined");
        // Made smooth at its end on the axis: its handle at right angles to it, so its mirror
        // image meets it smoothly.
        let half = slice(&[[100.0, 50.0], [150.0, 100.0], [100.0, 150.0]], sym(1, true)).joined(&[0], Join::Smooth);
        let h = half.handles(0)[1].expect("pulled out");
        assert!((h[1] - 50.0).abs() < 1e-9 && h[0] > 100.0, "{h:?}");
        // A slice of more segments than its copies may have, refused.
        let big = Outline {
            symmetry: Some(sym(crate::symmetry::MOST_ORDER, true)),
            segments: (0..20_000).map(|i| Segment::Line { to: [f64::from(i), 1.0] }).collect(),
            ..polygon(&[[0.0, 0.0], [1.0, 1.0]], true)
        };
        assert!(big.refused().is_some());
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
