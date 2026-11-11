//! A turned box on the canvas that is dragged by its handles: the Crop tool's, and the one
//! round the selection's last shape (`select_tool`). Its geometry, its numbers (as the box's
//! fields give them), how a drag changes it, which part of it is under the pointer, and its
//! handles. The Crop tool's own (the box over a canvas, and the canvas a box makes) are in
//! `crop`.

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Stroke, Vec2};
use opal_core::geom::Affine;
use opal_core::mask::Shape;

/// A box: a rectangle `size` wide and tall around `center` (document pixels), turned by
/// `angle` radians (clockwise).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditBox {
    pub center: [f64; 2],
    pub size: [f64; 2],
    pub angle: f64,
}

/// What a drag on a box does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Move,
    /// Drawing a new box from where the drag began.
    New,
    Rotate,
    Corner(usize),
    Edge(usize),
}

/// A drag in progress: what is dragged, where it began (document pixels), and the box then.
#[derive(Clone, Copy, Debug)]
pub struct BoxDrag {
    pub part: Part,
    pub from: [f64; 2],
    pub start: EditBox,
}

/// How far (points) outside a corner a drag rotates the box instead of drawing a new one.
const ROTATE_ZONE: f32 = 36.0;
const GRAB: f32 = 8.0;

impl EditBox {
    /// The box a shape is drawn in: the rectangle's, or the ellipse's (the rectangle round it,
    /// turned with it).
    pub fn of_shape(s: &Shape) -> EditBox {
        EditBox { center: s.center, size: s.size, angle: s.angle.to_radians() }
    }

    /// `like` drawn in this box instead.
    pub fn shape(&self, like: &Shape) -> Shape {
        Shape { center: self.center, size: self.size, angle: self.angle.to_degrees(), ..*like }
    }

