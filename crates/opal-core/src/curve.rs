//! Editable curves from 0–1 to 0–1, defined by control points.
//!
//! Settings everywhere in the editor are curves: brush dynamics, falloffs, and the
//! per-channel curves of color selection. A curve over a hue channel is *periodic*: it wraps
//! around, so the point at hue 359° flows smoothly into the one at 1°.

use serde::{Deserialize, Serialize};

/// How a curve passes between its control points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Interpolation {
    /// Straight lines.
    Linear,
    /// A smooth curve that never overshoots its points (monotone cubic, Fritsch–Carlson):
    /// between two points it stays within their values.
    #[default]
    Smooth,
    /// A smooth curve through the points (Catmull–Rom) that may overshoot between them; the
    /// result is kept within 0–1.
    Spline,
}

/// The smallest horizontal distance kept between neighboring points.
pub const MIN_GAP: f64 = 1e-3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CurveData")]
pub struct Curve {
    /// Sorted by `x`, at least [`MIN_GAP`] apart; both coordinates in 0–1 (for a periodic
    /// curve, `x` in 0–1 exclusive of 1). Never empty.
    points: Vec<[f64; 2]>,
    pub interpolation: Interpolation,
    periodic: bool,
}

/// A curve as stored, checked before it becomes a [`Curve`].
#[derive(Deserialize)]
struct CurveData {
    points: Vec<[f64; 2]>,
    interpolation: Interpolation,
    periodic: bool,
}

impl TryFrom<CurveData> for Curve {
    type Error = String;

    fn try_from(d: CurveData) -> Result<Curve, String> {
        if d.points.is_empty() {
            return Err("a curve needs at least one point".into());
        }
        if d.points.iter().flatten().any(|v| !v.is_finite()) {
            return Err("a curve's points must be finite numbers".into());
        }
        // Sorts, spaces and clamps the points as any new curve's are.
        Ok(Curve::new(&d.points, d.interpolation, d.periodic))
    }
}

impl Curve {
    /// The curve `y = x`.
    pub fn identity() -> Curve {
        Curve { points: vec![[0.0, 0.0], [1.0, 1.0]], interpolation: Interpolation::Smooth, periodic: false }
    }

