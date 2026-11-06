//! Adjustable settings: one framework for preferences and tool settings alike.
//!
//! A setting is declared once by a [`Def`] (what it is, its range, its default, whether it
//! is advanced). A [`Settings`] holds the values that differ from the defaults; the UI
//! ([`settings_ui`]) shows the basic settings, then the advanced ones in a collapsed
//! section, marks every changed setting and offers to reset it.

use crate::curve_editor;
use crate::theme::{Palette, icon};
use eframe::egui::{self, Color32, RichText};
use opal_core::curve::{Curve, Interpolation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of value a setting holds.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Bool,
    /// A number from `min` to `max`, shown with `decimals` places and `suffix` ("%",
    /// " px"). `log` makes the slider logarithmic.
    Float {
        min: f64,
        max: f64,
        decimals: usize,
        suffix: &'static str,
        log: bool,
    },
    /// One of several options, as `(id, label)`.
    Choice(&'static [(&'static str, &'static str)]),
    /// An sRGB color.
    Color,
    /// A curve from 0–1 to 0–1; `periodic` for curves over a hue.
    #[cfg_attr(not(test), expect(dead_code, reason = "first used by the feather falloff setting in milestone 2"))]
    Curve {
        periodic: bool,
    },
    /// A line of text (a command, a path), which must not be empty; `hint` is shown in the
    /// box when it is.
    Text {
        hint: &'static str,
    },
}

/// A default value that can be written in a `static` table.
#[derive(Clone, Copy, Debug)]
pub enum Init {
    Bool(bool),
    Float(f64),
    Choice(&'static str),
    Color([u8; 4]),
    #[cfg_attr(not(test), expect(dead_code, reason = "first used by the feather falloff setting in milestone 2"))]
    Curve(&'static [[f64; 2]], Interpolation),
    Text(&'static str),
}

/// A setting's value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Float(f64),
    Choice(String),
    Color([u8; 4]),
    Curve(Curve),
    Text(String),
}

/// The declaration of one setting.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    /// Stable key, used in saved preferences and presets.
    pub key: &'static str,
    pub label: &'static str,
    /// What the setting does, shown in its tooltip.
    pub help: &'static str,
    /// Heading it is listed under in the preferences.
    pub section: &'static str,
    pub kind: Kind,
    pub default: Init,
    /// Listed in the collapsed *Advanced* section.
    pub advanced: bool,
}

impl Def {
    pub fn default_value(&self) -> Value {
        match (self.default, self.kind) {
            (Init::Bool(b), _) => Value::Bool(b),
            (Init::Float(f), _) => Value::Float(f),
            (Init::Choice(c), _) => Value::Choice(c.to_owned()),
            (Init::Color(c), _) => Value::Color(c),
            (Init::Curve(pts, interp), Kind::Curve { periodic }) => Value::Curve(Curve::new(pts, interp, periodic)),
            (Init::Curve(pts, interp), _) => Value::Curve(Curve::new(pts, interp, false)),
            (Init::Text(t), _) => Value::Text(t.to_owned()),
        }
    }

    /// Whether `v` is a valid value for this setting.
    pub(crate) fn accepts(&self, v: &Value) -> bool {
        match (self.kind, v) {
            (Kind::Bool, Value::Bool(_)) | (Kind::Color, Value::Color(_)) => true,
            (Kind::Float { min, max, .. }, Value::Float(f)) => (min..=max).contains(f),
            (Kind::Choice(opts), Value::Choice(c)) => opts.iter().any(|(id, _)| id == c),
            (Kind::Curve { periodic }, Value::Curve(c)) => c.is_periodic() == periodic,
            (Kind::Text { .. }, Value::Text(t)) => !t.is_empty() && t.trim() == t,
            _ => false,
        }
    }
}

/// Values for a table of settings; only values that differ from the defaults are stored.
#[derive(Clone, Debug)]
pub struct Settings {
    defs: &'static [Def],
    values: BTreeMap<&'static str, Value>,
    revision: u64,
}

impl Settings {
    pub fn new(defs: &'static [Def]) -> Settings {
        Settings { defs, values: BTreeMap::new(), revision: 0 }
    }

    pub fn defs(&self) -> &'static [Def] {
        self.defs
    }

    /// A number that changes whenever a value may have changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn def(&self, key: &str) -> &'static Def {
        self.defs.iter().find(|d| d.key == key).unwrap_or_else(|| panic!("no setting {key:?}"))
    }

    /// The value of `key` (its default unless changed).
    pub fn get(&self, key: &str) -> Value {
        let def = self.def(key);
        self.values.get(def.key).cloned().unwrap_or_else(|| def.default_value())
    }

    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Value::Bool(b) => b,
            v => panic!("setting {key:?} is not a bool: {v:?}"),
        }
    }

    pub fn f64(&self, key: &str) -> f64 {
        match self.get(key) {
            Value::Float(f) => f,
            v => panic!("setting {key:?} is not a number: {v:?}"),
        }
    }

    pub fn choice(&self, key: &str) -> String {
        match self.get(key) {
            Value::Choice(c) => c,
            v => panic!("setting {key:?} is not a choice: {v:?}"),
        }
    }

    pub fn color(&self, key: &str) -> Color32 {
        match self.get(key) {
            Value::Color([r, g, b, a]) => Color32::from_rgba_unmultiplied(r, g, b, a),
            v => panic!("setting {key:?} is not a color: {v:?}"),
        }
    }

    pub fn text(&self, key: &str) -> String {
        match self.get(key) {
            Value::Text(t) => t,
            v => panic!("setting {key:?} is not text: {v:?}"),
        }
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "first used by the feather falloff setting in milestone 2"))]
    pub fn curve(&self, key: &str) -> Curve {
        match self.get(key) {
            Value::Curve(c) => c,
            v => panic!("setting {key:?} is not a curve: {v:?}"),
        }
    }

    /// Sets `key`; setting it to its default forgets the stored value.
    pub fn set(&mut self, key: &str, v: Value) {
        let def = self.def(key);
        assert!(def.accepts(&v), "invalid value {v:?} for setting {key:?}");
        self.revision += 1;
        if v == def.default_value() {
            self.values.remove(def.key);
        } else {
            self.values.insert(def.key, v);
        }
    }

    pub fn is_changed(&self, key: &str) -> bool {
        self.values.contains_key(self.def(key).key)
    }

    pub fn reset(&mut self, key: &str) {
        let k = self.def(key).key;
        self.revision += 1;
        self.values.remove(k);
    }

    pub fn reset_all(&mut self) {
        self.revision += 1;
        self.values.clear();
    }

    /// The changed values, for saving.
    pub fn to_map(&self) -> BTreeMap<String, Value> {
        self.values.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
    }

    /// Takes values from a saved map. Unknown keys and invalid values (from older or newer
    /// versions) are skipped, and their keys returned.
    pub fn load_map(&mut self, map: &BTreeMap<String, Value>) -> Vec<String> {
        let mut skipped = Vec::new();
        for (k, v) in map {
            match self.defs.iter().find(|d| d.key == k) {
                Some(def) if def.accepts(v) => self.set(def.key, v.clone()),
                _ => skipped.push(k.clone()),
            }
        }
        skipped
    }
}

