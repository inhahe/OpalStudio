//! The Measure tool: a line between two points of the image, its length and angle in the
//! user's unit, and a protractor's second arm from its start; kept on the image until the next
//! measure (not in its file, nor its history), going with it as it is turned, flipped or
//! resized, and back with that undone. Straighten turns the image, or the selected layers, by
//! the least that makes the line level or plumb.

use crate::doc::{DocState, Drag};
use crate::state::AppState;
use eframe::egui::{self, Color32, Pos2, Rect, Response, Stroke, Vec2};
use opal_core::geom::Affine;
use std::f64::consts::FRAC_PI_4;

/// How near (in points) a press must be to an end of a line measured to take it; the line
/// itself, to move it all, a little nearer.
const GRAB: f32 = 8.0;
const GRAB_LINE: f32 = 5.0;

/// The history state Straighten Layers makes.
pub const STRAIGHTEN_LAYERS: &str = "Straighten layers";

/// Why Straighten and the like cannot be done without a measure.
pub const NOTHING_MEASURED: &str = "Measure a line first: drag across the image with the Measure tool (N).";

/// Why Straighten cannot be done when the line is level or plumb.
pub const STRAIGHT_ALREADY: &str = "The line measured is level (or plumb) already.";

/// What was measured on an image: its points in document pixels, as the image was when its
/// `Document::carried` was `carried`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// Where the line starts (a protractor's vertex) and ends.
    pub start: [f64; 2],
    pub end: [f64; 2],
    /// A protractor's second arm's end (the arm from `start`).
    pub arm: Option<[f64; 2]>,
    pub carried: Affine,
}

impl Measure {
    /// `self` on an image whose `Document::carried` is `now`: carried by what has turned,
    /// flipped or resized the image since it was measured, or back by what of that was undone.
    pub fn now(&self, now: Affine) -> Measure {
        if now == self.carried {
            return *self;
        }
        let Some(m) = self.carried.inverse().map(|back| back.then(now)) else { return *self };
        self.mapped(&m, now)
    }

    /// `self` with its points carried by `m`, the image's coordinates then `carried`.
    pub fn mapped(&self, m: &Affine, carried: Affine) -> Measure {
        Measure { start: m.apply(self.start), end: m.apply(self.end), arm: self.arm.map(|a| m.apply(a)), carried }
    }

    /// How far the line's end is from its start, across and down (pixels).
    pub fn span(&self) -> [f64; 2] {
        [self.end[0] - self.start[0], self.end[1] - self.start[1]]
    }

    /// The line's length (pixels).
    pub fn length(&self) -> f64 {
        let [w, h] = self.span();
        w.hypot(h)
    }

    /// The line's angle, in degrees counterclockwise from the right, as a protractor reads
    /// it: from -180 to 180, a line rising to the right more than 0.
    pub fn angle(&self) -> f64 {
        bearing(self.start, self.end)
    }

    /// The second arm's length (pixels), if there is one.
    pub fn arm_length(&self) -> Option<f64> {
        self.arm.map(|a| (a[0] - self.start[0]).hypot(a[1] - self.start[1]))
    }

    /// The angle between the line and the second arm, if there is one: from 0 to 180 degrees.
    pub fn between(&self) -> Option<f64> {
        let d = (bearing(self.start, self.arm?) - self.angle()).rem_euclid(360.0);
        Some(if d > 180.0 { 360.0 - d } else { d })
    }

    /// How far (degrees clockwise) the image would turn for the line to be level or plumb,
    /// whichever is nearer: from -45 to 45 (0 for a line of no length).
    pub fn straightening(&self) -> f64 {
        if self.length() == 0.0 {
            return 0.0;
        }
        let a = self.angle();
        let t = a - (a / 90.0).round() * 90.0;
        // (Level or plumb to within what a pixel across the line's length could tell.)
        if t.abs() < 1e-9 { 0.0 } else { t }
    }

    /// Where the line's middle is.
    pub fn middle(&self) -> [f64; 2] {
        [(self.start[0] + self.end[0]) * 0.5, (self.start[1] + self.end[1]) * 0.5]
    }

