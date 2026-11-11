//! The selection panel (design.md, *The recipe*): the steps that make the active image's
//! selection, in the order they were taken, each to change or take away, the selection then
//! worked out again from them off the interface thread (`AppState::rework_selection`).

use crate::icons;
use crate::lengths;
use crate::state::AppState;
use crate::theme::icon;
use eframe::egui::{self, RichText};
use opal_core::geom::Affine;
use opal_core::mask::{Combine, ShapeKind};
use opal_core::modify::{Falloff, Reach, Side};
use opal_core::selection::{MaskOf, Step};
use opal_core::units::Unit;

/// What a row of the panel asked for.
enum Action {
    Edit(usize),
    Combine(usize, Combine),
    Remove(usize),
}

/// `name` (one of an option's `NAMES`) as a menu shows it: its first letter a capital.
fn shown(name: &str) -> String {
    let mut c = name.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

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
    let canvas = d.doc().canvas;
    let origin = [canvas.x as f64, canvas.y as f64];
    let refused = st.steps_refused();
    if let Some(why) = refused {
        ui.label(RichText::new(why).color(weak));
    }
    let mut action = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_enabled_ui(refused.is_none(), |ui| {
            for (i, step) in selection.steps().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(icon(step_icon(step), 15.0));
                    ui.label(describe(step, origin, unit, ppi));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let remove = ui.add(egui::Button::new(icon(icons::DELETE, 14.0)).frame(false));
                        if remove.on_hover_text("Take this step away").clicked() {
                            action = Some(Action::Remove(i));
                        }
                        if let Some(mut combine) = combine_of(step) {
                            let was = combine;
                            let named =
                                |c: Combine| Combine::NAMES[Combine::ALL.iter().position(|&a| a == c).expect("all")];
                            egui::ComboBox::from_id_salt(("selection-step-combine", i))
                                .selected_text(shown(named(combine)))
                                .show_ui(ui, |ui| {
                                    for c in Combine::ALL {
                                        ui.selectable_value(&mut combine, c, shown(named(c)));
                                    }
                                })
                                .response
                                .on_hover_text("How it combines with the steps before it");
                            if combine != was {
                                action = Some(Action::Combine(i, combine));
                            }
                        }
                        if step.is_modifier() {
                            let edit = ui.add(egui::Button::new(icon(icons::OPTIONS, 14.0)).frame(false));
                            if edit.on_hover_text("Change it").clicked() {
                                action = Some(Action::Edit(i));
                            }
                        }
                    });
                });
            }
        });
    });
    let ctx = ui.ctx().clone();
    match action {
        Some(Action::Edit(i)) => st.edit_selection_step(i, &ctx),
        Some(Action::Combine(i, combine)) => st.combine_selection_step(i, combine),
        Some(Action::Remove(i)) => st.remove_selection_step(i),
        None => {}
    }
}

/// How `step`, a maker (a shape, or a mask), combines with the steps before it.
fn combine_of(step: &Step) -> Option<Combine> {
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

/// What `step` does, in a few words, its lengths in `unit` at `ppi` pixels per inch, a shape
/// placed by its top-left corner (turned with it) from the canvas's corner, `origin`, as its
/// box's numbers place it.
fn describe(step: &Step, origin: [f64; 2], unit: Unit, ppi: f64) -> String {
    let len = |px: f64| lengths::show(px, unit, ppi);
    let side = |s: &Side| Side::NAMES[Side::ALL.iter().position(|a| a == s).expect("all")];
    match step {
        Step::Shape { shape, .. } => {
            let what = if shape.kind == ShapeKind::Ellipse { "Ellipse" } else { "Rectangle" };
            let [w, h] = shape.size;
            let [dx, dy] = Affine::rotate(shape.angle.to_radians()).apply([-w / 2.0, -h / 2.0]);
            let at = [shape.center[0] + dx - origin[0], shape.center[1] + dy - origin[1]];
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
        Step::Mask { of: MaskOf::LayerPixels, .. } => "A layer's pixels".to_owned(),
        Step::Mask { of: MaskOf::Carried, .. } => "The steps before, carried pixel by pixel".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::geom::IRect;
    use opal_core::mask::{Mask, Shape};
    use std::sync::Arc;

    #[test]
    fn a_step_is_described_as_it_was_made_and_placed_from_the_canvas_s_corner() {
        let px = |s: &Step| describe(s, [100.0, 50.0], Unit::Px, 72.0);
        // From the canvas's corner, at (100, 50); turned a quarter, by its corner turned too.
        let shape = Shape::rectangle(&IRect::new(110, 60, 150, 80).to_f(), false);
        assert_eq!(px(&Step::Shape { shape, combine: Combine::Add }), "Rectangle 40 × 20 px at 10 px, 10 px");
        let turned = Shape { angle: 90.0, ..shape };
        assert_eq!(
            px(&Step::Shape { shape: turned, combine: Combine::Add }),
            "Rectangle 40 × 20 px at 40 px, 0 px, turned 90.0°"
        );
        // A mask: a layer's pixels, replacing or not; or carried.
        let mask = Arc::new(Mask::of(&shape));
        let layer = Step::Mask { mask: mask.clone(), combine: Combine::Replace, of: MaskOf::LayerPixels };
        assert_eq!(px(&layer), "A layer's pixels");
        let carried = Step::Mask { mask, combine: Combine::Replace, of: MaskOf::Carried };
        assert_eq!(px(&carried), "The steps before, carried pixel by pixel");
    }
}
