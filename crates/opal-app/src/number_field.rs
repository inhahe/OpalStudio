//! The field every number and length is typed into: dragged like egui's `DragValue`, and
//! clicked, it takes an expression (`opal_core::expr`), worked out when entered. While it is
//! being typed, what it comes to shows below it, or what is wrong with it, and where; Up and
//! Down bring back what was entered in it before.

use eframe::egui::{self, Key, Modifiers};
use opal_core::expr::{Expr, Scope, Want};
use opal_core::units::Unit;
use std::ops::RangeInclusive;

/// How many entries a field remembers.
const REMEMBERED: usize = 20;

/// A field for a number held in `value`: a length in pixels (shown in its scope's unit), an
/// angle in degrees, or a plain number; worked out against `scope` when typed.
pub struct NumberField<'a> {
    value: &'a mut f64,
    want: Want,
    scope: Scope<'a>,
    range: RangeInclusive<f64>,
    clamp_existing: bool,
    speed: f64,
    decimals: RangeInclusive<usize>,
    prefix: String,
    suffix: String,
    remember: Option<egui::Id>,
}

/// What is being typed into a field.
#[derive(Clone, Default)]
struct Typing {
    text: String,
    /// The field's value the text was begun at: if it changes, the text is out of date.
    value: f64,
    /// Which remembered entry is shown (0 the latest), and the text typed before Up was.
    recalled: Option<(usize, String)>,
    /// Focus given back after Enter was pressed on what cannot be worked out, so the text
    /// stays as typed.
    kept: bool,
}