    /// Whether it measures anything: a line of some length, and an arm of some length if it
    /// has one.
    pub(crate) fn measures(&self) -> bool {
        self.start != self.end && self.arm != Some(self.start)
    }
}

/// What is measured on image `d`, as the `measurement` query tells it (see its help): null if
/// nothing is.
pub fn to_json(d: &DocState) -> serde_json::Value {
    let Some(m) = d.measured() else { return serde_json::Value::Null };
    let c = d.doc().canvas;
    let at = |p: [f64; 2]| serde_json::json!([p[0] - c.x as f64, p[1] - c.y as f64]);
    let [width, height] = m.span();
    let mut v = serde_json::json!({
        "start": at(m.start),
        "end": at(m.end),
        "width": width,
        "height": height,
        "length": m.length(),
        "angle": m.angle(),
        "straightening": m.straightening(),
    });
    if let Some(arm) = m.arm {
        v["arm"] = at(arm);
        v["arm_length"] = serde_json::json!(m.arm_length());
        v["between"] = serde_json::json!(m.between());
    }
    v
}

/// The bearing of `to` from `from`, in degrees counterclockwise from the right (y down).
fn bearing(from: [f64; 2], to: [f64; 2]) -> f64 {
    (from[1] - to[1]).atan2(to[0] - from[0]).to_degrees()
}

/// The largest rectangle of the shape of a `w × h` canvas, in whole pixels, that fits within
/// it turned by `degrees` about the same middle: what is left of a canvas straightened without
/// its corners going empty. Its sides differ from the canvas's by even numbers of pixels, so
/// that it lies on the canvas's grid about its middle; each is a pixel at least (two, for an
/// even side), which a canvas too thin for any to fit turned is given all the same.
pub fn inscribed([w, h]: [u32; 2], degrees: f64) -> [u32; 2] {
    let (s, c) = degrees.to_radians().sin_cos();
    let (s, c) = (s.abs(), c.abs());
    let (fw, fh) = (f64::from(w), f64::from(h));
    let k = (fw / (fw * c + fh * s)).min(fh / (fw * s + fh * c)).min(1.0);
    let side = |full: u32, f: f64| {
        let n = ((f * k + 1e-9).floor() as u32).min(full);
        // Of the canvas's parity, so as to lie on its grid about the same middle.
        let n = if (full - n).is_multiple_of(2) { n } else { n.saturating_sub(1) };
        n.max(if full.is_multiple_of(2) { 2 } else { 1 })
    };
    [side(w, fw), side(h, fh)]
}

/// What a drag of the Measure tool moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// A new line, from where the drag began.
    New,
    Start,
    End,
    /// The protractor's second arm's end (a new arm, from the end Alt+dragged).
    Arm,
    /// All of it.
    Whole,
}

/// A drag of the Measure tool: what it moves, the measure it began from (whose points it
/// moves), the image's measure before it (put back if it is given up, or if the drag
/// measures nothing), and where it began (document pixels, whole).
#[derive(Clone, Copy, Debug)]
pub struct MeasureDrag {
    part: Part,
    begun: Measure,
    pub(crate) was: Option<Measure>,
    from: [f64; 2],
}

impl DocState {
    /// What is measured on the image, as it is now (see [`Measure::now`]).
    pub fn measured(&self) -> Option<Measure> {
        self.measure.map(|m| m.now(self.doc().carried))
    }
}

/// `p` at the nearest corner between pixels: where the edges a measure is taken between lie.
fn whole(p: [f64; 2]) -> [f64; 2] {
    [p[0].round(), p[1].round()]
}

/// `to`, or with `steer`, its nearest point from `from` along a multiple of 45° (across, down
/// or diagonally, in the image's own axes), in whole pixels.
fn steered(from: [f64; 2], to: [f64; 2], steer: bool) -> [f64; 2] {
    if !steer {
        return to;
    }
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let a = (dy.atan2(dx) / FRAC_PI_4).round() * FRAC_PI_4;
    let (s, c) = a.sin_cos();
    let along = dx * c + dy * s;
    [from[0] + (c * along).round(), from[1] + (s * along).round()]
}

