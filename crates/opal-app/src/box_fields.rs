//! Exact numbers for the boxes on the canvas: a box's position, size and angle typed in the
//! user's units. The transform box (a paste, Transform) and the crop box show them in the
//! tool settings panel, and in a panel that opens from the numbers button by the box; a
//! box of any later tool (a selection's) is meant to take the same fields.

use crate::crop::CropBox;
use crate::doc::DocId;
use crate::lengths;
use crate::number_field::NumberField;
use crate::state::AppState;
use crate::transform::Params;
use eframe::egui;
use opal_core::document::{Document, Layer, LayerId};
use opal_core::expr::{Own, Scope};
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
    point_at(point_index(id))
}

/// Which of [`POINTS`] is called `id`: its index; the top-left corner's for any other name.
pub fn point_index(id: &str) -> usize {
    POINTS.iter().position(|(p, _)| *p == id).unwrap_or(0)
}

/// The point `POINTS[i]`, as fractions of a box's width and height from its top-left corner.
pub fn point_at(i: usize) -> [f64; 2] {
    assert!(i < POINTS.len(), "a box has no point {i}");
    [(i % 3) as f64 * 0.5, (i / 3) as f64 * 0.5]
}

/// What a box's position counts from: the canvas, or a layer, by the bounds of what it
/// holds. (A vector path's node, a text box's corner and a guide are to come as more.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Base {
    #[default]
    Canvas,
    Layer(LayerId),
}

/// The point a box's position counts from: point `point` of `base`, an index into
/// [`POINTS`] (so less than nine: [`point_at`] asserts it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    pub base: Base,
    pub point: usize,
}

impl Origin {
    /// Where it is in `doc`, in document pixels; `None` if its layer is gone, or holds
    /// nothing.
    pub fn locate(&self, doc: &Document) -> Option<[f64; 2]> {
        let r = match self.base {
            Base::Canvas => doc.canvas.rect().to_f(),
            Base::Layer(id) => doc.layer(id)?.bounds()?,
        };
        let [u, v] = point_at(self.point);
        Some([r.x0 + u * r.width(), r.y0 + v * r.height()])
    }
}

/// What can be counted from in `doc`, as the Layers panel lists layers (the top first, those
/// in a group after it, indented): the canvas, and the layers that hold something.
pub fn bases(doc: &Document) -> Vec<(Base, String)> {
    let mut out = vec![(Base::Canvas, "Canvas".to_owned())];
    fn walk(layers: &[std::sync::Arc<Layer>], depth: usize, out: &mut Vec<(Base, String)>) {
        for l in layers.iter().rev() {
            if l.bounds().is_some() {
                out.push((Base::Layer(l.id), format!("{}{}", "   ".repeat(depth), l.name)));
            }
            if let Some(children) = l.children() {
                walk(children, depth + 1, out);
            }
        }
    }
    walk(&doc.layers, 0, &mut out);
    out
}

/// What `base` is called in `doc`: "Canvas", or its layer's name.
fn base_name(doc: &Document, base: Base) -> String {
    match base {
        Base::Canvas => "Canvas".to_owned(),
        Base::Layer(id) => doc.layer(id).map_or_else(|| "Canvas".to_owned(), |l| l.name.clone()),
    }
}

