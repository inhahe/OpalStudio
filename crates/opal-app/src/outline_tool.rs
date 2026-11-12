//! The outline tool (design.md, *The outline tool*): a loop drawn around what to select. A
//! drag draws it freehand, selected once the button is let go; a click begins one point by
//! point instead, each click a straight side to it, a drag from the last point pulling that
//! side into a curve, and a drag elsewhere a freehand stretch; Enter, a double-click or a
//! click on its first point closes it, Backspace takes the last point back, and Esc gives it
//! up. It combines with what is selected as the keys held as it begins, or the tool's
//! setting, say, as the Select tool's shapes do.

use crate::doc::{DocState, Drag};
use crate::settings::Settings;
use eframe::egui::{self, PointerButton, Pos2, Rect, Response};
use opal_core::geom::Affine;
use opal_core::mask::Combine;
use opal_core::outline::{End, Join, Node, Outline, Segment};
use opal_core::selection::{MOST_POINTS, Step};
use opal_core::symmetry::{MOST_ORDER, Symmetry};
use std::sync::Arc;

/// How far the pointer moves (screen points) between the points a freehand stretch keeps.
const STEP: f32 = 2.0;

/// The history state an outline selected makes.
pub const LABEL: &str = "Outline selection";

/// What the tool's settings say.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub combine: Combine,
    pub smooth: bool,
    /// The symmetry an outline is drawn with: its order (1 for none) and whether it mirrors.
    pub order: u32,
    pub mirror: bool,
    /// How near a press must be (screen points) to a point, a handle, a side or the symmetry's
    /// handles to be on it.
    pub near: f32,
}

impl Options {
    /// The options `settings` (the outline tool's) give.
    pub fn of(settings: &Settings) -> Options {
        let combine = Combine::named(&settings.choice("outline.combine")).unwrap_or_default();
        let order = settings.f64("outline.symmetry").round().clamp(1.0, f64::from(MOST_ORDER)) as u32;
        let near = settings.f64("outline.near") as f32;
        Options {
            combine,
            smooth: settings.bool("outline.smooth"),
            order,
            mirror: settings.bool("outline.mirror"),
            near,
        }
    }
}

/// The symmetry an outline drawn on `d` with `options` has, if it has one: about the center
/// and axis `d` keeps (the canvas's middle, and straight up, until moved).
pub fn symmetry(d: &DocState, options: Options) -> Option<Symmetry> {
    (options.order > 1 || options.mirror).then(|| {
        let middle = d.doc().canvas.rect().to_f().center();
        Symmetry {
            order: options.order,
            mirror: options.mirror,
            center: d.symmetry_center.unwrap_or(middle),
            angle: d.symmetry_angle,
        }
    })
}

/// What a drag under way does to an outline drawn point by point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawing {
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
    pub dragging: Option<Drawing>,
    /// The handle the last point gives the side after it (a curve pulled at it), if any.
    pub onward: Option<[f64; 2]>,
    /// Where the pointer is, to show the side to it.
    pub pointer: Option<[f64; 2]>,
    /// The symmetry it is drawn with (as it began), if any: then it is one slice of the
    /// outline, which its copies complete.
    pub symmetry: Option<Symmetry>,
}

impl Outlining {
    fn begun(at: [f64; 2], combine: Combine, freehand: bool, symmetry: Option<Symmetry>) -> Outlining {
        Outlining {
            start: at,
            segments: Vec::new(),
            combine,
            freehand,
            dragging: None,
            onward: None,
            pointer: Some(at),
            symmetry,
        }
    }

    /// The fewest points it may be closed with: three, or, as a symmetric outline's slice,
    /// two.
    fn fewest(&self) -> usize {
        self.sketch().fewest_points()
    }

