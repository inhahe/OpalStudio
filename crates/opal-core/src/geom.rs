//! Geometry: integer rectangles of pixels, and affine transforms.

use serde::{Deserialize, Serialize};

/// A rectangle of canvas pixels, `x0 <= x < x1`, `y0 <= y < y1`. Coordinates are 64-bit
/// because layers are unbounded and may extend far past the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IRect {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl IRect {
    pub const fn new(x0: i64, y0: i64, x1: i64, y1: i64) -> IRect {
        IRect { x0, y0, x1, y1 }
    }

    /// The rectangle from the origin to `(w, h)`.
    pub const fn from_size(w: i64, h: i64) -> IRect {
        IRect::new(0, 0, w, h)
    }

    pub fn width(&self) -> i64 {
        (self.x1 - self.x0).max(0)
    }

    pub fn height(&self) -> i64 {
        (self.y1 - self.y0).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// The smallest rectangle containing both (an empty rectangle contributes nothing).
    pub fn union(&self, other: &IRect) -> IRect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        IRect::new(self.x0.min(other.x0), self.y0.min(other.y0), self.x1.max(other.x1), self.y1.max(other.y1))
    }

    pub fn intersect(&self, other: &IRect) -> IRect {
        IRect::new(self.x0.max(other.x0), self.y0.max(other.y0), self.x1.min(other.x1), self.y1.min(other.y1))
    }

    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    /// The rectangle moved by `(dx, dy)`.
    pub fn translate(&self, dx: i64, dy: i64) -> IRect {
        IRect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }

    /// The rectangle grown by `m` pixels on every side (shrunk, for negative `m`).
    pub fn expand(&self, m: i64) -> IRect {
        IRect::new(self.x0 - m, self.y0 - m, self.x1 + m, self.y1 + m)
    }

    /// The pixels a real-valued rectangle touches: the smallest pixel rectangle containing
    /// it. Values past the 64-bit range saturate.
    pub fn covering(r: &FRect) -> IRect {
        IRect::new(r.x0.floor() as i64, r.y0.floor() as i64, r.x1.ceil() as i64, r.y1.ceil() as i64)
    }

    /// The same rectangle with real coordinates.
    pub fn to_f(&self) -> FRect {
        FRect::new(self.x0 as f64, self.y0 as f64, self.x1 as f64, self.y1 as f64)
    }
}

/// A rectangle with real coordinates, `x0 <= x <= x1`, `y0 <= y <= y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FRect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl FRect {
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> FRect {
        FRect { x0, y0, x1, y1 }
    }

    /// The smallest rectangle containing every point.
    pub fn bounding(points: &[[f64; 2]]) -> FRect {
        let mut r = FRect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &[x, y] in points {
            r.x0 = r.x0.min(x);
            r.y0 = r.y0.min(y);
            r.x1 = r.x1.max(x);
            r.y1 = r.y1.max(y);
        }
        r
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, other: &FRect) -> FRect {
        FRect::new(self.x0.min(other.x0), self.y0.min(other.y0), self.x1.max(other.x1), self.y1.max(other.y1))
    }

    /// The rectangle grown by `m` on every side.
    pub fn expand(&self, m: f64) -> FRect {
        FRect::new(self.x0 - m, self.y0 - m, self.x1 + m, self.y1 + m)
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn center(&self) -> [f64; 2] {
        [(self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5]
    }

    /// The corners, clockwise from the top-left.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        [[self.x0, self.y0], [self.x1, self.y0], [self.x1, self.y1], [self.x0, self.y1]]
    }
}

/// An affine map of the plane: `(x, y)` goes to `(a·x + c·y + e, b·x + d·y + f)`.
///
/// Composition reads like function application: `p.then(q)` applies `p` first, then `q`.
/// Stored in a file as `[a, b, c, d, e, f]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f64; 6]", into = "[f64; 6]")]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl From<[f64; 6]> for Affine {
    fn from([a, b, c, d, e, f]: [f64; 6]) -> Affine {
        Affine { a, b, c, d, e, f }
    }
}

