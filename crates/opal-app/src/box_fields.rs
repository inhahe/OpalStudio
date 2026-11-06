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

/// The farthest a box's corner may be typed from the canvas's, in pixels.
const FAR: f64 = 1e7;

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

/// Fields for the box `p` (its corner from the canvas's), in `unit` at `ppi`, with a menu
/// of the units, as a two-column grid. Returns whether the user changed a number.
pub fn fields(ui: &mut egui::Ui, p: &mut Params, unit: &mut Unit, ppi: f64, how: &Fields) -> bool {
    let before = *p;
    egui::Grid::new(("box-numbers", how.salt)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Units");
        lengths::unit_menu(ui, unit, &format!("{}-unit", how.salt));
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
        .on_hover_text("Where the box's top-left corner is, from the canvas's top-left corner.");
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
            .on_hover_text("Clockwise, about the box's top-left corner.");
        ui.end_row();
    });
    *p != before
}

/// The transform box's fields for document `id` (its position, size, scale and angle);
/// what is typed moves the box at once.
pub fn transform(ui: &mut egui::Ui, st: &mut AppState, id: DocId, salt: &str) {
    let aspect = st.prefs.bool("transform.aspect_locked");
    let mut unit = st.unit();
    let Some(d) = st.doc_mut(id) else { return };
    let Some(s) = d.transform.as_ref() else { return };
    let (canvas, ppi) = (d.doc().canvas.rect(), d.doc().resolution);
    let (cx, cy) = (canvas.x0 as f64, canvas.y0 as f64);
    let p = s.params();
    let mut q = Params { x: p.x - cx, y: p.y - cy, ..p };
    let how = Fields {
        keep: aspect.then(|| p.width / p.height),
        scale_of: Some(s.frame.width()),
        px_decimals: 1,
        sides: 0.01..=1e6,
        salt,
    };
    if fields(ui, &mut q, &mut unit, ppi, &how) {
        let m = s.edited(&p, &Params { x: q.x + cx, y: q.y + cy, ..q });
        d.set_box(m, None);
    }
    if unit != st.unit() {
        st.set_unit(unit);
    }
}

/// The crop box's fields for document `id` (its position, size and angle); what is typed
/// moves the box at once. The box's proportions are kept as the tool's aspect setting says.
pub fn crop(ui: &mut egui::Ui, st: &mut AppState, id: DocId, salt: &str) {
    let settings = st.tool_settings(crate::tools::ToolId::Crop).clone();
    let mut unit = st.unit();
    let Some(d) = st.doc_mut(id) else { return };
    let (c, ppi) = (d.doc().canvas, d.doc().resolution);
    let b = *d.crop.get_or_insert_with(|| CropBox::of(&c));
    let origin = [c.x as f64, c.y as f64];
    let mut p = b.numbers(origin);
    let how = Fields {
        keep: crate::crop::aspect(&settings, f64::from(c.width) / f64::from(c.height)),
        scale_of: None,
        px_decimals: 0,
        sides: 1.0..=f64::from(crate::dialogs::MAX_SIDE),
        salt,
    };
    if fields(ui, &mut p, &mut unit, ppi, &how) {
        d.crop = Some(CropBox::from_numbers(&p, origin));
        d.revision += 1;
    }
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

/// Shows `show` in a panel by a box's toolbar, which is at `at` (its top-left corner) and
/// `toolbar_height` tall, inside `view`: below the toolbar if there is room, else above it.
pub fn panel_by(
    ctx: &egui::Context,
    id: egui::Id,
    view: egui::Rect,
    at: egui::Pos2,
    toolbar_height: f32,
    show: impl FnOnce(&mut egui::Ui),
) {
    // About how tall the fields are, to choose a side before they are laid out.
    const TALL: f32 = 170.0;
    let below = at.y + toolbar_height + 6.0;
    let (pos, pivot) = if below + TALL <= view.bottom() {
        (egui::pos2(at.x, below), egui::Align2::LEFT_TOP)
    } else {
        (egui::pos2(at.x, at.y - 6.0), egui::Align2::LEFT_BOTTOM)
    };
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).pivot(pivot).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, show);
    });
}