    /// An outline of its start alone, with its symmetry: what its rules are read from.
    fn sketch(&self) -> Outline {
        Outline { symmetry: self.symmetry, ..Outline::new(self.start, Vec::new(), true) }
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
        // (Its copies counted, as its whole outline has them.)
        if self.sketch().whole_size(self.segments.len() + 1) >= MOST_POINTS {
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

    /// The outline it makes, closed (or, with a symmetry, completed by the slice's copies),
    /// sharp if not `smooth`; `None` with too few points (see [`fewest`](Self::fewest)).
    pub fn outline(&self, smooth: bool) -> Option<Outline> {
        (self.points() >= self.fewest())
            .then(|| Outline { symmetry: self.symmetry, ..Outline::new(self.start, self.segments.clone(), smooth) })
    }
}

/// Closes the outline being drawn on `d` and selects what it encloses, combined as it says (a
/// history state, `LABEL`), unless there is not memory enough for it (the status bar says so);
/// with too few points, it is given up.
pub fn close(d: &mut DocState, smooth: bool) {
    let Some(o) = d.outlining.take() else { return };
    d.revision += 1;
    let Some(outline) = o.outline(smooth) else { return };
    if let Err(why) = d.select_with(Step::Outline { outline, combine: o.combine }, LABEL) {
        d.notice = Some(why);
    }
}

/// How far (screen points) from the symmetry's center the handle that turns its axis is.
const AXIS_HANDLE: f32 = 60.0;

/// Where the center of `s` and the handle turning its axis are on screen.
fn guide_handles(s: &Symmetry, to_screen: &dyn Fn([f64; 2]) -> Pos2) -> (Pos2, Pos2) {
    let center = to_screen(s.center);
    (center, center + on_screen(s.center, s.axis(), to_screen) * AXIS_HANDLE)
}

/// Which way on screen the canvas direction `dir` at canvas point `at` goes, as `to_screen`
/// maps the canvas (a view may be turned): through two points far apart, whose difference
/// keeps its precision.
fn on_screen(at: [f64; 2], dir: [f64; 2], to_screen: &dyn Fn([f64; 2]) -> Pos2) -> egui::Vec2 {
    const FAR: f64 = 1e3;
    (to_screen([at[0] + dir[0] * FAR, at[1] + dir[1] * FAR]) - to_screen(at)).normalized()
}

/// The axis angle (degrees, clockwise from up, as `Symmetry::angle` is) pointing from canvas
/// point `center` to canvas point `to`.
fn axis_angle(center: [f64; 2], to: [f64; 2]) -> f64 {
    (to[0] - center[0]).atan2(-(to[1] - center[1])).to_degrees()
}

/// Whose symmetry the canvas shows (see [`shown_symmetry`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The outline being drawn's, as it began: shown, not moved.
    Drawn,
    /// The outline edited's (the selection's last step, a slice): moving it changes that
    /// outline, as a history state.
    Edited,
    /// The one an outline drawn now would have (the tool's settings, about the image's center
    /// and axis).
    New,
}

/// The history state moving or turning the symmetry of the outline edited makes.
pub const MOVE_SYMMETRY: &str = "Move symmetry";

/// The symmetry the canvas shows for `d` with the tool's `options`, and whose it is: the
/// outline being drawn's, else the outline edited's if it is symmetric, else the one an
/// outline drawn now would have; none if that has none.
pub fn shown_symmetry(d: &DocState, options: Options) -> Option<(Symmetry, Shown)> {
    if let Some(o) = &d.outlining {
        return o.symmetry.map(|s| (s, Shown::Drawn));
    }
    if let Some((o, _)) = edited(d)
        && let Some(s) = o.symmetric()
    {
        return Some((s, Shown::Edited));
    }
    symmetry(d, options).map(|s| (s, Shown::New))
}

/// A drag of the symmetry's center (`center`) or its axis's handle; of the outline edited's,
/// that outline as it began, how it combines, and what the steps before it select.
#[derive(Clone, Debug)]
pub struct SymmetryDrag {
    center: bool,
    edited: Option<(Outline, Combine, opal_core::selection::Selection)>,
    /// The image's center and axis as it began, put back if it is given up.
    pub(crate) was: (Option<[f64; 2]>, f64),
}

/// What a frame's presses and drags on the canvas do to the symmetry shown (`shown`, see
/// [`shown_symmetry`]) on `d`: a drag from its center moves it, one from its axis's handle
/// turns the axis (Shift: by 15 degrees at a time); the outline edited's changes that outline
/// (a history state, pending while it goes), and the image's center and axis go with it, for
/// outlines drawn next. A press on a point of the outline edited, or a picked point's handle,
/// is the point's. Whether they were its.
fn guide(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    shown: (Symmetry, Shown),
    near: f32,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) -> bool {
    let (s, whose) = shown;
    if let Some(Drag::Symmetry(drag)) = d.drag.take_if(|g| matches!(g, Drag::Symmetry(_))) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            d.drag = Some(Drag::Symmetry(drag));
            d.give_up_drag();
            return true;
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            if d.pending_label() == Some(MOVE_SYMMETRY) {
                d.commit_pending();
            }
            return true;
        }
        if let Some(p) = response.interact_pointer_pos() {
            let (mut center, mut angle) = (s.center, s.angle);
            if drag.center {
                center = to_doc(p);
            } else {
                // In the canvas's own axes, however the view turns them.
                angle = axis_angle(s.center, to_doc(p));
                if ctx.input(|i| i.modifiers.shift) {
                    angle = (angle / 15.0).round() * 15.0;
                }
            }
            (d.symmetry_center, d.symmetry_angle) = (Some(center), angle);
            if let Some((start, combine, before)) = &drag.edited {
                let moved = start.symmetry.map(|sym| Symmetry { center, angle, ..sym });
                if moved != edited(d).and_then(|(o, _)| o.symmetry) {
                    let outline = Outline { symmetry: moved, ..start.clone() };
                    let step = Step::Outline { outline, combine: *combine };
                    crate::select_tool::reshape_with(d, before, step, MOVE_SYMMETRY);
                }
            }
        }
        d.drag = Some(Drag::Symmetry(drag));
        return true;
    }
    if whose == Shown::Drawn || !response.drag_started_by(PointerButton::Primary) {
        return false;
    }
    let Some(origin) = ctx.input(|i| i.pointer.press_origin()) else { return false };
    // A point of the outline edited, or a picked point's handle, under the press: the point's.
    if let Some((o, _)) = edited(d)
        && part_at(o, &o.nodes(), &picked(d), Reach { near, sides: false }, origin, to_doc, to_screen).is_some()
    {
        return false;
    }
    let (center, handle) = guide_handles(&s, to_screen);
    let on = if origin.distance(handle) <= near {
        false
    } else if origin.distance(center) <= near {
        true
    } else {
        return false;
    };
    let edited = (whose == Shown::Edited).then(|| {
        let (o, combine) = edited(d).expect("an outline edited");
        (o.clone(), combine, d.pixel_selection().expect("one with an outline").before_last())
    });
    let was = (d.symmetry_center, d.symmetry_angle);
    d.drag = Some(Drag::Symmetry(SymmetryDrag { center: on, edited, was }));
    true
}

/// Puts the center of the symmetry shown on `d` with `options` (see [`shown_symmetry`]) in
/// the canvas's middle, its axis straight up: the image's for outlines drawn next, and the
/// outline edited's, if it is shown, as a history state.
pub fn center_symmetry(d: &mut DocState, options: Options) {
    d.symmetry_center = None;
    d.symmetry_angle = 0.0;
    let Some((_, Shown::Edited)) = shown_symmetry(d, options) else { return };
    let Some((o, combine)) = edited(d).map(|(o, c)| (o.clone(), c)) else { return };
    let middle = d.doc().canvas.rect().to_f().center();
    let symmetry = o.symmetry.map(|s| Symmetry { center: middle, angle: 0.0, ..s });
    if symmetry != o.symmetry {
        let before = d.pixel_selection().expect("one with an outline").before_last();
        let step = Step::Outline { outline: Outline { symmetry, ..o }, combine };
        if let Ok(made) = crate::select::within_memory(|b| before.then_within(step, b)) {
            d.set_pixel_selection(Some(made), MOVE_SYMMETRY);
        }
    }
}

/// The symmetry `s` on `rect` (by `to_screen`): its seams (the axis turned round, and with a
/// mirror the mirror lines between), faint, the axis itself stronger, and the handles of its
/// center and its axis.
pub fn draw_symmetry(
    ui: &egui::Ui,
    s: &Symmetry,
    accent: egui::Color32,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let painter = ui.painter().with_clip_rect(rect);
    let (center, handle) = guide_handles(s, to_screen);
    let reach = rect.width() + rect.height() + center.distance(rect.center());
    let rays = s.copies();
    for k in 0..rays {
        let turned = (s.angle + 360.0 * k as f64 / rays as f64).to_radians();
        let dir = on_screen(s.center, [turned.sin(), -turned.cos()], to_screen);
        let stroke =
            if k == 0 { egui::Stroke::new(1.5, accent) } else { egui::Stroke::new(1.0, accent.gamma_multiply(0.45)) };
        painter.line_segment([center, center + dir * reach], stroke);
    }
    painter.circle(center, 5.0, egui::Color32::WHITE, egui::Stroke::new(1.5, accent));
    painter.rect(
        Rect::from_center_size(handle, egui::vec2(8.0, 8.0)),
        1.0,
        accent,
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Middle,
    );
}

/// The history state redrawing a stretch of the outline edited makes.
pub const RESHAPE: &str = "Reshape outline";

/// A right-drag (or one with R held, for touchpads) redrawing a stretch of the outline
/// edited.
#[derive(Clone, Debug)]
pub struct Reshaping {
    /// The outline as it was, how it combines, and what the steps before it select.
    start: Outline,
    combine: Combine,
    before: opal_core::selection::Selection,
    /// What carries the outline (a symmetric one's slice) to where the line is drawn: the copy
    /// it began on (itself, without symmetry), and back. The line redraws the slice as it lies
    /// on that copy.
    map: Affine,
    back: Affine,
    /// Where on the outline the drag began: a side, and how far along it.
    from: (usize, f64),
    /// The line drawn so far (document pixels, where it is drawn), from where it began.
    pub line: Vec<[f64; 2]>,
    /// Whether the other stretch between its ends is the one redrawn (Tab, which
    /// [`take_reshape_keys`] gives it).
    pub other: bool,
    /// The button it is drawn with.
    button: PointerButton,
    /// How near the outline (screen points) it must be let go to redraw it (the tool's reach as
    /// it began).
    near: f32,
    /// The selection it began on (weakly, compared by address): once it is another (undone or
    /// deselected meanwhile, say), the redrawing is given up.
    of: std::sync::Weak<opal_core::selection::Selection>,
    /// What letting go would make, as the line's length and `other` were when it was last
    /// worked out (see [`made`](Self::made)): shown while it is drawn.
    shown: Option<(Outline, Vec<[f64; 2]>)>,
    shown_for: (usize, bool),
}

