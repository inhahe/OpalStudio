//! The transform box on screen: its handles, its toolbar, and its numbers in the tool
//! settings panel. What a drag does to the box is in `transform.rs`.

use crate::doc::DocId;
use crate::icons;
use crate::state::AppState;
use crate::theme::icon;
use crate::transform::{Drag, DragInput, Handle, snap_targets};
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Response, RichText, Shape, Stroke, Vec2};
use opal_core::resample::{Method, Sampling};

/// How far from a handle (points) the pointer still grabs it.
const GRAB: f32 = 8.0;
/// How far above the box's top edge (points) the rotation handle sits.
const ROTATE_OUT: f32 = 26.0;

/// The box's geometry on screen, in points.
struct OnScreen {
    corners: [Pos2; 4],
    rotate: Pos2,
}

impl OnScreen {
    fn edge(&self, k: usize) -> Pos2 {
        self.corners[k].lerp(self.corners[(k + 1) % 4], 0.5)
    }

    fn center(&self) -> Pos2 {
        self.corners[0].lerp(self.corners[2], 0.5)
    }

    /// What dragging from `p` would do: a handle near it, inside the box moves it, anywhere
    /// else rotates it.
    fn handle_at(&self, p: Pos2) -> Handle {
        if p.distance(self.rotate) <= GRAB {
            return Handle::Rotate;
        }
        if let Some(k) = (0..4).find(|&k| p.distance(self.corners[k]) <= GRAB) {
            return Handle::Corner(k);
        }
        if let Some(k) = (0..4).find(|&k| p.distance(self.edge(k)) <= GRAB) {
            return Handle::Edge(k);
        }
        if inside(&self.corners, p) { Handle::Move } else { Handle::Rotate }
    }

    /// The cursor that says what dragging `h` does.
    fn cursor(&self, h: Handle) -> CursorIcon {
        let toward = |p: Pos2| {
            let d = p - self.center();
            // Directions repeat every half turn; pick the nearest of four.
            let a = d.y.atan2(d.x).to_degrees().rem_euclid(180.0);
            match a {
                a if !(22.5..157.5).contains(&a) => CursorIcon::ResizeHorizontal,
                a if a < 67.5 => CursorIcon::ResizeNwSe,
                a if a < 112.5 => CursorIcon::ResizeVertical,
                _ => CursorIcon::ResizeNeSw,
            }
        };
        match h {
            Handle::Move => CursorIcon::Move,
            Handle::Rotate => CursorIcon::Alias,
            Handle::Corner(k) => toward(self.corners[k]),
            Handle::Edge(k) => toward(self.edge(k)),
        }
    }
}

/// Whether `p` is inside the convex quadrilateral `q`.
fn inside(q: &[Pos2; 4], p: Pos2) -> bool {
    let side = |a: Pos2, b: Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
    let s: Vec<f32> = (0..4).map(|k| side(q[k], q[(k + 1) % 4])).collect();
    s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)
}

