//! The ribbon: tabs of grouped command buttons, the pinned tools, keytips and command
//! search, all generated from the command registry.
//!
//! When the window is too narrow, groups shrink from the right: large buttons become small
//! ones, then the whole group collapses into one button that opens its commands. The tools
//! never shrink.

use crate::commands::{Cmd, Group, RIBBON, availability, checked};
use crate::icons;
use crate::state::{AppState, HelpTopic, Keytips};
use crate::theme::{icon, icon_family};
use crate::tools::ToolId;
use crate::widgets::{command_tooltip, menu_command, shortcut_text};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, Vec2};

const TAB_HEIGHT: f32 = 28.0;
const LARGE_H: f32 = 62.0;
const SMALL_H: f32 = 20.0;
const LABEL_H: f32 = 16.0;
const SINGLE_H: f32 = 30.0;
const GROUP_PAD: f32 = 6.0;
const TOOL_SIZE: f32 = 30.0;

/// How a group is drawn, from roomiest to most compact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Large,
    Small,
    /// Single-line ribbon only: icons without labels.
    IconsOnly,
    Collapsed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Large,
    Small,
    Icon,
}

/// The keytip letters of the tabs, in tab order.
pub fn tab_keytips() -> Vec<char> {
    let mut used = Vec::new();
    for t in RIBBON {
        let c = t.name.chars().find(|c| c.is_alphabetic() && !used.contains(&c.to_ascii_uppercase())).unwrap_or('?');
        used.push(c.to_ascii_uppercase());
    }
    used
}

/// The keytip letters of the commands on tab `tab`, in order of appearance. Each is unique on
/// the tab: the first free initial of the name's words, else any free letter of the name,
/// else a digit.
pub fn button_keytips(tab: usize) -> Vec<(Cmd, char)> {
    let mut out: Vec<(Cmd, char)> = Vec::new();
    for g in RIBBON[tab].groups {
        for &cmd in g.cmds {
            if out.iter().any(|(c, _)| *c == cmd) {
                continue;
            }
            let name = cmd.info().name.to_ascii_uppercase();
            let taken = |c: char| out.iter().any(|(_, k)| *k == c);
            let initials = name.split(|c: char| !c.is_alphanumeric()).filter_map(|w| w.chars().next());
            let letters = name.chars().filter(|c| c.is_alphanumeric());
            let key = initials
                .chain(letters)
                .find(|&c| !taken(c))
                .or_else(|| ('1'..='9').find(|&c| !taken(c)))
                .unwrap_or('?');
            out.push((cmd, key));
        }
    }
    out
}

fn single_line(st: &AppState) -> bool {
    st.prefs.choice("ui.ribbon") == "single"
}

/// Draws the ribbon.
pub fn ribbon_ui(ui: &mut egui::Ui, st: &mut AppState) {
    st.ribbon.hovered = None;
    st.ribbon.hovered_tool = None;
    st.ribbon.hovered_group = None;
    let tabs = tab_keytips();
    tab_row(ui, st, &tabs);
    let tools_here = st.prefs.choice("ui.tools") == "ribbon";
    let single = single_line(st);
    let body_h = if single { SINGLE_H } else { LARGE_H + LABEL_H + 4.0 };
    let (body, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), body_h), Sense::hover());
    let painter = ui.painter_at(body);
    painter.rect_filled(body, 0.0, st.palette.ribbon);
    let mut x = body.left() + 6.0;
    if tools_here {
        let w = tools_group(ui, st, Rect::from_min_max(Pos2::new(x, body.top()), body.max), single);
        x += w;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(x + 2.0, body.top() + 4.0), Pos2::new(x + 4.0, body.bottom() - 4.0)),
            1.0,
            st.palette.separator,
        );
        x += 10.0;
    }
    let tab = st.ribbon.tab.min(RIBBON.len() - 1);
    let area = Rect::from_min_max(Pos2::new(x, body.top()), Pos2::new(body.right() - 6.0, body.bottom()));
    groups(ui, st, tab, area, single);
}