impl Reshaping {
    /// Where its line ends on the outline (on the copy it began on), if its last point is near
    /// it (within its reach on screen): a side, and how far along it.
    fn landing(&self, to_screen: &dyn Fn([f64; 2]) -> Pos2) -> Option<(usize, f64)> {
        let last = *self.line.last()?;
        let (side, t, _) = self.start.nearest(self.back.apply(last))?;
        let on = self.map.apply(self.start.along(side, t));
        (to_screen(on).distance(to_screen(last)) <= self.near).then_some((side, t))
    }

    /// The outline it would make let go now, and the stretch it would replace (where it is
    /// drawn); none, if its line does not end on the outline.
    fn made(&self, to_screen: &dyn Fn([f64; 2]) -> Pos2) -> Option<(Outline, Vec<[f64; 2]>)> {
        let line: Vec<[f64; 2]> = self.line.iter().map(|&p| self.back.apply(p)).collect();
        let (outline, gone) = self.start.reshaping(self.from, self.landing(to_screen)?, &line, self.other)?;
        Some((outline, gone.into_iter().map(|p| self.map.apply(p)).collect()))
    }

    /// Works out again what letting go would make, if the line or `other` changed since.
    fn show(&mut self, to_screen: &dyn Fn([f64; 2]) -> Pos2) {
        let now = (self.line.len(), self.other);
        if self.shown_for != now {
            self.shown = self.made(to_screen);
            self.shown_for = now;
        }
    }
}

/// Tab, while a redrawing is under way on one of `docs`, flips which stretch it redraws: taken
/// out of `raw`, the frame's input, before egui sees it, which would move the keyboard focus to
/// a widget.
pub fn take_reshape_keys(docs: &mut [DocState], raw: &mut egui::RawInput) {
    let Some(r) = docs.iter_mut().find_map(|d| match &mut d.drag {
        Some(Drag::Reshape(r)) => Some(r),
        _ => None,
    }) else {
        return;
    };
    raw.events.retain(|e| match e {
        egui::Event::Key { key: egui::Key::Tab, pressed, repeat, modifiers, .. } if modifiers.is_none() => {
            if *pressed && !*repeat {
                r.other = !r.other;
            }
            false
        }
        _ => true,
    });
}