/// Handles the box on document `id`'s canvas, if one is open: dragging it, and drawing it
/// with its toolbar. `response` is the canvas's, `rect` its area on screen.
pub fn canvas_box(ui: &mut egui::Ui, st: &mut AppState, id: DocId, response: &Response, rect: Rect) {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let aspect_locked = st.prefs.bool("transform.aspect_locked");
    let snapping = st.prefs.bool("transform.snapping");
    let snap_points = st.prefs.f64("transform.snap_distance") as f32;
    let rotate_step = st.prefs.f64("transform.rotate_step").to_radians();
    let draft = st.prefs.bool("transform.draft");
    let palette = st.palette;
    let Some(d) = st.doc_mut(id) else { return };
    let Some(s) = d.transform.as_ref() else { return };
    let view = d.view;
    let to_screen = |p: [f64; 2]| rect.min + view.to_screen(Vec2::new(p[0] as f32, p[1] as f32)) / ppp;
    let to_doc = |p: Pos2| {
        let c = view.to_canvas((p - rect.min) * ppp);
        [f64::from(c.x), f64::from(c.y)]
    };
    let corners = s.corners().map(to_screen);
    let top = corners[0].lerp(corners[1], 0.5);
    let center = corners[0].lerp(corners[2], 0.5);
    // Away from the box, perpendicular to its top edge (which may be mirrored or turned).
    let along = corners[1] - corners[0];
    let mut out = Vec2::new(along.y, -along.x).normalized();
    if (top + out - center).length() < (top - center).length() {
        out = -out;
    }
    let screen = OnScreen { corners, rotate: top + out * ROTATE_OUT };

    // ---- Dragging ---------------------------------------------------------------------
    let primary = response.dragged_by(egui::PointerButton::Primary);
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        let handle = screen.handle_at(origin);
        let start = s.current;
        if let Some(s) = d.transform.as_mut() {
            s.drag = Some(Drag { handle, from: to_doc(origin), start });
        }
    }
    let dragging = d.transform.as_ref().and_then(|s| s.drag);
    if let Some(drag) = dragging
        && primary
        && let Some(p) = response.interact_pointer_pos()
    {
        let m = ctx.input(|i| i.modifiers);
        let input = DragInput {
            pointer: to_doc(p),
            shift: m.shift,
            ctrl: m.command,
            alt: m.alt,
            aspect_locked,
            snapping,
            snap_distance: f64::from(snap_points * ppp / view.zoom),
            rotate_step,
        };
        let s = d.transform.as_ref().expect("checked above");
        let except: Vec<_> = s.targets.iter().map(|t| t.layer).collect();
        let targets = snap_targets(d.doc(), &except);
        let (m, guides) = s.dragged(&drag, &input, &targets);
        d.set_box(m, draft.then_some(Sampling::DRAFT));
        if let Some(s) = d.transform.as_mut() {
            s.guides = guides;
        }
    }
    if dragging.is_some() && !primary {
        // Released: draw with the chosen method again.
        let s = d.transform.as_mut().expect("checked above");
        s.drag = None;
        s.guides = Default::default();
        let m = s.current;
        d.set_box(m, None);
    }

    // ---- Drawing ----------------------------------------------------------------------
    let s = d.transform.as_ref().expect("checked above");
    let corners = s.corners().map(to_screen);
    let top = corners[0].lerp(corners[1], 0.5);
    let screen = OnScreen { rotate: top + out * ROTATE_OUT, corners };
    let painter = ui.painter().with_clip_rect(rect);
    let accent = palette.accent;
    let line = Stroke::new(1.5, accent);
    let halo = Stroke::new(3.5, Color32::from_black_alpha(90));
    let outline: Vec<Pos2> = screen.corners.to_vec();
    painter.add(Shape::closed_line(outline.clone(), halo));
    painter.add(Shape::closed_line(outline, line));
    painter.line_segment([top, screen.rotate], halo);
    painter.line_segment([top, screen.rotate], line);
    crate::canvas::draw_guides(ui, &s.guides, rect, &to_screen);
    let knob = |p: Pos2| {
        let r = Rect::from_center_size(p, Vec2::splat(8.0));
        painter.rect_filled(r, 1.0, Color32::WHITE);
        painter.rect_stroke(r, 1.0, Stroke::new(1.0, accent), egui::StrokeKind::Outside);
    };
    for k in 0..4 {
        knob(screen.corners[k]);
        knob(screen.edge(k));
    }
    painter.circle(screen.rotate, 5.0, Color32::WHITE, Stroke::new(1.0, accent));

    // What the pointer would do.
    if let Some(p) = response.hover_pos()
        && !ctx.input(|i| i.pointer.middle_down())
    {
        let h = match s.drag {
            Some(drag) => drag.handle,
            None => screen.handle_at(p),
        };
        ctx.set_cursor_icon(screen.cursor(h));
    }
    // While dragging, the size and angle by the pointer.
    if s.drag.is_some()
        && let Some(p) = response.interact_pointer_pos()
    {
        let q = s.params();
        let text = format!("{:.0} × {:.0} px   {:.1}°", q.width, q.height, q.angle);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
        let at = p + Vec2::new(16.0, 18.0);
        let bg = Rect::from_min_size(at, galley.size()).expand(4.0);
        painter.rect_filled(bg, 3.0, Color32::from_black_alpha(190));
        painter.galley(at, galley, Color32::WHITE);
    }
    toolbar(&ctx, st, id, rect, &screen);
}

/// What the toolbar under the box asked for.
enum Action {
    Commit,
    Cancel,
    Fit(bool),
    Aspect,
    Snap,
    IntoActive,
}