fn tab_row(ui: &mut egui::Ui, st: &mut AppState, keys: &[char]) {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), TAB_HEIGHT), Sense::hover());
    ui.painter().rect_filled(row, 0.0, st.palette.tab_bar);
    let mut x = row.left() + 8.0;
    let font = FontId::proportional(13.5);
    for (i, t) in RIBBON.iter().enumerate() {
        let w = ui.painter().layout_no_wrap(t.name.to_owned(), font.clone(), Color32::WHITE).size().x + 24.0;
        let rect = Rect::from_min_size(Pos2::new(x, row.top() + 2.0), Vec2::new(w, TAB_HEIGHT - 2.0));
        let r = ui.interact(rect, ui.scope_id().with(("ribbon-tab", i)), Sense::click());
        let selected = st.ribbon.tab == i;
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, t.name));
        if selected {
            ui.painter().rect_filled(rect, egui::CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, st.palette.ribbon);
            ui.painter().rect_filled(
                Rect::from_min_max(
                    Pos2::new(rect.left() + 8.0, rect.bottom() - 3.0),
                    Pos2::new(rect.right() - 8.0, rect.bottom() - 1.0),
                ),
                1.0,
                st.palette.accent,
            );
        } else if r.hovered() {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 },
                st.palette.ribbon.gamma_multiply(0.6),
            );
        }
        let color = if selected { st.palette.strong_text } else { st.palette.weak_text };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, t.name, font.clone(), color);
        if r.clicked() {
            st.ribbon.tab = i;
        }
        if st.ribbon.keytips == Keytips::Tabs {
            keytip(ui, st, rect.center_bottom(), keys[i]);
        }
        x += w + 2.0;
    }
    // Command search and the ribbon size toggle, at the right end.
    let toggle = Rect::from_min_size(Pos2::new(row.right() - 30.0, row.top() + 3.0), Vec2::new(24.0, TAB_HEIGHT - 6.0));
    let chevron = if single_line(st) { icons::CHEVRON_DOWN } else { icons::CHEVRON_UP };
    let r = ui.interact(toggle, ui.scope_id().with("ribbon-size"), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, Cmd::ToggleRibbon.info().name));
    if r.hovered() {
        ui.painter().rect_filled(toggle, 4.0, st.palette.ribbon);
    }
    ui.painter().text(
        toggle.center(),
        Align2::CENTER_CENTER,
        chevron.to_string(),
        FontId::new(14.0, icon_family()),
        st.palette.weak_text,
    );
    let r = r.on_hover_ui(|ui| command_tooltip(ui, st, Cmd::ToggleRibbon));
    if r.clicked() {
        st.queue.push(Cmd::ToggleRibbon);
    }
    let search_w = 260.0f32.min((row.right() - x - 60.0).max(120.0));
    let search = Rect::from_min_size(
        Pos2::new(toggle.left() - search_w - 8.0, row.top() + 3.0),
        Vec2::new(search_w, TAB_HEIGHT - 6.0),
    );
    search_box(ui, st, search);
}

/// The width a group needs at a stage.
/// The width a group needs at a stage. `single` is the single-line ribbon, where small
/// buttons sit in one row and groups have no label underneath.
fn group_width(ui: &egui::Ui, g: &Group, stage: Stage, single: bool) -> f32 {
    let font = FontId::proportional(13.0);
    let text_w = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    let content = match (stage, single) {
        (Stage::Large, _) => {
            let large: f32 = g.cmds[..g.large].iter().map(|c| large_width(ui, c.info().name)).sum();
            large + small_columns(ui, &g.cmds[g.large..])
        }
        (Stage::Small, true) => g.cmds.iter().map(|c| small_width(ui, c.info().name)).sum(),
        (Stage::Small, false) => small_columns(ui, g.cmds),
        (Stage::IconsOnly, _) => g.cmds.len() as f32 * (SMALL_H + 6.0),
        (Stage::Collapsed, true) => collapsed_single_width(ui, g),
        (Stage::Collapsed, false) => large_width(ui, g.name),
    };
    let label_w = if single { 0.0 } else { text_w(g.name) + 26.0 };
    content.max(label_w) + GROUP_PAD * 2.0
}

