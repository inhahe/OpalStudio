//! Exact numbers for the boxes on the canvas: a box's position, size and angle typed in the
//! user's units. The transform box (a paste, Transform) and the crop box show them in the
//! tool settings panel, and in a panel that opens from the numbers button by the box; a
//! box of any later tool (a selection's) is meant to take the same fields.

use crate::crop::CropBox;
use crate::doc::DocId;
use crate::lengths;
use crate::state::AppState;
use crate::transform::Params;
use eframe::egui;
use opal_core::units::Unit;

/// The farthest a box's corner may be put from the canvas's, in pixels: typed in its
/// fields, or given to a script's call.
pub const FAR: f64 = 1e7;

/// The points of a box its position can be given at, as `(id, label)`, row by row: the one
/// at `i` lies `i % 3` halves of the box's width and `i / 3` halves of its height from its
/// top-left corner. (Resize Canvas names its anchor by the same ids.)
pub const POINTS: [(&str, &str); 9] = [
    ("top-left", "Top left"),
    ("top", "Top"),
    ("top-right", "Top right"),
    ("left", "Left"),
    ("center", "Center"),
    ("right", "Right"),
    ("bottom-left", "Bottom left"),
    ("bottom", "Bottom"),
    ("bottom-right", "Bottom right"),
];

/// The point of a box called `id` (one of [`POINTS`]), as fractions of its width and height
/// from its top-left corner; the corner itself for any other.
pub fn point(id: &str) -> [f64; 2] {
    let i = POINTS.iter().position(|(p, _)| *p == id).unwrap_or(0);
    [(i % 3) as f64 * 0.5, (i / 3) as f64 * 0.5]
}

/// How a box's fields behave.
pub struct Fields<'a> {
    /// Keeps the width ÷ height while one side is typed, if given.
    pub keep: Option<f64>,
    /// The box's own width, for a scale field (in percent of it), if it has one.
    pub scale_of: Option<f64>,
    /// How many decimal places lengths show in pixels.
    pub px_decimals: usize,
    /// The sizes a side may have, in pixels.
    pub sides: std::ops::RangeInclusive<f64>,
    /// Keeps the fields' widgets apart from another set's on screen.
    pub salt: &'a str,
}

/// Fields for the box `p` (its position that of its point `POINTS[*reference]`, from the
/// canvas's corner), in `unit` at `ppi`, with a menu of the units and a grid of the points,
/// as a two-column grid. Returns whether the user changed a number (choosing another point
/// changes none: the caller shows the box at it next).
pub fn fields(
    ui: &mut egui::Ui,
    p: &mut Params,
    unit: &mut Unit,
    reference: &mut usize,
    ppi: f64,
    how: &Fields,
) -> bool {
    let before = *p;
    let point = POINTS[*reference].1.to_lowercase();
    egui::Grid::new(("box-numbers", how.salt)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Units");
        lengths::unit_menu(ui, unit, &format!("{}-unit", how.salt));
        ui.end_row();
        ui.label("Reference").on_hover_text(
            "Which point of the box the position gives, and what a size or angle typed scales and turns it about.",
        );
        reference_grid(ui, reference, how.salt);
        ui.end_row();
        let u = *unit;
        ui.label("Position");
        ui.horizontal(|ui| {
            ui.label("x");
            lengths::length(ui, &mut p.x, u, ppi, -FAR..=FAR, how.px_decimals);
            ui.label("y");
            lengths::length(ui, &mut p.y, u, ppi, -FAR..=FAR, how.px_decimals);
        })
        .response
        .on_hover_text(format!("Where the box's {point} is, from the canvas's top-left corner."));
        ui.end_row();
        ui.label("Size");
        ui.horizontal(|ui| {
            let w = lengths::length(ui, &mut p.width, u, ppi, how.sides.clone(), how.px_decimals);
            ui.label("×");
            let h = lengths::length(ui, &mut p.height, u, ppi, how.sides.clone(), how.px_decimals);
            if let Some(r) = how.keep {
                if w.changed() {
                    p.height = (p.width / r).clamp(*how.sides.start(), *how.sides.end());
                } else if h.changed() {
                    p.width = (p.height * r).clamp(*how.sides.start(), *how.sides.end());
                }
            }
        });
        ui.end_row();
        if let Some(own) = how.scale_of {
            ui.label("Scale");
            let mut pct = p.width / own * 100.0;
            let r = ui.add(egui::DragValue::new(&mut pct).speed(0.5).range(0.01..=1e6).suffix("%").max_decimals(2));
            if r.changed() {
                let k = pct / 100.0 * own / p.width;
                p.width *= k;
                p.height *= k;
            }
            ui.end_row();
        }
        ui.label("Angle");
        ui.add(egui::DragValue::new(&mut p.angle).speed(0.5).range(-360.0..=360.0).suffix("°").max_decimals(2))
            .on_hover_text(format!("Clockwise, about the box's {point}."));
        ui.end_row();
    });
    *p != before
}