impl<'a> NumberField<'a> {
    fn new(value: &'a mut f64, want: Want, scope: Scope<'a>) -> NumberField<'a> {
        NumberField {
            value,
            want,
            scope,
            range: f64::NEG_INFINITY..=f64::INFINITY,
            clamp_existing: true,
            speed: 1.0,
            decimals: 0..=3,
            prefix: String::new(),
            suffix: String::new(),
            remember: None,
        }
    }

    /// A field for the length `px` pixels, shown in `scope`'s unit with it after.
    pub fn length(px: &'a mut f64, scope: Scope<'a>) -> NumberField<'a> {
        let unit = scope.unit;
        let decimals = if unit == Unit::Px { 0 } else { unit.decimals(scope.ppi) };
        NumberField::new(px, Want::Length, scope).decimals(0, decimals).suffix(format!(" {unit}"))
    }

    /// A field for an angle of `degrees`.
    pub fn angle(degrees: &'a mut f64, scope: Scope<'a>) -> NumberField<'a> {
        NumberField::new(degrees, Want::Angle, scope).decimals(0, 2).suffix("°")
    }

    /// A field for the plain number `value`.
    pub fn number(value: &'a mut f64, scope: Scope<'a>) -> NumberField<'a> {
        NumberField::new(value, Want::Number, scope)
    }

    /// What the value may be (in pixels, for a length): what is typed or dragged is kept in it.
    pub fn range(mut self, range: RangeInclusive<f64>) -> Self {
        self.range = range;
        self
    }

    /// Whether a value already outside the range is brought into it (else it is shown as it
    /// is, until the user types or drags).
    pub fn clamp_existing(mut self, clamp: bool) -> Self {
        self.clamp_existing = clamp;
        self
    }

    /// How much the value changes for each point dragged (in pixels, for a length).
    pub fn speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    /// How many decimal places it shows: at least `least`, at most `most`.
    pub fn decimals(mut self, least: usize, most: usize) -> Self {
        self.decimals = least..=most.max(least);
        self
    }

    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    /// Remembers what is entered under `key`, shared by every field given it (else by the
    /// field alone).
    pub fn remember(mut self, key: impl std::hash::Hash + std::fmt::Debug) -> Self {
        self.remember = Some(egui::Id::unique(("number-field-entries", key)));
        self
    }

    /// The value as shown: a length in the scope's unit.
    fn shown_of(&self, v: f64) -> f64 {
        match self.want {
            Want::Length => self.scope.unit.from_px(v, self.scope.ppi),
            _ => v,
        }
    }

    fn value_of(&self, shown: f64) -> f64 {
        match self.want {
            Want::Length => self.scope.unit.to_px(shown, self.scope.ppi),
            _ => shown,
        }
    }

    fn clamp(&self, v: f64) -> f64 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        if v < lo {
            lo
        } else if v > hi {
            hi
        } else {
            v
        }
    }

    /// What `text` comes to, in the value's terms (pixels for a length), not yet clamped.
    fn work_out(&self, text: &str) -> Result<f64, opal_core::expr::Error> {
        Expr::parse(text)?.eval(&self.scope, self.want)
    }

    /// `v` as the field shows it, in its terms, without the prefix and suffix.
    fn text_of(&self, v: f64) -> String {
        show_number(self.shown_of(v), self.decimals.clone())
    }

    /// What `v` (in the value's terms) is, in words for the preview: "2.5 in".
    fn told(&self, v: f64) -> String {
        match self.want {
            Want::Length => {
                let shown = crate::lengths::show(v, self.scope.unit, self.scope.ppi);
                if self.scope.unit == Unit::Px { shown } else { format!("{shown} ({} px)", show_number(v, 0..=2)) }
            }
            Want::Angle => format!("{}°", show_number(v, 0..=4)),
            Want::Number => format!("{}{}", show_number(v, 0..=6), self.suffix),
        }
    }

    /// Shows the field; its response is changed if its value is.
    pub fn show(self, ui: &mut egui::Ui) -> egui::Response {
        // One id whether it is a button or a text edit, as egui's DragValue keeps.
        let id = ui.next_auto_id();
        let editing = ui.is_enabled()
            && ui.memory_mut(|m| {
                m.interested_in_focus(id, ui.layer_id());
                m.has_focus(id)
            });
        let old = *self.value;
        let mut value = if self.clamp_existing { self.clamp(old) } else { old };
        // Accessibility's requests: a value set, or a step either way.
        ui.input(|i| {
            use egui::accesskit::{Action, ActionData};
            for request in i.accesskit_action_requests(id, Action::SetValue) {
                if let Some(ActionData::NumericValue(v)) = request.data {
                    value = self.clamp(self.value_of(v));
                }
            }
            let steps = i.num_accesskit_action_requests(id, Action::Increment) as f64
                - i.num_accesskit_action_requests(id, Action::Decrement) as f64;
            if steps != 0.0 {
                value = self.clamp(value + steps * self.speed);
            }
        });
        let text = self.text_of(value);
        let remembered = self.remember.unwrap_or_else(|| id.with("entries"));
        let mut response = if editing {
            let mut typing = ui
                .data_mut(|d| d.remove_temp::<Typing>(id))
                .filter(|t| t.value == old)
                .unwrap_or(Typing { text: text.clone(), value: old, recalled: None, kept: false });
            let refocused = std::mem::take(&mut typing.kept);
            recall(ui, remembered, &mut typing, id);
            let style = ui.style().drag_value_text_style.clone();
            let r = ui.add(
                egui::TextEdit::singleline(&mut typing.text)
                    .id(id)
                    .desired_width(0.0)
                    .min_size(ui.spacing().interact_size)
                    .font(style),
            );
            if r.gained_focus() && !refocused {
                select_all(ui, id, &typing.text);
            }
            let result = self.work_out(&typing.text);
            if typing.text.trim().parse::<f64>().is_err() {
                preview(ui, id, &r, &typing.text, result.as_ref().map(|&v| self.told(self.clamp(v))));
            }
            if r.lost_focus() && !ui.input(|i| i.key_pressed(Key::Escape)) {
                match result {
                    Ok(v) => {
                        value = self.clamp(v);
                        keep_entry(ui, remembered, &typing.text);
                    }
                    // Enter on what cannot be worked out keeps it, to be put right; leaving
                    // the field another way leaves the value as it was.
                    Err(_) if ui.input(|i| i.key_pressed(Key::Enter)) => {
                        ui.memory_mut(|m| m.request_focus(id));
                        ui.data_mut(|d| d.insert_temp(id, Typing { kept: true, ..typing }));
                    }
                    Err(_) => {}
                }
            } else if !r.lost_focus() {
                ui.data_mut(|d| d.insert_temp(id, typing));
            }
            let mut r = r;
            // The text edit reports each key as a change; the field changes only with its value.
            r.flags.remove(egui::response::Flags::CHANGED);
            r
        } else {
            let style = ui.style().drag_value_text_style.clone();
            let label = egui::RichText::new(format!("{}{text}{}", self.prefix, self.suffix)).text_style(style);
            let button = egui::Button::new(label)
                .wrap_mode(egui::TextWrapMode::Extend)
                .sense(egui::Sense::click_and_drag())
                .min_size(ui.spacing().interact_size);
            let r = ui.add(button).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            // Not being typed in: what was typed last time is done with.
            ui.data_mut(|d| d.remove::<Typing>(id));
            if ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
                ui.data_mut(|d| d.remove::<f64>(id));
            }
            if r.clicked() {
                ui.data_mut(|d| d.remove::<Typing>(id));
                ui.memory_mut(|m| m.request_focus(id));
                select_all(ui, id, &text);
            } else if r.dragged() {
                let delta = r.drag_delta();
                let slow = ui.input(|i| i.modifiers.shift_only());
                let speed = if slow { self.speed / 10.0 } else { self.speed };
                let moved = f64::from(delta.x - delta.y) * speed;
                if moved != 0.0 {
                    // Rounded to what it shows, so the precise value is kept apart.
                    let precise = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(value) + moved;
                    let places = *self.decimals.end() as i32 + i32::from(slow);
                    let k = 10f64.powi(places);
                    value = self.clamp(self.value_of((self.shown_of(precise) * k).round() / k));
                    ui.data_mut(|d| d.insert_temp(id, precise));
                }
            }
            r
        };
        if value != old {
            *self.value = value;
            response.mark_changed();
        }
        let shown = self.shown_of(*self.value);
        response.widget_info(|| egui::WidgetInfo::drag_value(ui.is_enabled(), shown));
        let (lo, hi) = (self.shown_of(*self.range.start()), self.shown_of(*self.range.end()));
        ui.ctx().accesskit_node_builder(response.id, |b| {
            use egui::accesskit::Action;
            if lo.is_finite() {
                b.set_min_numeric_value(lo);
            }
            if hi.is_finite() {
                b.set_max_numeric_value(hi);
            }
            b.set_numeric_value_step(self.shown_of(self.speed).abs());
            b.add_action(Action::SetValue);
            b.add_action(Action::Increment);
            b.add_action(Action::Decrement);
            // A prefix or suffix is the value's visible label, so it names the field (as
            // egui's DragValue has it); `Response::labelled_by` names it otherwise.
            b.clear_label();
            let parts: Vec<&str> =
                [self.prefix.trim(), self.suffix.trim()].into_iter().filter(|p| !p.is_empty()).collect();
            if !parts.is_empty() {
                b.set_label(parts.join(" "));
            }
            if !editing {
                b.set_value(format!("{}{}{}", self.prefix, self.text_of(*self.value), self.suffix));
            }
        });
        response
    }
}