/// What a frame's right-drags (and drags with R held) do to the outline edited on `d`: from a
/// press on it (on a symmetric one, on any of its copies), a line drawn freehand, which, let
/// go on the outline (that copy), redraws the stretch between its ends (the one nearer it, or
/// with Tab the other), as a history state; let go away from it, or given up with Esc or by
/// the selection changing meanwhile, nothing (the status bar says why, let go away; and if the
/// outline would be larger than an outline may be, or select nothing). Whether they were its.
fn reshape(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    near: f32,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) -> bool {
    if let Some(Drag::Reshape(mut r)) = d.drag.take_if(|g| matches!(g, Drag::Reshape(_))) {
        if !d.pixel_selection().is_some_and(|s| std::sync::Weak::ptr_eq(&r.of, &Arc::downgrade(s))) {
            return true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return true;
        }
        if ctx.input(|i| i.pointer.button_down(r.button)) {
            if let Some(p) = response.interact_pointer_pos().or_else(|| response.hover_pos())
                && r.line.last().is_none_or(|&l| to_screen(l).distance(p) >= STEP)
            {
                r.line.push(to_doc(p));
            }
            r.show(to_screen);
            d.drag = Some(Drag::Reshape(r));
            return true;
        }
        // Let go: where it was let go the line's end (the last move and the release may come
        // in one frame), and on the outline, the stretch redrawn.
        if let Some(p) = ctx.input(|i| i.pointer.latest_pos())
            && r.line.last().is_none_or(|&l| to_screen(l).distance(p) >= LAST_STEP)
        {
            r.line.push(to_doc(p));
        }
        let_go(d, &r, to_screen);
        return true;
    }
    let button = if response.drag_started_by(PointerButton::Secondary) {
        PointerButton::Secondary
    } else if response.drag_started_by(PointerButton::Primary) && ctx.input(|i| i.key_down(egui::Key::R)) {
        PointerButton::Primary
    } else {
        return false;
    };
    let Some(origin) = ctx.input(|i| i.pointer.press_origin()) else { return false };
    let Some((outline, combine)) = edited(d).map(|(o, c)| (o.clone(), c)) else { return false };
    // The place nearest the press on the outline or any of its copies, and what carries the
    // outline there and back.
    let nearest = copies(&outline)
        .into_iter()
        .filter_map(|(map, back)| {
            let (side, t, _) = outline.nearest(back.apply(to_doc(origin)))?;
            let on = map.apply(outline.along(side, t));
            Some((to_screen(on).distance(origin), (side, t), on, map, back))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((far, from, on, map, back)) = nearest else { return false };
    if far > near {
        return false;
    }
    let s = d.pixel_selection().expect("one with an outline");
    let (before, of) = (s.before_last(), Arc::downgrade(s));
    d.drag = Some(Drag::Reshape(Box::new(Reshaping {
        start: outline,
        combine,
        before,
        map,
        back,
        from,
        line: vec![on],
        other: false,
        button,
        near,
        of,
        shown: None,
        shown_for: (1, false),
    })));
    true
}

/// What letting go of the redrawing `r` does on `d` (`to_screen` turning document pixels into
/// screen points): on the outline, the stretch redrawn, as a history state (if that selects
/// nothing, the status bar says so); away from it, or if the outline would be larger than an
/// outline may be, nothing, and the status bar says why.
fn let_go(d: &mut DocState, r: &Reshaping, to_screen: &dyn Fn([f64; 2]) -> Pos2) {
    let Some((outline, _)) = r.made(to_screen) else {
        d.notice = Some(LET_GO_ON_IT.to_owned());
        return;
    };
    if let Some(why) = outline.refused() {
        d.notice = Some(format!("The outline was not redrawn: {why}."));
        return;
    }
    let step = Step::Outline { outline, combine: r.combine };
    match crate::select::within_memory(|b| r.before.then_within(step, b)) {
        Ok(made) => d.select_made(made, RESHAPE),
        Err(why) => d.notice = Some(why),
    }
}

/// How far (screen points) from the line's last point a redrawing must be let go for that
/// place to end it as well.
const LAST_STEP: f32 = 0.5;

/// What the status bar says when a redrawing is let go away from the outline.
pub const LET_GO_ON_IT: &str = "Let go on the outline to redraw the stretch between.";

/// A redrawing under way, `r`, on `rect` (by `to_screen`): the stretch it would replace in
/// red, and its line in `accent`.
pub fn draw_reshaping(
    ui: &egui::Ui,
    r: &Reshaping,
    accent: egui::Color32,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let painter = ui.painter().with_clip_rect(rect);
    if let Some((_, gone)) = &r.shown {
        let red = egui::Color32::from_rgba_unmultiplied(230, 40, 40, 200);
        painter.add(egui::Shape::line(gone.iter().map(|&p| to_screen(p)).collect(), egui::Stroke::new(3.0, red)));
    }
    painter.add(egui::Shape::line(r.line.iter().map(|&p| to_screen(p)).collect(), egui::Stroke::new(2.0, accent)));
}

/// The history state a drag on the outline edited makes.
pub const EDIT: &str = "Edit outline";

/// Gives up what the tool is doing on `d`: an outline being drawn, a drag on the outline
/// edited or on its symmetry (its history state, pending, discarded), a redrawing.
pub fn give_up(d: &mut DocState) {
    if d.outlining.take().is_some() {
        d.revision += 1;
    }
    if matches!(d.drag, Some(Drag::Outline(_) | Drag::Reshape(_) | Drag::Symmetry(_))) {
        d.give_up_drag();
    }
}

/// The selection's last step, if it is an outline, and how it combines: the outline the tool
/// edits while none is being drawn.
pub fn edited(d: &DocState) -> Option<(&Outline, Combine)> {
    d.pixel_selection()?.last_outline()
}

/// The points picked on the outline edited (as [`Outline::points`] counts them, in order),
/// with the selection whose last step it is.
#[derive(Clone, Debug)]
pub struct Picked {
    /// (Weakly, so that an address is not reused while it is held: compared by address.)
    of: std::sync::Weak<opal_core::selection::Selection>,
    points: Vec<usize>,
    /// The point the last click picked, which a double-click on it turns smooth or sharp.
    clicked: Option<usize>,
}

/// The points picked on the outline edited now, in order: those a drag under way began with,
/// else those picked on the selection as it is (none once it is another, after an undo, say).
pub fn picked(d: &DocState) -> Vec<usize> {
    if let Some(Drag::Outline(drag)) = &d.drag {
        return drag.picked.clone();
    }
    match (&d.picked, d.pixel_selection()) {
        (Some(p), Some(s)) if std::sync::Weak::ptr_eq(&p.of, &Arc::downgrade(s)) => {
            let n = s.last_outline().map_or(0, |(o, _)| o.point_count());
            p.points.iter().copied().filter(|&i| i < n).collect()
        }
        _ => Vec::new(),
    }
}

/// Picks `points` on the outline edited as the selection is now, the last click's on `clicked`.
fn pick(d: &mut DocState, mut points: Vec<usize>, clicked: Option<usize>) {
    points.sort_unstable();
    points.dedup();
    d.picked = d.pixel_selection().map(|s| Picked { of: Arc::downgrade(s), points, clicked });
    d.revision += 1;
}

/// What a press on the outline edited may be on: its points and its picked points' handles,
/// and its sides too if `sides`, each within `near` screen points.
#[derive(Clone, Copy, Debug)]
struct Reach {
    near: f32,
    sides: bool,
}

/// What part of the outline edited a press is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Part {
    Point(usize),
    /// A picked point's handle.
    Handle(usize, End),
    /// A side, from point `.0`, `.1` (0 to 1) of the way along it.
    Side(usize, f64),
}

/// What carries outline `o` to each of the copies of it drawn, and back: those of its
/// symmetry, itself first (only itself, without one); none that could not be carried back (a
/// center so far that the numbers overflow, as a file may give).
fn copies(o: &Outline) -> Vec<(Affine, Affine)> {
    let maps = o.symmetric().map_or_else(|| vec![Affine::IDENTITY], |s| s.maps());
    maps.into_iter().filter_map(|map| Some((map, map.inverse()?))).collect()
}

/// How much nearer (screen points) a part on a copy must be than one on the outline itself (or
/// a handle than a point, or a later copy than an earlier) to be taken instead: where a copy
/// lies on the outline, the two differ only by rounding.
const TIE: f32 = 0.01;

/// The part of `o` (its points `nodes`, those `picked` in order) at `at` (screen points), on
/// it or on any copy of it drawn (see [`copies`]): the point or picked point's handle nearest
/// it, else a side, unless the `reach` leaves sides out; each within the reach. Of those as
/// near (within [`TIE`]), a point before a handle lying on it, and the outline itself before
/// a copy lying on it. With it, what carries the copy it is on back to `o`.
fn part_at(
    o: &Outline,
    nodes: &[Node],
    picked: &[usize],
    reach: Reach,
    at: Pos2,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) -> Option<(Part, Affine)> {
    let maps = copies(o);
    // The first of `found` (distance, part, back) as near as the nearest.
    let first_nearest = |found: Vec<(f32, Part, Affine)>| {
        let least = found.iter().map(|f| f.0).fold(f32::INFINITY, f32::min);
        found.into_iter().find(|f| f.0 <= least + TIE).map(|(_, part, back)| (part, back))
    };
    let on = |(map, back): (Affine, Affine)| {
        let far = move |p: [f64; 2]| to_screen(map.apply(p)).distance(at);
        let handles = picked.iter().flat_map(move |&i| {
            let n = nodes[i];
            [(n.into, End::In), (n.out, End::Out)]
                .into_iter()
                .filter_map(move |(h, end)| Some((far(h?), Part::Handle(i, end), back)))
        });
        let points = nodes.iter().enumerate().map(move |(i, n)| (far(n.at), Part::Point(i), back));
        points.chain(handles)
    };
    let found: Vec<_> = maps.iter().flat_map(|&m| on(m)).filter(|(d, _, _)| *d <= reach.near).collect();
    if !found.is_empty() || !reach.sides {
        return first_nearest(found);
    }
    let sides = maps.iter().filter_map(|&(map, back)| {
        let (side, t, _) = o.nearest(back.apply(to_doc(at)))?;
        let d = to_screen(map.apply(o.along(side, t))).distance(at);
        (d <= reach.near).then_some((d, Part::Side(side, t), back))
    });
    first_nearest(sides.collect())
}

/// A drag on the outline edited.
#[derive(Clone, Debug)]
pub struct EditDrag {
    pub part: Part,
    /// Where it began (document pixels).
    from: [f64; 2],
    /// The outline as it was, how it combines, and what the steps before it select.
    start: Outline,
    combine: Combine,
    before: opal_core::selection::Selection,
    /// The points picked as it began: those a point's drag moves, it among them.
    picked: Vec<usize>,
    /// Alt held as it began: a handle dragged alone (its point made sharp), or, from a point,
    /// the whole outline moved.
    alt: bool,
    /// What carries the copy it is on back to the outline itself (the identity, on itself).
    back: Affine,
}

impl EditDrag {
    /// The outline it makes, the pointer moved `by` (document pixels) since it began: on a
    /// copy, the outline itself as the copy is moved; the whole outline moved as dragged.
    fn made(&self, by: [f64; 2]) -> Outline {
        let o = &self.start;
        let b = &self.back;
        let on_it = [b.a * by[0] + b.c * by[1], b.b * by[0] + b.d * by[1]];
        match self.part {
            Part::Point(_) if self.alt => o.transformed(&Affine::translate(by[0], by[1])),
            Part::Point(_) => o.moved(&self.picked, on_it),
            Part::Handle(i, end) => {
                let h = o.handles(i)[usize::from(end == End::Out)].expect("a handle dragged is one there is");
                o.with_handle(i, end, [h[0] + on_it[0], h[1] + on_it[1]], self.alt)
            }
            Part::Side(i, t) => o.bent(i, t, on_it),
        }
    }
}

/// Whether the outline edited on `d` may be changed now: not while a box is open on it, or an
/// edit is under way.
fn editable(d: &DocState) -> bool {
    d.transform.is_none() && d.busy.is_none()
}

/// Makes `outline` the selection's last step in place of the outline edited, as a history
/// state labeled `label`, unless there is not memory enough for it (the status bar says so);
/// without one (`None`), the steps before it. Picks `points` on it.
fn replace(d: &mut DocState, outline: Option<Outline>, points: Vec<usize>, label: &str) {
    let Some((_, combine)) = edited(d) else { return };
    let before = d.pixel_selection().expect("one with an outline").before_last();
    let Some(outline) = outline else {
        d.picked = None;
        return d.set_pixel_selection(Some(before), label);
    };
    match crate::select::within_memory(|b| before.then_within(Step::Outline { outline, combine }, b)) {
        Ok(made) => {
            d.select_made(made, label);
            pick(d, points, None);
        }
        Err(why) => d.notice = Some(why),
    }
}

/// Turns the points picked on the outline edited sharp or smooth (`join`), as a history state.
pub fn join_picked(d: &mut DocState, join: Join) {
    let points = picked(d);
    if points.is_empty() || !editable(d) {
        return;
    }
    let Some((o, _)) = edited(d) else { return };
    let label = if join == Join::Sharp { "Sharp points" } else { "Smooth points" };
    let made = o.joined(&points, join);
    replace(d, Some(made), points, label);
}

/// Takes the points picked on the outline edited away, as a history state; with fewer points
/// left than an outline may have (three, a slice two), the outline goes.
pub fn delete_picked(d: &mut DocState) {
    let points = picked(d);
    if points.is_empty() || !editable(d) {
        return;
    }
    let Some((o, _)) = edited(d) else { return };
    let made = o.without(&points);
    replace(d, made, Vec::new(), "Delete points");
}

/// Picks every point of the outline edited.
pub fn pick_all(d: &mut DocState) {
    if let Some((o, _)) = edited(d) {
        let all = (0..o.point_count()).collect();
        pick(d, all, None);
    }
}

/// What a frame's presses and drags on the canvas do to the outline edited, if one is (the
/// selection's last step, none being drawn), the tool's `options` saying how a new one would
/// combine: whether they were its, or are left to begin another. A press on a point or a
/// picked point's handle is its; one on a side too, unless a new outline would combine with
/// what is selected (Shift or Alt held, or the tool's *Combine* not Replace: as away from it).
fn edit(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
    options: Options,
) -> bool {
    let modifiers = ctx.input(|i| i.modifiers);
    if let Some(Drag::Outline(drag)) = d.drag.take_if(|g| matches!(g, Drag::Outline(_))) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            d.discard_pending(EDIT);
            pick(d, drag.picked, None);
        } else if !ctx.input(|i| i.pointer.primary_down()) {
            // Let go: the edit is a history state of its own, its points still picked.
            if d.pending_label() == Some(EDIT) {
                d.commit_pending();
            }
            pick(d, drag.picked, None);
        } else {
            if let Some(p) = response.interact_pointer_pos() {
                let at = to_doc(p);
                let made = drag.made([at[0] - drag.from[0], at[1] - drag.from[1]]);
                if edited(d).map(|(o, _)| o) != Some(&made) {
                    let step = Step::Outline { outline: made, combine: drag.combine };
                    crate::select_tool::reshape_with(d, &drag.before, step, EDIT);
                }
            }
            d.drag = Some(Drag::Outline(drag));
        }
        return true;
    }
    let primary = PointerButton::Primary;
    if !(response.drag_started_by(primary) || response.clicked_by(primary)) {
        return false;
    }
    let Some((outline, combine)) = edited(d).map(|(o, c)| (o.clone(), c)) else { return false };
    let nodes = outline.nodes();
    let points = picked(d);
    let clicked_before = d
        .picked
        .as_ref()
        .filter(|p| d.pixel_selection().is_some_and(|s| std::sync::Weak::ptr_eq(&p.of, &Arc::downgrade(s))))
        .and_then(|p| p.clicked);
    let sides = !modifiers.shift && !modifiers.alt && options.combine == Combine::Replace;
    let part =
        |at: Pos2| part_at(&outline, &nodes, &points, Reach { near: options.near, sides }, at, to_doc, to_screen);
    if response.drag_started_by(primary)
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        && let Some((part, back)) = part(origin)
    {
        // A point not picked, dragged: picked alone (with Shift, besides the others).
        let mut points = points;
        if let Part::Point(i) = part
            && points.binary_search(&i).is_err()
        {
            if !modifiers.shift {
                points.clear();
            }
            points.push(i);
            points.sort_unstable();
        }
        let before = d.pixel_selection().expect("one with an outline").before_last();
        pick(d, points.clone(), None);
        let alt = modifiers.alt;
        let drag = EditDrag { part, from: to_doc(origin), start: outline, combine, before, picked: points, alt, back };
        d.drag = Some(Drag::Outline(drag));
        return true;
    }
    let Some(at) = response.interact_pointer_pos() else { return false };
    if response.double_clicked_by(primary) {
        // On the point the click before picked: it turned smooth if sharp, sharp if not.
        if let Some((Part::Point(i), _)) = part(at)
            && clicked_before == Some(i)
        {
            let join = if nodes[i].join == Join::Sharp { Join::Smooth } else { Join::Sharp };
            let label = if join == Join::Sharp { "Sharp points" } else { "Smooth points" };
            replace(d, Some(outline.joined(&[i], join)), vec![i], label);
        }
        // (Else, the second click of the double-click that closed an outline, say: nothing.)
        return true;
    }
    match part(at).map(|(part, _)| part) {
        Some(Part::Point(i)) if modifiers.alt => {
            let rest: Vec<usize> = points.iter().copied().filter(|&p| p != i).map(|p| p - usize::from(p > i)).collect();
            replace(d, outline.without(&[i]), rest, "Delete points");
        }
        Some(Part::Point(i)) => {
            let mut points = points;
            match (modifiers.shift, points.binary_search(&i)) {
                (true, Ok(k)) => {
                    points.remove(k);
                }
                (true, Err(k)) => points.insert(k, i),
                (false, _) => points = vec![i],
            }
            pick(d, points, Some(i));
        }
        Some(Part::Side(i, t)) => {
            // The point added is picked alone; those after it count one more.
            let added = outline.split(i, t);
            match added.refused() {
                None => replace(d, Some(added), vec![i + 1], "Add point"),
                Some(why) => d.notice = Some(format!("No point was added: {why}.")),
            }
        }
        Some(Part::Handle(..)) => {}
        // A click away from it: the points picked let go (with none picked, it begins another).
        None if !points.is_empty() => pick(d, Vec::new(), None),
        None => return false,
    }
    true
}

