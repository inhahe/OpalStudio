//! Dialogs and windows: messages, unsaved-changes questions, New Image, Export,
//! Preferences, About and the widget gallery.

use crate::icons;
use crate::lengths;
use crate::prefs::SECTIONS;
use crate::settings::{self, Show};
use crate::state::{AppState, Background, Confirm, ExportKind, MessageKind, NewImage};
use eframe::egui::{self, RichText, ViewportCommand};
use opal_core::units::{ResolutionUnit, Unit};
use opal_io::bytes;

/// The largest canvas side (Photoshop's limit, for its large-document format). A layer
/// filling a canvas this size would hold 720 GB, at 8 bytes a pixel: a canvas this large
/// is for content that leaves most of it empty or uniform (whose tiles cost nothing), and
/// work needing more memory than is free is refused (see `memory`).
pub const MAX_SIDE: u32 = 300_000;

pub fn show_all(ctx: &egui::Context, st: &mut AppState) {
    // Only one modal at a time: the first that has something to show.
    if st.confirm.is_some() {
        confirm(ctx, st);
    } else if !st.messages.is_empty() {
        message(ctx, st);
    } else if st.new_image.is_some() {
        new_image(ctx, st);
    } else if st.export.is_some() {
        export(ctx, st);
    } else if st.resize_image.is_some() {
        resize_image(ctx, st);
    } else if st.resize_canvas.is_some() {
        resize_canvas(ctx, st);
    } else if st.rotate_by.is_some() {
        rotate_by(ctx, st);
    }
    preferences(ctx, st);
    about(ctx, st);
    crate::assistant::window(ctx, st);
    gallery(ctx, st);
}

fn message(ctx: &egui::Context, st: &mut AppState) {
    let Some(m) = st.messages.front().cloned() else { return };
    let mut close = false;
    egui::Modal::new(egui::Id::unique("message")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        let (glyph, color) = match m.kind {
            MessageKind::Info => (icons::INFO, st.palette.accent),
            MessageKind::Warning => (icons::WARNING, st.palette.changed),
            MessageKind::Error => (icons::ERROR, ui.visuals().error_fg_color),
        };
        ui.horizontal(|ui| {
            ui.label(crate::theme::icon(glyph, 22.0).color(color));
            ui.heading(&m.title);
        });
        ui.add_space(4.0);
        ui.label(&m.body);
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("OK").clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape))
            {
                close = true;
            }
        });
    });
    if close {
        st.messages.pop_front();
    }
}

/// An answer to a question about unsaved changes.
enum Answer {
    Save,
    Discard,
    Cancel,
}

fn confirm(ctx: &egui::Context, st: &mut AppState) {
    let Some(c) = st.confirm.clone() else { return };
    let names = |ids: &[crate::doc::DocId]| -> Vec<String> {
        ids.iter().filter_map(|id| st.doc(*id)).map(|d| d.title.clone()).collect()
    };
    let mut choice: Option<Answer> = None;
    egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        match &c {
            Confirm::Close(id) => {
                let n = names(&[*id]);
                ui.heading(format!("Save changes to \"{}\"?", n.first().cloned().unwrap_or_default()));
                ui.label("Your changes will be lost if you close it without saving.");
            }
            Confirm::Exit(ids) => {
                ui.heading("Save changes before quitting?");
                ui.label("These images have unsaved changes:");
                for n in names(ids) {
                    ui.label(format!("  • {n}"));
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let save = if matches!(c, Confirm::Exit(_)) { "Save all" } else { "Save" };
            if ui.button(save).clicked() {
                choice = Some(Answer::Save);
            }
            if ui.button("Don't save").clicked() {
                choice = Some(Answer::Discard);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                choice = Some(Answer::Cancel);
            }
        });
    });
    let Some(choice) = choice else { return };
    st.confirm = None;
    // Saving closes (or quits) once written; a save that is cancelled or fails stops it (for
    // several documents, at the first one not saved).
    use crate::writer::Then;
    match (c, choice) {
        (Confirm::Close(id), Answer::Save) => {
            st.save(id, false, Then::Close, ctx);
        }
        (Confirm::Close(id), Answer::Discard) => st.close_now(id),
        (Confirm::Exit(ids), Answer::Save) => {
            // Quits once they are written (see `poll_writes`), unless one is not saved.
            st.exit_after_writes = ids.iter().all(|&id| st.save(id, false, Then::Nothing, ctx));
        }
        (Confirm::Exit(_), Answer::Discard) => {
            st.exit_confirmed = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        _ => {}
    }
}