/// A 3 × 3 grid of a box's points (see [`POINTS`]), the one at `chosen` marked; clicking
/// another chooses it.
fn reference_grid(ui: &mut egui::Ui, chosen: &mut usize, salt: &str) {
    egui::Grid::new(("box-reference", salt)).spacing([1.0, 1.0]).show(ui, |ui| {
        for row in 0..3 {
            for col in 0..3 {
                let i = row * 3 + col;
                let on = *chosen == i;
                let glyph = if on { "●" } else { "○" };
                let r = ui
                    .add(egui::Button::new(glyph).small().selected(on).min_size(egui::vec2(16.0, 16.0)))
                    .on_hover_text(POINTS[i].1);
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, POINTS[i].1));
                if r.clicked() {
                    *chosen = i;
                }
            }
            ui.end_row();
        }
    });
}

/// The point the preference chooses: its index in [`POINTS`].
fn reference(st: &AppState) -> usize {
    let id = st.prefs.choice("transform.reference");
    POINTS.iter().position(|(p, _)| *p == id).unwrap_or(0)
}

/// Makes `POINTS[i]` the point the boxes' positions give, if it is another.
fn set_reference(st: &mut AppState, i: usize) {
    if i != reference(st) {
        st.prefs.set("transform.reference", crate::settings::Value::Choice(POINTS[i].0.to_owned()));
        st.settings_touched();
    }
}

/// The transform box's fields for document `id` (its position, size, scale and angle), and
/// a rectangle to fit it into; what is typed moves the box at once.
pub fn transform(ui: &mut egui::Ui, st: &mut AppState, id: DocId, salt: &str) {
    let aspect = st.prefs.bool("transform.aspect_locked");
    let (mut unit, mut chosen) = (st.unit(), reference(st));
    let at = point(POINTS[chosen].0);
    let Some(d) = st.doc_mut(id) else { return };
    let Some(s) = d.transform.as_ref() else { return };
    let (canvas, ppi) = (d.doc().canvas.rect(), d.doc().resolution);
    let (cx, cy) = (canvas.x0 as f64, canvas.y0 as f64);
    let p = s.params_at(at);
    let mut q = Params { x: p.x - cx, y: p.y - cy, ..p };
    let how = Fields {
        keep: aspect.then(|| p.width / p.height),
        scale_of: Some(s.frame.width()),
        px_decimals: 1,
        sides: 0.01..=1e6,
        salt,
    };
    if fields(ui, &mut q, &mut unit, &mut chosen, ppi, &how) {
        let m = s.edited(&p, &Params { x: q.x + cx, y: q.y + cy, ..q }, at);
        d.set_box(m, None);
    }
    let size = [canvas.width() as f64, canvas.height() as f64];
    if let Some(fit) = fit_into(ui, &mut st.fit_rect, size, unit, ppi, at, salt) {
        let d = st.doc_mut(id).expect("shown above");
        if let Err(why) = crate::api::place_box(d, &fit) {
            st.notify(why);
        }
    }
    set_reference(st, chosen);
    if unit != st.unit() {
        st.set_unit(unit);
    }
}

