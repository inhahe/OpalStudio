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
use opal_core::selection::{MOST_POINTS, Step};

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

/// What a drag under way does to an outline drawn point by point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drag {
    /// From its last point: pulling the side to it into a curve.
    Pulling,
    /// From elsewhere: drawing a freehand stretch.
    Stretch,
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
    /// The drag under way, point by point.
    pub dragging: Option<Drag>,
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
            dragging: None,
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

    /// The side a point at `to` would add: straight, or a curve if the last point gives a
    /// handle onward.
    fn side_to(&self, to: [f64; 2]) -> Segment {
        match self.onward {
            Some(c1) => Segment::Curve { c1, c2: to, to },
            None => Segment::Line { to },
        }
    }

    /// Adds a side to `to` (see [`side_to`](Self::side_to)), unless it has as many points as an
    /// outline may; whether it did.
    fn add(&mut self, to: [f64; 2]) -> bool {
        if self.points() >= MOST_POINTS {
            return false;
        }
        self.segments.push(self.side_to(to));
        self.onward = None;
        true
    }

    /// Takes its last point back, with the handle it gave, and ends a drag under way; the
    /// point now last gives again the handle it gave the side taken back, if it gave one.
    /// Whether there was a point to take back (its first is not one).
    pub fn take_back(&mut self) -> bool {
        self.dragging = None;
        let gone = self.segments.pop();
        // A side pulled into a curve only at its far end begins at the point itself.
        self.onward = match gone {
            Some(Segment::Curve { c1, .. }) if c1 != self.last() => Some(c1),
            _ => None,
        };
        gone.is_some()
    }

    /// The outline it makes, closed, sharp if not `smooth`; `None` with fewer than three points.
    pub fn outline(&self, smooth: bool) -> Option<Outline> {
        (self.points() >= 3).then(|| Outline { start: self.start, segments: self.segments.clone(), smooth })
    }
}

/// Closes the outline being drawn on `d` and selects what it encloses, combined as it says (a
/// history state, `LABEL`), unless there is not memory enough for it (the status bar says so);
/// with fewer than three points, it is given up.
pub fn close(d: &mut DocState, smooth: bool) {
    let Some(o) = d.outlining.take() else { return };
    d.revision += 1;
    let Some(outline) = o.outline(smooth) else { return };
    if let Err(why) = d.select_with(Step::Outline { outline, combine: o.combine }, LABEL) {
        d.notice = Some(why);
    }
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
    // Ctrl+click picks a layer, as with every tool: it neither begins an outline nor adds to
    // one.
    let picking = modifiers.command;
    let Some(o) = d.outlining.as_mut() else {
        if picking {
            return;
        }
        let combine = crate::select_tool::combine_held(modifiers, options.combine);
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        {
            d.outlining = Some(Outlining::begun(to_doc(origin), combine, true));
            d.revision += 1;
        } else if response.clicked_by(PointerButton::Primary)
            // (Not the second click of a double-click, whose first closed one.)
            && !response.double_clicked_by(PointerButton::Primary)
            && !response.triple_clicked_by(PointerButton::Primary)
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
    let mut changed = false;
    if o.freehand {
        // Drawn while the button is held, closed once it is let go.
        if let Some(p) = pointer.filter(|_| held)
            && to_screen(o.last()).distance(p) >= STEP
        {
            changed |= o.add(to_doc(p));
        }
        if !held {
            return close(d, options.smooth);
        }
        if changed {
            d.revision += 1;
        }
        return;
    }
    // Point by point: a drag from the last point pulls a curve; elsewhere, a freehand stretch.
    if response.drag_started_by(PointerButton::Primary)
        && !picking
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        if near(o.last(), origin) && !o.segments.is_empty() {
            o.dragging = Some(Drag::Pulling);
        } else {
            o.dragging = Some(Drag::Stretch);
            o.add(to_doc(origin));
        }
        changed = true;
    }
    if let Some(p) = pointer.filter(|_| held) {
        let at = to_doc(p);
        match o.dragging {
            Some(Drag::Pulling) => {
                // The handle pulled from the last point goes on past it; the side to it bends
                // the other way, as smoothly through the point.
                let last = o.last();
                let back = [2.0 * last[0] - at[0], 2.0 * last[1] - at[1]];
                let from = o.segments.len().checked_sub(2).map_or(o.start, |i| o.segments[i].end());
                let side = o.segments.last_mut().expect("a drag pulls only a side there is");
                let c1 = match *side {
                    Segment::Curve { c1, .. } => c1,
                    Segment::Line { .. } => from,
                };
                let pulled = Segment::Curve { c1, c2: back, to: last };
                if *side != pulled || o.onward != Some(at) {
                    *side = pulled;
                    o.onward = Some(at);
                    changed = true;
                }
            }
            Some(Drag::Stretch) if to_screen(o.last()).distance(p) >= STEP => changed |= o.add(at),
            _ => {}
        }
    }
    if !held && o.dragging.take().is_some() {
        changed = true;
    }
    // A click: on the first point, closing it (or a double-click anywhere); else a point.
    if response.double_clicked() && !picking {
        return close(d, options.smooth);
    }
    if response.clicked_by(PointerButton::Primary)
        && !picking
        && let Some(p) = response.interact_pointer_pos()
    {
        if near(o.start, p) && o.points() >= 3 {
            return close(d, options.smooth);
        }
        changed |= o.add(to_doc(p));
    }
    if changed {
        d.revision += 1;
    }
}

