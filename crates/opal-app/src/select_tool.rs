//! The Select tool (design.md, *Selections*, *The Select tool*): a drag across the canvas
//! selects a rectangle or an ellipse, its edges on whole pixels, or a click or a drag a single
//! row or column of pixels across the canvas, combined with what is selected as the keys held
//! when it began, or the tool's setting, say. The selection's last shape has a box round it
//! whose handles move, size and turn it, and whose numbers (`box_fields::selection`) give it
//! exactly.

use crate::doc::{DocId, DocState};
use crate::edit_box::{BoxDrag, EditBox, OnScreen, Part};
use crate::settings::Settings;
use crate::state::AppState;
use eframe::egui::{self, Key, PointerButton, Pos2, Rect, Response, Stroke};
use opal_core::geom::IRect;
use opal_core::mask::{Combine, Shape, ShapeKind};
use opal_core::selection::{Selection, Step};
use std::sync::Arc;

/// What the tool selects: a shape drawn by a drag, or a single row or column of pixels across
/// the canvas, where the pointer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draws {
    Shape(ShapeKind),
    Row,
    Column,
}

/// What the tool's settings say.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub draws: Draws,
    pub combine: Combine,
    pub smooth: bool,
}

impl Options {
    /// The options `settings` (the Select tool's) give.
    pub fn of(settings: &Settings) -> Options {
        let draws = match settings.choice("select.shape").as_str() {
            "ellipse" => Draws::Shape(ShapeKind::Ellipse),
            "row" => Draws::Row,
            "column" => Draws::Column,
            _ => Draws::Shape(ShapeKind::Rectangle),
        };
        let combine = Combine::named(&settings.choice("select.combine")).unwrap_or_default();
        Options { draws, combine, smooth: settings.bool("select.smooth") }
    }

    /// Whether it selects a row or a column, not a shape.
    fn line(&self) -> bool {
        !matches!(self.draws, Draws::Shape(_))
    }
}

/// A drag of the Select tool.
#[derive(Clone, Copy, Debug)]
pub struct SelectDrag {
    /// Where it began and where it is, on the pixels' corners, in document pixels: the nearest
    /// corner, for a shape; for a row or column, the corner of the pixel under the pointer.
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
    /// A drag begun at `from` (see [`SelectDrag::from`]) with `modifiers` held, combining as
    /// `combine`.
    fn begun(from: [f64; 2], combine: Combine, modifiers: egui::Modifiers) -> SelectDrag {
        SelectDrag {
            from,
            to: from,
            combine,
            square: false,
            centered: false,
            shift_held: modifiers.shift,
            alt_held: modifiers.alt,
        }
    }

    /// The shape it selects with the tool's `options`, on an image whose canvas is `canvas`:
    /// for a row or column, the one it is on now, across the canvas (the nearest one in it).
    pub fn shape(&self, options: Options, canvas: &IRect) -> Shape {
        let kind = match options.draws {
            Draws::Shape(kind) => kind,
            Draws::Row => {
                let y = (self.to[1] as i64).clamp(canvas.y0, canvas.y1 - 1);
                return Shape::rectangle(&IRect::new(canvas.x0, y, canvas.x1, y + 1).to_f(), false);
            }
            Draws::Column => {
                let x = (self.to[0] as i64).clamp(canvas.x0, canvas.x1 - 1);
                return Shape::rectangle(&IRect::new(x, canvas.y0, x + 1, canvas.y1).to_f(), false);
            }
        };
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
        Shape { kind, center, size, angle: 0.0, smooth: options.smooth }
    }
}

/// A drag on the box round the selection's last shape, which edits that step.
#[derive(Clone, Debug)]
pub struct ShapeDrag {
    pub drag: BoxDrag,
    /// The shape as it was (its kind and smoothing are kept), and how it combines.
    shape: Shape,
    combine: Combine,
    /// What the steps before it select.
    before: Selection,
}

impl ShapeDrag {
    /// A drag of the box round `shape`, moving it, for tests.
    #[cfg(test)]
    pub(crate) fn testing(shape: Shape) -> ShapeDrag {
        let start = EditBox::of_shape(&shape);
        ShapeDrag {
            drag: BoxDrag { part: Part::Move, from: shape.center, start },
            shape,
            combine: Combine::Replace,
            before: Selection::of(Vec::new()),
        }
    }

    /// The history state a drag of `part` makes.
    pub(crate) fn label(part: Part) -> &'static str {
        match part {
            Part::Move => "Move selection",
            Part::Rotate => "Rotate selection",
            _ => "Resize selection",
        }
    }
}

