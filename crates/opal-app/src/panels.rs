//! The side panels: Layers, History and Tool settings.

use crate::commands::{Cmd, availability};
use crate::icons;
use crate::settings::{self, Show};
use crate::state::AppState;
use crate::theme::{icon, icon_family};
use crate::widgets::{icon_command, shortcut_text};
use eframe::egui::{self, Color32, CursorIcon, Rect, RichText, Sense, Stroke, Vec2};
use opal_core::document::{Document, Layer, LayerId, Position};
use opal_core::history::NodeId;
use std::collections::HashSet;
use std::sync::Arc;

/// What the user did in the Layers panel, applied once the panel is drawn.
enum Action {
    Select(LayerId),
    Flag(LayerId, bool),
    Expand(LayerId, bool),
    StartRename(LayerId),
    RenameText(LayerId, String),
    /// Ends renaming; `None` cancels.
    FinishRename(LayerId, Option<String>),
    Move(LayerId, Position),
    Command(Cmd),
}

const ROW_HEIGHT: f32 = 26.0;

/// The commands of a layer's context menu.
const LAYER_MENU: &[Cmd] = &[
    Cmd::NewLayer,
    Cmd::NewGroup,
    Cmd::Duplicate,
    Cmd::Rename,
    Cmd::ToggleVisible,
    Cmd::ToggleLock,
    Cmd::MergeDown,
    Cmd::DeleteLayer,
];

// ---- Layers ---------------------------------------------------------------------------

pub fn layers_panel(ui: &mut egui::Ui, st: &mut AppState) {
    if st.active().is_none() {
        ui.label(RichText::new("No image is open.").color(st.palette.weak_text));
        return;
    }
    ui.horizontal(|ui| {
        for cmd in
            [Cmd::NewLayer, Cmd::NewGroup, Cmd::Duplicate, Cmd::MoveUp, Cmd::MoveDown, Cmd::MergeDown, Cmd::DeleteLayer]
        {
            icon_command(ui, st, cmd);
        }
    });
    opacity_row(ui, st);
    ui.separator();

    let d = st.active().expect("checked above");
    let doc = d.doc().clone();
    let active = d.active_layer;
    let expanded = d.expanded.clone();
    let menu: Vec<MenuEntry> = LAYER_MENU
        .iter()
        .map(|&c| MenuEntry { cmd: c, available: availability(st, c), shortcut: shortcut_text(ui.ctx(), c) })
        .collect();
    // Scroll to the active layer when it changes, wherever the change came from.
    let last_key = ui.scope_id().with("last-active-layer");
    let scroll_to = ui.ctx().data(|m| m.get_temp::<Option<LayerId>>(last_key)) != Some(active);
    ui.ctx().data_mut(|m| m.insert_temp(last_key, active));
    let dragging: Option<LayerId> = egui::DragAndDrop::payload::<LayerId>(ui.ctx()).map(|p| *p);
    let released = ui.input(|i| i.pointer.any_released());
    let mut actions = Vec::new();
    let mut drop_target: Option<(Position, Rect, bool)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut rows = RowCtx {
            doc: &doc,
            active,
            scroll_to,
            expanded: &expanded,
            renaming: &st.renaming,
            dragging,
            menu: &menu,
            actions: &mut actions,
            drop_target: &mut drop_target,
            accent: st.palette.accent,
            weak: st.palette.weak_text,
        };
        layer_rows(ui, &mut rows, &doc.layers, None, 0);
    });
    if let Some((pos, line, into)) = drop_target {
        let p = ui.painter();
        if into {
            p.rect_stroke(line, 3.0, Stroke::new(2.0, st.palette.accent), egui::StrokeKind::Inside);
        } else {
            p.line_segment([line.left_center(), line.right_center()], Stroke::new(2.0, st.palette.accent));
        }
        if released && let Some(dragged) = dragging {
            actions.push(Action::Move(dragged, pos));
        }
    }
    if dragging.is_some() && released {
        egui::DragAndDrop::clear_payload(ui.ctx());
    }
    apply(st, actions);
}

struct MenuEntry {
    cmd: Cmd,
    available: Result<(), String>,
    shortcut: Option<String>,
}

struct RowCtx<'a> {
    doc: &'a Document,
    active: Option<LayerId>,
    scroll_to: bool,
    expanded: &'a HashSet<LayerId>,
    renaming: &'a Option<(LayerId, String)>,
    dragging: Option<LayerId>,
    menu: &'a [MenuEntry],
    actions: &'a mut Vec<Action>,
    drop_target: &'a mut Option<(Position, Rect, bool)>,
    accent: Color32,
    weak: Color32,
}

