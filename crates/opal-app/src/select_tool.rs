//! The Select tool (design.md, *Selections*): a drag across the canvas selects a rectangle or
//! an ellipse, its edges on whole pixels, combined with what is selected as the keys held when
//! it began, or the tool's setting, say.

use crate::doc::DocState;
use crate::settings::Settings;
use eframe::egui::{self, Key, PointerButton, Pos2, Rect, Response};
use opal_core::mask::{Combine, Shape, ShapeKind};

/// What the tool's settings say.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub kind: ShapeKind,
    pub combine: Combine,
    pub smooth: bool,
}

impl Options {
    /// The options `settings` (the Select tool's) give.
    pub fn of(settings: &Settings) -> Options {
        let kind = if settings.choice("select.shape") == "ellipse" { ShapeKind::Ellipse } else { ShapeKind::Rectangle };
        let combine = match settings.choice("select.combine").as_str() {
            "add" => Combine::Add,
            "subtract" => Combine::Subtract,
            "intersect" => Combine::Intersect,
            _ => Combine::Replace,
        };
        Options { kind, combine, smooth: settings.bool("select.smooth") }
    }
}

/// A drag of the Select tool.
#[derive(Clone, Copy, Debug)]
pub struct SelectDrag {
    /// Where it began and where it is, on the pixels' corners, in document pixels.
    pub from: [f64; 2],
    pub to: [f64; 2],
    /// How what it draws combines with what is selected (chosen as it began).
    pub combine: Combine,
    /// Whether it is a square (or circle), and drawn from its middle: Shift and Alt, pressed
    /// once it has begun.
    pub square: bool,
    pub centered: bool,
    /// Whether Shift and Alt have been held since it began (choosing how it combines, not
    /// yet squaring or centering it).
    shift_held: bool,
    alt_held: bool,
}

impl SelectDrag {
    /// The shape it draws, of `kind`, smoothed if `smooth`.
    pub fn shape(&self, kind: ShapeKind, smooth: bool) -> Shape {
        let (a, b) = (self.from, self.to);
        let mut d = [b[0] - a[0], b[1] - a[1]];
        if self.square {
            let m = d[0].abs().max(d[1].abs());
            d = [m.copysign(d[0]), m.copysign(d[1])];
        }
        let (center, size) = if self.centered {
            (a, [2.0 * d[0].abs(), 2.0 * d[1].abs()])
        } else {
            ([a[0] + d[0] / 2.0, a[1] + d[1] / 2.0], [d[0].abs(), d[1].abs()])
        };
        Shape { kind, center, size, angle: 0.0, smooth }
    }
}

/// A point on the nearest pixel corner.
fn on_corner(p: [f64; 2]) -> [f64; 2] {
    p.map(f64::round)
}

/// What the Select tool does with a frame of the canvas's `response` on document `d`:
/// `to_doc` turns a screen point into document pixels.
pub fn canvas(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    options: Options,
) {
    let modifiers = ctx.input(|i| i.modifiers);
    if d.select_drag.is_some() && ctx.input(|i| i.key_pressed(Key::Escape)) {
        d.select_drag = None;
        return;
    }
    // Ctrl+click picks a layer, as with every tool: no drag starts.
    if response.drag_started_by(PointerButton::Primary)
        && !modifiers.command
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        let from = on_corner(to_doc(origin));
        let combine = match (modifiers.shift, modifiers.alt) {
            (true, true) => Combine::Intersect,
            (true, false) => Combine::Add,
            (false, true) => Combine::Subtract,
            (false, false) => options.combine,
        };
        d.select_drag = Some(SelectDrag {
            from,
            to: from,
            combine,
            square: false,
            centered: false,
            shift_held: modifiers.shift,
            alt_held: modifiers.alt,
        });
    }
    if let Some(drag) = &mut d.select_drag {
        drag.shift_held &= modifiers.shift;
        drag.alt_held &= modifiers.alt;
        drag.square = modifiers.shift && !drag.shift_held;
        drag.centered = modifiers.alt && !drag.alt_held;
        if let Some(p) = response.interact_pointer_pos() {
            drag.to = on_corner(to_doc(p));
        }
    }
    // Let go: the shape is selected, unless it has no width or height (a click).
    if d.select_drag.is_some() && !ctx.input(|i| i.pointer.primary_down()) {
        let drag = d.select_drag.take().expect("checked");
        let shape = drag.shape(options.kind, options.smooth);
        if shape.size[0] >= 1.0 && shape.size[1] >= 1.0 {
            let made = opal_core::selection::with_shape(d.pixel_selection().map(|s| &**s), shape, drag.combine);
            let label = match options.kind {
                ShapeKind::Rectangle => "Rectangle selection",
                ShapeKind::Ellipse => "Ellipse selection",
            };
            d.set_pixel_selection(Some(made), label);
        }
        return;
    }
    // A click with nothing held selects nothing.
    if response.clicked() && !modifiers.any() && d.pixel_selection().is_some() {
        d.set_pixel_selection(None, "Deselect");
    }
}

/// While a drag goes on, the shape it would select, as marching ants: `to_screen` turns
/// document pixels into screen points.
pub fn draw(ui: &egui::Ui, drag: &SelectDrag, options: Options, rect: Rect, to_screen: &dyn Fn([f64; 2]) -> Pos2) {
    let shape = drag.shape(options.kind, options.smooth);
    let [cx, cy] = shape.center;
    let [w, h] = shape.size.map(|v| v / 2.0);
    let line: Vec<[f64; 2]> = match options.kind {
        ShapeKind::Rectangle => vec![[cx - w, cy - h], [cx + w, cy - h], [cx + w, cy + h], [cx - w, cy + h]],
        ShapeKind::Ellipse => (0..96)
            .map(|i| {
                let t = f64::from(i) / 96.0 * std::f64::consts::TAU;
                [cx + w * t.cos(), cy + h * t.sin()]
            })
            .collect(),
    };
    crate::canvas::draw_ants(ui, &[line], rect, to_screen, ui.ctx().input(|i| i.time));
}