/// Makes `shape` the selection's last step in place of the one there, which combined as
/// `combine` with what `before` selects, as the pending edit labeled `label`, unless there is
/// not memory enough for it (the status bar says so, and the selection stays as it was). A
/// selection that comes to nothing so keeps its steps while the edit goes on, so that the box
/// stays to be put right (once done, nothing is selected: `DocState::commit_pending`).
pub fn reshape(d: &mut DocState, before: &Selection, shape: Shape, combine: Combine, label: &str) {
    match crate::select::within_memory(|b| before.then_within(Step::Shape { shape, combine }, b)) {
        Ok(made) => {
            let made = Arc::new(made);
            d.edit_pending(label, |doc| doc.selection = Some(made));
        }
        Err(why) => d.notice = Some(why),
    }
}

/// Whether a drag begun now, with `modifiers` held and the tool's `options`, may edit the box
/// round the last shape: only when it would not combine a new one with what is selected,
/// and the tool draws shapes (round a row or column, a pixel thin, the box would catch the
/// presses meant for the next one).
fn edits_box(modifiers: egui::Modifiers, options: Options) -> bool {
    !modifiers.any() && options.combine == Combine::Replace && !options.line()
}

/// How what is drawn or picked with the keys `held` combines with what is selected: Shift
/// adds it, Alt takes it away, both keep what is in both; neither, `otherwise` (a tool's
/// setting, say). The Select and Outline tools' way, and a thumbnail's Ctrl+click's.
pub fn combine_held(held: egui::Modifiers, otherwise: Combine) -> Combine {
    match (held.shift, held.alt) {
        (true, true) => Combine::Intersect,
        (true, false) => Combine::Add,
        (false, true) => Combine::Subtract,
        (false, false) => otherwise,
    }
}

/// The history state selecting what `draws` makes.
pub fn label(draws: Draws) -> &'static str {
    match draws {
        Draws::Shape(ShapeKind::Rectangle) => "Rectangle selection",
        Draws::Shape(ShapeKind::Ellipse) => "Ellipse selection",
        Draws::Row => "Single row selection",
        Draws::Column => "Single column selection",
    }
}

/// Selects what `drag` selects on document `d` with the tool's `options`, unless that has no
/// width or height, or there is not memory enough for it (the status bar says so).
fn select(d: &mut DocState, drag: &SelectDrag, options: Options) {
    let shape = drag.shape(options, &d.doc().canvas.rect());
    if shape.size[0] >= 1.0
        && shape.size[1] >= 1.0
        && let Err(why) = d.select_with(Step::Shape { shape, combine: drag.combine }, label(options.draws))
    {
        d.notice = Some(why);
    }
}

/// The selection's last shape, if it has one, and how it combines: what its box edits.
pub fn editable(d: &DocState) -> Option<(Shape, Combine)> {
    d.pixel_selection()?.last_shape()
}

/// A point on the nearest pixel corner.
fn on_corner(p: [f64; 2]) -> [f64; 2] {
    p.map(f64::round)
}

/// What the Select tool does with a frame of the canvas's `response` on document `d`:
/// `to_doc` turns a screen point into document pixels, and `to_screen` back.
pub fn canvas(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
    options: Options,
) {
    let modifiers = ctx.input(|i| i.modifiers);
    // (A drag on an image no longer drawn is given up by `AppState::leave_undrawn`.)
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    if d.select_drag.is_some() && escape {
        d.select_drag = None;
        return;
    }
    if d.shape_drag.is_some() && escape {
        let s = d.shape_drag.take().expect("checked");
        d.discard_pending(ShapeDrag::label(s.drag.part));
        return;
    }
    // A drag on the box round the last shape edits it, unless what a drag does now is
    // combine a new one with what is selected (keys held, or the tool's setting).
    if response.drag_started_by(PointerButton::Primary)
        && edits_box(modifiers, options)
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        && let Some((shape, combine)) = editable(d)
    {
        let start = EditBox::of_shape(&shape);
        let part = OnScreen { corners: start.corners().map(to_screen) }.part_at(origin);
        if part != Part::New {
            let before = d.pixel_selection().expect("one with a shape").before_last();
            let drag = BoxDrag { part, from: to_doc(origin), start };
            d.shape_drag = Some(ShapeDrag { drag, shape, combine, before });
        }
    }
    if let Some(s) = d.shape_drag.clone() {
        if !ctx.input(|i| i.pointer.primary_down()) {
            // Let go: the edit is a history state of its own.
            d.shape_drag = None;
            if d.pending_label() == Some(ShapeDrag::label(s.drag.part)) {
                d.commit_pending();
            }
        } else if let Some(p) = response.interact_pointer_pos() {
            // Shift keeps its proportions; upright, its sides stay on the pixels' edges.
            let keep = modifiers.shift.then(|| s.drag.start.size[0] / s.drag.start.size[1]);
            let b = EditBox::dragged(&s.drag, to_doc(p), keep).on_pixels(&s.drag.start, s.drag.part);
            let shape = b.shape(&s.shape);
            if editable(d).map(|(now, _)| now) != Some(shape) {
                reshape(d, &s.before, shape, s.combine, ShapeDrag::label(s.drag.part));
            }
        }
        return;
    }
    let combine = combine_held(modifiers, options.combine);
    let point = |p: Pos2| if options.line() { to_doc(p).map(f64::floor) } else { on_corner(to_doc(p)) };
    // Ctrl+click picks a layer, as with every tool: no drag starts.
    if response.drag_started_by(PointerButton::Primary)
        && !modifiers.command
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        d.select_drag = Some(SelectDrag::begun(point(origin), combine, modifiers));
    }
    if let Some(drag) = &mut d.select_drag {
        drag.shift_held &= modifiers.shift;
        drag.alt_held &= modifiers.alt;
        drag.square = modifiers.shift && !drag.shift_held;
        drag.centered = modifiers.alt && !drag.alt_held;
        if let Some(p) = response.interact_pointer_pos() {
            drag.to = point(p);
        }
    }
    // Let go: what it draws is selected, unless it has no width or height (a click).
    if d.select_drag.is_some() && !ctx.input(|i| i.pointer.primary_down()) {
        let drag = d.select_drag.take().expect("checked");
        select(d, &drag, options);
        return;
    }
    // A click selects the row or column there; with a shape, and nothing held, nothing.
    if options.line()
        && response.clicked_by(PointerButton::Primary)
        && !modifiers.command
        && let Some(p) = response.interact_pointer_pos()
    {
        select(d, &SelectDrag::begun(point(p), combine, modifiers), options);
    } else if !options.line() && response.clicked() && !modifiers.any() {
        d.deselect_pixels();
    }
}