/// The outline edited `o` on `rect` (by `to_screen`), and each copy of it drawn: its points,
/// those `picked` (in order) filled in `accent`, with their handles drawn out of them. Where
/// points crowd closer than [`SPACED`] on screen (a freehand stretch's, every 2 points), those
/// not picked are drawn only that far apart, so that the squares stay few however many the
/// points and copies.
pub fn draw_edited(
    ui: &egui::Ui,
    o: &Outline,
    picked: &[usize],
    accent: egui::Color32,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let painter = ui.painter().with_clip_rect(rect);
    let line = egui::Stroke::new(1.0, accent);
    let nodes = o.nodes();
    for (map, _) in copies(o) {
        let to_screen = |p: [f64; 2]| to_screen(map.apply(p));
        for &i in picked {
            let p = to_screen(nodes[i].at);
            for h in [nodes[i].into, nodes[i].out].into_iter().flatten() {
                let h = to_screen(h);
                painter.line_segment([p, h], line);
                painter.circle(h, 3.5, egui::Color32::WHITE, line);
            }
        }
        let mut last: Option<Pos2> = None;
        for (i, n) in nodes.iter().enumerate() {
            let at = to_screen(n.at);
            let is_picked = picked.binary_search(&i).is_ok();
            if !is_picked && last.is_some_and(|l| l.distance(at) < SPACED) {
                continue;
            }
            last = Some(at);
            let fill = if is_picked { accent } else { egui::Color32::WHITE };
            let square = Rect::from_center_size(at, egui::vec2(7.0, 7.0));
            painter.rect(square, 0.0, fill, line, egui::StrokeKind::Middle);
        }
    }
}