/// The width of a collapsed group's button in the single-line ribbon: icon, name and arrow.
fn collapsed_single_width(ui: &egui::Ui, g: &Group) -> f32 {
    let font = FontId::proportional(13.0);
    ui.painter().layout_no_wrap(g.name.to_owned(), font, Color32::WHITE).size().x + 56.0
}

fn large_width(ui: &egui::Ui, name: &str) -> f32 {
    let font = FontId::proportional(12.5);
    let lines = split_label(name);
    lines
        .iter()
        .map(|l| ui.painter().layout_no_wrap((*l).to_owned(), font.clone(), Color32::WHITE).size().x)
        .fold(0.0, f32::max)
        .max(36.0)
        + 14.0
}

fn small_width(ui: &egui::Ui, name: &str) -> f32 {
    let font = FontId::proportional(13.0);
    ui.painter().layout_no_wrap(name.to_owned(), font, Color32::WHITE).size().x + 16.0 + 18.0
}

/// Small buttons stacked three to a column.
fn small_columns(ui: &egui::Ui, cmds: &[Cmd]) -> f32 {
    cmds.chunks(3).map(|col| col.iter().map(|c| small_width(ui, c.info().name)).fold(0.0, f32::max) + 2.0).sum()
}

/// A large button's label, broken into at most two lines at the space nearest the middle.
fn split_label(name: &str) -> Vec<&str> {
    if !name.contains(' ') || name.len() <= 8 {
        return vec![name];
    }
    let mid = name.len() / 2;
    let split = name.match_indices(' ').map(|(i, _)| i).min_by_key(|&i| i.abs_diff(mid)).expect("contains a space");
    vec![&name[..split], &name[split + 1..]]
}

/// Picks each group's stage so all fit in `width`, shrinking from the right.
fn stages(ui: &egui::Ui, groups: &[Group], width: f32, single: bool) -> Vec<Stage> {
    let ladder: &[Stage] = if single {
        &[Stage::Small, Stage::IconsOnly, Stage::Collapsed]
    } else {
        &[Stage::Large, Stage::Small, Stage::Collapsed]
    };
    let mut level = vec![0usize; groups.len()];
    let total = |level: &[usize]| -> f32 {
        groups.iter().zip(level).map(|(g, &l)| group_width(ui, g, ladder[l], single) + 4.0).sum()
    };
    'shrink: while total(&level) > width {
        for i in (0..groups.len()).rev() {
            if level[i] + 1 < ladder.len() {
                level[i] += 1;
                continue 'shrink;
            }
        }
        break;
    }
    level.into_iter().map(|l| ladder[l]).collect()
}

