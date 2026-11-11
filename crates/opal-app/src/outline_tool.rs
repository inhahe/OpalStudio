//! The outline tool (design.md, *The outline tool*): a loop drawn around what to select. A
//! drag draws it freehand, selected once the button is let go; a click begins one point by
//! point instead, each click a straight side to it, a drag from the last point pulling that
//! side into a curve, and a drag elsewhere a freehand stretch; Enter, a double-click or a
//! click on its first point closes it, Backspace takes the last point back, and Esc gives it
//! up. It combines with what is selected as the keys held as it begins, or the tool's
//! setting, say, as the Select tool's shapes do.

use crate::doc::DocState;
use crate::settings::Settings;
use eframe::egui::{self, PointerButton, Pos2, Rect, Response};
use opal_core::mask::Combine;
use opal_core::outline::{Outline, Segment};
use opal_core::selection::Step;

/// How near the first point (screen points) a click closes the loop, and how near the last
/// one a drag pulls a curve from it.
const NEAR: f32 = 7.0;

/// How far the pointer moves (screen points) between the points a freehand stretch keeps.
const STEP: f32 = 2.0;

/// The history state an outline selected makes.
pub const LABEL: &str = "Outline selection";

/// What the tool's settings say.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub combine: Combine,
    pub smooth: bool,
}

impl Options {
    /// The options `settings` (the outline tool's) give.
    pub fn of(settings: &Settings) -> Options {
        let combine = Combine::named(&settings.choice("outline.combine")).unwrap_or_default();
        Options { combine, smooth: settings.bool("outline.smooth") }
    }
}

/// An outline being drawn, in document pixels.
#[derive(Clone, Debug)]
pub struct Outlining {
    pub start: [f64; 2],
    pub segments: Vec<Segment>,
    /// How it combines with what is selected (chosen as it began).
    pub combine: Combine,
    /// Whether it is drawn freehand, closed once the button is let go (else point by point).
    pub freehand: bool,
    /// While a drag from the last point pulls the side to it into a curve: the handle pulled.
    pub pulling: Option<[f64; 2]>,
    /// While a drag elsewhere draws a freehand stretch.
    pub stretch: bool,
    /// The handle the last point gives the side after it (a curve pulled at it), if any.
    pub onward: Option<[f64; 2]>,
    /// Where the pointer is, to show the side to it.
    pub pointer: Option<[f64; 2]>,
}

impl Outlining {
    fn begun(at: [f64; 2], combine: Combine, freehand: bool) -> Outlining {
        Outlining {
            start: at,
            segments: Vec::new(),
            combine,
            freehand,
            pulling: None,
            stretch: false,
            onward: None,
            pointer: Some(at),
        }
    }

    /// Where its last point is.
    pub fn last(&self) -> [f64; 2] {
        self.segments.last().map_or(self.start, Segment::end)
    }

    /// Its points so far: the first, and each segment's end.
    fn points(&self) -> usize {
        self.segments.len() + 1
    }

    /// A side to `to`: straight, or a curve if the last point gives a handle onward.
    fn add(&mut self, to: [f64; 2]) {
        let side = match self.onward.take() {
            Some(c1) => Segment::Curve { c1, c2: to, to },
            None => Segment::Line { to },
        };
        self.segments.push(side);
    }

    /// Takes its last point back (and the handle it gave); whether any is left.
    pub fn take_back(&mut self) -> bool {
        self.onward = None;
        self.segments.pop().is_some()
    }

    /// The outline it makes, closed, sharp if not `smooth`; `None` with fewer than three points.
    pub fn outline(&self, smooth: bool) -> Option<Outline> {
        (self.points() >= 3).then(|| Outline { start: self.start, segments: self.segments.clone(), smooth })
    }
}

/// How what is drawn combines with what is selected: by the keys held as it begins, or the
/// tool's setting.
fn combine_of(modifiers: egui::Modifiers, options: Options) -> Combine {
    match (modifiers.shift, modifiers.alt) {
        (true, true) => Combine::Intersect,
        (true, false) => Combine::Add,
        (false, true) => Combine::Subtract,
        (false, false) => options.combine,
    }
}

