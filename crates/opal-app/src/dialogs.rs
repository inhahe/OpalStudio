//! Dialogs and windows: messages, unsaved-changes questions, New Image, Export,
//! Preferences, About and the widget gallery.

use crate::icons;
use crate::prefs::SECTIONS;
use crate::settings::{self, Show};
use crate::state::{AppState, Background, Confirm, ExportKind, MessageKind, NewImage};
use eframe::egui::{self, RichText, ViewportCommand};

/// The largest canvas side the New Image dialog accepts.
pub const MAX_SIDE: u32 = 100_000;

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
    }
    preferences(ctx, st);
    about(ctx, st);
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
    // A save that is cancelled or fails stops the close (for several documents, at the first
    // one not saved).
    let saved = |st: &mut AppState, ids: &[crate::doc::DocId]| ids.iter().all(|&id| st.save(id, false));
    match (c, choice) {
        (Confirm::Close(id), Answer::Save) if saved(st, &[id]) => st.close_now(id),
        (Confirm::Close(id), Answer::Discard) => st.close_now(id),
        (Confirm::Exit(ids), Answer::Save) if saved(st, &ids) => {
            st.exit_confirmed = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        (Confirm::Exit(_), Answer::Discard) => {
            st.exit_confirmed = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        _ => {}
    }
}

fn new_image(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.new_image.clone() else { return };
    let mut done: Option<bool> = None;
    egui::Modal::new(egui::Id::unique("new-image")).show(ctx, |ui| {
        ui.heading("New image");
        ui.add_space(4.0);
        egui::Grid::new("new-image-grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Width");
            ui.add(egui::DragValue::new(&mut spec.width).range(1..=MAX_SIDE).suffix(" px"));
            ui.end_row();
            ui.label("Height");
            ui.add(egui::DragValue::new(&mut spec.height).range(1..=MAX_SIDE).suffix(" px"));
            ui.end_row();
            ui.label("Size");
            ui.horizontal_wrapped(|ui| {
                for (name, w, h) in [
                    ("HD", 1920, 1080),
                    ("4K", 3840, 2160),
                    ("Square", 2048, 2048),
                    ("A4, 300 dpi", 2480, 3508),
                    ("Letter, 300 dpi", 2550, 3300),
                ] {
                    if ui.small_button(name).on_hover_text(format!("{w} × {h}")).clicked() {
                        spec.width = w;
                        spec.height = h;
                    }
                }
                if ui
                    .small_button(crate::theme::icon(icons::SWAP, 13.0))
                    .on_hover_text("Swap width and height")
                    .clicked()
                {
                    std::mem::swap(&mut spec.width, &mut spec.height);
                }
            });
            ui.end_row();
            ui.label("Background");
            ui.horizontal(|ui| {
                ui.radio_value(&mut spec.background, Background::White, "White");
                ui.radio_value(&mut spec.background, Background::Transparent, "Transparent");
            });
            ui.end_row();
        });
        let mp = f64::from(spec.width) * f64::from(spec.height) / 1e6;
        ui.label(RichText::new(format!("{mp:.1} megapixels")).small().color(st.palette.weak_text));
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

fn export(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut spec) = st.export.clone() else { return };
    let mut done: Option<bool> = None;
    let palette = st.palette;
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
        let changed = settings::settings_ui(ui, &mut st.prefs, Show::Section("Export"), &palette, "export");
        if changed {
            st.settings_touched();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Export…").clicked() {
                done = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    });
    match done {
        Some(true) => {
            st.export = None;
            st.export_to_file(&spec);
            // Remember the format for next time.
            st.last_export = spec.kind;
        }
        Some(false) => st.export = None,
        None => st.export = Some(spec),
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

/// A byte count as text ("1.5 GB").
pub fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.0} MB", n / 1e6)
    } else {
        format!("{:.0} KB", n / 1e3)
    }
}