fn groups(ui: &mut egui::Ui, st: &mut AppState, tab: usize, area: Rect, single: bool) {
    let gs = RIBBON[tab].groups;
    let stages = stages(ui, gs, area.width(), single);
    let keys = if st.ribbon.keytips == Keytips::Buttons(tab) { button_keytips(tab) } else { Vec::new() };
    let key_of = |cmd: Cmd| keys.iter().find(|(c, _)| *c == cmd).map(|(_, k)| *k);
    let mut x = area.left();
    for (gi, (g, stage)) in gs.iter().zip(stages).enumerate() {
        let w = group_width(ui, g, stage, single);
        let rect = Rect::from_min_max(Pos2::new(x, area.top()), Pos2::new((x + w).min(area.right()), area.bottom()));
        if ui.rect_contains_pointer(rect) {
            st.ribbon.hovered_group = Some((tab, gi));
        }
        let content = if single {
            rect.shrink2(Vec2::new(GROUP_PAD, 3.0))
        } else {
            Rect::from_min_max(
                rect.min + Vec2::new(GROUP_PAD, 3.0),
                Pos2::new(rect.right() - GROUP_PAD, rect.bottom() - LABEL_H - 2.0),
            )
        };
        let mut cx = content.left();
        match stage {
            Stage::Collapsed => {
                let bw = if single { collapsed_single_width(ui, g) } else { large_width(ui, g.name) };
                let b = Rect::from_min_size(Pos2::new(cx, content.top()), Vec2::new(bw, content.height()));
                collapsed_group(ui, st, tab, gi, g, b, single);
            }
            Stage::Large | Stage::Small => {
                let n_large = if stage == Stage::Large { g.large } else { 0 };
                for &cmd in &g.cmds[..n_large] {
                    let bw = large_width(ui, cmd.info().name);
                    let b = Rect::from_min_size(Pos2::new(cx, content.top()), Vec2::new(bw, content.height()));
                    ribbon_button(ui, st, cmd, b, Style::Large, key_of(cmd));
                    cx += bw;
                }
                if single {
                    for &cmd in &g.cmds[n_large..] {
                        let bw = small_width(ui, cmd.info().name);
                        let b = Rect::from_min_size(Pos2::new(cx, content.top()), Vec2::new(bw, content.height()));
                        ribbon_button(ui, st, cmd, b, Style::Small, key_of(cmd));
                        cx += bw;
                    }
                } else {
                    for col in g.cmds[n_large..].chunks(3) {
                        let cw = col.iter().map(|c| small_width(ui, c.info().name)).fold(0.0, f32::max);
                        for (row, &cmd) in col.iter().enumerate() {
                            let b = Rect::from_min_size(
                                Pos2::new(cx, content.top() + row as f32 * (SMALL_H + 1.0)),
                                Vec2::new(cw, SMALL_H),
                            );
                            ribbon_button(ui, st, cmd, b, Style::Small, key_of(cmd));
                        }
                        cx += cw + 2.0;
                    }
                }
            }
            Stage::IconsOnly => {
                for &cmd in g.cmds {
                    let b =
                        Rect::from_min_size(Pos2::new(cx, content.top()), Vec2::new(SMALL_H + 6.0, content.height()));
                    ribbon_button(ui, st, cmd, b, Style::Icon, key_of(cmd));
                    cx += SMALL_H + 6.0;
                }
            }
        }
        if !single {
            group_label(
                ui,
                st,
                tab,
                gi,
                g,
                Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - LABEL_H - 2.0), rect.max),
            );
        }
        x += w;
        if gi + 1 < gs.len() {
            let sep = Rect::from_min_max(Pos2::new(x + 1.0, area.top() + 6.0), Pos2::new(x + 2.0, area.bottom() - 6.0));
            ui.painter().rect_filled(sep, 0.0, st.palette.separator);
            x += 4.0;
        }
    }
}

/// A group's name below its buttons; hovering shows a ? that opens the group's help.
fn group_label(ui: &mut egui::Ui, st: &mut AppState, tab: usize, gi: usize, g: &Group, rect: Rect) {
    let r = ui.interact(rect, ui.scope_id().with(("group-label", tab, gi)), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Help on {}", g.name)));
    let color = st.palette.weak_text;
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, g.name, FontId::proportional(11.5), color);
    if r.hovered() {
        let font = FontId::proportional(11.5);
        let tw = ui.painter().layout_no_wrap(g.name.to_owned(), font, color).size().x;
        let q = Pos2::new(rect.center().x + tw / 2.0 + 10.0, rect.center().y);
        ui.painter().text(
            q,
            Align2::CENTER_CENTER,
            icons::HELP.to_string(),
            FontId::new(13.0, icon_family()),
            st.palette.accent,
        );
    }
    let r = r.on_hover_text(format!("{}\n\nClick for help on this group (or press F1).", g.description));
    if r.clicked() {
        st.help = Some(HelpTopic::Group(tab, gi));
    }
}