/// A size the New Image dialog offers: its name, its width and height in `unit`, and the
/// resolution it is made at.
struct Preset {
    name: &'static str,
    size: [f64; 2],
    unit: Unit,
    ppi: f64,
}

const fn preset(name: &'static str, w: f64, h: f64, unit: Unit, ppi: f64) -> Preset {
    Preset { name, size: [w, h], unit, ppi }
}

/// The sizes the New Image dialog offers, by row.
const PRESETS: &[(&str, &[Preset])] = &[
    (
        "Screen",
        &[
            preset("HD", 1920.0, 1080.0, Unit::Px, 72.0),
            preset("4K", 3840.0, 2160.0, Unit::Px, 72.0),
            preset("Square", 2048.0, 2048.0, Unit::Px, 72.0),
        ],
    ),
    (
        "Paper",
        &[
            preset("A5", 148.0, 210.0, Unit::Mm, 300.0),
            preset("A4", 210.0, 297.0, Unit::Mm, 300.0),
            preset("A3", 297.0, 420.0, Unit::Mm, 300.0),
            preset("Letter", 8.5, 11.0, Unit::In, 300.0),
            preset("Legal", 8.5, 14.0, Unit::In, 300.0),
            preset("Tabloid", 11.0, 17.0, Unit::In, 300.0),
        ],
    ),
    (
        "Photo",
        &[
            preset("4 × 6", 4.0, 6.0, Unit::In, 300.0),
            preset("5 × 7", 5.0, 7.0, Unit::In, 300.0),
            preset("8 × 10", 8.0, 10.0, Unit::In, 300.0),
        ],
    ),
];

fn new_image(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.new_image.clone() else { return };
    let (mut unit, mut per) = (st.unit(), st.resolution_unit());
    let mut done: Option<bool> = None;
    egui::Modal::new(egui::Id::unique("new-image")).show(ctx, |ui| {
        ui.heading("New image");
        ui.add_space(4.0);
        egui::Grid::new("new-image-grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Width");
            ui.horizontal(|ui| {
                lengths::side(ui, &mut spec.width, unit, spec.resolution);
                lengths::unit_menu(ui, &mut unit, "new-image-unit");
            });
            ui.end_row();
            ui.label("Height");
            ui.horizontal(|ui| {
                lengths::side(ui, &mut spec.height, unit, spec.resolution);
                if ui
                    .small_button(crate::theme::icon(icons::SWAP, 13.0))
                    .on_hover_text("Swap width and height")
                    .clicked()
                {
                    std::mem::swap(&mut spec.width, &mut spec.height);
                }
            });
            ui.end_row();
            ui.label("Resolution");
            ui.horizontal(|ui| {
                let before = spec.resolution;
                // A size shown in a unit of length stays that size; one in pixels, those pixels.
                if lengths::resolution(ui, &mut spec.resolution, &mut per, "new-image-ppi") && unit != Unit::Px {
                    let id = egui::Id::unique("new-image-print");
                    hold_print_size(ui, id, (&mut spec.width, &mut spec.height), before, spec.resolution);
                }
            });
            ui.end_row();
            for (row, presets) in PRESETS {
                ui.label(*row);
                ui.horizontal_wrapped(|ui| {
                    for &Preset { name, size: [w, h], unit: in_unit, ppi } in *presets {
                        let (pw, ph) =
                            (lengths::whole_side(in_unit.to_px(w, ppi)), lengths::whole_side(in_unit.to_px(h, ppi)));
                        let tip = if in_unit == Unit::Px {
                            format!("{pw} × {ph} px")
                        } else {
                            format!("{w} × {h} {in_unit} at {ppi} pixels/inch: {pw} × {ph} px")
                        };
                        if ui.small_button(name).on_hover_text(tip).clicked() {
                            (spec.width, spec.height, spec.resolution) = (pw, ph, ppi);
                        }
                    }
                });
                ui.end_row();
            }
            ui.label("Background");
            ui.horizontal(|ui| {
                ui.radio_value(&mut spec.background, Background::White, "White");
                ui.radio_value(&mut spec.background, Background::Transparent, "Transparent");
            });
            ui.end_row();
        });
        let note = size_note(spec.width, spec.height, crate::memory::free());
        ui.label(RichText::new(note).small().color(st.palette.weak_text));
        let print = lengths::print_note(spec.width, spec.height, spec.resolution, unit, per);
        ui.label(RichText::new(print).small().color(st.palette.weak_text));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Create").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    });
    remember_units(st, unit, per);
    match done {
        Some(true) => {
            st.new_image = None;
            st.create_image(&NewImage {
                width: spec.width.clamp(1, MAX_SIDE),
                height: spec.height.clamp(1, MAX_SIDE),
                ..spec
            });
        }
        Some(false) => st.new_image = None,
        None => st.new_image = Some(spec),
    }
}