/// How a box's numbers are measured, which its fields can change: in what unit, by which of
/// the box's points (an index into [`POINTS`]), and from what.
pub struct Measure {
    pub unit: Unit,
    pub reference: usize,
    pub origin: Origin,
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

/// Fields for the box `p` (its position that of its point `POINTS[m.reference]`, from
/// `m.origin`), in `m.unit` at `ppi`, with a menu of the units, a grid of the box's points,
/// and what the position counts from (the canvas or a layer of `doc`, the image as it was
/// before the box opened, and its point), as a two-column grid. Returns whether the user
/// changed a number (choosing another point, or another origin, changes none: the caller
/// shows the box measured so next).
pub fn fields(ui: &mut egui::Ui, p: &mut Params, m: &mut Measure, doc: &Document, ppi: f64, how: &Fields) -> bool {
    let before = *p;
    // What is typed is worked out with positions from where `p`'s are (before another origin
    // is chosen), and the box as it is.
    let counted = m.origin.locate(doc).unwrap_or([0.0, 0.0]);
    let point = POINTS[m.reference].1.to_lowercase();
    let name = base_name(doc, m.origin.base);
    let of = match m.origin.base {
        Base::Canvas => "the canvas".to_owned(),
        Base::Layer(_) => format!("“{name}”"),
    };
    let from = format!("{} of {of}", POINTS[m.origin.point].1.to_lowercase());
    egui::Grid::new(("box-numbers", how.salt)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Units");
        lengths::unit_menu(ui, &mut m.unit, &format!("{}-unit", how.salt));
        ui.end_row();
        ui.label("Reference").on_hover_text(
            "Which point of the box the position gives, and what a size or angle typed scales and turns it about.",
        );
        reference_grid(ui, &mut m.reference, None, how.salt);
        ui.end_row();
        ui.label("From").on_hover_text(
            "What the position counts from: a point of the canvas, or of what a layer holds (its bounds).",
        );
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("box-from", how.salt))
                .selected_text(name.as_str())
                .width(110.0)
                .show_ui(ui, |ui| {
                    // Listed only while the menu is open: a walk of every layer's bounds.
                    for (b, name) in bases(doc) {
                        ui.selectable_value(&mut m.origin.base, b, name);
                    }
                })
                .response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "From"));
            reference_grid(ui, &mut m.origin.point, Some("From"), &format!("{}-from", how.salt));
        });
        ui.end_row();
        let scope = Scope { doc: Some(doc), ppi, unit: m.unit, origin: counted, own: Some(own(&before)) };
        let field = |px: &mut f64, range, name: &str, ui: &mut egui::Ui| {
            lengths::length_field(px, scope, range, how.px_decimals).remember(("box", name)).show(ui)
        };
        ui.label("Position");
        ui.horizontal(|ui| {
            ui.label("x");
            field(&mut p.x, -FAR..=FAR, "x", ui);
            ui.label("y");
            field(&mut p.y, -FAR..=FAR, "y", ui);
        })
        .response
        .on_hover_text(format!("Where the box's {point} is, from the {from}."));
        ui.end_row();
        ui.label("Size");
        ui.horizontal(|ui| {
            let w = field(&mut p.width, how.sides.clone(), "width", ui);
            ui.label("×");
            let h = field(&mut p.height, how.sides.clone(), "height", ui);
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
            let pick = NumberField::number(&mut pct, scope).speed(0.5).range(0.01..=1e6).suffix("%").decimals(0, 2);
            let r = ui.add(pick.remember(("box", "scale")));
            if r.changed() {
                let k = pct / 100.0 * own / p.width;
                p.width *= k;
                p.height *= k;
            }
            ui.end_row();
        }
        ui.label("Angle");
        ui.add(NumberField::angle(&mut p.angle, scope).speed(0.5).range(-360.0..=360.0).remember(("box", "angle")))
            .on_hover_text(format!("Clockwise, about the box's {point}."));
        ui.end_row();
    });
    *p != before
}

/// The box `p`, as an expression names its numbers.
fn own(p: &Params) -> Own {
    Own { x: p.x, y: p.y, width: p.width, height: p.height, angle: p.angle }
}