/// A group too narrow for its buttons: one button that opens them as a menu.
fn collapsed_group(ui: &mut egui::Ui, st: &mut AppState, tab: usize, gi: usize, g: &Group, rect: Rect, single: bool) {
    let id = ui.scope_id().with(("collapsed", tab, gi));
    let r = ui.interact(rect, id, Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, g.name));
    paint_button_bg(ui, st, &r, rect, false, true);
    let first_icon = g.cmds.first().map_or(icons::OPTIONS, |c| c.info().icon);
    if single {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            format!("{}  {} {}", first_icon, g.name, icons::CHEVRON_DOWN),
            FontId::proportional(13.0),
            st.palette.strong_text,
        );
    } else {
        ui.painter().text(
            rect.center_top() + Vec2::new(0.0, 16.0),
            Align2::CENTER_CENTER,
            first_icon.to_string(),
            FontId::new(22.0, icon_family()),
            st.palette.strong_text,
        );
        ui.painter().text(
            rect.center_top() + Vec2::new(0.0, 42.0),
            Align2::CENTER_CENTER,
            format!("{} {}", g.name, icons::CHEVRON_DOWN),
            FontId::proportional(12.5),
            st.palette.strong_text,
        );
    }
    let r = r.on_hover_text(format!(
        "{}\n\nThe window is too narrow to show this group's buttons; click to open them.",
        g.description
    ));
    egui::Popup::menu(&r).show(|ui| {
        for &cmd in g.cmds {
            menu_command(ui, st, cmd);
        }
    });
}

fn paint_button_bg(ui: &egui::Ui, st: &AppState, r: &Response, rect: Rect, on: bool, enabled: bool) {
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 4.0, st.palette.accent.gamma_multiply(0.30));
        p.rect_stroke(rect, 4.0, Stroke::new(1.0, st.palette.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
    }
    if enabled && r.is_pointer_button_down_on() {
        p.rect_filled(rect, 4.0, st.palette.accent.gamma_multiply(0.45));
    } else if enabled && r.hovered() {
        p.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
}

/// One command button. Disabled commands are dimmed and say why in their tooltip.
fn ribbon_button(ui: &mut egui::Ui, st: &mut AppState, cmd: Cmd, rect: Rect, style: Style, key: Option<char>) {
    let info = cmd.info();
    let ok = availability(st, cmd).is_ok();
    let on = checked(st, cmd) == Some(true);
    let r =
        ui.interact(rect, ui.scope_id().with(("ribbon-cmd", cmd)), if ok { Sense::click() } else { Sense::hover() });
    r.widget_info(|| match checked(st, cmd) {
        Some(on) => egui::WidgetInfo::selected(egui::Role::CheckBox, ok, on, info.name),
        None => egui::WidgetInfo::labeled(egui::Role::Button, ok, info.name),
    });
    paint_button_bg(ui, st, &r, rect, on, ok);
    let color = if ok { st.palette.strong_text } else { st.palette.weak_text.gamma_multiply(0.6) };
    let p = ui.painter();
    match style {
        Style::Large => {
            p.text(
                rect.center_top() + Vec2::new(0.0, 16.0),
                Align2::CENTER_CENTER,
                info.icon.to_string(),
                FontId::new(24.0, icon_family()),
                color,
            );
            let lines = split_label(info.name);
            for (i, line) in lines.iter().enumerate() {
                p.text(
                    rect.center_top() + Vec2::new(0.0, 38.0 + i as f32 * 14.0),
                    Align2::CENTER_CENTER,
                    *line,
                    FontId::proportional(12.5),
                    color,
                );
            }
        }
        Style::Small => {
            p.text(
                rect.left_center() + Vec2::new(10.0, 0.0),
                Align2::CENTER_CENTER,
                info.icon.to_string(),
                FontId::new(16.0, icon_family()),
                color,
            );
            p.text(
                rect.left_center() + Vec2::new(22.0, 0.0),
                Align2::LEFT_CENTER,
                info.name,
                FontId::proportional(13.0),
                color,
            );
        }
        Style::Icon => {
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                info.icon.to_string(),
                FontId::new(17.0, icon_family()),
                color,
            );
        }
    }
    if let Some(k) = key {
        let at = match style {
            // Below the label, over the group's name, where it hides nothing.
            Style::Large => rect.center_bottom() + Vec2::new(0.0, 9.0),
            _ => rect.left_bottom() + Vec2::new(10.0, -2.0),
        };
        keytip(ui, st, at, k);
    }
    if r.hovered() {
        st.ribbon.hovered = Some(cmd);
    }
    let r = r.on_hover_ui(|ui| command_tooltip(ui, st, cmd));
    if ok && r.clicked() {
        st.queue.push(cmd);
    }
}