/// What a press at screen point `p` takes of measure `m` (drawn by `to_screen`), and the
/// measure the drag begins from: with Alt, a protractor's second arm from its start, or from
/// its end if it has none (which becomes its start); else the arm's end, an end (the end
/// first, where they meet), or the line to move it all. Nothing (a new measure) if none is
/// near.
fn grab(m: &Measure, p: Pos2, alt: bool, to_screen: &dyn Fn([f64; 2]) -> Pos2) -> Option<(Part, Measure)> {
    let near = |q: [f64; 2]| to_screen(q).distance(p) <= GRAB;
    if alt {
        if near(m.start) {
            return Some((Part::Arm, Measure { arm: Some(m.start), ..*m }));
        }
        if m.arm.is_none() && near(m.end) {
            return Some((Part::Arm, Measure { start: m.end, end: m.start, arm: Some(m.end), ..*m }));
        }
    }
    if m.arm.is_some_and(near) {
        return Some((Part::Arm, *m));
    }
    if near(m.end) {
        return Some((Part::End, *m));
    }
    if near(m.start) {
        return Some((Part::Start, *m));
    }
    let on = |q: [f64; 2]| to_segment(p, to_screen(m.start), to_screen(q)) <= GRAB_LINE;
    if on(m.end) || m.arm.is_some_and(on) {
        return Some((Part::Whole, *m));
    }
    None
}

/// How far `p` is from the segment `a`–`b`.
fn to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() == 0.0 { 0.0 } else { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

/// The measure `g` makes with the pointer at document point `p` (`steer`: Shift held).
fn dragged(g: &MeasureDrag, p: [f64; 2], steer: bool) -> Measure {
    let (m, p) = (g.begun, whole(p));
    match g.part {
        Part::New | Part::End => Measure { end: steered(m.start, p, steer), ..m },
        Part::Start => Measure { start: steered(m.end, p, steer), ..m },
        Part::Arm => Measure { arm: Some(steered(m.start, p, steer)), ..m },
        Part::Whole => {
            let by = Affine::translate(p[0] - g.from[0], p[1] - g.from[1]);
            m.mapped(&by, m.carried)
        }
    }
}

/// What a frame's presses and drags with the Measure tool do on `d`'s canvas (by `to_doc`
/// and `to_screen`): a drag measures anew, or moves an end, the second arm's end, or all of it
/// (Shift: along a multiple of 45°); Alt+drag from an end draws a protractor's second arm. A
/// drag that measures nothing (a click) leaves the measure as it was; Esc gives a drag up.
pub fn canvas(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let (pressed, down, at, modifiers) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers));
    if pressed
        && d.drag.is_none()
        && response.contains_pointer()
        && let Some(p) = ctx.input(|i| i.pointer.press_origin())
    {
        let carried = d.doc().carried;
        let from = whole(to_doc(p));
        let (part, begun) = d
            .measured()
            .and_then(|m| grab(&m, p, modifiers.alt, to_screen))
            .unwrap_or((Part::New, Measure { start: from, end: from, arm: None, carried }));
        d.drag = Some(Drag::Measure(MeasureDrag { part, begun, was: d.measure, from }));
    }
    let Some(Drag::Measure(g)) = d.drag else { return };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        d.give_up_drag();
        return;
    }
    if down && let Some(pos) = at {
        let m = dragged(&g, to_doc(pos), modifiers.shift);
        // (What measures nothing, a click, an end on the other, leaves what was measured.)
        let shown = if m.measures() { Some(m) } else { g.was };
        if shown != d.measure {
            d.measure = shown;
            d.revision += 1;
        }
    }
    if !down {
        d.drag = None;
        d.revision += 1;
    }
}

/// What a measure's labels say: by its line's end, by its second arm's end, and by its
/// vertex (the angle between).
pub struct Labels {
    pub line: String,
    pub arm: Option<String>,
    pub between: Option<String>,
}

/// The labels of `m`, its lengths in `unit` at `ppi` pixels per inch.
pub fn labels(m: &Measure, unit: opal_core::units::Unit, ppi: f64) -> Labels {
    let length = |px: f64| crate::lengths::show(px, unit, ppi);
    Labels {
        line: format!("{}  {}", length(m.length()), degrees(m.angle())),
        arm: m.arm_length().map(length),
        between: m.between().map(degrees),
    }
}

