//! The rulers along the top and left edges of the canvas view, outside the image: marks in
//! the unit sizes are shown in, counted from the canvas's top-left corner at the image's
//! resolution, with the pointer's place marked on each.

use crate::theme::Palette;
use eframe::egui::{self, Align2, Pos2, Rect, Stroke, Vec2};
use opal_core::units::Unit;

/// How thick a ruler is, in points.
pub const THICK: f32 = 18.0;

/// The fewest points between two labeled marks.
const LABEL_GAP: f64 = 64.0;

/// The fewest points between two marks of any kind.
const MARK_GAP: f64 = 5.0;

/// The most marks a ruler draws (a guard against a view zoomed to nothing).
const MOST: usize = 4000;

/// A mark on a ruler.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// Where it is, in points from the ruler's start.
    pub at: f32,
    /// How far it reaches across the ruler, as a share of its thickness.
    pub reach: f32,
    /// The length it stands at, for a labeled mark.
    pub label: Option<String>,
}

/// The marks of a ruler `length` points long whose start shows the length `start` (in
/// document pixels from the canvas's corner), going `per_point` document pixels a point,
/// in `unit` at `ppi` pixels per inch. Labeled marks fall on whole multiples of 1, 2 or 5
/// times a power of ten of the unit (of pixels, at least one), far enough apart for their
/// labels; the marks between them divide them in tens, quarters or fifths (or halves, or
/// none, where those would crowd).
pub fn marks(start: f64, per_point: f64, length: f32, unit: Unit, ppi: f64) -> Vec<Mark> {
    if !(per_point > 0.0 && per_point.is_finite() && length > 0.0) {
        return Vec::new();
    }
    let unit_per_point = unit.from_px(per_point, ppi);
    let want = unit_per_point * LABEL_GAP;
    let base = 10f64.powf(want.log10().floor());
    let (mut step, mut lead) = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| (m * base, if m == 10.0 { 1 } else { m as u32 }))
        .find(|&(s, _)| s >= want)
        .unwrap_or((10.0 * base, 1));
    if unit == Unit::Px && step < 1.0 {
        (step, lead) = (1.0, 1);
    }
    let mut parts: u32 = match lead {
        2 => 4,
        5 => 5,
        _ => 10,
    };
    // Fewer marks between, where they would crowd: halves (a whole step's, or a 5's in
    // fifths, which halve no further), or none.
    for fewer in [if lead == 5 { 1 } else { 2 }, 1] {
        if step / f64::from(parts) / unit_per_point >= MARK_GAP {
            break;
        }
        parts = fewer;
    }
    let minor = step / f64::from(parts);
    let first = unit.from_px(start, ppi);
    let last = unit.from_px(start + per_point * f64::from(length), ppi);
    let decimals = if step >= 1.0 { 0 } else { (-step.log10()).ceil() as usize };
    let mut out = Vec::new();
    let mut k = (first / minor).ceil() as i64;
    // (Within a hair of the end, which `k * minor` may miss by its rounding.)
    while (k as f64) * minor <= last + minor * 1e-9 && out.len() < MOST {
        let v = k as f64 * minor;
        let at = ((v - first) / unit_per_point) as f32;
        let major = k.rem_euclid(i64::from(parts)) == 0;
        let half = parts.is_multiple_of(2) && k.rem_euclid(i64::from(parts / 2)) == 0;
        let label = major.then(|| {
            let text = format!("{v:.decimals$}");
            // No "-0".
            if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { "0".to_owned() } else { text }
        });
        out.push(Mark {
            at,
            reach: if major {
                1.0
            } else if half {
                0.5
            } else {
                0.3
            },
            label,
        });
        k += 1;
    }
    out
}

/// What a ruler counts: the length at its start (document pixels from the canvas's corner,
/// along the canvas's axis that runs along the ruler), and how much that changes a point
/// along it (less than nought where the axis runs the other way: the view turned).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub start: f64,
    pub per_point: f64,
}

impl Scale {
    /// The marks of a ruler `length` points long counting as this does (see [`marks`]).
    fn marks(self, length: f32, unit: Unit, ppi: f64) -> Vec<Mark> {
        if self.per_point >= 0.0 {
            return marks(self.start, self.per_point, length, unit, ppi);
        }
        // Counting down: the marks of the ruler read from its end, put back in place.
        let end = self.start + self.per_point * f64::from(length);
        let mut out = marks(end, -self.per_point, length, unit, ppi);
        for m in &mut out {
            m.at = length - m.at;
        }
        out
    }
}