/// A rectangle to fit the transform box into (`rect`: its top-left corner from the canvas's,
/// and its size; the canvas, `canvas` wide and tall, until one is typed), shown at the point
/// `at` in `unit` at `ppi`, with buttons that fit the box inside it, make it fill it (each
/// keeping its proportions, centered on it), or stretch it to it, as scripts' `fit` does.
/// What a button asks for, as a script's arguments would.
fn fit_into(
    ui: &mut egui::Ui,
    rect: &mut Option<[f64; 4]>,
    canvas: [f64; 2],
    unit: Unit,
    ppi: f64,
    at: [f64; 2],
    salt: &str,
) -> Option<crate::api::BoxArgs> {
    use crate::api::{BoxArgs, Fit};
    let [x0, y0, w0, h0] = rect.unwrap_or([0.0, 0.0, canvas[0], canvas[1]]);
    let (mut x, mut y, mut w, mut h) = (x0 + at[0] * w0, y0 + at[1] * h0, w0, h0);
    let mut fit = None;
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Fit into a rectangle").id_salt(("fit-into", salt)).default_open(true).show(ui, |ui| {
        egui::Grid::new(("fit-rect", salt)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.label("x");
                lengths::length(ui, &mut x, unit, ppi, -FAR..=FAR, 1);
                ui.label("y");
                lengths::length(ui, &mut y, unit, ppi, -FAR..=FAR, 1);
            });
            ui.end_row();
            ui.label("Size");
            ui.horizontal(|ui| {
                lengths::length(ui, &mut w, unit, ppi, 0.01..=1e6, 1);
                ui.label("×");
                lengths::length(ui, &mut h, unit, ppi, 0.01..=1e6, 1);
            });
            ui.end_row();
        });
        ui.horizontal_wrapped(|ui| {
            let ways = [
                ("Fit inside", Fit::Inside, "As large as fits inside the rectangle, in its own proportions, centered."),
                ("Fill", Fit::Cover, "As small as covers the rectangle, in its own proportions, centered."),
                ("Stretch", Fit::Stretch, "Exactly the rectangle, its proportions changed if need be."),
            ];
            for (name, way, tip) in ways {
                if ui.button(name).on_hover_text(tip).clicked() {
                    fit = Some(way);
                }
            }
            if ui.button("Canvas").on_hover_text("Make the rectangle the canvas again.").clicked() {
                (x, y, w, h) = (at[0] * canvas[0], at[1] * canvas[1], canvas[0], canvas[1]);
            }
        });
    });
    let typed = [x - at[0] * w, y - at[1] * h, w, h];
    if typed != [x0, y0, w0, h0] {
        *rect = Some(typed);
    }
    fit.map(|fit| BoxArgs { x: Some(x), y: Some(y), width: Some(w), height: Some(h), angle: None, fit, at })
}

/// The crop box's fields for document `id` (its position, size and angle); what is typed
/// moves the box at once. The box's proportions are kept as the tool's aspect setting says.
pub fn crop(ui: &mut egui::Ui, st: &mut AppState, id: DocId, salt: &str) {
    let settings = st.tool_settings(crate::tools::ToolId::Crop).clone();
    let (mut unit, mut chosen) = (st.unit(), reference(st));
    let at = point(POINTS[chosen].0);
    let Some(d) = st.doc_mut(id) else { return };
    let (c, ppi) = (d.doc().canvas, d.doc().resolution);
    let b = *d.crop.get_or_insert_with(|| CropBox::of(&c));
    let origin = [c.x as f64, c.y as f64];
    let mut p = b.numbers(origin, at);
    let how = Fields {
        keep: crate::crop::aspect(&settings, f64::from(c.width) / f64::from(c.height)),
        scale_of: None,
        px_decimals: 0,
        sides: 1.0..=f64::from(crate::dialogs::MAX_SIDE),
        salt,
    };
    if fields(ui, &mut p, &mut unit, &mut chosen, ppi, &how) {
        d.crop = Some(CropBox::from_numbers(&p, origin, at));
        d.revision += 1;
    }
    set_reference(st, chosen);
    if unit != st.unit() {
        st.set_unit(unit);
    }
}

/// The numbers button by a box, which opens and closes its fields beside it (`open`, as
/// [`AppState::box_numbers`] holds it).
pub fn toggle(ui: &mut egui::Ui, open: &mut bool) {
    let tip = if *open {
        "Hide the exact position, size and angle"
    } else {
        "Type the box's exact position, size and angle, in any unit"
    };
    let b = egui::Button::new(crate::theme::icon(crate::icons::NUMBERS, 16.0)).selected(*open);
    if ui.add(b).on_hover_text(tip).clicked() {
        *open = !*open;
    }
}

/// Shows `show` in a panel beside what lies within `corners` on screen (a box, and its
/// toolbar), inside `view`: to its right if there is room, else to its left, else (a box
/// filling the view) in the view's top-right corner; never out of the view.
pub fn panel_by(
    ctx: &egui::Context,
    id: egui::Id,
    view: egui::Rect,
    corners: &[egui::Pos2],
    show: impl FnOnce(&mut egui::Ui),
) {
    // About how wide the fields are, to choose a side before they are laid out.
    const WIDE: f32 = 290.0;
    const GAP: f32 = 16.0;
    let around = egui::Rect::from_points(corners);
    let top = around.top().clamp(view.top() + 4.0, (view.bottom() - 40.0).max(view.top() + 4.0));
    let (pos, pivot) = if around.right() + GAP + WIDE <= view.right() {
        (egui::pos2(around.right() + GAP, top), egui::Align2::LEFT_TOP)
    } else if around.left() - GAP - WIDE >= view.left() {
        (egui::pos2(around.left() - GAP, top), egui::Align2::RIGHT_TOP)
    } else {
        (egui::pos2(view.right() - 4.0, view.top() + 4.0), egui::Align2::RIGHT_TOP)
    };
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).pivot(pivot).constrain_to(view).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, show);
    });
}