/// Which settings [`settings_ui`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show<'a> {
    All,
    Section(&'a str),
}

/// Shows the settings: basic ones first, then advanced ones in a collapsed section. Returns
/// whether any value changed.
pub fn settings_ui(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    show: Show<'_>,
    palette: &Palette,
    id_salt: &str,
) -> bool {
    let defs: Vec<&'static Def> = settings
        .defs()
        .iter()
        .filter(|d| match show {
            Show::All => true,
            Show::Section(s) => d.section == s,
        })
        .collect();
    let mut changed = false;
    let basic: Vec<_> = defs.iter().filter(|d| !d.advanced).collect();
    let advanced: Vec<_> = defs.iter().filter(|d| d.advanced).collect();
    egui::Grid::new(("settings", id_salt)).num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
        for d in &basic {
            changed |= setting_row(ui, settings, d, palette);
        }
    });
    if !advanced.is_empty() {
        let n_changed = advanced.iter().filter(|d| settings.is_changed(d.key)).count();
        let title = if n_changed > 0 { format!("Advanced ({n_changed} changed)") } else { "Advanced".to_owned() };
        egui::CollapsingHeader::new(title).id_salt(("advanced", id_salt)).default_open(false).show(ui, |ui| {
            egui::Grid::new(("settings-adv", id_salt)).num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                for d in &advanced {
                    changed |= setting_row(ui, settings, d, palette);
                }
            });
        });
    }
    changed
}