/// An angle in degrees, as the measure shows them.
pub fn degrees(a: f64) -> String {
    format!("{a:.2}°")
}

/// Draws measure `m` on `rect` (by `to_screen`): its line, its second arm and the arc between
/// them, a handle at each point, and its labels; dark under light, to show on any image.
pub fn draw(
    ui: &egui::Ui,
    m: &Measure,
    labels: &Labels,
    accent: Color32,
    rect: Rect,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let painter = ui.painter().with_clip_rect(rect);
    let halo = Stroke::new(3.5, Color32::from_black_alpha(150));
    let line = Stroke::new(1.5, Color32::WHITE);
    let (start, end) = (to_screen(m.start), to_screen(m.end));
    let arm = m.arm.map(to_screen);
    let segments: Vec<[Pos2; 2]> = std::iter::once([start, end]).chain(arm.map(|a| [start, a])).collect();
    for stroke in [halo, line] {
        for &s in &segments {
            painter.line_segment(s, stroke);
        }
    }
    // The arc between the arms, the short way round, at a radius short of either.
    if let Some(a) = arm {
        let radius = 28.0_f32.min((end - start).length() * 0.5).min((a - start).length() * 0.5);
        let (from, to) = ((end - start).angle(), (a - start).angle());
        let mut sweep = to - from;
        if sweep > std::f32::consts::PI {
            sweep -= std::f32::consts::TAU;
        } else if sweep < -std::f32::consts::PI {
            sweep += std::f32::consts::TAU;
        }
        let points: Vec<Pos2> =
            (0..=24).map(|k| start + Vec2::angled(from + sweep * k as f32 / 24.0) * radius).collect();
        for stroke in [halo, line] {
            painter.add(egui::Shape::line(points.clone(), stroke));
        }
        if let Some(text) = &labels.between {
            // Outside the arc, on the line halving the angle.
            let mid = start + Vec2::angled(from + sweep * 0.5) * radius;
            label(&painter, start, mid, text);
        }
    }
    for p in std::iter::once(start).chain(std::iter::once(end)).chain(arm) {
        painter.circle(p, 4.0, Color32::WHITE, Stroke::new(1.5, accent));
    }
    // Each label beyond its end, along its line.
    label(&painter, start, end, &labels.line);
    if let (Some(a), Some(text)) = (arm, &labels.arm) {
        label(&painter, start, a, text);
    }
}

/// `text` in a small dark chip just beyond `to` on the line from `from`, clear of the handle
/// there, whichever way the line goes.
fn label(painter: &egui::Painter, from: Pos2, to: Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), Color32::WHITE);
    let pad = egui::vec2(4.0, 2.0);
    let half = galley.size() * 0.5 + pad;
    let dir = if to == from { Vec2::X } else { (to - from).normalized() };
    // Far enough along the line for the chip's nearest edge to clear the handle.
    let reach = dir.x.abs() * half.x + dir.y.abs() * half.y + 8.0;
    let r = Rect::from_center_size(to + dir * reach, galley.size());
    painter.rect_filled(r.expand2(pad), 3.0, Color32::from_black_alpha(190));
    painter.galley(r.min, galley, Color32::WHITE);
}

/// What the tool settings say of the Measure tool before anything is measured.
const HOW: &str = "Drag across the image to measure the distance and angle between two points. Drag an end \
                   to move it, or the line to move it all; Alt+drag from an end draws a protractor's second \
                   arm. Shift keeps to multiples of 45°.";