/// The tools, pinned at the ribbon's left end: two rows of icon buttons. Returns its width.
fn tools_group(ui: &mut egui::Ui, st: &mut AppState, area: Rect, single: bool) -> f32 {
    let rows = if single { 1 } else { 2 };
    let n = ToolId::ALL.len();
    let cols = n.div_ceil(rows);
    let label_h = if single { 0.0 } else { LABEL_H };
    let top = area.top() + 3.0;
    for (i, &t) in ToolId::ALL.iter().enumerate() {
        let (c, r) = (i / rows, i % rows);
        let rect = Rect::from_min_size(
            Pos2::new(area.left() + c as f32 * (TOOL_SIZE + 2.0), top + r as f32 * (TOOL_SIZE + 2.0)),
            Vec2::splat(TOOL_SIZE.min(area.height() - 6.0)),
        );
        tool_button(ui, st, t, rect);
    }
    let w = cols as f32 * (TOOL_SIZE + 2.0);
    if !single {
        let lrect = Rect::from_min_max(
            Pos2::new(area.left(), area.bottom() - label_h - 2.0),
            Pos2::new(area.left() + w.max(40.0), area.bottom()),
        );
        ui.painter().text(
            lrect.center(),
            Align2::CENTER_CENTER,
            "Tools",
            FontId::proportional(11.5),
            st.palette.weak_text,
        );
    }
    w.max(40.0)
}

/// A tool button, as used in the ribbon and in the side strips.
pub fn tool_button(ui: &mut egui::Ui, st: &mut AppState, t: ToolId, rect: Rect) {
    let info = t.info();
    let r = ui.interact(rect, ui.scope_id().with(("tool", info.id)), Sense::click());
    let on = st.tool() == t;
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, format!("{} tool", info.name)));
    paint_button_bg(ui, st, &r, rect, on, true);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        info.icon.to_string(),
        FontId::new(19.0, icon_family()),
        st.palette.strong_text,
    );
    let r = r.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(info.name).strong());
            ui.label(RichText::new(format!("({})", key_char(info.key))).color(st.palette.weak_text));
        });
        ui.label(info.description);
        ui.label(RichText::new("F1: more help").small().color(st.palette.weak_text));
    });
    if r.hovered() {
        st.ribbon.hovered_tool = Some(t);
    }
    if r.clicked() {
        st.set_tool(t);
    }
}

/// The letter a key is labeled with.
pub fn key_char(k: egui::Key) -> char {
    k.name().chars().next().unwrap_or('?')
}

fn keytip(ui: &egui::Ui, st: &AppState, at: Pos2, key: char) {
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("keytips")));
    let r = Rect::from_center_size(at, Vec2::new(18.0, 16.0));
    p.rect_filled(r, 3.0, st.palette.keytip_bg);
    p.text(r.center(), Align2::CENTER_CENTER, key.to_string(), FontId::proportional(11.5), st.palette.keytip_fg);
}

// ---- Command search -------------------------------------------------------------------

