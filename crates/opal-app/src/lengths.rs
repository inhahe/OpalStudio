//! Fields for lengths and resolutions in the user's units (see `opal_core::units`).
//!
//! A length is always held in pixels; a field shows it in a unit, converted at the image's
//! resolution, and writes back only what the user changes, so showing a length never
//! moves it. What is typed is an expression (`opal_core::expr`): a number in the field's
//! unit, one with a unit of its own ("2 cm"), or arithmetic on lengths and what the
//! field's scope lets it name.

use crate::dialogs::MAX_SIDE;
use crate::number_field::NumberField;
use crate::settings::Value;
use crate::state::AppState;
use eframe::egui;
use opal_core::expr::Scope;
use opal_core::units::{MAX_PPI, MIN_PPI, ResolutionUnit, Unit};
use std::ops::RangeInclusive;

/// A field for a length of `px` pixels, shown in `scope`'s unit at its resolution, worked
/// out in it when typed, and kept in `range` (in pixels). In pixels it shows `px_decimals`
/// places; in other units, enough for a pixel to show.
pub fn length(
    ui: &mut egui::Ui,
    px: &mut f64,
    scope: Scope,
    range: RangeInclusive<f64>,
    px_decimals: usize,
) -> egui::Response {
    length_field(px, scope, range, px_decimals).show(ui)
}

/// The field [`length`] shows, to be given more (what it remembers entries under, say).
pub fn length_field<'a>(
    px: &'a mut f64,
    scope: Scope<'a>,
    range: RangeInclusive<f64>,
    px_decimals: usize,
) -> NumberField<'a> {
    let unit = scope.unit;
    let decimals = if unit == Unit::Px { px_decimals } else { unit.decimals(scope.ppi) };
    NumberField::length(px, scope)
        .decimals(0, decimals)
        .range(range)
        // Kept as it is if outside the range (a box dragged far), until the user types.
        .clamp_existing(false)
        // A pixel for each point dragged.
        .speed(1.0)
}

/// What a field about image `doc` (if one) works out what is typed in: lengths in `unit` at
/// `ppi`, positions from its canvas's top-left corner, and no box.
pub fn image_scope(doc: Option<&opal_core::document::Document>, unit: Unit, ppi: f64) -> Scope<'_> {
    let origin = doc.map_or([0.0, 0.0], |d| [d.canvas.x as f64, d.canvas.y as f64]);
    Scope { doc, ppi, unit, origin, own: None }
}

/// A canvas side of `px` pixels, in whole pixels from 1 to [`MAX_SIDE`].
pub fn whole_side(px: f64) -> u32 {
    (px.round() as u32).clamp(1, MAX_SIDE)
}

/// A field for a canvas side of `n` pixels (1 to [`MAX_SIDE`]), in `scope`.
pub fn side(ui: &mut egui::Ui, n: &mut u32, scope: Scope) -> egui::Response {
    let mut px = f64::from(*n);
    let r = length(ui, &mut px, scope, 1.0..=f64::from(MAX_SIDE), 0);
    if r.changed() {
        *n = whole_side(px);
    }
    r
}

/// A field for a whole number of pixels `n` in `range`, in `scope`.
pub fn whole(ui: &mut egui::Ui, n: &mut i64, scope: Scope, range: RangeInclusive<i64>) -> egui::Response {
    let mut px = *n as f64;
    let r = length(ui, &mut px, scope, *range.start() as f64..=*range.end() as f64, 0);
    if r.changed() {
        *n = (px.round() as i64).clamp(*range.start(), *range.end());
    }
    r
}

/// A menu of the units, showing `unit`. Returns whether the user chose another.
pub fn unit_menu(ui: &mut egui::Ui, unit: &mut Unit, id: &str) -> bool {
    let before = *unit;
    egui::ComboBox::from_id_salt(id)
        .selected_text(unit.name())
        .show_ui(ui, |ui| {
            for u in Unit::ALL {
                ui.selectable_value(unit, u, u.name());
            }
        })
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Units"));
    *unit != before
}

/// A field for a resolution of `ppi` pixels per inch, shown in `in_unit`, with a menu of
/// the resolution units; what is typed is worked out in `scope`. Returns whether the
/// resolution changed (not just its unit).
pub fn resolution(ui: &mut egui::Ui, ppi: &mut f64, in_unit: &mut ResolutionUnit, id: &str, scope: Scope) -> bool {
    let unit = *in_unit;
    let mut shown = unit.from_ppi(*ppi);
    let r = ui.add(
        NumberField::number(&mut shown, scope)
            .range(unit.from_ppi(MIN_PPI)..=unit.from_ppi(MAX_PPI))
            .speed(1.0)
            .decimals(0, 2),
    );
    if r.changed() {
        *ppi = unit.to_ppi(shown).clamp(MIN_PPI, MAX_PPI);
    }
    r.widget_info(|| egui::WidgetInfo {
        label: Some("Resolution".into()),
        ..egui::WidgetInfo::drag_value(true, shown)
    });
    egui::ComboBox::from_id_salt(id)
        .selected_text(unit.name())
        .show_ui(ui, |ui| {
            for u in ResolutionUnit::ALL {
                ui.selectable_value(in_unit, u, u.name());
            }
        })
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Resolution unit"));
    r.changed()
}

