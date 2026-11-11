//! Symmetry (design.md, *Symmetry*): one setting, shared by the outline tool (and, as they
//! come, vector shapes and the brushes): copies of what is drawn turned round a center, of
//! an order, with or without their mirror images across an axis.

use crate::geom::Affine;

/// The most copies round its center a symmetry makes.
pub const MOST_ORDER: u32 = 32;

/// Symmetry of `order` (that many copies, turned evenly round `center`; 1 for none), with or
/// without a `mirror` across the axis through `center`, `angle` degrees clockwise from straight
/// up. Order 1 with a mirror is one mirror axis; order n with one, a kaleidoscope of 2n parts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Symmetry {
    pub order: u32,
    pub mirror: bool,
    pub center: [f64; 2],
    pub angle: f64,
}

impl Symmetry {
    /// Whether it makes any copy at all.
    pub fn is_on(&self) -> bool {
        self.order > 1 || self.mirror
    }

    /// How many copies of what is drawn it makes, that included.
    pub fn copies(&self) -> usize {
        self.order.max(1) as usize * if self.mirror { 2 } else { 1 }
    }

    /// The axis's direction: straight up, turned `angle` clockwise (a unit's length).
    pub fn axis(&self) -> [f64; 2] {
        let (s, c) = self.angle.to_radians().sin_cos();
        [s, -c]
    }

    /// The reflection across the axis.
    pub fn reflection(&self) -> Affine {
        let [x, y] = self.axis();
        let (a, b, d) = (2.0 * x * x - 1.0, 2.0 * x * y, 2.0 * y * y - 1.0);
        Affine { a, b, c: b, d, e: 0.0, f: 0.0 }.about(self.center[0], self.center[1])
    }

    /// The turn `k` steps round (a step `1/order` of the circle, clockwise; back for `k` less
    /// than 0).
    pub fn turn(&self, k: i64) -> Affine {
        let step = std::f64::consts::TAU / f64::from(self.order.max(1));
        Affine::rotate(step * k as f64).about(self.center[0], self.center[1])
    }

    /// The symmetry `m` carries this one to, if it carries shapes to shapes alike (turning,
    /// moving, flipping, or scaling alike both ways): none, if it stretches one way more than
    /// the other, which no longer keeps it.
    pub fn transformed(&self, m: &Affine) -> Option<Symmetry> {
        let (x, y) = ([m.a, m.b], [m.c, m.d]);
        let (lx, ly) = (x[0].hypot(x[1]), y[0].hypot(y[1]));
        let square = (x[0] * y[0] + x[1] * y[1]).abs() <= 1e-9 * lx * ly;
        if !(lx > 0.0 && (lx - ly).abs() <= 1e-9 * lx.max(ly) && square) {
            return None;
        }
        let u = self.axis();
        let v = [m.a * u[0] + m.c * u[1], m.b * u[0] + m.d * u[1]];
        Some(Symmetry { center: m.apply(self.center), angle: v[0].atan2(-v[1]).to_degrees(), ..*self })
    }

    /// Why it could not have been made in the editor, if it could not (as one read from a file
    /// may be): an order past [`MOST_ORDER`], or a center or an angle that is not a number.
    pub fn refused(&self) -> Option<&'static str> {
        let ok = (1..=MOST_ORDER).contains(&self.order)
            && self.center.iter().all(|v| v.is_finite())
            && self.angle.is_finite();
        (!ok).then_some("a symmetry is of no order the editor makes, or not a number")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn a_symmetry_turns_and_reflects_about_its_center_and_axis() {
        let s = Symmetry { order: 4, mirror: true, center: [100.0, 100.0], angle: 0.0 };
        assert_eq!(s.copies(), 8);
        // Up the axis; a quarter of the circle clockwise is right.
        assert!(close(s.axis(), [0.0, -1.0]));
        assert!(close(s.turn(1).apply([100.0, 50.0]), [150.0, 100.0]));
        assert!(close(s.turn(-1).apply([100.0, 50.0]), [50.0, 100.0]));
        // Across the upright axis: left and right swap, up and down stay.
        assert!(close(s.reflection().apply([130.0, 60.0]), [70.0, 60.0]));
        let tilted = Symmetry { angle: 90.0, ..s };
        assert!(close(tilted.axis(), [1.0, 0.0]));
        assert!(close(tilted.reflection().apply([130.0, 60.0]), [130.0, 140.0]));
        // Carried by a turn and a move: its center and axis with it; by a stretch, not at all.
        let m = Affine::rotate(std::f64::consts::FRAC_PI_2).then(Affine::translate(10.0, 0.0));
        let t = s.transformed(&m).expect("a turn keeps it");
        assert!(close(t.center, m.apply([100.0, 100.0])));
        assert!((t.angle - 90.0).abs() < 1e-9, "{}", t.angle);
        assert!(s.transformed(&Affine::scale(2.0, 1.0)).is_none());
        assert!(s.transformed(&Affine::scale(-3.0, 3.0)).is_some(), "a flip, scaled alike");
        // What a file may not hold.
        assert!(Symmetry { order: 0, ..s }.refused().is_some());
        assert!(Symmetry { order: MOST_ORDER + 1, ..s }.refused().is_some());
        assert!(Symmetry { angle: f64::NAN, ..s }.refused().is_some());
        assert!(s.refused().is_none());
    }
}