    /// The box with its sides on the pixels' edges, if it is upright (turned, it is left as
    /// it is): each side moved to the nearest, or, if `moved`, the box moved to the nearest
    /// place where they are, keeping its size. Never less than a pixel either way.
    pub fn on_pixels(self, moved: bool) -> EditBox {
        if self.angle != 0.0 {
            return self;
        }
        let [w, h] = self.size;
        let (x0, y0) = (self.center[0] - w / 2.0, self.center[1] - h / 2.0);
        let (x0, y0, x1, y1) = if moved {
            let (x0, y0) = (x0.round(), y0.round());
            (x0, y0, x0 + w.round(), y0 + h.round())
        } else {
            (x0.round(), y0.round(), (x0 + w).round(), (y0 + h).round())
        };
        let (x1, y1) = (x1.max(x0 + 1.0), y1.max(y0 + 1.0));
        EditBox { center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0], size: [x1 - x0, y1 - y0], angle: 0.0 }
    }

    /// From the box's own frame (centered, unturned) into the document.
    fn frame(&self) -> Affine {
        Affine::rotate(self.angle).then(Affine::translate(self.center[0], self.center[1]))
    }

    /// The corners in the document, clockwise from the top-left.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        let [w, h] = [self.size[0] * 0.5, self.size[1] * 0.5];
        let m = self.frame();
        [[-w, -h], [w, -h], [w, h], [-w, h]].map(|p| m.apply(p))
    }

    /// The box as numbers: its point `at` (see [`box_fields::POINTS`]), turned with it, from
    /// `origin` (the point positions count from, in the document), its size, and its angle
    /// in degrees.
    ///
    /// [`box_fields::POINTS`]: crate::box_fields::POINTS
    pub fn numbers(&self, origin: [f64; 2], at: [f64; 2]) -> crate::transform::Params {
        let [x, y] = self.frame().apply([(at[0] - 0.5) * self.size[0], (at[1] - 0.5) * self.size[1]]);
        crate::transform::Params {
            x: x - origin[0],
            y: y - origin[1],
            width: self.size[0],
            height: self.size[1],
            angle: self.angle.to_degrees(),
            mirrored: false,
        }
    }

    /// The box `p` describes, as [`numbers`](Self::numbers) gives it for the point `at`: so
    /// a size or angle typed scales and turns the box about that point.
    pub fn from_numbers(p: &crate::transform::Params, origin: [f64; 2], at: [f64; 2]) -> EditBox {
        let angle = p.angle.to_radians();
        // From the point to the center, turned with the box.
        let [dx, dy] = Affine::rotate(angle).apply([(0.5 - at[0]) * p.width, (0.5 - at[1]) * p.height]);
        let center = [p.x + origin[0] + dx, p.y + origin[1] + dy];
        EditBox { center, size: [p.width, p.height], angle }
    }

    /// The box after dragging `drag` to `p`, keeping width ÷ height at `aspect` if given.
    pub fn dragged(drag: &BoxDrag, p: [f64; 2], aspect: Option<f64>) -> EditBox {
        let s = drag.start;
        let Some(inv) = s.frame().inverse() else { return s };
        match drag.part {
            Part::Move => {
                let d = [p[0] - drag.from[0], p[1] - drag.from[1]];
                EditBox { center: [s.center[0] + d[0], s.center[1] + d[1]], ..s }
            }
            Part::Rotate => {
                let a0 = (drag.from[1] - s.center[1]).atan2(drag.from[0] - s.center[0]);
                let a1 = (p[1] - s.center[1]).atan2(p[0] - s.center[0]);
                EditBox { angle: s.angle + (a1 - a0), ..s }
            }
            Part::New => {
                let (a, b) = (drag.from, p);
                let mut w = (b[0] - a[0]).abs();
                let mut h = (b[1] - a[1]).abs();
                if let Some(r) = aspect {
                    (w, h) = fit_aspect(w, h, r);
                }
                let center = [a[0] + w * 0.5 * (b[0] - a[0]).signum(), a[1] + h * 0.5 * (b[1] - a[1]).signum()];
                EditBox { center, size: [w.max(1.0), h.max(1.0)], angle: 0.0 }
            }
            Part::Corner(k) | Part::Edge(k) => {
                // Work in the box's own frame, where it is centered and unturned.
                let [hw, hh] = [s.size[0] * 0.5, s.size[1] * 0.5];
                let local = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
                let q = inv.apply(p);
                let (mut x0, mut y0, mut x1, mut y1) = (-hw, -hh, hw, hh);
                let corner = matches!(drag.part, Part::Corner(_));
                if corner {
                    let held = local[(k + 2) % 4];
                    (x0, x1) = (held[0].min(q[0]), held[0].max(q[0]));
                    (y0, y1) = (held[1].min(q[1]), held[1].max(q[1]));
                    if let Some(r) = aspect {
                        let (w, h) = fit_aspect(x1 - x0, y1 - y0, r);
                        // Grow away from the held corner.
                        let (sx, sy) = ((q[0] - held[0]).signum(), (q[1] - held[1]).signum());
                        (x0, x1) = (held[0].min(held[0] + sx * w), held[0].max(held[0] + sx * w));
                        (y0, y1) = (held[1].min(held[1] + sy * h), held[1].max(held[1] + sy * h));
                    }
                } else {
                    match k {
                        0 => y0 = q[1].min(hh - 1.0),
                        1 => x1 = q[0].max(-hw + 1.0),
                        2 => y1 = q[1].max(-hh + 1.0),
                        _ => x0 = q[0].min(hw - 1.0),
                    }
                    if let Some(r) = aspect {
                        // The other side follows, about the middle.
                        if k % 2 == 0 {
                            let w = (y1 - y0) * r;
                            (x0, x1) = (-w * 0.5, w * 0.5);
                        } else {
                            let h = (x1 - x0) / r;
                            (y0, y1) = (-h * 0.5, h * 0.5);
                        }
                    }
                }
                let c = s.frame().apply([(x0 + x1) * 0.5, (y0 + y1) * 0.5]);
                EditBox { center: c, size: [(x1 - x0).max(1.0), (y1 - y0).max(1.0)], angle: s.angle }
            }
        }
    }
}

/// The largest `w′ × h′` with `w′ / h′ = r` that the drag covers: the side that is too
/// long for the ratio grows the other.
fn fit_aspect(w: f64, h: f64, r: f64) -> (f64, f64) {
    if w / r >= h { (w, w / r) } else { (h * r, h) }
}

/// The box on screen, in points.
pub struct OnScreen {
    pub corners: [Pos2; 4],
}

impl OnScreen {
    pub fn edge(&self, k: usize) -> Pos2 {
        self.corners[k].lerp(self.corners[(k + 1) % 4], 0.5)
    }

    /// What a drag from `p` does.
    pub fn part_at(&self, p: Pos2) -> Part {
        if let Some(k) = (0..4).find(|&k| p.distance(self.corners[k]) <= GRAB) {
            return Part::Corner(k);
        }
        if let Some(k) = (0..4).find(|&k| p.distance(self.edge(k)) <= GRAB) {
            return Part::Edge(k);
        }
        let side = |a: Pos2, b: Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        let s: Vec<f32> = (0..4).map(|k| side(self.corners[k], self.corners[(k + 1) % 4])).collect();
        if s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0) {
            return Part::Move;
        }
        if self.corners.iter().any(|c| p.distance(*c) <= ROTATE_ZONE) { Part::Rotate } else { Part::New }
    }
}

