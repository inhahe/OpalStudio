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
    /// A click on a layer's row, with the modifiers held: plain selects it alone, Ctrl
    /// adds or removes it, Shift selects the range from the active layer.
    Click(LayerId, egui::Modifiers),
    /// Makes the layer active, keeping the selection if it is already in it (for a context
    /// menu or a drag).
    Touch(LayerId),
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
    let active = d.active_layer();
    let selected: HashSet<LayerId> = doc.layer_ids().into_iter().filter(|&l| d.is_selected(l)).collect();
    let expanded = d.expanded.clone();
    let thumb = match st.prefs.choice("ui.layer_thumbs").as_str() {
        "small" => 28.0,
        "large" => 44.0,
        _ => 0.0,
    };
    let menu: Vec<MenuEntry> = LAYER_MENU
        .iter()
        .map(|&c| MenuEntry { cmd: c, available: availability(st, c), shortcut: shortcut_text(ui.ctx(), c) })
        .collect();
    // Scroll to the active layer when it changes, wherever the change came from.
    let last_key = ui.scope_id().with("last-active-layer");
    let scroll_to = ui.ctx().data(|m| m.get_temp::<Option<LayerId>>(last_key)) != Some(active);
    ui.ctx().data_mut(|m| m.insert_temp(last_key, active));
    // Only a rename begun in this document shows here.
    let renaming =
        st.renaming.as_ref().filter(|r| Some(r.0) == st.active_doc).map(|(_, layer, text)| (*layer, text.clone()));
    let dragging: Option<LayerId> = egui::DragAndDrop::payload::<LayerId>(ui.ctx()).map(|p| *p);
    let released = ui.input(|i| i.pointer.any_released());
    let mut actions = Vec::new();
    let mut drop_target: Option<(Position, Rect, bool)> = None;
    let mut order = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut rows = RowCtx {
            doc: &doc,
            thumb,
            thumbs: &mut st.thumbs,
            active,
            selected: &selected,
            order: &mut order,
            scroll_to,
            expanded: &expanded,
            renaming: &renaming,
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
    apply(st, actions, &order);
}

struct MenuEntry {
    cmd: Cmd,
    available: Result<(), String>,
    shortcut: Option<String>,
}

struct RowCtx<'a> {
    doc: &'a Document,
    /// The size of layer thumbnails, in points (0 for none).
    thumb: f32,
    thumbs: &'a mut crate::thumbs::Thumbnails,
    active: Option<LayerId>,
    selected: &'a HashSet<LayerId>,
    /// The layers in the order their rows are drawn, top to bottom (for Shift+click).
    order: &'a mut Vec<LayerId>,
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