/// Draws `layers` top to bottom (they are stored bottom to top).
fn layer_rows(ui: &mut egui::Ui, cx: &mut RowCtx<'_>, layers: &[Arc<Layer>], parent: Option<LayerId>, depth: usize) {
    for (index, layer) in layers.iter().enumerate().rev() {
        layer_row(ui, cx, layer, Position { parent, index }, depth);
        if let Some(children) = layer.children()
            && cx.expanded.contains(&layer.id)
        {
            layer_rows(ui, cx, children, Some(layer.id), depth + 1);
        }
    }
}

fn layer_row(ui: &mut egui::Ui, cx: &mut RowCtx<'_>, layer: &Layer, pos: Position, depth: usize) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click_and_drag());
    let is_active = cx.active == Some(layer.id);
    let visuals = ui.visuals().clone();
    if is_active {
        ui.painter().rect_filled(rect, 4.0, cx.accent.gamma_multiply(0.35));
        if cx.scroll_to {
            response.scroll_to_me(None);
        }
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let inner = rect.shrink2(Vec2::new(4.0, 2.0));
    let mut row =
        ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    row.add_space(depth as f32 * 16.0);
    let eye = if layer.visible { icons::EYE } else { icons::EYE_OFF };
    let eye_color = if layer.visible { visuals.text_color() } else { cx.weak };
    let r = row.add(egui::Button::new(icon(eye, 15.0).color(eye_color)).frame(false)).on_hover_text(if layer.visible {
        "Hide this layer"
    } else {
        "Show this layer"
    });
    if r.clicked() {
        cx.actions.push(Action::Flag(layer.id, true));
    }
    let lock = if layer.locked { icons::LOCK } else { icons::UNLOCK };
    let lock_color = if layer.locked { visuals.text_color() } else { cx.weak.gamma_multiply(0.6) };
    let r = row
        .add(egui::Button::new(icon(lock, 15.0).color(lock_color)).frame(false))
        .on_hover_text(if layer.locked { "Unlock this layer" } else { "Lock this layer against changes" });
    if r.clicked() {
        cx.actions.push(Action::Flag(layer.id, false));
    }
    if layer.is_group() {
        let open = cx.expanded.contains(&layer.id);
        let chevron = if open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
        let r = row.add(egui::Button::new(icon(chevron, 13.0)).frame(false)).on_hover_text(if open {
            "Collapse"
        } else {
            "Expand"
        });
        if r.clicked() {
            cx.actions.push(Action::Expand(layer.id, !open));
        }
        row.label(icon(icons::GROUP, 15.0));
    } else {
        row.label(icon(icons::LAYER, 15.0).color(cx.weak));
    }
    match cx.renaming {
        Some((id, name)) if *id == layer.id => {
            let mut text = name.clone();
            let r = row.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.changed() {
                cx.actions.push(Action::RenameText(layer.id, text.clone()));
            }
            if r.lost_focus() {
                let cancel = row.input(|i| i.key_pressed(egui::Key::Escape));
                cx.actions.push(Action::FinishRename(layer.id, (!cancel).then_some(text)));
            }
        }
        _ => {
            let name =
                if layer.visible { RichText::new(&layer.name) } else { RichText::new(&layer.name).color(cx.weak) };
            row.label(name);
            if layer.opacity < 1.0 {
                row.label(RichText::new(format!("{:.0}%", layer.opacity * 100.0)).small().color(cx.weak));
            }
        }
    }

    if response.clicked() || response.secondary_clicked() || response.drag_started() {
        cx.actions.push(Action::Select(layer.id));
    }
    if response.double_clicked() {
        cx.actions.push(Action::StartRename(layer.id));
    }
    if response.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), layer.id);
    }
    if response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }

    // Where a layer being dragged would land if dropped here.
    if let Some(dragged) = cx.dragging
        && dragged != layer.id
        && !cx.doc.is_descendant(layer.id, dragged)
        && let Some(p) = ui.ctx().pointer_latest_pos()
        && rect.contains(p)
    {
        let t = (p.y - rect.top()) / rect.height();
        let edge = |y: f32| Rect::from_center_size(egui::pos2(rect.center().x, y), Vec2::new(rect.width(), 0.0));
        let (mut target, line, into) = if layer.is_group() && (0.3..0.7).contains(&t) {
            let n = layer.children().map_or(0, <[_]>::len);
            (Position { parent: Some(layer.id), index: n }, rect, true)
        } else if t < 0.5 {
            (Position { parent: pos.parent, index: pos.index + 1 }, edge(rect.top()), false)
        } else {
            (pos, edge(rect.bottom()), false)
        };
        // The move's index counts positions after the dragged layer is taken out.
        if let Some(from) = cx.doc.position(dragged)
            && from.parent == target.parent
            && from.index < target.index
        {
            target.index -= 1;
        }
        *cx.drop_target = Some((target, line, into));
    }

    response.context_menu(|ui| {
        ui.label(RichText::new(&layer.name).strong());
        ui.separator();
        for e in cx.menu {
            let info = e.cmd.info();
            let mut b = egui::Button::new(format!("{}  {}", info.icon, info.name));
            if let Some(s) = &e.shortcut {
                b = b.shortcut_text(s.as_str());
            }
            let r = ui.add_enabled(e.available.is_ok(), b);
            let r = match &e.available {
                Ok(()) => r.on_hover_text(info.description),
                Err(why) => r.on_disabled_hover_text(why.as_str()),
            };
            if r.clicked() {
                cx.actions.push(Action::Select(layer.id));
                cx.actions.push(Action::Command(e.cmd));
                ui.close();
            }
        }
    });
}