/// The commands matching `query`, best first: every word of the query must appear in the
/// name or the description; matches in the name rank higher.
pub fn search(query: &str) -> Vec<Cmd> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u32, Cmd)> = Cmd::ALL
        .iter()
        .filter_map(|&c| {
            let info = c.info();
            let name = info.name.to_lowercase();
            let desc = info.description.to_lowercase();
            let mut score = 0;
            for w in &words {
                if name.starts_with(w.as_str()) {
                    score += 30;
                } else if name.split(|c: char| !c.is_alphanumeric()).any(|p| p.starts_with(w.as_str())) {
                    score += 20;
                } else if name.contains(w.as_str()) {
                    score += 10;
                } else if desc.contains(w.as_str()) {
                    score += 3;
                } else {
                    return None;
                }
            }
            Some((score, c))
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    scored.into_iter().map(|(_, c)| c).collect()
}

fn search_box(ui: &mut egui::Ui, st: &mut AppState, rect: Rect) {
    let id = egui::Id::unique("command-search");
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let edit = egui::TextEdit::singleline(&mut st.ribbon.search)
        .id(id)
        .hint_text(format!(
            "{}  Search commands ({})",
            icons::SEARCH,
            shortcut_text(ui.ctx(), Cmd::Search).unwrap_or_default()
        ))
        .desired_width(rect.width());
    let r = child.add(edit);
    if std::mem::take(&mut st.ribbon.focus_search) {
        r.request_focus();
    }
    let results = search(&st.ribbon.search);
    if r.changed() {
        st.ribbon.search_selected = 0;
    }
    if !r.has_focus() && !r.lost_focus() {
        return;
    }
    let (up, down, enter, esc) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if esc {
        st.ribbon.search.clear();
        r.surrender_focus();
        return;
    }
    let n = results.len().min(10);
    if n > 0 {
        if down {
            st.ribbon.search_selected = (st.ribbon.search_selected + 1) % n;
        }
        if up {
            st.ribbon.search_selected = (st.ribbon.search_selected + n - 1) % n;
        }
    }
    let sel = st.ribbon.search_selected.min(n.saturating_sub(1));
    if enter && r.lost_focus() {
        if let Some(&cmd) = results.get(sel) {
            st.queue.push(cmd);
        }
        st.ribbon.search.clear();
        return;
    }
    if st.ribbon.search.trim().is_empty() {
        return;
    }
    let mut chosen = None;
    egui::Area::new(id.with("results"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.left_bottom() + Vec2::new(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(rect.width().max(320.0));
                if n == 0 {
                    ui.label(RichText::new("No command matches.").color(st.palette.weak_text));
                }
                for (i, &cmd) in results.iter().take(n).enumerate() {
                    let info = cmd.info();
                    let avail = availability(st, cmd);
                    let row = ui.horizontal(|ui| {
                        ui.label(icon(info.icon, 16.0));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(info.name).strong());
                                if let Some(s) = shortcut_text(ui.ctx(), cmd) {
                                    ui.label(RichText::new(s).small().color(st.palette.weak_text));
                                }
                            });
                            match &avail {
                                Ok(()) => ui.label(RichText::new(info.description).small().color(st.palette.weak_text)),
                                Err(why) => ui.label(RichText::new(why).small().color(st.palette.changed)),
                            };
                        });
                    });
                    let r = ui.interact(row.response.rect, id.with(("result", i)), Sense::click());
                    if i == sel || r.hovered() {
                        ui.painter().rect_filled(
                            row.response.rect.expand(2.0),
                            4.0,
                            st.palette.accent.gamma_multiply(0.2),
                        );
                    }
                    if r.clicked() {
                        chosen = Some(cmd);
                    }
                }
            });
        });
    if let Some(cmd) = chosen {
        st.queue.push(cmd);
        st.ribbon.search.clear();
        r.surrender_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keytips_are_unique_per_tab() {
        let tabs = tab_keytips();
        let mut t = tabs.clone();
        t.sort_unstable();
        t.dedup();
        assert_eq!(t.len(), tabs.len());
        for (i, tab) in RIBBON.iter().enumerate() {
            let keys: Vec<char> = button_keytips(i).into_iter().map(|(_, k)| k).collect();
            let mut k = keys.clone();
            k.sort_unstable();
            k.dedup();
            assert_eq!(k.len(), keys.len(), "tab {}", tab.name);
            assert!(!keys.contains(&'?'));
        }
    }

    #[test]
    fn search_ranks_names_above_descriptions() {
        let r = search("zoom");
        assert!(r.starts_with(&[Cmd::ZoomIn, Cmd::ZoomOut]) || r.starts_with(&[Cmd::ZoomOut, Cmd::ZoomIn]), "{r:?}");
        assert!(search("merge").contains(&Cmd::MergeDown));
        assert_eq!(search("layer new").first(), Some(&Cmd::NewLayer));
        assert!(search("qqqq").is_empty());
        assert!(search("   ").is_empty());
    }

    #[test]
    fn long_labels_break_near_the_middle() {
        assert_eq!(split_label("Fit to Window"), vec!["Fit to", "Window"]);
        assert_eq!(split_label("Open"), vec!["Open"]);
        assert_eq!(split_label("New Layer"), vec!["New", "Layer"]);
    }
}