/// `px` pixels in `unit` at `ppi`, with as many places as a field shows, and the unit.
pub fn show(px: f64, unit: Unit, ppi: f64) -> String {
    let decimals = if unit == Unit::Px { 0 } else { unit.decimals(ppi) };
    let text = format!("{:.*}", decimals, unit.from_px(px, ppi));
    // "8.50" reads as 8.5.
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    format!("{text} {unit}")
}

/// A `w × h` pixel size in `unit` at `ppi` ("8.5 × 11 in").
pub fn show_size(w: f64, h: f64, unit: Unit, ppi: f64) -> String {
    let w = show(w, unit, ppi);
    let h = show(h, unit, ppi);
    format!("{} × {h}", w.rsplit_once(' ').map_or(w.as_str(), |(n, _)| n))
}

/// `ppi` in `unit`, with the unit's name ("300 pixels/inch").
pub fn show_resolution(ppi: f64, unit: ResolutionUnit) -> String {
    format!("{} {}", resolution_number(ppi, unit), unit.name().to_lowercase())
}

/// `ppi` in `unit`, to two places at most ("300", "118.11").
fn resolution_number(ppi: f64, unit: ResolutionUnit) -> String {
    let text = format!("{:.2}", unit.from_ppi(ppi));
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// How large a `width × height` image prints at `ppi`, told in `unit` (or, for pixels, in
/// inches or centimeters as resolutions are shown in `per`).
pub fn print_note(width: u32, height: u32, ppi: f64, unit: Unit, per: ResolutionUnit) -> String {
    let size = show_size(f64::from(width), f64::from(height), print_unit(unit, per), ppi);
    format!("At {} it prints {size}.", show_resolution(ppi, per))
}

/// `ppi` in `unit`, short ("300 ppi"), as the status bar shows it.
pub fn show_resolution_short(ppi: f64, unit: ResolutionUnit) -> String {
    format!("{} {}", resolution_number(ppi, unit), unit.id())
}

/// The unit a print size is told in: `unit` if it is one, else inches or centimeters as
/// the resolution is shown.
pub fn print_unit(unit: Unit, resolution: ResolutionUnit) -> Unit {
    match (unit, resolution) {
        (Unit::Px, ResolutionUnit::PerInch) => Unit::In,
        (Unit::Px, ResolutionUnit::PerCm) => Unit::Cm,
        (u, _) => u,
    }
}

impl AppState {
    /// The unit lengths are shown in.
    pub fn unit(&self) -> Unit {
        crate::prefs::unit(&self.prefs)
    }

    /// Makes `unit` the one lengths are shown in, everywhere.
    pub fn set_unit(&mut self, unit: Unit) {
        self.prefs.set("ui.unit", Value::Choice(unit.id().to_owned()));
        self.settings_touched();
    }

    /// The resolution an image gets whose file does not say (a preference).
    pub fn default_resolution(&self) -> f64 {
        self.prefs.f64("ui.default_resolution")
    }

    /// The unit resolutions are shown in.
    pub fn resolution_unit(&self) -> ResolutionUnit {
        crate::prefs::resolution_unit(&self.prefs)
    }

    /// Makes `unit` the one resolutions are shown in.
    pub fn set_resolution_unit(&mut self, unit: ResolutionUnit) {
        self.prefs.set("ui.resolution_unit", Value::Choice(unit.id().to_owned()));
        self.settings_touched();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_outside_its_field_s_range_is_shown_as_it_is() {
        // A crop box dragged past the largest canvas, say: showing it must not move it.
        let ctx = egui::Context::default();
        let mut px = 400_000.0;
        let mut changed = false;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            changed |= length(ui, &mut px, Scope::bare(Unit::In, 300.0), 1.0..=300_000.0, 0).changed();
        });
        out.textures_delta.clear();
        assert_eq!((px, changed), (400_000.0, false));
    }

    #[test]
    fn lengths_show_as_a_field_would() {
        assert_eq!(show(2550.0, Unit::In, 300.0), "8.5 in");
        assert_eq!(show(2550.0, Unit::Px, 300.0), "2550 px");
        assert_eq!(show(2480.0, Unit::Mm, 300.0), "209.97 mm");
        assert_eq!(show_size(2550.0, 3300.0, Unit::In, 300.0), "8.5 × 11 in");
        assert_eq!(show_resolution(300.0, ResolutionUnit::PerInch), "300 pixels/inch");
        assert_eq!(show_resolution(299.72, ResolutionUnit::PerCm), "118 pixels/cm");
    }
}