/// Makes `unit` and `per` the units lengths and resolutions are shown in, if a dialog
/// changed them.
fn remember_units(st: &mut AppState, unit: Unit, per: ResolutionUnit) {
    if unit != st.unit() {
        st.set_unit(unit);
    }
    if per != st.resolution_unit() {
        st.set_resolution_unit(per);
    }
}

/// Changes `size` with the resolution, from `before` to `after` pixels per inch, so that it
/// prints as large. It works from the print size it had when the resolution began to
/// change (kept under `id`), not from the size made of it at each step, so that rounding to
/// whole pixels does not add up over a drag; a size or resolution changed since otherwise
/// is taken afresh.
fn hold_print_size(ui: &egui::Ui, id: egui::Id, size: (&mut u32, &mut u32), before: f64, after: f64) {
    let (w, h) = size;
    // The print size, and the pixel size and resolution last made of it.
    let held: Option<([f64; 2], [u32; 2], f64)> = ui.data(|d| d.get_temp(id));
    let print = match held {
        Some((print, made, at)) if made == [*w, *h] && at == before => print,
        _ => [f64::from(*w) / before, f64::from(*h) / before],
    };
    (*w, *h) = (lengths::whole_side(print[0] * after), lengths::whole_side(print[1] * after));
    ui.data_mut(|d| d.insert_temp(id, (print, [*w, *h], after)));
}

/// What the New Image dialog says of a `width × height` canvas: its pixels, and the memory
/// a layer covering it takes, which on a large canvas may be more than `free` has room for.
fn size_note(width: u32, height: u32, free: Option<crate::memory::Free>) -> String {
    let pixels = u64::from(width) * u64::from(height);
    let full = pixels * 8;
    let mut note =
        format!("{:.1} megapixels; a layer covering it all takes about {} of memory", pixels as f64 / 1e6, bytes(full));
    if let Some(free) = free
        && let Err(short) = free.check(crate::memory::Need::tiles(full))
    {
        let room = if short.disk { "free in memory and on disk" } else { "free" };
        note += &format!(
            " (more than the {} {room}: fill only part of it; a white background costs next to nothing)",
            bytes(short.have)
        );
    }
    note
}