/// The buttons that go with the box: commit, cancel, fit, fill, the aspect lock, snapping,
/// and for a paste, putting it into the active layer.
fn toolbar(ctx: &egui::Context, st: &mut AppState, id: DocId, rect: Rect, screen: &OnScreen) {
    let Some(d) = st.doc(id) else { return };
    let Some(s) = d.transform.as_ref() else { return };
    let paste = s.paste.clone();
    let aspect = st.prefs.bool("transform.aspect_locked");
    let snapping = st.prefs.bool("transform.snapping");
    let lowest = screen.corners.iter().chain([&screen.rotate]).fold(f32::NEG_INFINITY, |m, p| m.max(p.y));
    let center_x = screen.corners.iter().map(|p| p.x).sum::<f32>() / 4.0;
    let width = 300.0;
    let x = (center_x - width * 0.5).clamp(rect.left() + 4.0, (rect.right() - width - 4.0).max(rect.left() + 4.0));
    let y = (lowest + 12.0).clamp(rect.top() + 4.0, rect.bottom() - 40.0);
    let mut action = None;
    egui::Area::new(egui::Id::unique(("transform-toolbar", id.0)))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::new(x, y))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let b = |ui: &mut egui::Ui, glyph: char, on: bool, tip: &str| {
                        let text = icon(glyph, 16.0);
                        ui.add(egui::Button::new(text).selected(on)).on_hover_text(tip)
                    };
                    if b(ui, icons::CHECK, false, "Apply (Enter)").clicked() {
                        action = Some(Action::Commit);
                    }
                    if b(ui, icons::CLOSE, false, "Cancel (Esc)").clicked() {
                        action = Some(Action::Cancel);
                    }
                    ui.separator();
                    if b(ui, icons::FIT, false, "Fit inside the canvas").clicked() {
                        action = Some(Action::Fit(false));
                    }
                    if b(ui, icons::FILL, false, "Fill the canvas").clicked() {
                        action = Some(Action::Fit(true));
                    }
                    let lock = if aspect { icons::LINK } else { icons::UNLOCK };
                    let tip = if aspect {
                        "The aspect ratio is kept (Shift while dragging frees it). Click to free it."
                    } else {
                        "The aspect ratio is free (Shift while dragging keeps it). Click to keep it."
                    };
                    if b(ui, lock, aspect, tip).clicked() {
                        action = Some(Action::Aspect);
                    }
                    let tip = if snapping {
                        "Snapping is on (Alt while dragging turns it off). Click to turn it off."
                    } else {
                        "Snapping is off (Alt while dragging turns it on). Click to turn it on."
                    };
                    if b(ui, icons::SNAP, snapping, tip).clicked() {
                        action = Some(Action::Snap);
                    }
                    if let Some(p) = &paste {
                        ui.separator();
                        let r = ui.add_enabled(
                            p.active.is_some(),
                            egui::Button::new(icon(icons::INTO_LAYER, 16.0)).selected(p.into_active),
                        );
                        let r = if p.active.is_some() {
                            r.on_hover_text(if p.into_active {
                                "Going into the active layer, drawn into its pixels when applied. Click to put \
                                 it on a new layer instead."
                            } else {
                                "Going onto a new layer. Click to put it into the active layer instead (drawn \
                                 into its pixels when applied)."
                            })
                        } else {
                            r.on_disabled_hover_text("There is no plain, unlocked active layer to put it into.")
                        };
                        if r.clicked() {
                            action = Some(Action::IntoActive);
                        }
                    }
                });
            });
        });
    let Some(action) = action else { return };
    match action {
        Action::Aspect => {
            st.prefs.set("transform.aspect_locked", crate::settings::Value::Bool(!aspect));
            st.settings_touched();
        }
        Action::Snap => {
            st.prefs.set("transform.snapping", crate::settings::Value::Bool(!snapping));
            st.settings_touched();
        }
        _ => {
            let Some(d) = st.doc_mut(id) else { return };
            match action {
                Action::Commit => d.commit_pending(),
                Action::Cancel => d.cancel_transform(),
                Action::Fit(fill) => {
                    let canvas = d.doc().canvas.rect();
                    if let Some(m) = d.transform.as_ref().map(|s| s.fitted(&canvas, fill)) {
                        d.set_box(m, None);
                    }
                }
                Action::IntoActive => d.toggle_paste_into_active(),
                Action::Aspect | Action::Snap => {}
            }
        }
    }
}