/// The Measure tool's part of the tool settings: what is measured on the active image, in
/// fields (its ends, from the canvas's corner; its length and angle, typed to move its end),
/// its width and height, a protractor's second arm and the angle between; the unit they are
/// in; and the commands that use it.
pub fn panel(ui: &mut egui::Ui, st: &mut AppState) {
    use crate::lengths::{image_scope, length_field};
    use crate::number_field::NumberField;
    ui.separator();
    let weak = st.palette.weak_text;
    let mut unit = st.unit();
    let Some(d) = st.active() else { return };
    let Some(m) = d.measured() else {
        ui.add(egui::Label::new(egui::RichText::new(HOW).color(weak)).wrap());
        return;
    };
    let doc = d.doc();
    let ppi = doc.resolution;
    let scope = image_scope(Some(doc), unit, ppi);
    let corner = [doc.canvas.x as f64, doc.canvas.y as f64];
    let far = crate::box_fields::FAR;
    let mut v = m;
    egui::Grid::new("measure-fields").num_columns(3).show(ui, |ui| {
        ui.label("");
        ui.label(egui::RichText::new("X").color(weak));
        ui.label(egui::RichText::new("Y").color(weak));
        ui.end_row();
        for (name, p, ids) in [
            ("Start", &mut v.start, ["measure.start.x", "measure.start.y"]),
            ("End", &mut v.end, ["measure.end.x", "measure.end.y"]),
        ] {
            ui.label(name);
            for k in 0..2 {
                let mut px = p[k] - corner[k];
                if length_field(&mut px, scope, -far..=far, 1).remember(ids[k]).show(ui).changed() {
                    p[k] = px + corner[k];
                }
            }
            ui.end_row();
        }
        // Typed, the end moves: along the line for a length, round its start for an angle.
        let (length, angle) = (m.length(), m.angle());
        let mut typed_length = length;
        let label = ui.label("Length");
        // (Some length: a line of none measures nothing.)
        let r = length_field(&mut typed_length, scope, 0.01..=far, 2).remember("measure.length").show(ui);
        if r.labelled_by(label.id).changed() {
            let k = typed_length / length;
            v.end = [m.start[0] + (m.end[0] - m.start[0]) * k, m.start[1] + (m.end[1] - m.start[1]) * k];
        }
        ui.end_row();
        let mut typed_angle = angle;
        let label = ui.label("Angle");
        let field = NumberField::angle(&mut typed_angle, scope).range(-360.0..=360.0).speed(0.25);
        if ui.add(field.remember("measure.angle")).labelled_by(label.id).changed() {
            let a = typed_angle.to_radians();
            v.end = [m.start[0] + length * a.cos(), m.start[1] - length * a.sin()];
        }
        ui.label(egui::RichText::new("counterclockwise").color(weak));
        ui.end_row();
    });
    let show = |px: f64| crate::lengths::show(px, unit, ppi);
    let [w, h] = m.span();
    ui.label(egui::RichText::new(format!("Width {}, height {}", show(w), show(h))).color(weak));
    if let (Some(arm), Some(between)) = (m.arm_length(), m.between()) {
        ui.label(format!("Second arm {}, between them {}", show(arm), degrees(between)));
    }
    ui.horizontal(|ui| {
        ui.label("Unit");
        if crate::lengths::unit_menu(ui, &mut unit, "measure-unit") {
            st.set_unit(unit);
        }
    });
    if v != m
        && let Some(d) = st.active_mut()
    {
        d.measure = Some(v);
        d.revision += 1;
    }
    use crate::commands::Cmd;
    crate::panels::command_buttons(ui, st, &[Cmd::Straighten, Cmd::StraightenLayers, Cmd::ClearMeasurement]);
}

impl AppState {
    /// Straighten: the active image turned by the least that makes the line measured level or
    /// plumb ([`Measure::straightening`]), about its canvas's middle, as an edit worked out off
    /// the interface thread; with `crop`, the canvas then the largest rectangle of its shape
    /// within what was there ([`inscribed`]; the crop box's straightening, so nothing goes
    /// empty), else grown to hold all of it (Rotate by Angle's). Why not, if it cannot be.
    pub(crate) fn straighten(&mut self, crop: bool) -> Result<(), String> {
        let d = self.active().ok_or("Open or create an image first.")?;
        let m = d.measured().ok_or(NOTHING_MEASURED)?;
        let t = m.straightening();
        if t == 0.0 {
            return Err(STRAIGHT_ALREADY.into());
        }
        if !crop {
            self.rotate_by(&crate::image_ops::RotateBy { degrees: t, grow: true });
            return Ok(());
        }
        let c = d.doc().canvas;
        let [w, h] = inscribed([c.width, c.height], t);
        let b = crate::edit_box::EditBox {
            center: c.rect().to_f().center(),
            size: [f64::from(w), f64::from(h)],
            angle: -t.to_radians(),
        };
        let sampling = crate::prefs::sampling(&self.prefs, "transform");
        let id = self.active_doc.expect("found above");
        self.crop_with(id, b, false, sampling)
    }