fn export(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.export.clone() else { return };
    let mut done: Option<bool> = None;
    let palette = st.palette;
    let (mut unit, per) = (st.unit(), st.resolution_unit());
    egui::Modal::new(egui::Id::unique("export")).show(ctx, |ui| {
        ui.heading("Export");
        ui.label(
            "Writes the canvas, with the visible layers combined, to an image file. The image itself keeps its layers.",
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Format");
            ui.radio_value(&mut spec.kind, ExportKind::Png, "PNG");
            ui.radio_value(&mut spec.kind, ExportKind::Jpeg, "JPEG");
        });
        ui.add_space(4.0);
        if let Some((c, ppi)) = st.doc(spec.doc).map(|d| (d.doc().canvas, d.doc().resolution)) {
            egui::Grid::new("export-size").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                let size = (&mut spec.width, &mut spec.height);
                size_fields(ui, size, &mut spec.keep_aspect, (c.width, c.height), &mut unit, ppi);
            });
            if (spec.width, spec.height) != (c.width, c.height) {
                ui.label(
                    RichText::new(
                        "Resized as it is written, with the resampling below; the image itself stays as it is.",
                    )
                    .small()
                    .color(palette.weak_text),
                );
            }
            let note = format!(
                "The file records the image's resolution. {}",
                lengths::print_note(spec.width, spec.height, ppi, unit, per)
            );
            ui.label(RichText::new(note).small().color(palette.weak_text));
        }
        let changed = settings::settings_ui(ui, &mut st.prefs, Show::Section("Export"), &palette, "export");
        if changed {
            st.settings_touched();
        }
        let max = opal_io::image_files::JPEG_MAX_SIDE;
        let too_large = spec.kind == ExportKind::Jpeg && spec.width.max(spec.height) > max;
        if too_large {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "A JPEG image can be at most {max} pixels wide and tall: choose PNG, or a smaller size."
                ))
                .color(ui.visuals().error_fg_color),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!too_large, egui::Button::new("Export…")).clicked() {
                done = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    });
    remember_units(st, unit, per);
    match done {
        Some(true) => {
            st.export = None;
            st.export_to_file(&spec, ctx);
            // Remember the format for next time.
            st.last_export = spec.kind;
        }
        Some(false) => st.export = None,
        None => st.export = Some(spec),
    }
}

/// Width and height fields of `size`, in `unit` at `ppi` and in percent of `original`, kept
/// in proportion while `keep` is set, with a menu of the units. For a two-column grid.
fn size_fields(
    ui: &mut egui::Ui,
    size: (&mut u32, &mut u32),
    keep: &mut bool,
    original: (u32, u32),
    unit: &mut Unit,
    ppi: f64,
) {
    let (w, h) = size;
    let (ow, oh) = (f64::from(original.0), f64::from(original.1));
    let before = (*w, *h);
    ui.label("Width");
    ui.horizontal(|ui| {
        lengths::side(ui, w, *unit, ppi);
        let mut pct = f64::from(*w) / ow * 100.0;
        if ui.add(egui::DragValue::new(&mut pct).range(0.01..=10_000.0).suffix("%").max_decimals(2)).changed() {
            *w = lengths::whole_side(ow * pct / 100.0);
        }
        lengths::unit_menu(ui, unit, "size-unit");
    });
    ui.end_row();
    ui.label("Height");
    ui.horizontal(|ui| {
        lengths::side(ui, h, *unit, ppi);
        let mut pct = f64::from(*h) / oh * 100.0;
        if ui.add(egui::DragValue::new(&mut pct).range(0.01..=10_000.0).suffix("%").max_decimals(2)).changed() {
            *h = lengths::whole_side(oh * pct / 100.0);
        }
    });
    ui.end_row();
    ui.label("");
    ui.checkbox(keep, "Keep the aspect ratio");
    ui.end_row();
    if *keep {
        if *w != before.0 {
            *h = lengths::whole_side(f64::from(*w) * oh / ow);
        } else if *h != before.1 {
            *w = lengths::whole_side(f64::from(*h) * ow / oh);
        }
    }
}

/// OK and Cancel (also Enter and Esc): `Some(true)` for OK.
fn ok_cancel(ui: &mut egui::Ui, ok: &str) -> Option<bool> {
    let mut done = None;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(ok).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            done = Some(true);
        }
        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = Some(false);
        }
    });
    done
}