/// The box's numbers and drawing method, in the tool settings panel.
pub fn settings(ui: &mut egui::Ui, st: &mut AppState) {
    let palette = st.palette;
    let aspect = st.prefs.bool("transform.aspect_locked");
    let Some(d) = st.active_mut() else { return };
    let Some(s) = d.transform.as_ref() else { return };
    ui.horizontal(|ui| {
        ui.label(icon(icons::TRANSFORM, 20.0));
        ui.heading(s.label());
    });
    ui.label(
        RichText::new(
            "Drag the box to move, a corner or edge to scale, the round handle or anywhere outside to rotate. \
             Enter applies, Esc cancels.",
        )
        .color(palette.weak_text),
    );
    ui.add_space(4.0);
    let canvas = d.doc().canvas.rect();
    let mut p = s.params();
    let before = p;
    let (cx, cy) = (canvas.x0 as f64, canvas.y0 as f64);
    let mut x = p.x - cx;
    let mut y = p.y - cy;
    let frame = s.frame;
    egui::Grid::new("transform-numbers").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Position");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut x).speed(1.0).prefix("x ").suffix(" px").max_decimals(1));
            ui.add(egui::DragValue::new(&mut y).speed(1.0).prefix("y ").suffix(" px").max_decimals(1));
        })
        .response
        .on_hover_text("Where the image's top-left corner is, from the canvas's top-left corner.");
        ui.end_row();
        ui.label("Size");
        ui.horizontal(|ui| {
            let w =
                ui.add(egui::DragValue::new(&mut p.width).speed(1.0).range(0.01..=1e6).suffix(" px").max_decimals(1));
            let h =
                ui.add(egui::DragValue::new(&mut p.height).speed(1.0).range(0.01..=1e6).suffix(" px").max_decimals(1));
            // Typing one side with the aspect ratio locked changes the other too.
            if aspect {
                if w.changed() {
                    p.height = before.height * p.width / before.width;
                } else if h.changed() {
                    p.width = before.width * p.height / before.height;
                }
            }
        });
        ui.end_row();
        ui.label("Scale");
        let mut pct = p.width / frame.width() * 100.0;
        let r = ui.add(egui::DragValue::new(&mut pct).speed(0.5).range(0.01..=1e6).suffix("%").max_decimals(2));
        if r.changed() {
            let k = pct / 100.0 * frame.width() / p.width;
            p.width *= k;
            p.height *= k;
        }
        ui.end_row();
        ui.label("Angle");
        ui.add(egui::DragValue::new(&mut p.angle).speed(0.5).range(-360.0..=360.0).suffix("°").max_decimals(2));
        ui.end_row();
    });
    p.x = x + cx;
    p.y = y + cy;
    if p != before {
        let m = s.with_params(&p);
        d.set_box(m, None);
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Drawing").strong());
    let s = d.transform.as_ref().expect("still open");
    let mut sampling = s.sampling;
    if sampling_ui(ui, &mut sampling, "transform") {
        d.set_box_sampling(sampling);
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new("Defaults for pasting, snapping and the aspect lock are in Preferences › Transform.")
            .small()
            .color(palette.weak_text),
    );
}

/// A compact editor for a [`Sampling`]: the method, its parameters, and anti-ringing.
/// Returns whether anything changed. `salt` keeps two editors on screen apart.
pub fn sampling_ui(ui: &mut egui::Ui, s: &mut Sampling, salt: &str) -> bool {
    let before = *s;
    let name = |m: &Method| match m {
        Method::Nearest => "Nearest",
        Method::Bilinear => "Bilinear",
        Method::Bicubic { .. } => "Bicubic",
        Method::Lanczos { .. } => "Lanczos",
    };
    ui.horizontal(|ui| {
        let label = ui.label("Resampling").id;
        egui::ComboBox::from_id_salt(("sampling-method", salt))
            .selected_text(name(&s.method))
            .show_ui(ui, |ui| {
                for m in
                    [Method::Nearest, Method::Bilinear, opal_core::resample::CATMULL_ROM, Method::Lanczos { lobes: 3 }]
                {
                    let same = std::mem::discriminant(&m) == std::mem::discriminant(&s.method);
                    if ui.selectable_label(same, name(&m)).clicked() && !same {
                        s.method = m;
                    }
                }
            })
            .response
            .labelled_by(label);
    });
    match &mut s.method {
        Method::Bicubic { b, c } => {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(b).speed(0.01).range(0.0..=1.0).prefix("B ").max_decimals(3))
                    .on_hover_text("Blur: 0 is sharpest; 1/3 with C = 1/3 is Mitchell's softer choice.");
                ui.add(egui::DragValue::new(c).speed(0.01).range(0.0..=1.0).prefix("C ").max_decimals(3))
                    .on_hover_text("Sharpening: more sharpens edges, and rings more.");
            });
        }
        Method::Lanczos { lobes } => {
            ui.horizontal(|ui| {
                ui.label("Lobes");
                ui.add(egui::Slider::new(lobes, 2..=4));
            });
        }
        _ => {}
    }
    if !matches!(s.method, Method::Nearest) {
        ui.checkbox(&mut s.anti_ringing, "Anti-ringing").on_hover_text(
            "Keeps each pixel within the range of the source pixels nearest it, which removes the halos \
             sharp filters draw along hard edges.",
        );
    }
    *s != before
}