/// How far apart (screen points) the points of the outline edited are drawn at least, but
/// those picked (see [`draw_edited`]).
const SPACED: f32 = 6.0;

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
    let near = |a: [f64; 2], b: Pos2| to_screen(a).distance(b) <= options.near;
    // Ctrl+click picks a layer, as with every tool: it neither begins an outline nor adds to
    // one.
    let picking = modifiers.command;
    // A stretch of the outline edited redrawn, by a right-drag (or with R held) from it.
    if d.outlining.is_none() && reshape(ctx, d, response, options.near, to_doc, to_screen) {
        return;
    }
    // The symmetry's center and axis, by their handles, while no outline is drawn.
    if let Some(shown) = shown_symmetry(d, options)
        && guide(ctx, d, response, shown, options.near, to_doc, to_screen)
    {
        return;
    }
    let Some(o) = d.outlining.as_mut() else {
        if picking {
            return;
        }
        // On the outline edited (the selection's last step), a press edits it.
        if edit(ctx, d, response, to_doc, to_screen, options) {
            return;
        }
        let combine = crate::select_tool::combine_held(modifiers, options.combine);
        let symmetry = symmetry(d, options);
        if response.drag_started_by(PointerButton::Primary)
            && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        {
            d.outlining = Some(Outlining::begun(to_doc(origin), combine, true, symmetry));
            d.revision += 1;
        } else if response.clicked_by(PointerButton::Primary)
            // (Not the second click of a double-click, whose first closed one.)
            && !response.double_clicked_by(PointerButton::Primary)
            && !response.triple_clicked_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            d.outlining = Some(Outlining::begun(to_doc(p), combine, false, symmetry));
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
            o.dragging = Some(Drawing::Pulling);
        } else {
            o.dragging = Some(Drawing::Stretch);
            o.add(to_doc(origin));
        }
        changed = true;
    }
    if let Some(p) = pointer.filter(|_| held) {
        let at = to_doc(p);
        match o.dragging {
            Some(Drawing::Pulling) => {
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
            Some(Drawing::Stretch) if to_screen(o.last()).distance(p) >= STEP => changed |= o.add(at),
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
        if near(o.start, p) && o.points() >= o.fewest() {
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
    let to_pointer = o.pointer.filter(|_| !o.freehand && o.dragging.is_none()).map(|p| o.side_to(p));
    // With a symmetry, the whole outline it would make, its copies too.
    if o.symmetry.is_some() {
        let segments = o.segments.iter().copied().chain(to_pointer).collect();
        return Outline { symmetry: o.symmetry, ..Outline::new(o.start, segments, true) }.polygon();
    }
    let mut points = opal_core::outline::flattened(o.start, &o.segments);
    if let Some(side) = to_pointer {
        points.extend(opal_core::outline::flattened(o.last(), &[side]).into_iter().skip(1));
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_symmetry_s_guide_and_its_handle_go_through_a_turned_view() {
        use egui::Vec2;
        // A view turned a quarter clockwise: canvas up is the screen's right.
        let mut v = crate::doc::View::default();
        v.viewport = Vec2::new(400.0, 300.0);
        v.turn_to(90.0);
        let to_screen = |p: [f64; 2]| v.to_screen(Vec2::new(p[0] as f32, p[1] as f32)).to_pos2();
        let s = Symmetry { order: 1, mirror: true, center: [100.0, 80.0], angle: 0.0 };
        let (center, handle) = guide_handles(&s, &to_screen);
        let along = handle - center;
        assert!((along - Vec2::new(AXIS_HANDLE, 0.0)).length() < 1e-2, "the handle is {along:?} from the center");
        // A point dragged to on the screen's right of the center is the canvas's up: angle 0;
        // below it, the canvas's right: 90.
        let to_doc = |p: Pos2| {
            let c = v.to_canvas(p.to_vec2());
            [f64::from(c.x), f64::from(c.y)]
        };
        assert!(axis_angle(s.center, to_doc(center + Vec2::new(50.0, 0.0))).abs() < 1e-3);
        assert!((axis_angle(s.center, to_doc(center + Vec2::new(0.0, 50.0))) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn a_redrawing_too_large_or_selecting_nothing_is_said_so() {
        use crate::state::{AppState, Background, NewImage};
        use opal_core::selection::Selection;
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let d = st.active_mut().unwrap();
        // On a screen one point a pixel.
        let to_screen = |p: [f64; 2]| Pos2::new(p[0] as f32, p[1] as f32);
        let redrawing = |start: Outline, line: Vec<[f64; 2]>, other: bool| Reshaping {
            start,
            combine: Combine::Replace,
            before: Selection::of(Vec::new()),
            map: Affine::IDENTITY,
            back: Affine::IDENTITY,
            from: (0, 0.25),
            line,
            other,
            button: PointerButton::Secondary,
            near: 7.0,
            of: std::sync::Weak::new(),
            shown: None,
            shown_for: (0, false),
        };
        // A slice whose 64 copies come just within the most points an outline may have: a
        // dozen more drawn into its first side, refused.
        let sym = Symmetry { order: 32, mirror: true, center: [150.0, 100.0], angle: 0.0 };
        let n = MOST_POINTS / 64 - 2;
        let zigzag = (1..=n).map(|i| Segment::Line { to: [i as f64, (i % 2) as f64] }).collect();
        let slice = Outline { symmetry: Some(sym), ..Outline::new([0.0, 0.0], zigzag, false) };
        assert_eq!(slice.refused(), None);
        let bulge = (0..12)
            .map(|k| {
                let u = f64::from(k) / 11.0;
                [0.25 + 0.5 * u, 0.25 + 0.5 * u - 0.2 * (std::f64::consts::PI * u).sin()]
            })
            .collect();
        let_go(d, &redrawing(slice, bulge, false), &to_screen);
        let said = d.notice.take().unwrap_or_default();
        assert!(said.starts_with("The outline was not redrawn: "), "{said}");
        assert!(d.pixel_selection().is_none());
        // A square's top kept, and the rest drawn again along it: nothing enclosed, and said so.
        let square = Outline::new(
            [0.0, 0.0],
            vec![
                Segment::Line { to: [100.0, 0.0] },
                Segment::Line { to: [100.0, 100.0] },
                Segment::Line { to: [0.0, 100.0] },
            ],
            false,
        );
        let along_the_top = vec![[25.0, 0.0], [50.0, 0.0], [75.0, 0.0]];
        let_go(d, &redrawing(square, along_the_top, true), &to_screen);
        assert_eq!(d.notice.take().as_deref(), Some(crate::select::NOTHING_SELECTED));
    }

    #[test]
    fn an_image_not_drawn_has_the_tool_s_drags_given_up_their_edits_with_them() {
        use crate::state::{AppState, Background, NewImage};
        use opal_core::selection::Selection;
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let lines = vec![Segment::Line { to: [100.0, 10.0] }, Segment::Line { to: [100.0, 100.0] }];
        let o = Outline::new([10.0, 10.0], lines, false);
        let replace = Combine::Replace;
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(Some(Selection::new(Step::Outline { outline: o.clone(), combine: replace })), "Outline");
        let s = d.pixel_selection().unwrap();
        let (before, of) = (s.before_last(), Arc::downgrade(s));
        // Each drag, its edit pending (as its label says), the image drawn in a pass: a
        // point's, a redrawing, the symmetry's, and the Move tool's.
        let point = Drag::Outline(EditDrag {
            part: Part::Point(0),
            from: [10.0, 10.0],
            start: o.clone(),
            combine: replace,
            before: before.clone(),
            picked: vec![0],
            alt: false,
            back: Affine::IDENTITY,
        });
        let reshaping = Drag::Reshape(Box::new(Reshaping {
            start: o.clone(),
            combine: replace,
            before: before.clone(),
            map: Affine::IDENTITY,
            back: Affine::IDENTITY,
            from: (0, 0.5),
            line: Vec::new(),
            other: false,
            button: PointerButton::Secondary,
            near: 7.0,
            of,
            shown: None,
            shown_for: (0, false),
        }));
        let symmetry = Drag::Symmetry(SymmetryDrag { center: true, edited: None, was: (None, 0.0) });
        let moving = Drag::Move(crate::move_tool::MoveDrag {
            from: [0.0, 0.0],
            bounds: None,
            guides: Default::default(),
            shown: None,
        });
        let drags = [
            (point, Some(EDIT)),
            (reshaping, None),
            (symmetry, Some(MOVE_SYMMETRY)),
            (moving, Some(crate::move_tool::MOVE)),
        ];
        for (pass, (drag, label)) in (10..).step_by(10).zip(drags) {
            let d = st.active_mut().unwrap();
            if let Some(label) = label {
                let moved = Step::Outline { outline: o.moved(&[0], [5.0, 0.0]), combine: replace };
                crate::select_tool::reshape_with(d, &before, moved, label);
            }
            (d.drag, d.drawn, d.symmetry_center) = (Some(drag), pass, Some([5.0, 5.0]));
            // Drawn in the pass before: kept.
            st.leave_undrawn(pass + 1);
            assert!(st.active().unwrap().drag.is_some());
            // Not: given up, its edit with it.
            st.leave_undrawn(pass + 2);
            let d = st.active().unwrap();
            assert!(d.drag.is_none(), "the drag was left");
            // The symmetry's, the image's center put back where it began (none).
            let center = if label == Some(MOVE_SYMMETRY) { None } else { Some([5.0, 5.0]) };
            assert_eq!(d.symmetry_center, center);
            assert_eq!((d.pending_label(), edited(d).map(|(e, _)| e.clone())), (None, Some(o.clone())));
        }
    }

    #[test]
    fn a_press_on_a_point_is_on_it_though_a_handle_lies_there_and_on_a_side_only_if_it_would_not_combine() {
        // Point 1's handle into it lies on it (a side pulled into a curve at its far end).
        let o = Outline::new(
            [0.0, 0.0],
            vec![
                Segment::Curve { c1: [30.0, -30.0], c2: [100.0, 0.0], to: [100.0, 0.0] },
                Segment::Line { to: [100.0, 100.0] },
                Segment::Line { to: [0.0, 100.0] },
            ],
            true,
        );
        let (to_doc, to_screen) =
            (|p: Pos2| [f64::from(p.x), f64::from(p.y)], |p: [f64; 2]| Pos2::new(p[0] as f32, p[1] as f32));
        let at = |sides: bool, p: [f32; 2]| {
            let reach = Reach { near: 7.0, sides };
            part_at(&o, &o.nodes(), &[1], reach, Pos2::new(p[0], p[1]), &to_doc, &to_screen).map(|(part, _)| part)
        };
        assert_eq!(at(true, [100.0, 0.0]), Some(Part::Point(1)));
        // Its handle out of point 0 is not picked: no handle there; but the point's own is.
        assert_eq!(at(true, [30.0, -30.0]), None);
        assert_eq!(at(true, [100.0, 50.0]), Some(Part::Side(1, 0.5)));
        assert_eq!(at(false, [100.0, 50.0]), None, "a side taken where a new outline would combine");
        // A quarter of a diamond, turned four times round (200, 150): a press on a copy's point
        // is on that point of the slice, the copy's turn with it; where the slice's end and the
        // next copy's start lie together, on the slice's own.
        let sym = Symmetry { order: 4, mirror: false, center: [200.0, 150.0], angle: 0.0 };
        let slice = Outline {
            symmetry: Some(sym),
            ..Outline::new([200.0, 100.0], vec![Segment::Line { to: [250.0, 150.0] }], false)
        };
        let on = |p: [f32; 2]| {
            let reach = Reach { near: 7.0, sides: true };
            part_at(&slice, &slice.nodes(), &[], reach, Pos2::new(p[0], p[1]), &to_doc, &to_screen)
        };
        let (part, back) = on([200.0, 200.0]).expect("on a copy's point");
        assert_eq!((part, back.apply([200.0, 200.0]).map(f64::round)), (Part::Point(1), [250.0, 150.0]));
        assert_eq!(on([250.0, 150.0]), Some((Part::Point(1), Affine::IDENTITY)));
        // Turned a fifth of the way at a slant, the slice's end a thousandth of a pixel from where
        // its next copy begins (as rounding may leave them): the slice's own end.
        let fifth = Symmetry { order: 5, mirror: false, center: [200.0, 150.0], angle: 17.0 };
        let end = fifth.turn(1).apply([200.0, 100.0]);
        let ends = vec![Segment::Line { to: [end[0] + 1e-3, end[1]] }];
        let slanted = Outline { symmetry: Some(fifth), ..Outline::new([200.0, 100.0], ends, false) };
        let reach = Reach { near: 7.0, sides: true };
        let press = Pos2::new(end[0] as f32, end[1] as f32);
        let found = part_at(&slanted, &slanted.nodes(), &[], reach, press, &to_doc, &to_screen);
        assert_eq!(found, Some((Part::Point(1), Affine::IDENTITY)));
        // A center so far that turning about it overflows: only the copies that can be carried
        // back are tried (here, the outline itself).
        let far = Symmetry { center: [1e308, 1e308], ..sym };
        let unturnable = Outline { symmetry: Some(far), ..slice.clone() };
        assert_eq!(copies(&unturnable).len(), 1);
        // A copy's side: the slice's, as far along.
        let (part, _) = on([175.0, 175.0]).expect("on a copy's side");
        assert!(matches!(part, Part::Side(0, t) if (t - 0.5).abs() < 1e-9), "{part:?}");
    }

    #[test]
    fn the_points_picked_go_with_a_drag_and_nothing_is_changed_while_a_box_is_open() {
        use crate::state::{AppState, Background, NewImage};
        use opal_core::selection::Selection;
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let square = Outline::new(
            [20.0, 20.0],
            vec![
                Segment::Line { to: [120.0, 20.0] },
                Segment::Line { to: [120.0, 120.0] },
                Segment::Line { to: [20.0, 120.0] },
            ],
            true,
        );
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(
            Some(Selection::new(Step::Outline { outline: square.clone(), combine: Combine::Replace })),
            "Select",
        );
        pick(d, vec![1, 2], None);
        // A drag under way: its picks, though the selection it makes is another.
        let before = d.pixel_selection().unwrap().before_last();
        d.drag = Some(Drag::Outline(EditDrag {
            part: Part::Point(1),
            from: [120.0, 20.0],
            start: square.clone(),
            combine: Combine::Replace,
            before: before.clone(),
            picked: vec![1, 2],
            alt: false,
            back: Affine::IDENTITY,
        }));
        let moved = square.moved(&[1, 2], [5.0, 5.0]);
        crate::select_tool::reshape_with(d, &before, Step::Outline { outline: moved, combine: Combine::Replace }, EDIT);
        assert_eq!(picked(d), vec![1, 2], "the points picked lost mid-drag");
        d.drag = None;
        d.commit_pending();
        // A box open: the buttons change nothing.
        pick(d, vec![1], None);
        st.run(crate::commands::Cmd::TransformSelection, &eframe::egui::Context::default());
        let d = st.active_mut().unwrap();
        assert!(d.transform.is_some(), "no box");
        let was = d.doc().selection.clone();
        delete_picked(d);
        join_picked(d, Join::Smooth);
        assert_eq!(d.doc().selection, was, "changed with a box open");
    }

    #[test]
    fn a_symmetric_slice_stops_where_its_copies_would_be_too_many_and_the_guide_shows_what_is_drawn() {
        use crate::state::{AppState, Background, NewImage};
        // Sixty-four copies (32, mirrored): a slice of as many points as a million allows.
        let sym = Symmetry { order: MOST_ORDER, mirror: true, center: [0.0, 0.0], angle: 0.0 };
        let mut o = Outlining::begun([0.0, 0.0], Combine::Replace, false, Some(sym));
        let mut added = 0;
        while o.add([added as f64, 1.0]) {
            added += 1;
        }
        let made = o.outline(true).expect("closed");
        assert!(made.refused().is_none() && added > 1000, "{added} points");
        // The guide: the outline being drawn's symmetry, as it began, whatever the settings say
        // since; else the outline edited's; else the one a new outline would have.
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let options = Options { combine: Combine::Replace, smooth: true, order: 6, mirror: false, near: 7.0 };
        let d = st.active_mut().unwrap();
        assert_eq!(
            shown_symmetry(d, options).map(|(s, w)| (s.order, s.center, w)),
            Some((6, [150.0, 100.0], Shown::New))
        );
        d.outlining = Some(Outlining::begun([10.0, 10.0], Combine::Replace, false, Some(Symmetry { order: 4, ..sym })));
        assert_eq!(shown_symmetry(d, options).map(|(s, w)| (s.order, w)), Some((4, Shown::Drawn)));
        d.outlining = None;
        let slice = Outline {
            symmetry: Some(Symmetry { order: 3, mirror: false, center: [40.0, 40.0], angle: 0.0 }),
            ..Outline::new([40.0, 10.0], vec![Segment::Line { to: [60.0, 50.0] }], true)
        };
        let step = Step::Outline { outline: slice, combine: Combine::Replace };
        d.set_pixel_selection(Some(opal_core::selection::Selection::new(step)), "Select");
        assert_eq!(
            shown_symmetry(d, options).map(|(s, w)| (s.order, s.center, w)),
            Some((3, [40.0, 40.0], Shown::Edited))
        );
        // Centered on the canvas: the outline edited's, as a history state, and new outlines'.
        center_symmetry(d, options);
        assert_eq!(edited(d).and_then(|(o, _)| o.symmetry).map(|s| s.center), Some([150.0, 100.0]));
        assert_eq!(d.history.undo_label(), Some(MOVE_SYMMETRY));
    }

    #[test]
    fn the_panel_s_buttons_join_delete_and_pick_every_point() {
        use crate::state::{AppState, Background, NewImage};
        use opal_core::selection::Selection;
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        let square = Outline::new(
            [20.0, 20.0],
            vec![
                Segment::Line { to: [120.0, 20.0] },
                Segment::Line { to: [120.0, 120.0] },
                Segment::Line { to: [20.0, 120.0] },
            ],
            true,
        );
        let d = st.active_mut().unwrap();
        d.set_pixel_selection(
            Some(Selection::new(Step::Outline { outline: square, combine: Combine::Replace })),
            "Select",
        );
        // Nothing picked: the buttons change nothing.
        join_picked(d, Join::Smooth);
        delete_picked(d);
        assert_eq!(d.history.undo_label(), Some("Select"));
        pick_all(d);
        assert_eq!(picked(d), vec![0, 1, 2, 3]);
        join_picked(d, Join::Smooth);
        let o = edited(d).unwrap().0.clone();
        assert!((0..4).all(|i| o.join(i) == Join::Smooth), "{:?}", o.joins);
        assert_eq!((picked(d), d.history.undo_label()), (vec![0, 1, 2, 3], Some("Smooth points")));
        // All taken away: the outline goes, and with it what it selected.
        delete_picked(d);
        assert!(d.pixel_selection().is_none());
        assert_eq!(d.history.undo_label(), Some("Delete points"));
    }

    #[test]
    fn taking_a_point_back_gives_back_the_handle_and_the_side_to_the_pointer_is_the_one_a_click_adds() {
        let (a, b, c) = ([0.0, 0.0], [100.0, 0.0], [100.0, 100.0]);
        let mut o = Outlining::begun(a, Combine::Replace, false, None);
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
        o.dragging = Some(Drawing::Pulling);
        assert!(o.take_back());
        assert_eq!((o.dragging, o.segments.len()), (None, 0));
        assert!(!o.take_back(), "the first point was taken back");
    }
}