    /// Straighten Layers: the selected layers turned about the line measured's middle by the
    /// least that makes it level or plumb, as one history state, through a box applied at
    /// once (drawn as a transform is); the measure turned with them, as it lies on what they
    /// hold. Why not, if it cannot be.
    pub(crate) fn straighten_layers(&mut self) -> Result<(), String> {
        let sampling = crate::prefs::sampling(&self.prefs, "transform");
        let d = self.active_mut().ok_or("Open or create an image first.")?;
        let m = d.measured().ok_or(NOTHING_MEASURED)?;
        let t = m.straightening();
        if t == 0.0 {
            return Err(STRAIGHT_ALREADY.into());
        }
        if let Some(name) = crate::move_tool::locked_in_selected_layers(d) {
            return Err(format!("\"{name}\" is locked. Unlock it first."));
        }
        if !d.begin_transform(sampling) {
            return Err(crate::transform::NOTHING_TO_TRANSFORM.into());
        }
        let [x, y] = m.middle();
        let turn = Affine::rotate(t.to_radians()).about(x, y);
        let to = d.transform.as_ref().expect("begun").start.then(turn);
        if let Some(why) = d.box_refused(&to) {
            d.cancel_transform();
            return Err(format!("It cannot be turned so: {why}."));
        }
        d.set_box(to, None);
        if let Some(p) = d.pending.as_mut() {
            p.label = STRAIGHTEN_LAYERS.to_owned();
        }
        d.commit_pending();
        d.measure = Some(m.mapped(&turn, m.carried));
        d.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: [f64; 2], end: [f64; 2]) -> Measure {
        Measure { start, end, arm: None, carried: Affine::IDENTITY }
    }