/// What runs along a ruler lying along the view's edge from `at` (device pixels from the
/// view's corner) in direction `dir` (device pixels, a point long), as `view` shows the
/// canvas: the canvas's axis that does, counted from `corner` (the canvas's top-left, in
/// document pixels); none if neither does (the view turned by other than a multiple of
/// 90°).
pub fn scale(view: &crate::doc::View, at: Vec2, dir: Vec2, corner: Vec2) -> Option<Scale> {
    let a = view.to_canvas(at) - corner;
    // From the direction, not two points, which far out would lose their precision.
    let step = view.unturn(dir) / view.zoom;
    let tiny = step.length() * 1e-4;
    let (start, per_point) = if step.y.abs() <= tiny {
        (a.x, step.x)
    } else if step.x.abs() <= tiny {
        (a.y, step.y)
    } else {
        return None;
    };
    Some(Scale { start: f64::from(start), per_point: f64::from(per_point) })
}

/// What the rulers are drawn with.
pub struct Ruling {
    /// What the top ruler counts, and the left: none where no axis of the canvas runs along
    /// it (the view turned by other than a multiple of 90°), which then marks only the
    /// pointer.
    pub top: Option<Scale>,
    pub left: Option<Scale>,
    pub unit: Unit,
    pub ppi: f64,
    /// The pointer, if it is over the view.
    pub pointer: Option<Pos2>,
}

