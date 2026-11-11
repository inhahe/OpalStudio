//! The selection panel (design.md, *The recipe*): the steps that make the active image's
//! selection, in the order they were taken, each to change or take away, the selection then
//! worked out again from them off the interface thread (`AppState::rework_selection`).

use crate::icons;
use crate::lengths;
use crate::state::AppState;
use crate::theme::icon;
use eframe::egui::{self, RichText};
use opal_core::mask::{Combine, ShapeKind};
use opal_core::modify::{Falloff, Reach, Side};
use opal_core::selection::Step;
use opal_core::units::Unit;

/// What a row of the panel asked for.
enum Action {
    Edit(usize),
    Combine(usize, Combine),
    Remove(usize),
}

/// The ways a maker combines, by name, in the order the panel's menus list them.
const COMBINES: [(Combine, &str); 4] = [
    (Combine::Replace, "Replace"),
    (Combine::Add, "Add"),
    (Combine::Subtract, "Subtract"),
    (Combine::Intersect, "Intersect"),
];

pub fn selection_panel(ui: &mut egui::Ui, st: &mut AppState) {
    let weak = st.palette.weak_text;
    let Some(d) = st.active() else {
        ui.label(RichText::new("No image is open.").color(weak));
        return;
    };
    let Some(selection) = d.pixel_selection().cloned() else {
        ui.label(
            RichText::new(
                "Nothing is selected. The steps that make a selection are listed here, to change or take away.",
            )
            .color(weak),
        );
        return;
    };
    let (unit, ppi) = (st.unit(), d.doc().resolution);
    let mut action = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, step) in selection.steps().iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(icon(step_icon(step), 15.0));
                ui.label(describe(step, unit, ppi));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let remove = ui.add(egui::Button::new(icon(icons::DELETE, 14.0)).frame(false));
                    if remove.on_hover_text("Take this step away").clicked() {
                        action = Some(Action::Remove(i));
                    }
                    if let Some(mut combine) = combine_of(step) {
                        let was = combine;
                        let shown = COMBINES.iter().find(|(c, _)| *c == combine).map_or("", |(_, name)| *name);
                        egui::ComboBox::from_id_salt(("selection-step-combine", i))
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for (c, name) in COMBINES {
                                    ui.selectable_value(&mut combine, c, name);
                                }
                            })
                            .response
                            .on_hover_text("How it combines with the steps before it");
                        if combine != was {
                            action = Some(Action::Combine(i, combine));
                        }
                    }
                    if is_modifier(step) {
                        let edit = ui.add(egui::Button::new(icon(icons::OPTIONS, 14.0)).frame(false));
                        if edit.on_hover_text("Change it").clicked() {
                            action = Some(Action::Edit(i));
                        }
                    }
                });
            });
        }
    });
    let ctx = ui.ctx().clone();
    match action {
        Some(Action::Edit(i)) => st.edit_selection_step(i, &ctx),
        Some(Action::Combine(i, combine)) => st.combine_selection_step(i, combine),
        Some(Action::Remove(i)) => st.remove_selection_step(i),
        None => {}
    }
}

/// Whether `step` is a modifier with settings to change (in its dialog).
fn is_modifier(step: &Step) -> bool {
    matches!(step, Step::Grow { .. } | Step::Feather { .. } | Step::Smooth { .. } | Step::Border { .. })
}

/// How `step`, a maker (a shape, or a mask), combines with the steps before it.
pub fn combine_of(step: &Step) -> Option<Combine> {
    match step {
        Step::Shape { combine, .. } | Step::Mask { combine, .. } => Some(*combine),
        _ => None,
    }
}

fn step_icon(step: &Step) -> char {
    match step {
        Step::Shape { shape, .. } if shape.kind == ShapeKind::Ellipse => icons::ELLIPSE,
        Step::Shape { .. } => icons::RECTANGLE,
        Step::Invert => icons::INVERT_SELECTION,
        Step::Grow { .. } => icons::GROW,
        Step::Feather { .. } => icons::FEATHER,
        Step::Smooth { .. } => icons::SMOOTH,
        Step::Border { .. } => icons::BORDER_SELECTION,
        Step::Mask { .. } => icons::LAYER_PIXELS,
    }
}

/// What `step` does, in a few words, its lengths in `unit` at `ppi` pixels per inch.
pub fn describe(step: &Step, unit: Unit, ppi: f64) -> String {
    let len = |px: f64| lengths::show(px, unit, ppi);
    let side = |s: &Side| match s {
        Side::Inward => "inward",
        Side::Outward => "outward",
        Side::Centered => "centered",
    };
    match step {
        Step::Shape { shape, .. } => {
            let what = if shape.kind == ShapeKind::Ellipse { "Ellipse" } else { "Rectangle" };
            let [w, h] = shape.size;
            let at = [shape.center[0] - w / 2.0, shape.center[1] - h / 2.0];
            let mut s = format!("{what} {} at {}, {}", lengths::show_size(w, h, unit, ppi), len(at[0]), len(at[1]));
            if shape.angle != 0.0 {
                s.push_str(&format!(", turned {:.1}°", shape.angle));
            }
            s
        }
        Step::Invert => "Inverted".to_owned(),
        Step::Grow { by, corners } => {
            let how = if *by < 0.0 { "Shrunk" } else { "Grown" };
            let corners = match corners {
                Reach::Round => "",
                Reach::Square => ", square corners",
                Reach::Diamond => ", diamond corners",
            };
            format!("{how} by {}{corners}", len(by.abs()))
        }
        Step::Feather { width, side: s, falloff } => {
            let falloff = match falloff {
                Falloff::Gaussian => "Gaussian",
                Falloff::Linear => "linear",
            };
            format!("Feathered {}, {}, {falloff}", len(*width), side(s))
        }
        Step::Smooth { radius } => format!("Smoothed over {}", len(*radius)),
        Step::Border { width, side: s } => format!("Border {} wide, {}", len(*width), side(s)),
        Step::Mask { combine: Combine::Replace, .. } => "As it was carried, pixel by pixel".to_owned(),
        Step::Mask { .. } => "A layer's pixels".to_owned(),
    }
}