fn resize_image(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.resize_image.clone() else { return };
    let Some((c, locked)) = st.active().map(|d| (d.doc().canvas, crate::commands::locked_layer(d.doc()))) else {
        st.resize_image = None;
        return;
    };
    let palette = st.palette;
    let (mut unit, mut per) = (st.unit(), st.resolution_unit());
    let mut done = None;
    egui::Modal::new(egui::Id::unique("resize-image")).show(ctx, |ui| {
        ui.heading("Resize image");
        ui.label("Scales everything in the image to a new size, or gives it a new resolution.");
        ui.add_space(4.0);
        egui::Grid::new("resize-image-size").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            let size = (&mut spec.width, &mut spec.height);
            size_fields(ui, size, &mut spec.keep_aspect, (c.width, c.height), &mut unit, spec.resolution);
            ui.label("Resolution");
            ui.horizontal(|ui| {
                let before = spec.resolution;
                if lengths::resolution(ui, &mut spec.resolution, &mut per, "resize-ppi") && spec.keep_print_size {
                    let id = egui::Id::unique("resize-print");
                    hold_print_size(ui, id, (&mut spec.width, &mut spec.height), before, spec.resolution);
                }
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut spec.keep_print_size, "Keep the print size").on_hover_text(
                "On, a new resolution changes the size in pixels with it, so the image prints as large as \
                 before. Off, the pixels stay as they are, and the image prints larger or smaller.",
            );
            ui.end_row();
        });
        let print = lengths::print_note(spec.width, spec.height, spec.resolution, unit, per);
        ui.label(RichText::new(print).small().color(palette.weak_text));
        if let Some(name) = &locked
            && (spec.width, spec.height) != (c.width, c.height)
        {
            let why = format!("\"{name}\" is locked: only the resolution can change while it is.");
            ui.label(RichText::new(why).color(ui.visuals().error_fg_color));
        }
        ui.add_space(4.0);
        if settings::settings_ui(ui, &mut st.prefs, Show::Section("Resize"), &palette, "resize") {
            st.settings_touched();
        }
        let note = if st.prefs.bool("resize.resample") {
            "Pixel layers will be resampled at the new size now: a smaller image takes less memory and a \
             smaller file, and enlarging it again later cannot bring the detail back."
        } else {
            "Layers keep their pixels and are drawn at the new size, so resizing back later loses nothing; \
             the file stays as large as it was."
        };
        ui.label(RichText::new(note).small().color(palette.weak_text));
        done = ok_cancel(ui, "Resize");
    });
    remember_units(st, unit, per);
    match done {
        Some(true) => {
            st.resize_image = None;
            let resample = st.prefs.bool("resize.resample");
            st.resize_image(&spec, resample);
        }
        Some(false) => st.resize_image = None,
        None => st.resize_image = Some(spec),
    }
}