/// Draws the rulers in the strips `top` and `left` of the view (with `corner` where they
/// meet), as `ruling` says.
pub fn draw(ui: &egui::Ui, corner: Rect, top: Rect, left: Rect, ruling: &Ruling, palette: &Palette) {
    let painter = ui.painter();
    let back = ui.visuals().extreme_bg_color;
    let ink = palette.weak_text;
    let font = egui::FontId::proportional(9.5);
    for r in [corner, top, left] {
        painter.rect_filled(r, 0.0, back);
    }
    let edge = Stroke::new(1.0, palette.separator);
    painter.line_segment([top.left_bottom(), top.right_bottom()], edge);
    painter.line_segment([left.right_top(), left.right_bottom()], edge);
    painter.line_segment([corner.left_bottom(), corner.right_bottom()], edge);
    painter.line_segment([corner.right_top(), corner.right_bottom()], edge);
    painter.text(corner.center(), Align2::CENTER_CENTER, ruling.unit.id(), font.clone(), ink);
    // Across the top: x from the canvas's left edge (or whichever axis runs along it).
    let top_clip = painter.with_clip_rect(top);
    let marks_of =
        |scale: Option<Scale>, length: f32| scale.map(|s| s.marks(length, ruling.unit, ruling.ppi)).unwrap_or_default();
    for m in marks_of(ruling.top, top.width()) {
        let x = top.left() + m.at;
        let y0 = top.bottom() - THICK * m.reach;
        top_clip.line_segment([Pos2::new(x, y0), Pos2::new(x, top.bottom())], Stroke::new(1.0, ink));
        if let Some(label) = m.label {
            top_clip.text(Pos2::new(x + 2.0, top.top() + 1.0), Align2::LEFT_TOP, label, font.clone(), ink);
        }
    }
    // Down the left: y from the canvas's top edge (or whichever), labels turned to run up the
    // ruler.
    let left_clip = painter.with_clip_rect(left);
    for m in marks_of(ruling.left, left.height()) {
        let y = left.top() + m.at;
        let x0 = left.right() - THICK * m.reach;
        left_clip.line_segment([Pos2::new(x0, y), Pos2::new(left.right(), y)], Stroke::new(1.0, ink));
        if let Some(label) = m.label {
            let galley = ui.fonts_mut(|f| f.layout_no_wrap(label, font.clone(), ink));
            let at = Pos2::new(left.left() + 1.0, y - 2.0);
            left_clip.add(egui::epaint::TextShape::new(at, galley, ink).with_angle(-std::f32::consts::FRAC_PI_2));
        }
    }
    // The pointer, on both.
    if let Some(p) = ruling.pointer {
        let mark = Stroke::new(1.0, palette.accent);
        if (top.left()..=top.right()).contains(&p.x) {
            painter.line_segment([Pos2::new(p.x, top.top()), Pos2::new(p.x, top.bottom())], mark);
        }
        if (left.top()..=left.bottom()).contains(&p.y) {
            painter.line_segment([Pos2::new(left.left(), p.y), Pos2::new(left.right(), p.y)], mark);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_labeled_at_round_numbers_far_enough_apart() {
        // At 100%: a point a pixel; labels every 100 px, marks every 10.
        let m = marks(0.0, 1.0, 300.0, Unit::Px, 72.0);
        let labels: Vec<_> = m.iter().filter_map(|m| m.label.clone()).collect();
        assert_eq!(labels, ["0", "100", "200", "300"]);
        assert_eq!(m.iter().filter(|m| m.label.is_none()).count(), 27);
        assert_eq!(m[5].reach, 0.5, "the half-way mark is longer");
        // Zoomed in so far that a tenth of a pixel would have room: a label every pixel, no
        // closer.
        let m = marks(10.0, 0.001, 3000.0, Unit::Px, 72.0);
        assert_eq!(m.iter().filter_map(|m| m.label.clone()).collect::<Vec<_>>(), ["10", "11", "12", "13"]);
    }

    #[test]
    fn other_units_are_counted_from_the_canvas_at_the_resolution() {
        // 300 ppi, 3 pixels a point: an inch is 100 points; labels every inch, from -1.
        let m = marks(-300.0, 3.0, 400.0, Unit::In, 300.0);
        let labeled: Vec<(f32, String)> = m.iter().filter_map(|m| Some((m.at.round(), m.label.clone()?))).collect();
        let want = [(0.0, "-1"), (100.0, "0"), (200.0, "1"), (300.0, "2"), (400.0, "3")];
        assert_eq!(labeled, want.map(|(at, l)| (at, l.to_owned())));
        // Millimeters at 72 ppi, 1 pixel a point: 64 points are 22.6 mm, so labels every 50.
        let m = marks(0.0, 1.0, 300.0, Unit::Mm, 72.0);
        assert_eq!(m.iter().filter_map(|m| m.label.clone()).collect::<Vec<_>>(), ["0", "50", "100"]);
        // Tenths of an inch, zoomed in, labeled with their decimal (and nothing with "0.0").
        let m = marks(0.0, 0.25, 300.0, Unit::In, 300.0);
        assert_eq!(m.iter().filter_map(|m| m.label.clone()).take(3).collect::<Vec<_>>(), ["0", "0.1", "0.2"]);
    }

    #[test]
    fn a_ruler_the_canvas_runs_the_other_way_along_counts_down() {
        // At 100%, from 300 px at its start down to 0 at 300 points along.
        let m = Scale { start: 300.0, per_point: -1.0 }.marks(300.0, Unit::Px, 72.0);
        let labeled: Vec<(f32, String)> = m.iter().filter_map(|m| Some((m.at.round(), m.label.clone()?))).collect();
        let want = [(300.0, "0"), (200.0, "100"), (100.0, "200"), (0.0, "300")];
        assert_eq!(labeled, want.map(|(at, l)| (at, l.to_owned())));
    }

    #[test]
    fn a_turned_view_s_rulers_count_the_axis_along_them_or_none() {
        use crate::doc::View;
        let corner = Vec2::new(-50.0, 0.0);
        let mut v = View::default();
        (v.zoom, v.offset, v.viewport) = (2.0, Vec2::new(100.0, 40.0), Vec2::new(600.0, 400.0));
        // Straight: x across the top, from the canvas's corner, half a pixel a device pixel.
        let top = scale(&v, Vec2::ZERO, Vec2::X, corner).unwrap();
        assert_eq!(top, Scale { start: f64::from(v.to_canvas(Vec2::ZERO).x + 50.0), per_point: 0.5 });
        // A quarter turn clockwise: y along the top, counting down; x down the left.
        v.turn_to(90.0);
        let (top, left) =
            (scale(&v, Vec2::ZERO, Vec2::X, corner).unwrap(), scale(&v, Vec2::ZERO, Vec2::Y, corner).unwrap());
        let at = v.to_canvas(Vec2::ZERO) - corner;
        assert!((top.start - f64::from(at.y)).abs() < 1e-3 && (top.per_point + 0.5).abs() < 1e-6, "{top:?}");
        assert!((left.start - f64::from(at.x)).abs() < 1e-3 && (left.per_point - 0.5).abs() < 1e-6, "{left:?}");
        // Turned otherwise, neither axis runs along either.
        v.turn_to(30.0);
        assert_eq!((scale(&v, Vec2::ZERO, Vec2::X, corner), scale(&v, Vec2::ZERO, Vec2::Y, corner)), (None, None));
    }

    #[test]
    fn nothing_is_marked_for_a_view_of_nothing() {
        assert!(marks(0.0, 0.0, 300.0, Unit::Px, 72.0).is_empty());
        assert!(marks(0.0, f64::NAN, 300.0, Unit::Px, 72.0).is_empty());
        assert!(marks(0.0, 1.0, 0.0, Unit::Px, 72.0).is_empty());
        assert!(marks(0.0, 1e-9, 1e6, Unit::Px, 72.0).len() <= MOST);
    }
}