/// A 3 × 3 grid of a box's points (see [`POINTS`]), the one at `chosen` marked; clicking
/// another chooses it. Each is read out by its name, after `what` the grid chooses if it
/// says ("From: Center"). (Resize Canvas's anchor is chosen with it too.)
pub fn reference_grid(ui: &mut egui::Ui, chosen: &mut usize, what: Option<&str>, salt: &str) {
    egui::Grid::new(("box-reference", salt)).spacing([1.0, 1.0]).show(ui, |ui| {
        for row in 0..3 {
            for col in 0..3 {
                let i = row * 3 + col;
                let on = *chosen == i;
                let glyph = if on { "●" } else { "○" };
                let r = ui
                    .add(egui::Button::new(glyph).small().selected(on).min_size(egui::vec2(16.0, 16.0)))
                    .on_hover_text(POINTS[i].1);
                let name = what.map_or_else(|| POINTS[i].1.to_owned(), |w| format!("{w}: {}", POINTS[i].1));
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, &name));
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
    point_index(&st.prefs.choice("transform.reference"))
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
    let (unit, reference) = (st.unit(), reference(st));
    let at = point_at(reference);
    let Some(d) = st.doc_mut(id) else { return };
    let Some(s) = d.transform.as_ref() else { return };
    let (canvas, ppi) = (d.doc().canvas.rect(), d.doc().resolution);
    // Counted from as it was before the box opened: a layer the box moves, from where it was.
    let before = d.history.doc();
    let origin = counted_from(d.origin, before);
    let [cx, cy] = origin.locate(before).expect("counted_from gives one there");
    let mut m = Measure { unit, reference, origin };
    let p = s.params_at(at);
    let mut q = Params { x: p.x - cx, y: p.y - cy, ..p };
    let how = Fields {
        keep: aspect.then(|| p.width / p.height),
        scale_of: Some(s.frame.width()),
        px_decimals: 1,
        sides: 0.01..=1e6,
        salt,
    };
    let typed = fields(ui, &mut q, &mut m, before, ppi, &how);
    if typed {
        let moved = s.edited(&p, &Params { x: q.x + cx, y: q.y + cy, ..q }, at);
        d.set_box(moved, None);
    }
    // Kept only when chosen: one shown in its stead (a layer gone) is not the user's choice.
    if m.origin != origin {
        d.origin = m.origin;
    }
    let before = d.history.doc();
    let scope = Scope { doc: Some(before), ppi, unit: m.unit, origin: [cx, cy], own: Some(own(&q)) };
    let rect = Rect { canvas: canvas.to_f(), from: [cx, cy], at, scope };
    if let Some(fit) = fit_into(ui, &mut d.fit_rect, &rect, salt)
        && let Err(why) = crate::api::place_box(d, &fit)
    {
        st.notify(why);
    }
    set_reference(st, m.reference);
    if m.unit != st.unit() {
        st.set_unit(m.unit);
    }
}

/// `origin`, if it can be counted from in `doc` (its layer there and holding something);
/// else the canvas's top-left corner.
fn counted_from(origin: Origin, doc: &Document) -> Origin {
    if origin.locate(doc).is_some() { origin } else { Origin::default() }
}

/// What the rectangle to fit a box into is measured in and from: the canvas (its default),
/// the point positions count from (`from`, in the document), the box's point `at` that
/// positions give, and what is typed is worked out in (its unit and resolution those the
/// fields show).
struct Rect<'a> {
    canvas: opal_core::geom::FRect,
    from: [f64; 2],
    at: [f64; 2],
    scope: Scope<'a>,
}

/// A rectangle to fit the transform box into (`rect`: its top-left corner from the canvas's,
/// and its size; the canvas, whatever it is then, until one is typed), shown as `how` says,
/// with buttons that fit the box inside it or make it fill it (as it is turned, keeping its
/// proportions, centered on it), or make it exactly the rectangle, upright, as scripts' `fit`
/// does. What a button asks for, as a script's arguments would.
fn fit_into(ui: &mut egui::Ui, rect: &mut Option<[f64; 4]>, how: &Rect, salt: &str) -> Option<crate::api::BoxArgs> {
    use crate::api::{BoxArgs, Fit};
    let (canvas, at, scope) = ([how.canvas.width(), how.canvas.height()], how.at, how.scope);
    let field = |px: &mut f64, range, name: &str, ui: &mut egui::Ui| {
        lengths::length_field(px, scope, range, 1).remember(("fit", name)).show(ui).changed()
    };
    // Kept from the canvas's corner; shown from the point positions count from.
    let shift = [how.canvas.x0 - how.from[0], how.canvas.y0 - how.from[1]];
    let [x0, y0, w0, h0] = rect.unwrap_or([0.0, 0.0, canvas[0], canvas[1]]);
    let (mut x, mut y, mut w, mut h) = (x0 + at[0] * w0 + shift[0], y0 + at[1] * h0 + shift[1], w0, h0);
    let (mut fit, mut canvas_again, mut edited) = (None, false, false);
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Fit into a rectangle").id_salt(("fit-into", salt)).default_open(true).show(ui, |ui| {
        egui::Grid::new(("fit-rect", salt)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.label("x");
                edited |= field(&mut x, -FAR..=FAR, "x", ui);
                ui.label("y");
                edited |= field(&mut y, -FAR..=FAR, "y", ui);
            });
            ui.end_row();
            ui.label("Size");
            ui.horizontal(|ui| {
                edited |= field(&mut w, 0.01..=1e6, "width", ui);
                ui.label("×");
                edited |= field(&mut h, 0.01..=1e6, "height", ui);
            });
            ui.end_row();
        });
        ui.horizontal_wrapped(|ui| {
            let ways = [
                ("Fit inside", Fit::Inside, "As large as fits inside the rectangle, in its own proportions, centered."),
                ("Fill", Fit::Cover, "As small as covers the rectangle, in its own proportions, centered."),
                ("Stretch", Fit::Stretch, "Exactly the rectangle, upright, its proportions changed if need be."),
            ];
            for (name, way, tip) in ways {
                if ui.button(name).on_hover_text(tip).clicked() {
                    fit = Some(way);
                }
            }
            if ui.button("Canvas").on_hover_text("Make the rectangle the canvas again.").clicked() {
                canvas_again = true;
            }
        });
    });
    let typed = [x - shift[0] - at[0] * w, y - shift[1] - at[1] * h, w, h];
    if canvas_again {
        // The canvas, whatever it is when the box is fitted to it.
        *rect = None;
        (x, y, w, h) = (at[0] * canvas[0] + shift[0], at[1] * canvas[1] + shift[1], canvas[0], canvas[1]);
    } else if edited {
        *rect = Some(typed);
    }
    // Stretched, the box is exactly the rectangle typed, so upright; fitted, it keeps its turn.
    let angle = (fit == Some(Fit::Stretch)).then_some(0.0);
    let from = how.from;
    fit.map(|fit| BoxArgs { x: Some(x), y: Some(y), width: Some(w), height: Some(h), angle, fit, at, from })
}