fn resize_canvas(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.resize_canvas.clone() else { return };
    let Some((c, ppi)) = st.active().map(|d| (d.doc().canvas, d.doc().resolution)) else {
        st.resize_canvas = None;
        return;
    };
    let palette = st.palette;
    let mut unit = st.unit();
    let mut done = None;
    egui::Modal::new(egui::Id::unique("resize-canvas")).show(ctx, |ui| {
        ui.heading("Resize canvas");
        ui.label("Changes the canvas, not what is on it: nothing past its new edges is removed.");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.radio_value(&mut spec.by_margins, false, "New size");
            ui.radio_value(&mut spec.by_margins, true, "Add to each side");
            ui.add_space(8.0);
            lengths::unit_menu(ui, &mut unit, "canvas-unit");
        });
        ui.add_space(4.0);
        if spec.by_margins {
            egui::Grid::new("resize-canvas-margins").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                for (name, i) in [("Left", 0), ("Top", 1), ("Right", 2), ("Bottom", 3)] {
                    ui.label(name);
                    let side = i64::from(MAX_SIDE);
                    lengths::whole(ui, &mut spec.margins[i], unit, ppi, -side..=side);
                    ui.end_row();
                }
            });
            ui.label(RichText::new("Negative numbers cut that side.").small().color(palette.weak_text));
        } else {
            egui::Grid::new("resize-canvas-size").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Width");
                lengths::side(ui, &mut spec.width, unit, ppi);
                ui.end_row();
                ui.label("Height");
                lengths::side(ui, &mut spec.height, unit, ppi);
                ui.end_row();
                ui.label("Anchor").on_hover_text("Where the old canvas sits in the new one.");
                egui::Grid::new("anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                    for y in 0..3u8 {
                        for x in 0..3u8 {
                            let on = spec.anchor == [x, y];
                            let glyph = if on { "●" } else { "○" };
                            if ui.add(egui::Button::new(glyph).selected(on).min_size(egui::vec2(22.0, 22.0))).clicked()
                            {
                                spec.anchor = [x, y];
                            }
                        }
                        ui.end_row();
                    }
                });
                ui.end_row();
            });
        }
        let n = spec.canvas(&c);
        let (w, h) = (f64::from(n.width), f64::from(n.height));
        let mut size = format!("{} × {} px", n.width, n.height);
        if unit != Unit::Px {
            size += &format!(" ({})", lengths::show_size(w, h, unit, ppi));
        }
        ui.label(RichText::new(format!("The canvas will be {size}.")).small().color(palette.weak_text));
        done = ok_cancel(ui, "Resize");
    });
    if unit != st.unit() {
        st.set_unit(unit);
    }
    match done {
        Some(true) => {
            st.resize_canvas = None;
            st.resize_canvas(&spec);
        }
        Some(false) => st.resize_canvas = None,
        None => st.resize_canvas = Some(spec),
    }
}

fn rotate_by(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.rotate_by.clone() else { return };
    let palette = st.palette;
    let mut done = None;
    egui::Modal::new(egui::Id::unique("rotate-by")).show(ctx, |ui| {
        ui.heading("Rotate by angle");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Angle");
            ui.add(
                egui::DragValue::new(&mut spec.degrees).range(-360.0..=360.0).speed(0.25).suffix("°").max_decimals(2),
            );
            ui.label(RichText::new("clockwise").color(palette.weak_text));
        });
        ui.checkbox(&mut spec.grow, "Grow the canvas so nothing is cut off");
        ui.label(
            RichText::new("Layers keep their pixels, so turning again later loses nothing.")
                .small()
                .color(palette.weak_text),
        );
        done = ok_cancel(ui, "Rotate");
    });
    match done {
        Some(true) => {
            st.rotate_by = None;
            if spec.degrees != 0.0 {
                st.rotate_by(&spec);
            }
        }
        Some(false) => st.rotate_by = None,
        None => st.rotate_by = Some(spec),
    }
}

fn preferences(ctx: &egui::Context, st: &mut AppState) {
    if !st.prefs_open {
        return;
    }
    let mut open = true;
    let mut changed = false;
    let palette = st.palette;
    egui::Window::new(format!("{}  Preferences", icons::SETTINGS))
        .open(&mut open)
        .default_pos(ctx.content_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .default_size([560.0, 420.0])
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    for (i, s) in SECTIONS.iter().enumerate() {
                        let n =
                            st.prefs.defs().iter().filter(|d| d.section == *s && st.prefs.is_changed(d.key)).count();
                        let label = if n > 0 { format!("{s}  ●") } else { (*s).to_owned() };
                        ui.selectable_value(&mut st.prefs_section, i, label);
                    }
                    ui.add_space(12.0);
                    if ui.button("Reset all").on_hover_text("Put every preference back to its default").clicked() {
                        st.prefs.reset_all();
                        changed = true;
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let section = SECTIONS[st.prefs_section.min(SECTIONS.len() - 1)];
                    ui.heading(section);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        changed |= settings::settings_ui(ui, &mut st.prefs, Show::Section(section), &palette, section);
                    });
                });
            });
        });
    st.prefs_open = open;
    if changed {
        st.apply_prefs(ctx);
        st.settings_touched();
    }
}