/// The box round the selection's last shape on document `id`'s canvas (`rect` on screen),
/// with its handles and the button that shows its numbers beside it, unless a new shape is
/// being drawn; the pointer shows what a drag from it would do. `to_screen` turns document
/// pixels into screen points.
pub fn draw_box(
    ui: &egui::Ui,
    st: &mut AppState,
    id: DocId,
    response: &Response,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let ctx = ui.ctx().clone();
    let accent = st.palette.accent;
    let mut numbers = st.box_numbers;
    let options = Options::of(st.tool_settings(crate::tools::ToolId::Select));
    let Some(d) = st.doc(id) else { return };
    // Not round a row or column: see `edits_box`.
    let Some((shape, _)) = editable(d).filter(|_| d.select_drag.is_none() && !options.line()) else { return };
    let corners = EditBox::of_shape(&shape).corners().map(to_screen);
    let screen = OnScreen { corners };
    let painter = ui.painter().with_clip_rect(rect);
    painter.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(1.0, accent.gamma_multiply(0.6))));
    crate::edit_box::draw_handles(&painter, &screen, accent);
    let dragging = d.shape_drag.as_ref().map(|s| s.drag.part);
    let hovered = response.hover_pos().filter(|_| edits_box(ctx.input(|i| i.modifiers), options));
    if let Some(part) = dragging.or_else(|| hovered.map(|p| screen.part_at(p))) {
        // Elsewhere, or with a drag combining a new shape, the tool's own pointer.
        if part != Part::New {
            ctx.set_cursor_icon(crate::edit_box::cursor(part));
        }
    }
    // The numbers button under the box.
    let bar_id = egui::Id::unique(("select-toolbar", id.0));
    let at = crate::edit_box::toolbar_at(&ctx, rect, &corners, &[], bar_id, 40.0);
    let bar = egui::Area::new(bar_id).order(egui::Order::Foreground).fixed_pos(at).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| crate::box_fields::toggle(ui, &mut numbers));
    });
    st.box_numbers = numbers;
    if numbers {
        let panel = egui::Id::unique(("select-numbers", id.0));
        let around = [corners.as_slice(), &[bar.response.rect.min, bar.response.rect.max]].concat();
        crate::box_fields::panel_by(&ctx, panel, rect, &around, |ui| {
            crate::box_fields::selection(ui, st, id, "select-by-box");
        });
    }
}

/// While a drag goes on, the shape it would select on an image whose canvas is `canvas`, as
/// marching ants: `to_screen` turns document pixels into screen points.
pub fn draw(
    ui: &egui::Ui,
    drag: &SelectDrag,
    options: Options,
    canvas: &IRect,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let shape = drag.shape(options, canvas);
    let [cx, cy] = shape.center;
    let [w, h] = shape.size.map(|v| v / 2.0);
    let line: Vec<[f64; 2]> = match shape.kind {
        ShapeKind::Rectangle => vec![[cx - w, cy - h], [cx + w, cy - h], [cx + w, cy + h], [cx - w, cy + h]],
        ShapeKind::Ellipse => (0..96)
            .map(|i| {
                let t = f64::from(i) / 96.0 * std::f64::consts::TAU;
                [cx + w * t.cos(), cy + h * t.sin()]
            })
            .collect(),
    };
    let seen = crate::canvas::draw_ants(ui, &[line], rect, to_screen, ui.ctx().input(|i| i.time));
    crate::canvas::march(ui.ctx(), seen);
}