    #[test]
    fn a_line_s_angle_is_counterclockwise_and_its_straightening_the_least_turn() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Rising to the right (y down): positive; a turn clockwise levels it.
        let m = line([0.0, 100.0], [100.0, 90.0]);
        assert!(close(m.angle(), (0.1_f64).atan().to_degrees()));
        assert!(close(m.straightening(), m.angle()));
        // Nearly plumb: turned to plumb, the other way.
        let m = line([0.0, 0.0], [10.0, -100.0]);
        assert!(close(m.straightening(), m.angle() - 90.0) && m.straightening() < 0.0);
        // Drawn right to left: the same line, level the same way.
        let m = line([100.0, 90.0], [0.0, 100.0]);
        assert!(close(m.straightening(), (0.1_f64).atan().to_degrees()));
        // Level, plumb, or of no length: nothing.
        for m in [line([0.0, 5.0], [9.0, 5.0]), line([3.0, 0.0], [3.0, -9.0]), line([1.0, 1.0], [1.0, 1.0])] {
            assert_eq!(m.straightening(), 0.0, "{m:?}");
        }
        assert_eq!(line([0.0, 0.0], [30.0, 40.0]).length(), 50.0);
    }

    #[test]
    fn a_protractor_measures_the_angle_between_its_arms_the_short_way_round() {
        let mut m = line([0.0, 0.0], [10.0, 0.0]);
        for (arm, between) in
            [([0.0, -10.0], 90.0), ([-10.0, -10.0], 135.0), ([-10.0, 10.0], 135.0), ([10.0, 10.0], 45.0)]
        {
            m.arm = Some(arm);
            assert!((m.between().unwrap() - between).abs() < 1e-9, "{arm:?}: {:?}", m.between());
        }
        assert_eq!(m.arm_length(), Some(200.0_f64.sqrt()));
    }

    #[test]
    fn a_measure_goes_with_the_image_turned_and_back_with_that_undone() {
        let m = line([10.0, 20.0], [30.0, 20.0]);
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let turned = m.now(quarter);
        let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
        assert!(close(turned.start, [-20.0, 10.0]) && close(turned.end, [-20.0, 30.0]), "{turned:?}");
        let back = turned.now(Affine::IDENTITY);
        assert!(close(back.start, m.start) && close(back.end, m.end));
    }

    #[test]
    fn shift_steers_a_line_along_a_multiple_of_45_degrees_in_whole_pixels() {
        assert_eq!(steered([0.0, 0.0], [10.0, 2.0], true), [10.0, 0.0]);
        assert_eq!(steered([0.0, 0.0], [9.0, -11.0], true), [10.0, -10.0]);
        assert_eq!(steered([5.0, 5.0], [6.0, -20.0], true), [5.0, -20.0]);
        assert_eq!(steered([0.0, 0.0], [9.0, -11.0], false), [9.0, -11.0]);
    }

    #[test]
    fn straighten_crops_the_image_turned_plumb_to_what_was_there_and_undone_the_line_is_as_measured() {
        use crate::commands::{Cmd, availability};
        use crate::state::{Background, NewImage};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 400, height: 300, background: Background::White, resolution: 72.0 });
        // Nearly plumb, leaning right as it goes down.
        let measured = line([380.0, 0.0], [400.0, 200.0]);
        st.active_mut().unwrap().measure = Some(measured);
        let t = measured.straightening();
        assert!((t - (0.1_f64).atan().to_degrees()).abs() < 1e-9, "{t}");
        st.straighten(true).unwrap();
        st.finish_edits();
        let d = st.active().unwrap();
        let c = d.doc().canvas;
        assert_eq!([c.width, c.height], inscribed([400, 300], t));
        assert_eq!(c.rect().to_f().center(), [200.0, 150.0]);
        assert_eq!(d.history.undo_label(), Some("Crop and straighten"));
        let m = d.measured().unwrap();
        assert!((m.angle().abs() - 90.0).abs() < 1e-9, "not plumb: {m:?}");
        assert_eq!(availability(&st, Cmd::Straighten), Err(STRAIGHT_ALREADY.into()));
        // Undone: the line as it was measured, to straighten again.
        assert!(st.active_mut().unwrap().undo());
        assert_eq!(st.active().unwrap().measured(), Some(measured));
        // A layer locked: neither straightens, saying so.
        let bg = st.active().unwrap().doc().layers[0].id;
        st.active_mut()
            .unwrap()
            .apply("Lock", |doc, _| {
                doc.layer_mut(bg).unwrap().locked = true;
                Ok::<(), String>(())
            })
            .unwrap();
        for cmd in [Cmd::Straighten, Cmd::StraightenLayers] {
            assert!(availability(&st, cmd).is_err_and(|why| why.contains("locked")), "{cmd:?}");
        }
        assert!(st.straighten_layers().is_err_and(|why| why.contains("locked")));
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Lock"));
        // Nothing measured: nothing to straighten by.
        st.active_mut().unwrap().measure = None;
        assert_eq!(availability(&st, Cmd::Straighten), Err(NOTHING_MEASURED.into()));
    }

    #[test]
    fn the_rectangle_left_by_straightening_fits_within_the_turned_canvas_on_its_grid() {
        for ([w, h], t) in
            [([400, 300], 10.0), ([301, 200], -30.0), ([1000, 10], 2.0), ([7, 9], 45.0), ([400, 300], 0.0)]
        {
            let [iw, ih] = inscribed([w, h], t);
            assert!(iw >= 1 && ih >= 1 && (w - iw) % 2 == 0 && (h - ih) % 2 == 0, "{w}x{h} at {t}: {iw}x{ih}");
            // Its corners, turned back onto the canvas, within it.
            let (s, c) = t.to_radians().sin_cos();
            for [x, y] in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
                let (x, y) = (x * f64::from(iw) / 2.0, y * f64::from(ih) / 2.0);
                let (u, v) = (x * c - y * s, x * s + y * c);
                assert!(u.abs() <= f64::from(w) / 2.0 + 1e-6 && v.abs() <= f64::from(h) / 2.0 + 1e-6, "{w}x{h} at {t}");
            }
        }
        // Too thin for any whole pixel to fit turned: the least there may be, on the grid.
        assert_eq!(inscribed([1, 100], 45.0), [1, 2]);
        // Not turned: the canvas itself; turned a little, nearly so.
        assert_eq!(inscribed([400, 300], 0.0), [400, 300]);
        let [iw, _] = inscribed([400, 300], 1.0);
        assert!(iw > 380, "{iw}");
    }
}
