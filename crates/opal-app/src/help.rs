//! The built-in help pages, generated from the command registry and the tools.

use crate::commands::{Cmd, RIBBON, availability};
use crate::ribbon::key_char;
use crate::state::{AppState, HelpTopic};
use crate::theme::icon;
use crate::tools::ToolId;
use crate::widgets::shortcut_text;
use eframe::egui::{self, RichText};

const START: &str = "\
Opal Studio keeps everything editable and tells you what is going on.

• Commands are on the ribbon at the top, grouped by tab. Rest the pointer on a button for \
what it does, and, if it is grayed out, why. Press F1 over a button, or click a group's name, \
for its help page.
• Press Ctrl+K (or click the search box) to find any command by name.
• Press Alt to show a letter on every tab and button; type the letters to run a command.
• Tools (what the pointer does on the canvas) are at the left end of the ribbon. You can move \
them to a side of the window (View › Move Tools).
• Layers have no edges: what you put on a layer can extend past the canvas. It is kept, and \
shown dimmed, but not exported.
• Undo never throws anything away. Doing something new after undoing starts a branch in the \
History panel, and you can go back to any step on any branch.
• Every setting that affects a result can be changed. Basic settings are shown; the rest are \
under Advanced. A changed setting is marked with a dot and can be reset.";

/// Shows the help window if it is open.
pub fn help_window(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut topic) = st.help.clone() else { return };
    let mut open = true;
    egui::Window::new(format!("{}  Help", crate::icons::BOOK))
        .open(&mut open)
        .default_pos(ctx.content_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .default_size([720.0, 520.0])
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    egui::ScrollArea::vertical().id_salt("help-topics").show(ui, |ui| {
                        ui.selectable_value(&mut topic, HelpTopic::Start, "Getting started");
                        ui.selectable_value(&mut topic, HelpTopic::Tools, "Tools");
                        ui.selectable_value(&mut topic, HelpTopic::Shortcuts, "Keyboard shortcuts");
                        for (ti, tab) in RIBBON.iter().enumerate() {
                            ui.add_space(4.0);
                            ui.label(RichText::new(tab.name).strong());
                            for (gi, g) in tab.groups.iter().enumerate() {
                                ui.selectable_value(&mut topic, HelpTopic::Group(ti, gi), format!("   {}", g.name));
                            }
                        }
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().id_salt("help-page").show(ui, |ui| page(ui, st, &topic));
                });
            });
        });
    st.help = open.then_some(topic);
}

fn page(ui: &mut egui::Ui, st: &AppState, topic: &HelpTopic) {
    let weak = st.palette.weak_text;
    match topic {
        HelpTopic::Start => {
            ui.heading("Getting started");
            ui.label(START);
        }
        HelpTopic::Tools => {
            ui.heading("Tools");
            ui.label("A tool decides what dragging and clicking on the canvas do. Its settings are in the Tool settings panel.");
            for t in ToolId::ALL {
                let info = t.info();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(icon(info.icon, 18.0));
                    ui.label(RichText::new(info.name).strong());
                    ui.label(RichText::new(format!("key {}", key_char(info.key))).color(weak));
                });
                ui.label(info.description);
                ui.label(RichText::new(info.help).color(weak));
            }
        }
        HelpTopic::Shortcuts => {
            ui.heading("Keyboard shortcuts");
            egui::Grid::new("shortcuts").striped(true).num_columns(2).show(ui, |ui| {
                for c in Cmd::ALL {
                    let info = c.info();
                    if info.shortcuts.is_empty() {
                        continue;
                    }
                    let keys: Vec<String> = info.shortcuts.iter().map(|s| ui.ctx().format_shortcut(s)).collect();
                    ui.label(keys.join("  or  "));
                    ui.label(format!("{}  {}", info.icon, info.name));
                    ui.end_row();
                }
                for t in ToolId::ALL {
                    ui.label(key_char(t.info().key).to_string());
                    ui.label(format!("{} tool", t.info().name));
                    ui.end_row();
                }
                for (k, what) in [
                    ("Space + drag", "Scroll the view, with any tool"),
                    ("Middle button drag", "Scroll the view"),
                    ("Mouse wheel", "Zoom around the pointer (or scroll; see Preferences › Navigation)"),
                    ("Shift + wheel", "Scroll sideways"),
                    ("Ctrl + wheel, pinch", "Zoom around the pointer"),
                    ("Alt", "Show keytips on the ribbon"),
                    ("F1", "Help for the button under the pointer"),
                ] {
                    ui.label(k);
                    ui.label(what);
                    ui.end_row();
                }
            });
        }
        HelpTopic::Group(t, g) => {
            let (Some(tab), Some(group)) = (RIBBON.get(*t), RIBBON.get(*t).and_then(|tab| tab.groups.get(*g))) else {
                return;
            };
            ui.label(RichText::new(tab.name).color(weak));
            ui.heading(group.name);
            ui.label(group.description);
            for &c in group.cmds {
                let info = c.info();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(icon(info.icon, 18.0));
                    ui.label(RichText::new(info.name).strong());
                    if let Some(s) = shortcut_text(ui.ctx(), c) {
                        ui.label(RichText::new(s).color(weak));
                    }
                });
                ui.label(info.description);
                if !info.help.is_empty() {
                    ui.label(RichText::new(info.help).color(weak));
                }
                if let Err(why) = availability(st, c) {
                    ui.label(RichText::new(format!("Not available right now: {why}")).color(st.palette.changed));
                }
            }
        }
    }
}