/// The outline being drawn `o`, as marching ants (closed back to its first point, as it would
/// select), with the side a click would add to the pointer while drawn point by point, and its
/// first point marked in `accent`: `to_screen` turns document pixels into screen points.
pub fn draw(ui: &egui::Ui, o: &Outlining, accent: egui::Color32, rect: Rect, to_screen: &dyn Fn([f64; 2]) -> Pos2) {
    let seen = crate::canvas::draw_ants(ui, &[shown(o)], rect, to_screen, ui.ctx().input(|i| i.time));
    crate::canvas::march(ui.ctx(), seen);
    if !o.freehand {
        let painter = ui.painter().with_clip_rect(rect);
        painter.circle(to_screen(o.start), 4.0, egui::Color32::WHITE, egui::Stroke::new(1.5, accent));
    }
}

/// The points `o` is drawn through (document pixels): its own, and, while it is drawn point by
/// point with no drag under way, those of the side a click at the pointer would add.
fn shown(o: &Outlining) -> Vec<[f64; 2]> {
    let mut points = opal_core::outline::flattened(o.start, &o.segments);
    if !o.freehand
        && o.dragging.is_none()
        && let Some(p) = o.pointer
    {
        points.extend(opal_core::outline::flattened(o.last(), &[o.side_to(p)]).into_iter().skip(1));
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_a_point_back_gives_back_the_handle_and_the_side_to_the_pointer_is_the_one_a_click_adds() {
        let (a, b, c) = ([0.0, 0.0], [100.0, 0.0], [100.0, 100.0]);
        let mut o = Outlining::begun(a, Combine::Replace, false);
        o.add(b);
        // A handle pulled at B: the side to C is a curve from it; taken back, B gives it again.
        o.onward = Some([150.0, 0.0]);
        o.add(c);
        assert!(matches!(o.segments[1], Segment::Curve { c1: [150.0, 0.0], .. }));
        assert!(o.take_back());
        assert_eq!(o.onward, Some([150.0, 0.0]), "the handle pulled at B was lost");
        // While B gives a handle, the side shown to the pointer is the curve a click would add.
        o.pointer = Some(c);
        let curve = opal_core::outline::flattened(b, &[Segment::Curve { c1: [150.0, 0.0], c2: c, to: c }]);
        assert!(curve.len() > 2);
        assert!(shown(&o).ends_with(&curve[1..]), "the side shown is not the curve: {:?}", shown(&o));
        // A side pulled into a curve at its far end only (it begins at its first point): taken
        // back, nothing is given onward.
        o.onward = None;
        o.add(c);
        o.segments[1] = Segment::Curve { c1: b, c2: [120.0, 80.0], to: c };
        assert!(o.take_back());
        assert_eq!(o.onward, None);
        // A drag under way ends with the point it pulled from taken back.
        o.dragging = Some(Drag::Pulling);
        assert!(o.take_back());
        assert_eq!((o.dragging, o.segments.len()), (None, 0));
        assert!(!o.take_back(), "the first point was taken back");
    }
}