/// The pointer for dragging `part` of a box.
pub fn cursor(part: Part) -> CursorIcon {
    match part {
        Part::Move => CursorIcon::Move,
        Part::New => CursorIcon::Crosshair,
        Part::Rotate => CursorIcon::Alias,
        Part::Corner(k) if k % 2 == 0 => CursorIcon::ResizeNwSe,
        Part::Corner(_) => CursorIcon::ResizeNeSw,
        Part::Edge(k) if k % 2 == 0 => CursorIcon::ResizeVertical,
        Part::Edge(_) => CursorIcon::ResizeHorizontal,
    }
}

/// Draws the handles of the box `screen` (its corners, and its sides' middles), outlined in
/// `accent`.
pub fn draw_handles(painter: &egui::Painter, screen: &OnScreen, accent: Color32) {
    for k in 0..4 {
        for p in [screen.corners[k], screen.edge(k)] {
            let r = Rect::from_center_size(p, Vec2::splat(8.0));
            painter.rect_filled(r, 1.0, Color32::WHITE);
            painter.rect_stroke(r, 1.0, Stroke::new(1.0, accent), egui::StrokeKind::Outside);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn corners_hold_the_opposite_corner_and_keep_a_ratio() {
        let b = EditBox { center: [50.0, 50.0], size: [100.0, 100.0], angle: 0.0 };
        let drag = BoxDrag { part: Part::Corner(2), from: [100.0, 100.0], start: b };
        let n = EditBox::dragged(&drag, [80.0, 60.0], None);
        assert!(close(n.corners()[0], [0.0, 0.0]) && close(n.corners()[2], [80.0, 60.0]));
        let n = EditBox::dragged(&drag, [80.0, 60.0], Some(2.0));
        assert!((n.size[0] / n.size[1] - 2.0).abs() < 1e-9, "{n:?}");
        assert!(close(n.corners()[0], [0.0, 0.0]));
    }

    #[test]
    fn a_box_s_numbers_are_its_turned_top_left_corner_size_and_angle() {
        let b = EditBox { center: [50.0, 40.0], size: [60.0, 20.0], angle: std::f64::consts::FRAC_PI_6 };
        let origin = [10.0, -5.0];
        let p = b.numbers(origin, [0.0, 0.0]);
        let [x, y] = b.corners()[0];
        assert!(close([p.x, p.y], [x - 10.0, y + 5.0]), "{p:?}");
        assert!((p.angle - 30.0).abs() < 1e-9 && p.width == 60.0 && p.height == 20.0);
        let back = EditBox::from_numbers(&p, origin, [0.0, 0.0]);
        assert!(close(back.center, b.center) && (back.angle - b.angle).abs() < 1e-12, "{back:?}");
        // Typing a new angle turns it about that corner, which stays where it was.
        let turned = EditBox::from_numbers(&crate::transform::Params { angle: -45.0, ..p }, origin, [0.0, 0.0]);
        assert!(close(turned.corners()[0], b.corners()[0]), "{turned:?}");
        // At its center: the center is given, and a size typed keeps it where it is.
        let p = b.numbers(origin, [0.5, 0.5]);
        assert!(close([p.x, p.y], [40.0, 45.0]), "{p:?}");
        let wider = EditBox::from_numbers(&crate::transform::Params { width: 90.0, ..p }, origin, [0.5, 0.5]);
        assert!(close(wider.center, b.center) && wider.size == [90.0, 20.0], "{wider:?}");
    }

    #[test]
    fn an_upright_box_is_put_on_the_pixels_edges_and_a_shape_drawn_in_it() {
        let b = EditBox { center: [50.2, 40.7], size: [20.6, 9.2], angle: 0.0 };
        // Each side to the nearest edge: 39.9..60.5 across, 36.1..45.3 down.
        let sides = b.on_pixels(false);
        assert_eq!((sides.center, sides.size), ([50.5, 40.5], [21.0, 9.0]));
        // Moved, its size whole and kept: its corner (39.9, 36.1) to (40, 36).
        let moved = b.on_pixels(true);
        assert_eq!((moved.center, moved.size), ([50.5, 40.5], [21.0, 9.0]));
        let small = EditBox { center: [5.0, 5.0], size: [0.2, 0.2], angle: 0.0 }.on_pixels(false);
        assert_eq!(small.size, [1.0, 1.0], "less than a pixel");
        let turned = EditBox { angle: 0.3, ..b };
        assert_eq!(turned.on_pixels(false), turned, "a turned box was moved");
        // A shape drawn in its box and back is the same shape, its kind and edge kept.
        let s = Shape {
            kind: opal_core::mask::ShapeKind::Ellipse,
            center: [3.0, 4.0],
            size: [10.0, 6.0],
            angle: 30.0,
            smooth: false,
        };
        let back = EditBox::of_shape(&s).shape(&s);
        assert!((back.angle - 30.0).abs() < 1e-12 && back.center == s.center && back.size == s.size, "{back:?}");
        assert_eq!((back.kind, back.smooth), (s.kind, s.smooth));
    }
}