/// Closes the outline being drawn on `d` and selects what it encloses, combined as it says (a
/// history state, `LABEL`); with fewer than three points, it is given up.
pub fn close(d: &mut DocState, smooth: bool) {
    let Some(o) = d.outlining.take() else { return };
    d.revision += 1;
    let Some(outline) = o.outline(smooth) else { return };
    let step = Step::Outline { outline, combine: o.combine };
    let made = opal_core::selection::with_maker(d.pixel_selection().map(|s| &**s), step);
    d.set_pixel_selection(Some(made), LABEL);
}

/// What the outline tool does with a frame of the canvas's `response` on document `d`:
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
    let held = ctx.input(|i| i.pointer.primary_down());
    let pointer = response.hover_pos().or_else(|| response.interact_pointer_pos());
    let near = |a: [f64; 2], b: Pos2| to_screen(a).distance(b) <= NEAR;
    let Some(o) = d.outlining.as_mut() else {
        // Ctrl+click picks a layer, as with every tool.
        if modifiers.command {
            return;
        }
        let combine = combine_of(modifiers, options);
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        {
            d.outlining = Some(Outlining::begun(to_doc(origin), combine, true));
            d.revision += 1;
        } else if response.clicked_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            d.outlining = Some(Outlining::begun(to_doc(p), combine, false));
            d.revision += 1;
        }
        return;
    };
    // (The pointer moving brings a frame of its own: only a change to the outline itself is
    // the image's.)
    o.pointer = pointer.map(to_doc);
    let had = (o.segments.clone(), o.pulling);
    if o.freehand {
        // Drawn while the button is held, closed once it is let go.
        if let Some(p) = pointer.filter(|_| held)
            && to_screen(o.last()).distance(p) >= STEP
        {
            o.add(to_doc(p));
            d.revision += 1;
        }
        if !held {
            return close(d, options.smooth);
        }
        return;
    }
    // Point by point: a drag from the last point pulls a curve; elsewhere, a freehand stretch.
    if response.drag_started_by(PointerButton::Primary)
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        if near(o.last(), origin) && !o.segments.is_empty() {
            o.pulling = Some(o.last());
        } else {
            o.stretch = true;
            o.add(to_doc(origin));
        }
    }
    if let Some(p) = pointer.filter(|_| held) {
        let at = to_doc(p);
        if o.pulling.is_some() {
            // The handle pulled from the last point goes on past it; the side to it bends the
            // other way, as smoothly through the point.
            let last = o.last();
            let back = [2.0 * last[0] - at[0], 2.0 * last[1] - at[1]];
            let from = o.segments.len().checked_sub(2).map_or(o.start, |i| o.segments[i].end());
            let c1 = match o.segments.last() {
                Some(Segment::Curve { c1, .. }) => *c1,
                _ => from,
            };
            *o.segments.last_mut().expect("a side to the last point") = Segment::Curve { c1, c2: back, to: last };
            o.pulling = Some(at);
            o.onward = Some(at);
        } else if o.stretch && to_screen(o.last()).distance(p) >= STEP {
            o.add(at);
        }
    }
    if !held {
        o.pulling = None;
        o.stretch = false;
    }
    // A click: on the first point, closing it (or a double-click anywhere); else a point.
    if response.double_clicked() {
        return close(d, options.smooth);
    }
    if response.clicked_by(PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
    {
        if near(o.start, p) && o.points() >= 3 {
            return close(d, options.smooth);
        }
        o.add(to_doc(p));
    }
    if (&o.segments, o.pulling) != (&had.0, had.1) {
        d.revision += 1;
    }
}

/// The outline being drawn `o`, as marching ants (closed back to its first point, as it would
/// select), with the side to the pointer while drawn point by point, and its first point marked
/// in `accent`: `to_screen` turns document pixels into screen points.
pub fn draw(
    ui: &egui::Ui,
    o: &Outlining,
    smooth: bool,
    accent: egui::Color32,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let mut drawn = Outlining { onward: None, ..o.clone() };
    if !o.freehand
        && o.pulling.is_none()
        && !o.stretch
        && let Some(p) = o.pointer
    {
        drawn.add(p);
    }
    let points = Outline { start: drawn.start, segments: drawn.segments.clone(), smooth }.polygon();
    let seen = crate::canvas::draw_ants(ui, &[points], rect, to_screen, ui.ctx().input(|i| i.time));
    crate::canvas::march(ui.ctx(), seen);
    if !o.freehand {
        let painter = ui.painter().with_clip_rect(rect);
        painter.circle(to_screen(o.start), 4.0, egui::Color32::WHITE, egui::Stroke::new(1.5, accent));
    }
}