fn setting_row(ui: &mut egui::Ui, settings: &mut Settings, def: &'static Def, palette: &Palette) -> bool {
    let is_changed = settings.is_changed(def.key);
    let default_text = describe(def, &def.default_value());
    let label = ui.label(def.label);
    let label_id = label.id;
    let tip = format!("{}\n\nDefault: {default_text}", def.help);
    label.on_hover_text(&tip);
    let mut changed = false;
    let mut value = settings.get(def.key);
    let response = match (&mut value, def.kind) {
        (Value::Bool(b), _) => ui.checkbox(b, ""),
        (Value::Float(f), Kind::Float { min, max, decimals, suffix, log }) => ui.add(
            egui::Slider::new(f, min..=max)
                .logarithmic(log)
                .fixed_decimals(decimals)
                .suffix(suffix)
                .clamping(egui::SliderClamping::Always),
        ),
        (Value::Choice(c), Kind::Choice(opts)) => {
            let current = opts.iter().find(|(id, _)| id == c).map_or("?", |(_, l)| l);
            let mut r = egui::ComboBox::from_id_salt(def.key)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (id, l) in opts {
                        if ui.selectable_label(c == id, *l).clicked() && c != id {
                            *c = (*id).to_owned();
                            changed = true;
                        }
                    }
                })
                .response
                .labelled_by(label_id);
            if changed {
                r.mark_changed();
            }
            r
        }
        (Value::Color(rgba), _) => {
            let mut c = Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
            let r = ui.color_edit_button_srgba(&mut c);
            *rgba = c.to_srgba_unmultiplied();
            r
        }
        (Value::Curve(curve), _) => curve_editor::curve_editor(ui, curve, &curve_editor::Options::small(def.key)),
        (Value::Text(t), Kind::Text { hint }) => {
            // Typed into a copy kept while the box has the focus: the setting takes it whenever
            // it is not empty, so clearing the box to type anew leaves the setting as it was,
            // and a box left empty shows the setting again.
            let id = ui.make_persistent_id(("text setting", def.key));
            let mut typed = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| t.clone());
            let r = ui
                .add(egui::TextEdit::singleline(&mut typed).hint_text(hint).desired_width(220.0))
                .labelled_by(label_id);
            if r.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, typed.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(id));
            }
            if r.changed() && !typed.trim().is_empty() {
                *t = typed.trim().to_owned();
            }
            r
        }
        _ => unreachable!("setting {} has a value of the wrong kind", def.key),
    };
    let response = response.on_hover_text(&tip);
    if response.changed() || changed {
        settings.set(def.key, value);
        changed = true;
    }
    ui.horizontal(|ui| {
        if is_changed {
            ui.label(RichText::new("●").color(palette.changed).small()).on_hover_text("Changed from the default");
            if ui
                .small_button(icon(crate::icons::RESET, 13.0))
                .on_hover_text(format!("Reset to the default ({default_text})"))
                .clicked()
            {
                settings.reset(def.key);
                changed = true;
            }
        }
    });
    ui.end_row();
    changed
}