fn about(ctx: &egui::Context, st: &mut AppState) {
    if !st.about_open {
        return;
    }
    let mut open = true;
    egui::Window::new("About Opal Studio")
        .open(&mut open)
        .default_pos(ctx.content_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading(format!("Opal Studio {}", env!("CARGO_PKG_VERSION")));
            ui.label("A 2D graphics editor. Free software under the GNU General Public License, version 3 or later.");
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Built with egui, wgpu and the image crate. Icons: Microsoft Fluent UI System Icons (MIT).",
                )
                .small(),
            );
            ui.add_space(6.0);
            egui::Grid::new("about").num_columns(2).show(ui, |ui| {
                ui.label("Graphics");
                ui.label(format!("{} ({})", st.gpu.adapter, st.gpu.backend));
                ui.end_row();
                ui.label("GPU memory");
                ui.label(match st.gpu.memory {
                    Some((used, budget)) => format!("{} used of {} available", bytes(used), bytes(budget)),
                    None => "not reported on this system".to_owned(),
                });
                ui.end_row();
                ui.label("Canvas textures");
                ui.label(st.gpu.textures.to_string());
                ui.end_row();
                ui.label("Settings");
                ui.label(
                    crate::config::settings_path()
                        .map_or("(no settings folder)".to_owned(), |p| p.display().to_string()),
                );
                ui.end_row();
            });
        });
    st.about_open = open;
}

fn gallery(ctx: &egui::Context, st: &mut AppState) {
    if !st.gallery_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Widget gallery")
        .open(&mut open)
        .default_pos(ctx.content_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .default_size([380.0, 560.0])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| st.gallery.ui(ui));
        });
    st.gallery_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_canvas_larger_than_memory_allows_says_so() {
        use crate::memory::Free;
        let plain = size_note(1000, 1000, Some(Free::memory(1 << 30)));
        assert_eq!(plain, "1.0 megapixels; a layer covering it all takes about 8 MB of memory");
        let huge = size_note(MAX_SIDE, MAX_SIDE, Some(Free::memory(16_000_000_000)));
        assert!(huge.contains("720.0 GB") && huge.contains("more than the 16.0 GB free:"), "{huge}");
        // With room on disk for tiles past memory.
        let disk = Some(Free { memory: 16_000_000_000, reclaimable: 0, disk: 100_000_000_000, resident: 0 });
        let note = size_note(MAX_SIDE, MAX_SIDE, disk);
        assert!(note.contains("more than the 116.0 GB free in memory and on disk"), "{note}");
        assert!(!size_note(30_000, 30_000, disk).contains("more than"), "7.2 GB fits");
        assert!(!size_note(MAX_SIDE, MAX_SIDE, None).contains("free"), "said without knowing");
    }

    #[test]
    fn a_held_print_size_is_taken_afresh_once_the_resolution_changed_otherwise() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("held-print-test");
        let (mut w, mut h) = (2550, 3300);
        let step = |w: &mut u32, h: &mut u32, before: f64, after: f64| {
            let mut out =
                ctx.run_ui(Default::default(), |ui| hold_print_size(ui, id, (&mut *w, &mut *h), before, after));
            out.textures_delta.clear();
        };
        // Held from 300 to 150 ppi: 8.5 × 11 in.
        step(&mut w, &mut h, 300.0, 150.0);
        assert_eq!((w, h), (1275, 1650));
        // Then, not held, from 150 to 100: the pixels stay, and it prints 12.75 × 16.5 in.
        // Held again from 100 to 200: that print size, not the 8.5 × 11 held before.
        step(&mut w, &mut h, 100.0, 200.0);
        assert_eq!((w, h), (2550, 3300));
    }
}