impl From<Affine> for [f64; 6] {
    fn from(m: Affine) -> [f64; 6] {
        m.to_array()
    }
}

impl Default for Affine {
    fn default() -> Affine {
        Affine::IDENTITY
    }
}

/// How an affine map moves the pixel grid, when it moves it exactly onto itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridMap {
    /// The linear part, a signed permutation: entries are -1, 0 or 1.
    pub a: i64,
    pub b: i64,
    pub c: i64,
    pub d: i64,
    /// Where the corner `(0, 0)` goes.
    pub e: i64,
    pub f: i64,
}

impl GridMap {
    /// The pixel that pixel `(x, y)` lands on. A pixel is the unit square at its
    /// coordinates, so a flip lands pixel `x` on `e - x - 1`.
    pub fn pixel(&self, x: i64, y: i64) -> (i64, i64) {
        // Map the pixel's center, then take the pixel around the result.
        let cx = 2 * x + 1;
        let cy = 2 * y + 1;
        let mx = self.a * cx + self.c * cy + 2 * self.e;
        let my = self.b * cx + self.d * cy + 2 * self.f;
        ((mx - 1).div_euclid(2), (my - 1).div_euclid(2))
    }

    /// The inverse map.
    pub fn inverse(&self) -> GridMap {
        // A signed permutation's inverse is its transpose.
        let (a, b, c, d) = (self.a, self.c, self.b, self.d);
        GridMap { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) }
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub const fn translate(x: f64, y: f64) -> Affine {
        Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: x, f: y }
    }

    pub const fn scale(sx: f64, sy: f64) -> Affine {
        Affine { a: sx, b: 0.0, c: 0.0, d: sy, e: 0.0, f: 0.0 }
    }

    /// A rotation by `radians` about the origin; positive turns clockwise on screen, where
    /// y points down.
    pub fn rotate(radians: f64) -> Affine {
        let (s, c) = radians.sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    /// `self` about the point `(x, y)` instead of the origin.
    pub fn about(self, x: f64, y: f64) -> Affine {
        Affine::translate(-x, -y).then(self).then(Affine::translate(x, y))
    }

    /// The map that applies `self`, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        let m = next;
        Affine {
            a: m.a * self.a + m.c * self.b,
            b: m.b * self.a + m.d * self.b,
            c: m.a * self.c + m.c * self.d,
            d: m.b * self.c + m.d * self.d,
            e: m.a * self.e + m.c * self.f + m.e,
            f: m.b * self.e + m.d * self.f + m.f,
        }
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse map, or `None` if `self` flattens the plane (or is not finite).
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.determinant();
        if !det.is_normal() || !self.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        let (a, b, c, d) = (self.d * inv, -self.b * inv, -self.c * inv, self.a * inv);
        let inverse = Affine { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) };
        inverse.is_finite().then_some(inverse)
    }

    pub fn is_finite(&self) -> bool {
        self.to_array().iter().all(|v| v.is_finite())
    }

    pub fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f]
    }

    /// The bounding box of `r`'s image.
    pub fn bounds(&self, r: &FRect) -> FRect {
        FRect::bounding(&r.corners().map(|p| self.apply(p)))
    }

    /// How much the map stretches lengths along x and along y of the *result*: for the
    /// inverse of an object's transform, how many source pixels one canvas pixel spans in
    /// each source direction.
    pub fn row_norms(&self) -> [f64; 2] {
        [self.a.hypot(self.c), self.b.hypot(self.d)]
    }

    /// The map as a pixel-grid permutation, if it is one: its linear part a quarter turn
    /// or flip (or neither), and its translation whole pixels.
    pub fn grid_map(&self) -> Option<GridMap> {
        // Within a billionth of a whole number counts as whole, so a quarter turn computed
        // with sines and cosines is still recognized.
        let int = |v: f64| ((v - v.round()).abs() <= 1e-9 && v.abs() < 9.0e15).then_some(v.round() as i64);
        let (a, b, c, d) = (int(self.a)?, int(self.b)?, int(self.c)?, int(self.d)?);
        let one = |v: i64| v.abs() == 1;
        let permutation = (b == 0 && c == 0 && one(a) && one(d)) || (a == 0 && d == 0 && one(b) && one(c));
        permutation.then_some(())?;
        Some(GridMap { a, b, c, d, e: int(self.e)?, f: int(self.f)? })
    }

    pub fn to_array(&self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    /// The map's values as bits, for exact comparison and hashing.
    pub fn to_bits(&self) -> [u64; 6] {
        self.to_array().map(f64::to_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(p: [f64; 2], q: [f64; 2]) -> bool {
        (p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9
    }

    #[test]
    fn composition_applies_left_to_right() {
        let m = Affine::scale(2.0, 3.0).then(Affine::translate(10.0, 20.0));
        assert_eq!(m.apply([1.0, 1.0]), [12.0, 23.0]);
        let r = Affine::rotate(std::f64::consts::FRAC_PI_2);
        assert!(close(r.apply([1.0, 0.0]), [0.0, 1.0]), "a quarter turn clockwise on screen");
        let about = r.about(5.0, 5.0);
        assert!(close(about.apply([5.0, 5.0]), [5.0, 5.0]));
    }

    #[test]
    fn inverse_undoes_the_map() {
        let m = Affine::rotate(0.3).then(Affine::scale(2.0, 0.5)).then(Affine::translate(-7.0, 3.0));
        let inv = m.inverse().unwrap();
        for p in [[0.0, 0.0], [3.5, -2.0], [100.0, 40.0]] {
            assert!(close(inv.apply(m.apply(p)), p));
        }
        assert!(Affine::scale(0.0, 1.0).inverse().is_none());
        assert!(Affine::scale(f64::NAN, 1.0).inverse().is_none());
    }

    #[test]
    fn grid_maps_are_recognized_and_land_pixels_exactly() {
        assert!(Affine::translate(3.0, -4.0).grid_map().is_some());
        assert!(Affine::translate(0.5, 0.0).grid_map().is_none());
        assert!(Affine::scale(2.0, 2.0).grid_map().is_none());
        assert!(Affine::rotate(0.1).grid_map().is_none());
        let shear = Affine { c: 1.0, ..Affine::IDENTITY };
        assert!(shear.grid_map().is_none(), "a shear of whole pixels is not a permutation");
        // A horizontal flip of a 10-pixel-wide image onto itself: pixel 0 lands on 9.
        let flip = Affine::scale(-1.0, 1.0).then(Affine::translate(10.0, 0.0)).grid_map().unwrap();
        assert_eq!(flip.pixel(0, 0), (9, 0));
        assert_eq!(flip.pixel(9, 5), (0, 5));
        // A quarter turn clockwise of a 4×2 image into a 2×4 one.
        let turn = Affine::rotate(std::f64::consts::FRAC_PI_2).then(Affine::translate(2.0, 0.0)).grid_map().unwrap();
        assert_eq!(turn.pixel(0, 0), (1, 0));
        assert_eq!(turn.pixel(0, 1), (0, 0));
        assert_eq!(turn.pixel(3, 1), (0, 3));
        for (x, y) in [(0, 0), (3, 1), (-5, 7)] {
            let (u, v) = turn.pixel(x, y);
            assert_eq!(turn.inverse().pixel(u, v), (x, y));
        }
    }

    #[test]
    fn row_norms_measure_stretch_without_rotation_blurring_it() {
        let n = Affine::rotate(0.7).row_norms();
        assert!((n[0] - 1.0).abs() < 1e-12 && (n[1] - 1.0).abs() < 1e-12);
        assert_eq!(Affine::scale(0.25, 4.0).row_norms(), [0.25, 4.0]);
    }

    #[test]
    fn rectangles_cover_and_move() {
        let r = IRect::covering(&FRect::new(-0.5, 1.0, 2.25, 3.0));
        assert_eq!(r, IRect::new(-1, 1, 3, 3));
        assert_eq!(IRect::new(0, 0, 2, 2).translate(5, -1), IRect::new(5, -1, 7, 1));
        assert_eq!(IRect::new(0, 0, 2, 2).expand(1), IRect::new(-1, -1, 3, 3));
    }
}