/// A value as text, for tooltips.
fn describe(def: &Def, v: &Value) -> String {
    match (v, def.kind) {
        (Value::Bool(b), _) => if *b { "on" } else { "off" }.to_owned(),
        (Value::Float(f), Kind::Float { decimals, suffix, .. }) => format!("{f:.decimals$}{suffix}"),
        (Value::Choice(c), Kind::Choice(opts)) => {
            opts.iter().find(|(id, _)| id == c).map_or(c.clone(), |(_, l)| (*l).to_owned())
        }
        (Value::Color([r, g, b, _]), _) => format!("#{r:02x}{g:02x}{b:02x}"),
        (Value::Curve(c), _) => format!("a curve of {} points", c.points().len()),
        (Value::Text(t), _) => t.clone(),
        _ => "?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static DEFS: &[Def] = &[
        Def {
            key: "a",
            label: "A",
            help: "",
            section: "s",
            kind: Kind::Float { min: 0.0, max: 10.0, decimals: 1, suffix: "", log: false },
            default: Init::Float(5.0),
            advanced: false,
        },
        Def {
            key: "b",
            label: "B",
            help: "",
            section: "s",
            kind: Kind::Choice(&[("x", "X"), ("y", "Y")]),
            default: Init::Choice("x"),
            advanced: true,
        },
    ];

    #[test]
    fn only_changed_values_are_stored() {
        let mut s = Settings::new(DEFS);
        assert_eq!(s.f64("a"), 5.0);
        assert!(!s.is_changed("a"));
        s.set("a", Value::Float(7.0));
        assert!(s.is_changed("a"));
        s.set("a", Value::Float(5.0));
        assert!(!s.is_changed("a"), "setting the default forgets the value");
        s.set("b", Value::Choice("y".into()));
        assert_eq!(s.to_map().len(), 1);
    }

    static CURVE_DEFS: &[Def] = &[Def {
        key: "c",
        label: "C",
        help: "",
        section: "s",
        kind: Kind::Curve { periodic: true },
        default: Init::Curve(&[[0.0, 1.0], [0.5, 0.0]], Interpolation::Linear),
        advanced: false,
    }];

    #[test]
    fn curve_settings_keep_their_periodicity() {
        let mut s = Settings::new(CURVE_DEFS);
        let c = s.curve("c");
        assert!(c.is_periodic(), "a hue curve's default wraps around");
        let mut edited = c.clone();
        edited.insert(0.25, 0.5);
        s.set("c", Value::Curve(edited.clone()));
        assert!(s.is_changed("c"));
        assert_eq!(s.curve("c"), edited);
        assert!(!CURVE_DEFS[0].accepts(&Value::Curve(Curve::identity())), "a curve that does not wrap is refused");
    }

    static TEXT_DEFS: &[Def] = &[Def {
        key: "t",
        label: "T",
        help: "",
        section: "s",
        kind: Kind::Text { hint: "python" },
        default: Init::Text("python"),
        advanced: false,
    }];

    #[test]
    fn a_text_setting_is_never_blank() {
        let mut s = Settings::new(TEXT_DEFS);
        assert_eq!(s.text("t"), "python");
        s.set("t", Value::Text("C:/venv/Scripts/python.exe".into()));
        assert_eq!(s.text("t"), "C:/venv/Scripts/python.exe");
        let d = &TEXT_DEFS[0];
        assert!(!d.accepts(&Value::Text(String::new())) && !d.accepts(&Value::Text("  ".into())));
        assert!(!d.accepts(&Value::Text(" python".into())), "kept trimmed");
        // Saved blank by hand: skipped on loading.
        let skipped = s.load_map(&BTreeMap::from([("t".to_owned(), Value::Text(String::new()))]));
        assert_eq!(skipped, ["t"]);
    }

    #[test]
    fn loading_skips_unknown_and_invalid_values() {
        let mut s = Settings::new(DEFS);
        let mut map = BTreeMap::new();
        map.insert("a".to_owned(), Value::Float(99.0));
        map.insert("b".to_owned(), Value::Choice("y".into()));
        map.insert("gone".to_owned(), Value::Bool(true));
        let mut skipped = s.load_map(&map);
        skipped.sort();
        assert_eq!(skipped, vec!["a".to_owned(), "gone".to_owned()]);
        assert_eq!(s.choice("b"), "y");
        assert_eq!(s.f64("a"), 5.0);
    }
}