    /// The curve through `points` (sorted and spaced as needed). `periodic` makes it wrap
    /// around at 1 back to 0.
    pub fn new(points: &[[f64; 2]], interpolation: Interpolation, periodic: bool) -> Curve {
        assert!(!points.is_empty(), "a curve needs at least one point");
        let mut pts: Vec<[f64; 2]> = points
            .iter()
            .map(|p| {
                let x = if periodic { p[0].rem_euclid(1.0) } else { p[0].clamp(0.0, 1.0) };
                [x, p[1].clamp(0.0, 1.0)]
            })
            .collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|b, a| b[0] - a[0] < MIN_GAP);
        Curve { points: pts, interpolation, periodic }
    }

    /// A curve with the constant value `y`.
    pub fn constant(y: f64) -> Curve {
        Curve::new(&[[0.0, y], [1.0, y]], Interpolation::Linear, false)
    }

    pub fn points(&self) -> &[[f64; 2]] {
        &self.points
    }

    pub fn is_periodic(&self) -> bool {
        self.periodic
    }

    /// Adds a point; returns its index.
    pub fn insert(&mut self, x: f64, y: f64) -> usize {
        let x = if self.periodic { x.rem_euclid(1.0) } else { x.clamp(0.0, 1.0) };
        let y = y.clamp(0.0, 1.0);
        let i = self.points.partition_point(|p| p[0] < x);
        let too_close = |j: usize| self.points.get(j).is_some_and(|p| (p[0] - x).abs() < MIN_GAP);
        if too_close(i) {
            self.points[i][1] = y;
            return i;
        }
        if i > 0 && too_close(i - 1) {
            self.points[i - 1][1] = y;
            return i - 1;
        }
        self.points.insert(i, [x, y]);
        i
    }

    /// Moves point `i` to `(x, y)`. Its `x` stays between its neighbors, so points keep their
    /// order and indices while being dragged.
    pub fn move_point(&mut self, i: usize, x: f64, y: f64) {
        let lo = if i == 0 { 0.0 } else { self.points[i - 1][0] + MIN_GAP };
        let hi = match self.points.get(i + 1) {
            Some(p) => p[0] - MIN_GAP,
            None if self.periodic => 1.0 - MIN_GAP,
            None => 1.0,
        };
        self.points[i] = [x.clamp(lo, hi.max(lo)), y.clamp(0.0, 1.0)];
    }

    /// Removes point `i`, unless it is the only one. Returns whether it was removed.
    pub fn remove(&mut self, i: usize) -> bool {
        if self.points.len() <= 1 {
            return false;
        }
        self.points.remove(i);
        true
    }

    /// The curve's value at `x` (at 0 when `x` is not a number).
    pub fn eval(&self, x: f64) -> f64 {
        let x = if x.is_nan() { 0.0 } else { x };
        let pts = self.extended();
        let x = if self.periodic { x.rem_euclid(1.0) } else { x };
        let n = pts.len();
        if n == 1 {
            return pts[0][1];
        }
        if x <= pts[0][0] {
            return pts[0][1];
        }
        if x >= pts[n - 1][0] {
            return pts[n - 1][1];
        }
        let k = pts.partition_point(|p| p[0] <= x) - 1;
        let ([x0, y0], [x1, y1]) = (pts[k], pts[k + 1]);
        let h = x1 - x0;
        let t = (x - x0) / h;
        let y = match self.interpolation {
            Interpolation::Linear => y0 + (y1 - y0) * t,
            Interpolation::Smooth | Interpolation::Spline => {
                let m = self.tangents(&pts);
                let (t2, t3) = (t * t, t * t * t);
                (2.0 * t3 - 3.0 * t2 + 1.0) * y0
                    + (t3 - 2.0 * t2 + t) * h * m[k]
                    + (-2.0 * t3 + 3.0 * t2) * y1
                    + (t3 - t2) * h * m[k + 1]
            }
        };
        y.clamp(0.0, 1.0)
    }

    /// The points evaluation works on: for a periodic curve, with copies of the last two
    /// points before 0 and of the first two after 1, so the curve wraps smoothly.
    fn extended(&self) -> Vec<[f64; 2]> {
        if !self.periodic {
            return self.points.clone();
        }
        let n = self.points.len();
        let mut v = Vec::with_capacity(n + 4);
        for j in [2usize, 1] {
            if n >= j {
                let p = self.points[n - j];
                v.push([p[0] - 1.0, p[1]]);
            }
        }
        v.extend_from_slice(&self.points);
        for j in 0..2.min(n) {
            let p = self.points[j];
            v.push([p[0] + 1.0, p[1]]);
        }
        v
    }

    /// The slope of the curve at each point of `pts`.
    fn tangents(&self, pts: &[[f64; 2]]) -> Vec<f64> {
        let n = pts.len();
        let d: Vec<f64> = pts.windows(2).map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0])).collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        match self.interpolation {
            Interpolation::Spline => {
                for k in 1..n - 1 {
                    m[k] = (pts[k + 1][1] - pts[k - 1][1]) / (pts[k + 1][0] - pts[k - 1][0]);
                }
            }
            _ => {
                for k in 1..n - 1 {
                    m[k] = if d[k - 1] * d[k] <= 0.0 { 0.0 } else { (d[k - 1] + d[k]) / 2.0 };
                }
                for k in 0..n - 1 {
                    if d[k] == 0.0 {
                        m[k] = 0.0;
                        m[k + 1] = 0.0;
                        continue;
                    }
                    let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
                    let s = a * a + b * b;
                    if s > 9.0 {
                        let tau = 3.0 / s.sqrt();
                        m[k] = tau * a * d[k];
                        m[k + 1] = tau * b * d[k];
                    }
                }
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_identity_for_every_interpolation() {
        for interp in [Interpolation::Linear, Interpolation::Smooth, Interpolation::Spline] {
            let mut c = Curve::identity();
            c.interpolation = interp;
            for i in 0..=10 {
                let x = f64::from(i) / 10.0;
                assert!((c.eval(x) - x).abs() < 1e-12, "{interp:?} at {x}");
            }
        }
    }

    #[test]
    fn smooth_never_overshoots_but_spline_may() {
        // A flat stretch at 0.5 followed by a steep rise.
        let pts = [[0.0, 0.5], [0.4, 0.5], [0.5, 1.0], [1.0, 1.0]];
        let smooth = Curve::new(&pts, Interpolation::Smooth, false);
        for i in 0..=40 {
            let x = f64::from(i) / 100.0;
            assert!((smooth.eval(x) - 0.5).abs() < 1e-12, "the flat stretch bulged at {x}");
        }
        // Catmull–Rom carries the rise's slope into the flat stretch and dips below it.
        let spline = Curve::new(&pts, Interpolation::Spline, false);
        assert!(spline.eval(0.35) < 0.5 - 1e-3, "{}", spline.eval(0.35));
    }

    #[test]
    fn values_hold_beyond_the_end_points() {
        let c = Curve::new(&[[0.2, 0.3], [0.8, 0.9]], Interpolation::Linear, false);
        assert_eq!(c.eval(0.0), 0.3);
        assert_eq!(c.eval(1.0), 0.9);
        assert!((c.eval(0.5) - 0.6).abs() < 1e-12);
    }

    #[test]
    fn periodic_curves_wrap_around() {
        let c = Curve::new(&[[0.1, 1.0], [0.6, 0.0]], Interpolation::Linear, true);
        // Between 0.6 and 1.1 (0.1 wrapped) the curve rises from 0 to 1.
        assert!((c.eval(0.85) - 0.5).abs() < 1e-12);
        assert!((c.eval(0.0) - c.eval(1.0)).abs() < 1e-12);
        assert!((c.eval(0.05) - 0.9).abs() < 1e-12);
        let s = Curve::new(&[[0.1, 1.0], [0.6, 0.0]], Interpolation::Smooth, true);
        assert!((s.eval(0.999_999) - s.eval(0.0)).abs() < 1e-4, "no jump at the wrap");
    }

    #[test]
    fn editing_keeps_points_ordered() {
        let mut c = Curve::identity();
        let i = c.insert(0.5, 0.2);
        assert_eq!(i, 1);
        c.move_point(1, 2.0, -1.0);
        assert_eq!(c.points()[1], [1.0 - MIN_GAP, 0.0]);
        assert_eq!(c.insert(0.0, 0.7), 0, "a point too close to another replaces its value");
        assert_eq!(c.points().len(), 3);
        assert!(c.remove(0) && c.remove(0));
        assert!(!c.remove(0), "the last point stays");
        assert_eq!(c.eval(0.3), 1.0);
    }

    #[test]
    fn evaluating_at_nan_does_not_panic() {
        for interp in [Interpolation::Linear, Interpolation::Smooth, Interpolation::Spline] {
            let c = Curve::new(&[[0.2, 0.3], [0.8, 0.9]], interp, false);
            assert!((0.0..=1.0).contains(&c.eval(f64::NAN)));
        }
        let periodic = Curve::new(&[[0.2, 0.3], [0.8, 0.9]], Interpolation::Smooth, true);
        assert!((0.0..=1.0).contains(&periodic.eval(f64::NAN)));
    }
}