/// The opacity slider for the active layer. Dragging it changes the image live and becomes
/// one history step when released.
fn opacity_row(ui: &mut egui::Ui, st: &mut AppState) {
    let Some(d) = st.active_mut() else { return };
    let Some(id) = d.active_layer else { return };
    let Some(layer) = d.doc().layer(id) else { return };
    let locked = layer.locked;
    let mut pct = layer.opacity * 100.0;
    ui.horizontal(|ui| {
        let label = ui.label("Opacity");
        let r = ui.add_enabled(!locked, egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").fixed_decimals(0));
        let r = r.labelled_by(label.id);
        let r = if locked { r.on_disabled_hover_text("The layer is locked.") } else { r };
        if r.changed() {
            d.edit_pending("Change opacity", |doc| {
                if let Some(l) = doc.layer_mut(id) {
                    l.opacity = (pct / 100.0).clamp(0.0, 1.0);
                }
            });
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            d.commit_pending();
        }
    });
}

fn apply(st: &mut AppState, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Select(id) => {
                if let Some(d) = st.active_mut() {
                    d.active_layer = Some(id);
                }
            }
            Action::Flag(id, visibility) => st.set_layer_flag(id, visibility),
            Action::Expand(id, open) => {
                if let Some(d) = st.active_mut() {
                    if open {
                        d.expanded.insert(id);
                    } else {
                        d.expanded.remove(&id);
                    }
                }
            }
            Action::StartRename(id) => {
                let name = st.active().and_then(|d| d.doc().layer(id)).map(|l| l.name.clone()).unwrap_or_default();
                st.renaming = Some((id, name));
            }
            Action::RenameText(id, text) => st.renaming = Some((id, text)),
            Action::FinishRename(id, text) => {
                st.renaming = None;
                if let Some(text) = text {
                    st.rename_layer(id, text);
                }
            }
            Action::Move(id, pos) => st.move_layer_to(id, pos),
            Action::Command(cmd) => st.queue.push(cmd),
        }
    }
}

// ---- History --------------------------------------------------------------------------

