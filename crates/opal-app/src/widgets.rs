//! Small shared pieces of interface built from the command registry.

use crate::commands::{Cmd, availability, checked};
use crate::state::AppState;
use crate::theme::icon;
use eframe::egui::{self, RichText};

/// The text of a command's first shortcut ("Ctrl+S"), if it has one.
pub fn shortcut_text(ctx: &egui::Context, cmd: Cmd) -> Option<String> {
    cmd.info().shortcuts.first().map(|s| ctx.format_shortcut(s))
}

/// The tooltip of a command: name and shortcut, description, and why it is disabled.
pub fn command_tooltip(ui: &mut egui::Ui, st: &AppState, cmd: Cmd) {
    let info = cmd.info();
    ui.horizontal(|ui| {
        ui.label(RichText::new(info.name).strong());
        if let Some(s) = shortcut_text(ui.ctx(), cmd) {
            ui.label(RichText::new(format!("({s})")).color(st.palette.weak_text));
        }
    });
    ui.label(info.description);
    if let Err(why) = availability(st, cmd) {
        ui.label(RichText::new(why).color(st.palette.changed));
    }
    ui.label(RichText::new("F1: more help").small().color(st.palette.weak_text));
}

/// A command as a menu entry (icon, name, shortcut), disabled with its reason when it cannot
/// run. Clicking queues the command.
pub fn menu_command(ui: &mut egui::Ui, st: &mut AppState, cmd: Cmd) {
    let info = cmd.info();
    let ok = availability(st, cmd).is_ok();
    let mut text = format!("{}  {}", info.icon, info.name);
    if checked(st, cmd) == Some(true) {
        text.push_str(&format!("  {}", crate::icons::CHECK));
    }
    let mut button = egui::Button::new(text);
    if let Some(s) = shortcut_text(ui.ctx(), cmd) {
        button = button.shortcut_text(s);
    }
    let r = ui.add_enabled(ok, button);
    let r = if ok {
        r.on_hover_ui(|ui| command_tooltip(ui, st, cmd))
    } else {
        r.on_disabled_hover_ui(|ui| command_tooltip(ui, st, cmd))
    };
    if r.clicked() {
        st.queue.push(cmd);
        ui.close();
    }
}

/// A small icon button for a command (as in a panel's header).
pub fn icon_command(ui: &mut egui::Ui, st: &mut AppState, cmd: Cmd) {
    let ok = availability(st, cmd).is_ok();
    let r = ui.add_enabled(ok, egui::Button::new(icon(cmd.info().icon, 16.0)).frame(false));
    let r = if ok {
        r.on_hover_ui(|ui| command_tooltip(ui, st, cmd))
    } else {
        r.on_disabled_hover_ui(|ui| command_tooltip(ui, st, cmd))
    };
    if r.hovered() {
        st.ribbon.hovered = Some(cmd);
    }
    if r.clicked() {
        st.queue.push(cmd);
    }
}