fn layer_row(ui: &mut egui::Ui, cx: &mut RowCtx<'_>, layer: &Arc<Layer>, pos: Position, depth: usize) {
    let height = ROW_HEIGHT.max(cx.thumb + 6.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    cx.order.push(layer.id);
    let is_active = cx.active == Some(layer.id);
    let visuals = ui.visuals().clone();
    if is_active {
        ui.painter().rect_filled(rect, 4.0, cx.accent.gamma_multiply(0.35));
        if cx.scroll_to {
            response.scroll_to_me(None);
        }
    } else if cx.selected.contains(&layer.id) {
        ui.painter().rect_filled(rect, 4.0, cx.accent.gamma_multiply(0.18));
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
    } else if cx.thumb == 0.0 {
        row.label(icon(icons::LAYER, 15.0).color(cx.weak));
    }
    if cx.thumb > 0.0 {
        let ppp = row.ctx().pixels_per_point();
        let size = (cx.thumb * ppp).round() as u32;
        let texture = cx.thumbs.layer(row.ctx(), layer, cx.doc.canvas.rect(), size);
        let (r, _) = row.allocate_exact_size(Vec2::splat(cx.thumb), Sense::hover());
        let fit = texture.size_vec2() / ppp;
        let shown = Rect::from_center_size(r.center(), fit);
        checkerboard(row.painter(), shown);
        row.painter().image(
            texture.id(),
            shown,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        row.painter().rect_stroke(shown, 0.0, Stroke::new(1.0, cx.weak.gamma_multiply(0.5)), egui::StrokeKind::Outside);
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

    if response.clicked() {
        cx.actions.push(Action::Click(layer.id, ui.input(|i| i.modifiers)));
    } else if response.secondary_clicked() || response.drag_started() {
        cx.actions.push(Action::Touch(layer.id));
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
        // On a group's row, the middle means "into the group, on top"; so does the bottom
        // of an expanded group's row, since its top child is drawn just below it.
        let open = cx.expanded.contains(&layer.id);
        let into_group = layer.is_group() && ((0.3..0.7).contains(&t) || (open && t >= 0.7));
        let (mut target, line, into) = if into_group {
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
                cx.actions.push(Action::Touch(layer.id));
                cx.actions.push(Action::Command(e.cmd));
                ui.close();
            }
        }
    });
}

/// A small light-and-dark checkerboard behind a thumbnail, so transparency shows.
fn checkerboard(p: &egui::Painter, r: Rect) {
    let cell = 5.0;
    p.rect_filled(r, 0.0, Color32::from_gray(230));
    let (nx, ny) = ((r.width() / cell).ceil() as i32, (r.height() / cell).ceil() as i32);
    for j in 0..ny {
        for i in 0..nx {
            if (i + j) % 2 == 1 {
                let c = Rect::from_min_size(r.min + Vec2::new(i as f32 * cell, j as f32 * cell), Vec2::splat(cell));
                p.rect_filled(c.intersect(r), 0.0, Color32::from_gray(190));
            }
        }
    }
}

/// The opacity slider for the active layer. Dragging it changes the image live and becomes
/// one history step when released.
fn opacity_row(ui: &mut egui::Ui, st: &mut AppState) {
    let unit = st.unit();
    let Some(d) = st.active_mut() else { return };
    let Some(id) = d.active_layer() else { return };
    let Some(layer) = d.doc().layer(id) else { return };
    let locked = layer.locked;
    let mut pct = f64::from(layer.opacity) * 100.0;
    ui.horizontal(|ui| {
        let label = ui.label("Opacity");
        // Dragged on the slider, or typed (an expression) into the field beside it.
        let slider =
            ui.add_enabled(!locked, egui::Slider::new(&mut pct, 0.0..=100.0).fixed_decimals(0).show_value(false));
        let scope = crate::lengths::image_scope(Some(d.doc()), unit, d.doc().resolution);
        let field = crate::number_field::NumberField::number(&mut pct, scope)
            .range(0.0..=100.0)
            .suffix("%")
            .decimals(0, 1)
            .remember("opacity");
        let field = ui.add_enabled(!locked, field);
        let r = slider.labelled_by(label.id) | field.labelled_by(label.id);
        let r = if locked { r.on_disabled_hover_text("The layer is locked.") } else { r };
        if r.changed() {
            d.edit_pending("Change opacity", |doc| {
                if let Some(l) = doc.layer_mut(id) {
                    l.opacity = (pct / 100.0).clamp(0.0, 1.0) as f32;
                }
            });
        }
        // One history step per change: commit once the slider is let go, or the typed value
        // is finished (the field loses focus), not on every keystroke.
        if d.pending_label() == Some("Change opacity") && !r.dragged() && !r.has_focus() {
            d.commit_pending();
        }
    });
}

fn apply(st: &mut AppState, actions: Vec<Action>, order: &[LayerId]) {
    for a in actions {
        match a {
            Action::Click(id, m) => {
                if let Some(d) = st.active_mut() {
                    if m.command {
                        d.toggle_selected(id);
                    } else if m.shift {
                        d.select_range(id, order);
                    } else {
                        d.select_only(id);
                    }
                }
            }
            Action::Touch(id) => {
                if let Some(d) = st.active_mut() {
                    if d.is_selected(id) {
                        d.make_active(id);
                    } else {
                        d.select_only(id);
                    }
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
                if let Some(d) = st.active() {
                    let name = d.doc().layer(id).map(|l| l.name.clone()).unwrap_or_default();
                    st.renaming = Some((d.id, id, name));
                }
            }
            Action::RenameText(id, text) => {
                if let Some(r) = st.renaming.as_mut()
                    && r.1 == id
                {
                    r.2 = text;
                }
            }
            Action::FinishRename(id, text) => {
                let keep = text.is_some();
                if let (Some(r), Some(text)) = (st.renaming.as_mut(), text)
                    && r.1 == id
                {
                    r.2 = text;
                }
                st.finish_rename(keep);
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
    let weak = st.palette.weak_text;
    let (memory, dropped) = st.active().map(|d| (d.history.memory(), d.history.dropped())).expect("checked above");
    let limit = crate::prefs::bytes(&st.prefs, "history.memory");
    ui.horizontal(|ui| {
        icon_command(ui, st, Cmd::Undo);
        icon_command(ui, st, Cmd::Redo);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut tip = format!(
                "What the earlier states hold beyond the image as it is now. History may use up to {} \
                 (Preferences › History).",
                opal_io::bytes(limit)
            );
            if dropped > 0 {
                tip.push_str(&format!("\n{dropped} of the oldest states have been dropped to stay within it."));
            }
            ui.label(RichText::new(opal_io::bytes(memory)).color(weak)).on_hover_text(tip);
        });
    });
    ui.separator();
    let accent = st.palette.accent;
    // The document and the thumbnails, borrowed apart.
    let active = st.active_doc;
    let d = st.docs.iter().find(|d| Some(d.id) == active).expect("checked above");
    let st_thumbs = &mut st.thumbs;
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
            let thumbs = &mut *st_thumbs;
            let r = r.on_hover_ui(|ui| {
                ui.label(&tip);
                let ppp = ui.ctx().pixels_per_point();
                let texture = thumbs.document(ui.ctx(), node.doc(), (160.0 * ppp) as u32);
                let size = texture.size_vec2() / ppp;
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                checkerboard(ui.painter(), rect);
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            });
            if r.clicked() && n != current {
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
    // While the transform box is open, its settings come first.
    if st.active().is_some_and(|d| d.transform.is_some()) {
        crate::transform_ui::settings(ui, st);
        return;
    }
    let tool = st.tool;
    let info = tool.info();
    let has_settings = !info.settings.is_empty();
    ui.horizontal(|ui| {
        heading(ui, info.icon, info.name, info.description);
        if has_settings {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| presets_menu(ui, st));
        }
    });
    if !has_settings {
        ui.label(RichText::new("This tool has no settings.").color(st.palette.weak_text));
        return;
    }
    preset_naming(ui, st);
    ui.separator();
    let palette = st.palette;
    let settings = st.tool_settings.get_mut(&tool).expect("every tool has settings");
    if settings::settings_ui(ui, settings, Show::All, &palette, info.id) {
        st.settings_touched();
    }
    // The crop box, in numbers.
    if tool == crate::tools::ToolId::Crop
        && let Some(id) = st.active_doc
    {
        ui.separator();
        ui.label(RichText::new("Box").strong());
        crate::box_fields::crop(ui, st, id, "crop-panel");
    }
}

/// The heading of what the tool settings show: `glyph` and `name`, which say on hover what
/// it `does` (the status bar says it too, kept short, so that the settings show below it).
pub fn heading(ui: &mut egui::Ui, glyph: char, name: &str, does: &str) {
    ui.horizontal(|ui| {
        ui.label(icon(glyph, 20.0));
        ui.heading(name);
    })
    .response
    .on_hover_text(does);
}

/// A menu of the current tool's presets: saving its settings under a name (typed below the
/// heading, [`preset_naming`]), applying saved ones, and deleting them.
fn presets_menu(ui: &mut egui::Ui, st: &mut AppState) {
    let tool = st.tool;
    let names: Vec<String> = st.presets.get(&tool).map(|p| p.keys().cloned().collect()).unwrap_or_default();
    let weak = st.palette.weak_text;
    let (mut apply, mut delete, mut save) = (None, None, false);
    ui.menu_button("Presets", |ui| {
        save = ui.button("Save as preset…").on_hover_text("Save these settings under a name").clicked();
        ui.separator();
        if names.is_empty() {
            ui.label(RichText::new("No presets saved yet.").color(weak));
        } else {
            for n in &names {
                if ui.button(n).on_hover_text("Apply these settings").clicked() {
                    apply = Some(n.clone());
                }
            }
            ui.separator();
            // Apart from the presets applied, so that a slip of the pointer deletes none.
            ui.menu_button("Delete a preset", |ui| {
                for n in &names {
                    let r = ui.button(n).on_hover_text("Delete this preset");
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Delete {n}")));
                    if r.clicked() {
                        delete = Some(n.clone());
                    }
                }
            });
        }
    })
    .response
    .on_hover_text("Save these settings under a name, or apply or delete saved ones");
    if save {
        // A name being typed already is kept, to be finished.
        st.preset_name.get_or_insert_with(String::new);
        // The field takes the keyboard from the next pass: in this one, the click on the menu
        // is a click elsewhere, which would take it away again.
        let pass = ui.ctx().cumulative_pass_nr();
        ui.data_mut(|d| d.insert_temp(preset_name_id(), pass));
    }
    if let Some(n) = apply {
        let map = st.presets[&tool][&n].clone();
        let s = st.tool_settings.get_mut(&tool).expect("every tool has settings");
        s.reset_all();
        s.load_map(&map);
        st.settings_touched();
    }
    if let Some(n) = delete {
        if let Some(p) = st.presets.get_mut(&tool) {
            p.remove(&n);
        }
        st.settings_touched();
    }
}

/// The field a preset's name is typed in.
fn preset_name_id() -> egui::Id {
    egui::Id::unique("preset-name")
}

/// While a preset's name is being typed (after the menu's Save as preset…, which gives the
/// field the keyboard): the name, and buttons to save the current tool's settings under it
/// or not. Enter in the field saves, Esc in it does not.
fn preset_naming(ui: &mut egui::Ui, st: &mut AppState) {
    let tool = st.tool;
    let Some(mut name) = st.preset_name.take() else { return };
    // Kept being typed, unless saved or let go.
    let mut kept = true;
    ui.horizontal(|ui| {
        let field = egui::TextEdit::singleline(&mut name).id(preset_name_id()).hint_text("Preset name");
        let r = ui.add(field.desired_width(110.0));
        // Given the keyboard once, when Save as preset… asks (in an earlier pass); not taken
        // back from another field the user goes to meanwhile.
        let asked = ui.data(|d| d.get_temp::<u64>(preset_name_id()));
        if asked.is_some_and(|pass| pass < ui.ctx().cumulative_pass_nr()) {
            ui.data_mut(|d| d.remove::<u64>(preset_name_id()));
            r.request_focus();
        } else if asked.is_some() {
            ui.ctx().request_repaint();
        }
        let left_by = |key| r.lost_focus() && ui.input(|i| i.key_pressed(key));
        let (enter, escape) = (left_by(egui::Key::Enter), left_by(egui::Key::Escape));
        let trimmed = name.trim().to_owned();
        if (enter || ui.button("Save").clicked()) && !trimmed.is_empty() {
            let map = st.tool_settings[&tool].to_map();
            st.presets.entry(tool).or_default().insert(trimmed, map);
            st.settings_touched();
            kept = false;
        } else if ui.button("Cancel").clicked() || escape {
            kept = false;
        }
    });
    if kept {
        st.preset_name = Some(name);
    }
}