pub fn history_panel(ui: &mut egui::Ui, st: &mut AppState) {
    if st.active().is_none() {
        ui.label(RichText::new("No image is open.").color(st.palette.weak_text));
        return;
    }
    ui.horizontal(|ui| {
        icon_command(ui, st, Cmd::Undo);
        icon_command(ui, st, Cmd::Redo);
    });
    ui.separator();
    let accent = st.palette.accent;
    let weak = st.palette.weak_text;
    let d = st.active().expect("checked above");
    let h = &d.history;
    let current = h.current();
    let saved = d.saved_at;
    let on_path: HashSet<NodeId> = h.path_from_root(current).into_iter().collect();
    let last_key = ui.scope_id().with("last-current-node");
    let scroll_to = ui.ctx().data(|m| m.get_temp::<NodeId>(last_key)) != Some(current);
    ui.ctx().data_mut(|m| m.insert_temp(last_key, current));
    let mut goto: Option<NodeId> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // Depth first. Of a node's children, the one its branch continues with stays at the
        // same indent and comes last; the others are indented as side branches.
        let mut stack = vec![(h.root(), 0usize)];
        while let Some((n, depth)) = stack.pop() {
            let node = h.node(n);
            let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
            if n == current {
                ui.painter().rect_filled(rect, 4.0, accent.gamma_multiply(0.35));
                if scroll_to {
                    r.scroll_to_me(None);
                }
            } else if r.hovered() {
                ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let x = rect.left() + 6.0 + depth as f32 * 14.0;
            let font = egui::FontId::proportional(13.0);
            if depth > 0 {
                // A branch: a corner from the row above into this one.
                let stroke = Stroke::new(1.0, weak);
                let cx = x - 9.0;
                ui.painter().line_segment([egui::pos2(cx, rect.top()), egui::pos2(cx, rect.center().y)], stroke);
                ui.painter()
                    .line_segment([egui::pos2(cx, rect.center().y), egui::pos2(x - 3.0, rect.center().y)], stroke);
            }
            let color = if on_path.contains(&n) { ui.visuals().text_color() } else { weak };
            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &node.label, font, color);
            if saved == Some(n) {
                ui.painter().text(
                    rect.right_center() - egui::vec2(6.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    icons::SAVE.to_string(),
                    egui::FontId::new(13.0, icon_family()),
                    weak,
                );
            }
            let age = match node.created.elapsed() {
                Ok(t) if t.as_secs() < 60 => format!("{} seconds ago", t.as_secs()),
                Ok(t) if t.as_secs() < 3600 => format!("{} minutes ago", t.as_secs() / 60),
                Ok(t) => format!("{} hours ago", t.as_secs() / 3600),
                Err(_) => String::new(),
            };
            let mut tip = format!("{}\n{age}", node.label);
            if saved == Some(n) {
                tip.push_str("\nThe saved file matches this state.");
            }
            if !on_path.contains(&n) {
                tip.push_str("\nOn another branch: click to go there.");
            }
            if r.on_hover_text(tip).clicked() && n != current {
                goto = Some(n);
            }
            let main = node.last_child.or_else(|| node.children.last().copied());
            if let Some(m) = main {
                stack.push((m, depth));
            }
            for &c in node.children.iter().rev() {
                if Some(c) != main {
                    stack.push((c, depth + 1));
                }
            }
        }
    });
    if let Some(n) = goto
        && let Some(d) = st.active_mut()
    {
        d.goto(n);
    }
}

// ---- Tool settings --------------------------------------------------------------------

pub fn tool_settings_panel(ui: &mut egui::Ui, st: &mut AppState) {
    let tool = st.tool;
    let info = tool.info();
    ui.horizontal(|ui| {
        ui.label(icon(info.icon, 20.0));
        ui.heading(info.name);
    });
    ui.label(RichText::new(info.description).color(st.palette.weak_text));
    ui.add_space(4.0);
    if info.settings.is_empty() {
        ui.label(RichText::new("This tool has no settings.").color(st.palette.weak_text));
        return;
    }
    presets_row(ui, st);
    ui.separator();
    let palette = st.palette;
    let settings = st.tool_settings.get_mut(&tool).expect("every tool has settings");
    if settings::settings_ui(ui, settings, Show::All, &palette, info.id) {
        st.settings_touched();
    }
}

/// Saving the current tool's settings under a name, and applying saved ones.
fn presets_row(ui: &mut egui::Ui, st: &mut AppState) {
    let tool = st.tool;
    let names: Vec<String> = st.presets.get(&tool).map(|p| p.keys().cloned().collect()).unwrap_or_default();
    let weak = st.palette.weak_text;
    ui.horizontal(|ui| {
        let mut apply: Option<String> = None;
        egui::ComboBox::from_id_salt("tool-preset").selected_text("Presets").show_ui(ui, |ui| {
            if names.is_empty() {
                ui.label(RichText::new("No presets saved yet.").color(weak));
            }
            for n in &names {
                if ui.button(n).on_hover_text("Apply these settings").clicked() {
                    apply = Some(n.clone());
                }
            }
        });
        if let Some(n) = apply {
            let map = st.presets[&tool][&n].clone();
            let s = st.tool_settings.get_mut(&tool).expect("every tool has settings");
            s.reset_all();
            s.load_map(&map);
            st.settings_touched();
        }
        match st.preset_name.as_mut() {
            None => {
                if ui.button("Save as preset…").clicked() {
                    st.preset_name = Some(String::new());
                }
            }
            Some(name) => {
                let r = ui.add(egui::TextEdit::singleline(name).hint_text("Preset name").desired_width(110.0));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let trimmed = name.trim().to_owned();
                if (enter || ui.button("Save").clicked()) && !trimmed.is_empty() {
                    let map = st.tool_settings[&tool].to_map();
                    st.presets.entry(tool).or_default().insert(trimmed, map);
                    st.preset_name = None;
                    st.settings_touched();
                } else if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    st.preset_name = None;
                }
            }
        }
    });
    if !names.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Delete:").small().color(weak));
            for n in &names {
                if ui.small_button(format!("{n} ×")).on_hover_text("Delete this preset").clicked() {
                    if let Some(p) = st.presets.get_mut(&tool) {
                        p.remove(n);
                    }
                    st.settings_touched();
                }
            }
        });
    }
}