impl egui::Widget for NumberField<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        self.show(ui)
    }
}

/// `v` with from `places.start()` to `places.end()` decimal places: as few as show it.
pub fn show_number(v: f64, places: RangeInclusive<usize>) -> String {
    let mut s = format!("{:.*}", *places.end(), v);
    if s.contains('.') {
        let least = *places.start();
        let dot = s.find('.').expect("checked");
        while s.len() > dot + 1 + least && s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".to_owned() } else { s }
}

/// Up and Down while typing: what was entered before, the latest first, and back again to
/// what was being typed.
fn recall(ui: &mut egui::Ui, remembered: egui::Id, typing: &mut Typing, id: egui::Id) {
    let (up, down) = ui.input_mut(|i| {
        (
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
            i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
        )
    });
    if up == 0 && down == 0 {
        return;
    }
    let entries: Vec<String> = ui.data(|d| d.get_temp(remembered)).unwrap_or_default();
    if entries.is_empty() {
        return;
    }
    let now = typing.recalled.as_ref().map(|(i, _)| *i as isize).unwrap_or(-1);
    let next = (now + up as isize - down as isize).clamp(-1, entries.len() as isize - 1);
    if next == now {
        return;
    }
    let draft = typing.recalled.take().map_or_else(|| typing.text.clone(), |(_, d)| d);
    if next < 0 {
        typing.text = draft;
    } else {
        typing.text = entries[next as usize].clone();
        typing.recalled = Some((next as usize, draft));
    }
    // The caret at the end of what is brought back.
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let end = egui::text::CCursor::new(typing.text.chars().count());
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ui.ctx(), id);
}

/// Remembers `text` as entered, the latest first, once.
fn keep_entry(ui: &mut egui::Ui, remembered: egui::Id, text: &str) {
    let text = text.trim().to_owned();
    ui.data_mut(|d| {
        let entries: &mut Vec<String> = d.get_temp_mut_or_default(remembered);
        entries.retain(|e| *e != text);
        entries.insert(0, text);
        entries.truncate(REMEMBERED);
    });
}

fn select_all(ui: &egui::Ui, id: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::select_all_str(text)));
    state.store(ui.ctx(), id);
}

/// Below the field `r`: what `text` comes to (`told`), or what is wrong with it, the part at
/// fault marked.
fn preview(ui: &egui::Ui, id: egui::Id, r: &egui::Response, text: &str, told: Result<String, &opal_core::expr::Error>) {
    let pos = r.rect.left_bottom() + egui::vec2(0.0, 3.0);
    egui::Area::new(id.with("preview")).order(egui::Order::Tooltip).fixed_pos(pos).interactable(false).show(
        ui.ctx(),
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                match told {
                    Ok(told) => {
                        ui.label(format!("= {told}"));
                    }
                    Err(e) => {
                        let style = ui.style();
                        let font = egui::TextStyle::Monospace.resolve(style);
                        let plain = egui::TextFormat::simple(font.clone(), style.visuals.text_color());
                        let wrong = egui::TextFormat {
                            color: style.visuals.error_fg_color,
                            underline: egui::Stroke::new(1.5, style.visuals.error_fg_color),
                            ..egui::TextFormat::simple(font, style.visuals.error_fg_color)
                        };
                        let (a, b) = (e.at.start.min(text.len()), e.at.end.min(text.len()));
                        if text.is_char_boundary(a) && text.is_char_boundary(b) {
                            let mut job = egui::text::LayoutJob::default();
                            job.append(&text[..a], 0.0, plain.clone());
                            // An empty part at fault (something missing) is marked as a gap.
                            job.append(if a == b { " " } else { &text[a..b] }, 0.0, wrong);
                            job.append(&text[b..], 0.0, plain);
                            ui.label(job);
                        }
                        ui.colored_label(ui.style().visuals.error_fg_color, &e.message);
                    }
                }
            });
        },
    );
}