/// The crop box's fields for document `id` (its position, size and angle); what is typed
/// moves the box at once. The box's proportions are kept as the tool's aspect setting says.
pub fn crop(ui: &mut egui::Ui, st: &mut AppState, id: DocId, salt: &str) {
    let settings = st.tool_settings(crate::tools::ToolId::Crop).clone();
    let (unit, reference) = (st.unit(), reference(st));
    let at = point_at(reference);
    let Some(d) = st.doc_mut(id) else { return };
    let (c, ppi) = (d.doc().canvas, d.doc().resolution);
    let b = *d.crop.get_or_insert_with(|| CropBox::of(&c));
    let doc = d.history.doc();
    let origin = counted_from(d.origin, doc);
    let from = origin.locate(doc).expect("counted_from gives one there");
    let mut m = Measure { unit, reference, origin };
    let mut p = b.numbers(from, at);
    let how = Fields {
        keep: crate::crop::aspect(&settings, f64::from(c.width) / f64::from(c.height)),
        scale_of: None,
        px_decimals: 0,
        sides: 1.0..=f64::from(crate::dialogs::MAX_SIDE),
        salt,
    };
    if fields(ui, &mut p, &mut m, doc, ppi, &how) {
        d.crop = Some(CropBox::from_numbers(&p, from, at));
        d.revision += 1;
    }
    if m.origin != origin {
        d.origin = m.origin;
    }
    set_reference(st, m.reference);
    if m.unit != st.unit() {
        st.set_unit(m.unit);
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

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::document::{IdGen, Position};
    use opal_core::geom::IRect;
    use opal_core::raster::RasterImage;
    use opal_core::tile::pixel_from_f32;

    #[test]
    fn positions_count_from_the_canvas_or_a_layer_the_box_does_not_move() {
        let mut ids = IdGen::default();
        let mut doc = Document::new(100, 100);
        let filled = |r: IRect| RasterImage::filled(&r, pixel_from_f32([1.0; 4]));
        let top = |index| Position { parent: None, index };
        let x = ids.layer();
        doc.insert(top(0), Layer::new_raster(x, ids.object(), "x", filled(IRect::new(0, 0, 10, 10)))).unwrap();
        let g = ids.layer();
        doc.insert(top(1), Layer::new_group(g, "g")).unwrap();
        let (a, b) = (ids.layer(), ids.layer());
        let inside = |index| Position { parent: Some(g), index };
        doc.insert(inside(0), Layer::new_raster(a, ids.object(), "a", filled(IRect::new(20, 30, 40, 50)))).unwrap();
        doc.insert(inside(1), Layer::new_raster(b, ids.object(), "b", filled(IRect::new(60, 0, 70, 5)))).unwrap();
        doc.insert(top(2), Layer::new_empty(ids.layer(), "empty")).unwrap();
        // As the Layers panel lists them, the top first; an empty layer is nothing to count from.
        let names = |bases: Vec<(Base, String)>| bases.into_iter().map(|(_, n)| n).collect::<Vec<_>>();
        assert_eq!(names(bases(&doc)), ["Canvas", "g", "   b", "   a", "x"]);
        // A layer's points are its bounds'.
        let center_of_a = Origin { base: Base::Layer(a), point: 4 };
        assert_eq!(center_of_a.locate(&doc), Some([30.0, 40.0]));
        let canvas_end = Origin { base: Base::Canvas, point: 8 };
        assert_eq!(canvas_end.locate(&doc), Some([100.0, 100.0]));
        // One not to count from any more (its layer gone) is the canvas's corner.
        let gone = Origin { base: Base::Layer(LayerId(999)), point: 2 };
        assert_eq!(counted_from(gone, &doc), Origin::default());
        assert_eq!(counted_from(center_of_a, &doc), center_of_a);
        let _ = b;
    }
}
