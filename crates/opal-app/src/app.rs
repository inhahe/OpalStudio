//! The application: the window's layout, keyboard handling, and the panel tree.

use crate::canvas;
use crate::commands::Cmd;
use crate::config;
use crate::dialogs;
use crate::doc::DocId;
use crate::gpu::CanvasRenderer;
use crate::gpu_memory::GpuMemory;
use crate::help;
use crate::icons;
use crate::panels;
use crate::ribbon::{self, button_keytips, tab_keytips};
use crate::state::{AppState, Confirm, Keytips, ScreenMode};
use crate::theme;
use crate::tools::ToolId;
use eframe::egui::{self, Event, Key, Modifiers, RichText, ViewportCommand};
use egui_tiles::{SimplificationOptions, TileId, Tiles, UiResponse};
use opal_io::bytes;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What a tile of the panel tree shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// Shown where the images go while none is open.
    Welcome,
    Document(DocId),
    Layers,
    /// The steps that make the active image's selection (`selection_panel`).
    Selection,
    History,
    ToolSettings,
    /// What scripts run from the editor print.
    Console,
}

pub struct OpalApp {
    st: AppState,
    tree: egui_tiles::Tree<Pane>,
    gpu_memory: Option<GpuMemory>,
    last_gpu_poll: Option<Instant>,
    /// Whether Alt was down last frame, and whether it has been pressed alone (for keytips).
    alt_down: bool,
    alt_alone: bool,
    /// Whether a field had the keyboard at the end of the last frame: the keys pressed since
    /// are its, though egui lets go of the focus on Esc before the frame begins.
    field_had_keys: bool,
    /// Whether the window has been put where windows go on its monitor ([`fit_to_monitor`]):
    /// once, at the start, as soon as where it is can be told.
    fitted_to_monitor: bool,
    /// Where windows go on the monitor at a point ([`work_area`]; another in tests).
    work_area: fn(egui::Pos2) -> Option<egui::Rect>,
    /// What the status bar is to show of what other threads change, in place of reading it,
    /// in tests ([`OpalApp::pretend_status`]).
    #[cfg(test)]
    status_given: Option<StatusSeen>,
    /// Whether settings are saved (not in tests).
    persist: bool,
    /// The API's connection for scripts and assistants (not in tests); `None` if it could
    /// not be opened.
    server: Option<crate::server::Server>,
    /// Whether another editor served scripts when this one started: this one takes over
    /// once that one has gone (see [`OpalApp::take_over_serving`]).
    server_waits: bool,
    /// When this one last looked whether it could, and last asked the other one.
    server_looked: (std::time::Instant, std::time::Instant),
    /// MCP over HTTP, while the preference has it served (and this editor serves scripts).
    mcp_http: Option<crate::mcp_http::HttpServer>,
    /// The port it could not be served on, not tried again until the preferences change.
    mcp_http_failed: Option<u16>,
    /// Whether the window was last asked to be full screen (see `AppState::screen`), and
    /// whether it has been seen so since.
    full_screen: bool,
    full_screen_seen: bool,
}

impl OpalApp {
    /// Sets up the app; `files` are opened at once (from the command line).
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> OpalApp {
        OpalApp::create(cc, files, true)
    }

    /// The app as tests use it: the user's saved settings are neither read nor written.
    #[cfg(test)]
    pub fn for_tests(cc: &eframe::CreationContext<'_>) -> OpalApp {
        OpalApp::create(cc, Vec::new(), false)
    }

    fn create(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>, persist: bool) -> OpalApp {
        theme::install_fonts(&cc.egui_ctx);
        crate::paste_keys::wake(&cc.egui_ctx);
        // Ctrl+Plus and Ctrl+Minus zoom the canvas; the interface's size is a preference.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let mut st = AppState::new();
        st.repaint = cc.egui_ctx.clone();
        if persist {
            config::load(&mut st);
        }
        st.apply_prefs(&cc.egui_ctx);
        crate::number_field::load_entries(&cc.egui_ctx, st.field_entries.clone());
        // Before any file is opened, so that its tiles may go to disk.
        st.apply_tile_budget();
        let mut gpu_memory = None;
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            CanvasRenderer::install(rs);
            let info = rs.adapter.get_info();
            st.gpu.adapter = info.name.clone();
            st.gpu.backend = format!("{:?}", info.backend);
            gpu_memory = Some(GpuMemory::new(info.vendor, info.device));
        }
        for f in files {
            st.open_path(&f);
        }
        let tree = default_tree(&st);
        st.opened.clear();
        let (server, server_waits) = if persist { start_server(&mut st, &cc.egui_ctx) } else { (None, false) };
        OpalApp {
            st,
            tree,
            gpu_memory,
            last_gpu_poll: None,
            alt_down: false,
            field_had_keys: false,
            fitted_to_monitor: false,
            full_screen: false,
            full_screen_seen: false,
            #[cfg(test)]
            status_given: None,
            work_area,
            alt_alone: false,
            persist,
            server,
            server_waits,
            server_looked: (std::time::Instant::now(), std::time::Instant::now()),
            mcp_http: None,
            mcp_http_failed: None,
        }
    }

    /// The state, for tests.
    #[cfg(test)]
    pub fn state(&self) -> &AppState {
        &self.st
    }

    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.st
    }

    /// The documents whose tabs are showing, for tests.
    #[cfg(test)]
    pub fn shown_docs(&self) -> Vec<DocId> {
        shown_docs(&self.tree.tiles)
    }

    /// Sets the share of the root container's first child (the documents), for tests;
    /// returns it.
    #[cfg(test)]
    pub fn set_root_share(&mut self, share: f32) -> f32 {
        let root = self.tree.root().expect("the tree has a root");
        let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(l))) = self.tree.tiles.get_mut(root) else {
            panic!("the root is a row")
        };
        let first = l.children[0];
        l.shares.set_share(first, share);
        share
    }

    /// The share of the root container's first child, for tests.
    #[cfg(test)]
    pub fn root_share(&self) -> f32 {
        let root = self.tree.root().expect("the tree has a root");
        let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(l))) = self.tree.tiles.get(root) else {
            panic!("the root is a row")
        };
        l.shares[l.children[0]]
    }

    /// Gives the tabs holding `pane` the share `share` of the column or row they are in, for
    /// tests.
    #[cfg(test)]
    pub fn set_pane_share(&mut self, pane: Pane, share: f32) {
        let tiles = &mut self.tree.tiles;
        let tabs = tiles.find_pane(&pane).and_then(|p| tiles.parent_of(p)).expect("the pane is in tabs");
        let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(l))) =
            tiles.parent_of(tabs).and_then(|c| tiles.get_mut(c))
        else {
            panic!("its tabs are in a column or row")
        };
        l.shares.set_share(tabs, share);
    }

    /// Where `pane`'s contents showed in the last frame (below its tab, within its inset), for
    /// tests.
    #[cfg(test)]
    pub fn pane_rect(&self, pane: Pane) -> egui::Rect {
        let tile = self.tree.tiles.find_pane(&pane).expect("the pane is in the tree");
        self.tree.tiles.rect(tile).expect("the pane was shown").shrink(f32::from(PANE_INSET))
    }

    /// Makes the status bar show `scripts` connected (none in a step, none shut out) and
    /// `on_disk` bytes of tiles on disk, whatever the server and the tile store say, for tests.
    #[cfg(test)]
    pub fn pretend_status(&mut self, scripts: usize, on_disk: u64) {
        let tiles = opal_core::store::Status { on_disk, ..Default::default() };
        self.status_given = Some(StatusSeen { clients: scripts, steps: Vec::new(), shut_out: false, tiles });
    }

    /// Makes the GPU memory shown `memory` (used and budget, in bytes), no longer polled, for
    /// tests.
    #[cfg(test)]
    pub fn pin_gpu_memory(&mut self, memory: (u64, u64)) {
        self.st.gpu.memory = Some(memory);
        self.gpu_memory = None;
    }

    /// Makes `f` say where windows go on the monitor at a point, for tests.
    #[cfg(test)]
    pub fn set_work_area(&mut self, f: fn(egui::Pos2) -> Option<egui::Rect>) {
        self.work_area = f;
    }

    /// Whether the start page is in the panel tree, for tests.
    #[cfg(test)]
    pub fn shows_welcome(&self) -> bool {
        self.tree.tiles.find_pane(&Pane::Welcome).is_some()
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Taken every frame, so a press made while a dialog is open or a field has the
        // keyboard does not paste later.
        let paste_presses = crate::paste_keys::take();
        // While a dialog is open, keys belong to it: no command may change what is behind
        // it, or the dialog's own fields.
        if self.st.dialog_open() {
            self.alt_down = false;
            self.alt_alone = false;
            return;
        }
        // Esc typed into a field leaves the field, not the box the field is about.
        let typing = ctx.egui_wants_keyboard_input() || self.field_had_keys;
        // While a mouse button is held on an image, or a drag on it is under way (one may
        // outlive the button, Space making the tool the hand), keys that would change it do
        // nothing (as in Photoshop): a drag works its edit out again from where it began, and
        // would carry on past what they did. Esc still gives up what it gives up (a paste being
        // read first, as ever), and the view's keys act.
        let held = self.st.canvas_held && ctx.input(|i| i.pointer.any_down());
        if !typing && (held || self.st.active().is_some_and(crate::doc::DocState::dragging)) {
            if keys_held_back(ctx) || paste_presses > 0 {
                self.st.notify(KEYS_WAIT);
            }
            self.cancel_paste_key(ctx);
            self.editing_keys(ctx);
            // Alt held with the button is not alone (and is down, when let go of after it).
            (self.alt_down, self.alt_alone) = (ctx.input(|i| i.modifiers.alt), false);
            return self.view_keys(ctx);
        }
        if !typing {
            self.clipboard_events(ctx, paste_presses);
            self.cancel_paste_key(ctx);
            self.editing_keys(ctx);
        }
        // Keytips: Alt pressed and released on its own toggles them. Held with a button down
        // (pressed in the middle of a drag, to draw from the middle, say), it is not alone.
        let (alt, other_input) = ctx.input(|i| {
            let other = i.pointer.any_down()
                || i.events.iter().any(|e| {
                    matches!(
                        e,
                        Event::Key { pressed: true, .. } | Event::PointerButton { .. } | Event::MouseWheel { .. }
                    )
                });
            (i.modifiers.alt, other)
        });
        if alt && !self.alt_down {
            self.alt_alone = true;
        }
        if alt && other_input {
            self.alt_alone = false;
        }
        if !alt && self.alt_down && self.alt_alone && !typing {
            self.st.ribbon.keytips = if self.st.ribbon.keytips == Keytips::Off { Keytips::Tabs } else { Keytips::Off };
        }
        self.alt_down = alt;
        if self.st.ribbon.keytips != Keytips::Off {
            self.keytip_keys(ctx);
            return;
        }
        if typing {
            return;
        }
        for &(s, cmd) in shortcuts_most_specific_first() {
            if ctx.input_mut(|i| i.consume_shortcut(&s)) {
                self.st.queue.push(cmd);
            }
        }
        for t in ToolId::ALL {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, t.info().key)) {
                self.st.set_tool(t);
            }
        }
    }

    /// The view's commands' shortcuts (zooming, screen modes, what the canvas shows), which
    /// act even while a mouse button is held on an image.
    fn view_keys(&mut self, ctx: &egui::Context) {
        for &(s, cmd) in shortcuts_most_specific_first().iter().filter(|(_, c)| acts_during_drag(*c)) {
            if ctx.input_mut(|i| i.consume_shortcut(&s)) {
                self.st.queue.push(cmd);
            }
        }
    }

    /// Copy and Paste arrive as events of their own rather than as keys (the windowing layer
    /// turns Ctrl+C, Ctrl+V and their alternatives into them): outside text fields, they
    /// are the image commands.
    ///
    /// Where the paste keys are counted by `paste_keys` (Windows), egui's paste events are
    /// dropped and `presses` pastes instead, since egui reports no event for some clipboards.
    fn clipboard_events(&mut self, ctx: &egui::Context, presses: u32) {
        let counted = crate::paste_keys::counted();
        let mut events = ctx.input_mut(|i| {
            let mut out = Vec::new();
            i.events.retain(|e| match e {
                Event::Copy => {
                    out.push(Cmd::Copy);
                    false
                }
                Event::Cut => {
                    out.push(Cmd::Cut);
                    false
                }
                Event::Paste(_) | Event::PasteImage(_) => {
                    if !counted {
                        out.push(Cmd::Paste);
                    }
                    false
                }
                _ => true,
            });
            out
        });
        events.extend(std::iter::repeat_n(Cmd::Paste, presses as usize));
        for cmd in events {
            // Copying with nothing open does nothing rather than explaining itself.
            if cmd == Cmd::Copy && self.st.active().is_none() {
                continue;
            }
            self.st.queue.push(cmd);
        }
    }

    /// Esc cancels a paste still being read, or waiting to be placed, and files being read
    /// (see `opening`), unless a box on the image shown takes it first.
    fn cancel_paste_key(&mut self, ctx: &egui::Context) {
        if self.st.reading.is_none() && !self.st.reading_files() {
            return;
        }
        if !self.st.box_takes_escape() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.st.reading.is_some() {
                self.st.cancel_paste();
            }
            if self.st.reading_files() {
                self.st.cancel_files();
            }
        }
    }

    /// Enter, Esc and the arrow keys: applying or cancelling the transform box, and moving
    /// it or the selected layers by a pixel (or, with Shift, by the nudge distance).
    fn editing_keys(&mut self, ctx: &egui::Context) {
        let far = self.st.prefs.f64("nav.nudge").round() as i64;
        let crop_tool = self.st.tool() == ToolId::Crop;
        let crop_delete = self.st.tool_settings(ToolId::Crop).bool("crop.delete");
        let sampling = crate::prefs::sampling(&self.st.prefs, "transform");
        let outline_smooth = self.st.tool_settings(ToolId::Outline).bool("outline.smooth");
        let outline_tool = self.st.tool() == ToolId::Outline;
        let Some(d) = self.st.active_mut() else { return };
        // While an edit is under way on the image, they change nothing (see `edits`).
        if d.busy.is_some() {
            return;
        }
        // Esc gives up a Move drag, the layers back where they were (the tools' own drags see
        // to their Esc themselves).
        if matches!(d.drag, Some(crate::doc::Drag::Move(_)))
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            return d.give_up_drag();
        }
        let cropping = crop_tool && d.crop.is_some();
        // An outline being drawn takes Enter (closing it), Esc (giving it up) and Backspace
        // (taking its last point back), before any command does.
        if d.outlining.is_some() {
            let smooth = outline_smooth;
            let (enter, escape, back) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Backspace),
                )
            });
            if enter {
                crate::outline_tool::close(d, smooth);
            } else if escape {
                d.outlining = None;
                d.revision += 1;
            } else if back {
                let left = d.outlining.as_mut().is_some_and(|o| o.take_back());
                if !left {
                    d.outlining = None;
                }
                d.revision += 1;
            }
            return;
        }
        // Points picked on the outline edited take Delete and Backspace (taking them away) and
        // Esc (letting go of them), before any command does; not while a box is open (whose
        // keys they are) or a drag on the outline is under way (whose Esc gives it up).
        if outline_tool && d.transform.is_none() && d.drag.is_none() && !crate::outline_tool::picked(d).is_empty() {
            let (delete, escape) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if delete {
                return crate::outline_tool::delete_picked(d);
            }
            if escape {
                d.picked = None;
                d.revision += 1;
                return;
            }
        }
        let boxed = d.transform.is_some() || cropping;
        let mut refused = None;
        let (enter, escape, arrows, held) = ctx.input_mut(|i| {
            let step = if i.modifiers.shift { far } else { 1 };
            let mut d = [0i64, 0];
            for (key, dx, dy) in
                [(Key::ArrowLeft, -1, 0), (Key::ArrowRight, 1, 0), (Key::ArrowUp, 0, -1), (Key::ArrowDown, 0, 1)]
            {
                let n = i.count_and_consume_key(Modifiers::SHIFT, key) + i.count_and_consume_key(Modifiers::NONE, key);
                d[0] += dx * step * n as i64;
                d[1] += dy * step * n as i64;
            }
            let held = [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown].iter().any(|k| i.key_down(*k));
            // Enter and Esc belong to the box only while there is one.
            let enter = boxed && i.consume_key(Modifiers::NONE, Key::Enter);
            let escape = boxed && i.consume_key(Modifiers::NONE, Key::Escape);
            (enter, escape, d, held)
        });
        if cropping && d.transform.is_none() {
            if enter {
                let b = d.crop.expect("cropping");
                if b.canvas() != d.doc().canvas || b.angle != 0.0 || crop_delete {
                    let id = d.id;
                    refused = self.st.crop_with(id, b, crop_delete, sampling).err();
                }
            } else if escape {
                d.crop = None;
                d.revision += 1;
            }
            if let Some(why) = refused {
                self.st.message(crate::state::MessageKind::Info, "Crop", why);
            }
            return;
        }
        if d.transform.is_some() {
            if enter {
                let id = d.id;
                self.st.commit_box(id);
            } else if escape {
                d.cancel_transform();
            } else if arrows != [0, 0]
                && let Some(s) = d.transform.as_ref()
            {
                let m = s.current.then(opal_core::geom::Affine::translate(arrows[0] as f64, arrows[1] as f64));
                d.set_box(m, None);
            }
            return;
        }
        if arrows != [0, 0] && !d.selected_layers().is_empty() {
            match crate::move_tool::locked_in_selected_layers(d) {
                Some(name) => self.st.notify(format!("\"{name}\" is locked, so it cannot be moved. Unlock it first.")),
                None => d.nudge_selected_layers(arrows),
            }
        } else if !held && d.pending_label() == Some("Nudge") {
            d.commit_pending();
        }
    }

    /// While keytips are shown, letters pick tabs and commands instead of doing what they
    /// usually do.
    fn keytip_keys(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.any_pressed());
        if pointer {
            self.st.ribbon.keytips = Keytips::Off;
            return;
        }
        let keys: Vec<Key> = ctx.input_mut(|i| {
            let mut keys = Vec::new();
            i.events.retain(|e| match e {
                Event::Key { key, pressed: true, .. } => {
                    keys.push(*key);
                    false
                }
                Event::Text(_) | Event::Key { .. } => false,
                _ => true,
            });
            keys
        });
        for key in keys {
            if key == Key::Escape {
                self.st.ribbon.keytips = match self.st.ribbon.keytips {
                    Keytips::Buttons(_) => Keytips::Tabs,
                    _ => Keytips::Off,
                };
                continue;
            }
            let Some(c) = key.name().chars().next().filter(|_| key.name().len() == 1) else {
                self.st.ribbon.keytips = Keytips::Off;
                continue;
            };
            match self.st.ribbon.keytips {
                Keytips::Tabs => {
                    if let Some(t) = tab_keytips().iter().position(|&k| k == c) {
                        self.st.ribbon.tab = t;
                        self.st.ribbon.keytips = Keytips::Buttons(t);
                    } else {
                        self.st.ribbon.keytips = Keytips::Off;
                    }
                }
                Keytips::Buttons(t) => {
                    if let Some((cmd, _)) = button_keytips(t).iter().copied().find(|&(_, k)| k == c) {
                        self.st.queue.push(cmd);
                    }
                    self.st.ribbon.keytips = Keytips::Off;
                }
                Keytips::Off => {}
            }
        }
    }

    /// Files dropped on the window open as documents.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        self.st.dropped.extend(dropped);
        // Those waiting open in the frame after the dialog closes, which closing it brings
        // (the dialogs are part of `signature`).
        self.st.open_dropped(ctx);
    }

    /// Closing the window, or Exit, asks about unsaved changes first.
    fn handle_exit(&mut self, ctx: &egui::Context) {
        let window_close = ctx.input(|i| i.viewport().close_requested());
        let wants = window_close || std::mem::take(&mut self.st.exit_requested);
        if !wants || self.st.exit_confirmed {
            return;
        }
        // A question about closing an image is answered first (the quit asks after it); one
        // about quitting is being asked already.
        if let Some(c) = &self.st.confirm {
            if window_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            if matches!(c, Confirm::Close(_)) {
                self.st.exit_requested = true;
            }
            return;
        }
        // An edit under way is finished first, then the image's question asked.
        if self.st.editing() {
            if window_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            self.st.exit_after_work = true;
            self.st.notify("Quitting once the edit under way is done (Cancel in the status bar gives it up).");
            return;
        }
        // Files still being written are finished first.
        if self.st.writer.busy() {
            if window_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            self.st.exit_after_work = true;
            self.st.notify("Quitting once the files being written are done.");
            return;
        }
        let dirty = self.st.dirty_docs();
        if dirty.is_empty() {
            self.st.exit_confirmed = true;
            if !window_close {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        } else {
            if window_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            self.st.confirm = Some(Confirm::Exit(dirty));
        }
    }

    fn poll_gpu(&mut self) {
        let due = self.last_gpu_poll.is_none_or(|t| t.elapsed() > Duration::from_secs(1));
        if due && let Some(m) = &self.gpu_memory {
            self.st.gpu.memory = m.query();
            self.last_gpu_poll = Some(Instant::now());
        }
    }

    fn save_settings_if_due(&mut self, ctx: &egui::Context) {
        let Some(changed) = self.st.settings_changed.filter(|_| self.persist) else { return };
        let wait = Duration::from_secs(1);
        if changed.elapsed() < wait {
            // Come back when it is due, even if nothing else happens meanwhile.
            ctx.request_repaint_after(wait - changed.elapsed());
            return;
        }
        self.st.settings_changed = None;
        if let Err(e) = config::save(&self.st) {
            self.st.message(crate::state::MessageKind::Warning, "Settings not saved", e.to_string());
        }
    }

    /// Adds tabs for newly opened documents and shows the start page when none is open.
    fn sync_panes(&mut self, ctx: &egui::Context) {
        // A closed document's pane is still in the tree until the next frame drops it (see
        // `retain_pane`), so the documents' container can still be found through it now.
        let anchor = find_docs_container(&self.tree.tiles);
        let tiles = &mut self.tree.tiles;
        for id in std::mem::take(&mut self.st.opened) {
            let pane = tiles.insert_pane(Pane::Document(id));
            match anchor.and_then(|a| tiles.get_mut(a)) {
                Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) => tabs.add_child(pane),
                _ => {
                    self.tree = default_tree(&self.st);
                    return;
                }
            }
        }
        // The start page is shown exactly when no image is open.
        let welcome = tiles.find_pane(&Pane::Welcome);
        match (self.st.docs.is_empty(), welcome) {
            (false, Some(w)) => {
                if let Some(parent) = tiles.parent_of(w)
                    && let Some(egui_tiles::Tile::Container(c)) = tiles.get_mut(parent)
                {
                    c.remove_child(w);
                }
                tiles.remove(w);
            }
            (true, None) => {
                let pane = tiles.insert_pane(Pane::Welcome);
                match anchor.and_then(|a| tiles.get_mut(a)) {
                    Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) => {
                        tabs.add_child(pane);
                        tabs.set_active(pane);
                    }
                    _ => self.tree = default_tree(&self.st),
                }
            }
            _ => {}
        }
        // The active document is the one on screen: a document a command made active has
        // its tab brought to the front; otherwise, if the tab shown changed some other way,
        // the active document follows it.
        // The panel tree is drawn already this frame: a tab brought to the front shows in
        // the next, which is asked for.
        if std::mem::take(&mut self.st.show_console) {
            show_pane(&mut self.tree.tiles, Pane::Console);
            ctx.request_repaint();
        }
        let reveal = std::mem::take(&mut self.st.reveal);
        let shown = shown_docs(&self.tree.tiles);
        if let Some(id) = reveal.filter(|id| self.st.doc(*id).is_some()) {
            show_pane(&mut self.tree.tiles, Pane::Document(id));
            ctx.request_repaint();
        } else if self.st.active_doc.is_none_or(|a| !shown.contains(&a))
            && let Some(&first) = shown.iter().find(|id| self.st.doc(**id).is_some())
        {
            self.st.active_doc = Some(first);
        }
    }

    /// A digest of everything the window shows that can change without input: the
    /// documents and their views, the settings, the tool, the open windows.
    pub(crate) fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let st = &self.st;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for d in &st.docs {
            d.id.hash(&mut h);
            d.history.current().hash(&mut h);
            d.pending.is_some().hash(&mut h);
            for l in &d.doc().layers {
                (std::sync::Arc::as_ptr(l) as usize).hash(&mut h);
            }
            d.active_layer().hash(&mut h);
            d.selected_layers().hash(&mut h);
            d.expanded.len().hash(&mut h);
            d.view.zoom.to_bits().hash(&mut h);
            d.view.offset.x.to_bits().hash(&mut h);
            d.view.offset.y.to_bits().hash(&mut h);
            d.title.hash(&mut h);
            d.saved_at.hash(&mut h);
            d.revision.hash(&mut h);
        }
        st.active_doc.hash(&mut h);
        // A position reported this frame is shown in the next one.
        st.cursor_canvas.hash(&mut h);
        st.prefs.revision().hash(&mut h);
        for t in ToolId::ALL {
            st.tool_settings(t).revision().hash(&mut h);
        }
        st.tool().hash(&mut h);
        (st.ribbon.tab, &st.ribbon.keytips, st.renaming.is_some(), st.messages.len(), st.confirm.is_some())
            .hash(&mut h);
        (st.new_image.is_some(), st.export.is_some(), st.prefs_open, st.about_open, st.gallery_open, &st.help)
            .hash(&mut h);
        (st.assistant_open, &st.mcp_http).hash(&mut h);
        (st.resize_image.is_some(), st.resize_canvas.is_some(), st.rotate_by.is_some(), st.modifying.is_some())
            .hash(&mut h);
        st.reading.is_some().hash(&mut h);
        st.writer.busy().hash(&mut h);
        st.notice.as_ref().map(|n| &n.0).hash(&mut h);
        h.finish()
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let weak = self.st.palette.weak_text;
        // What the canvas does: the transform box's, while one is open, takes its drags.
        let chosen = self.st.tool();
        let (glyph, does) = match self.st.active().and_then(|d| d.transform.as_ref()) {
            Some(_) => (crate::icons::TRANSFORM, crate::transform_ui::HINT),
            None => (chosen.icon_with(self.st.tool_settings(chosen)), chosen.info().description),
        };
        // A recent notice, for a few seconds, instead of what the canvas does.
        const SHOWN: std::time::Duration = crate::state::NOTICE_SHOWN;
        let said = match &self.st.notice {
            Some((text, at)) if at.elapsed() < SHOWN => {
                ui.ctx().request_repaint_after(SHOWN.saturating_sub(at.elapsed()));
                RichText::new(format!("{}  {text}", crate::icons::INFO)).color(self.st.palette.strong_text)
            }
            _ => RichText::new(format!("{glyph}  {does}")).color(weak),
        };
        // What other threads change, read once, so that the parts are chosen and drawn by it.
        let seen = self.status_seen();
        // The parts at the bar's right, as wide as they were when last shown: the least needed
        // left out until the rest leave the text room enough.
        let widths_id = egui::Id::unique("status-bar-widths");
        let mut widths: StatusWidths = ui.data(|d| d.get_temp(widths_id)).unwrap_or_default();
        let room = ui.available_width() - ui.spacing().item_spacing.x - STATUS_TEXT_ROOM;
        let mut shown = self.status_parts(&seen);
        for least in StatusPart::LEAST_NEEDED_FIRST {
            if widths.of(&shown) <= room {
                break;
            }
            shown.retain(|p| *p != least);
        }
        let parts_wide = widths.of(&shown);
        ui.horizontal(|ui| {
            // The text first, so that it is read out first, in the room the parts leave: cut
            // short where it does not fit (whole on hover), rather than run under them.
            let gap = ui.spacing().item_spacing.x;
            let room = egui::vec2((ui.available_width() - gap - parts_wide).max(0.0), ui.available_height());
            ui.allocate_ui_with_layout(room, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(said).truncate());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (i, &part) in shown.iter().enumerate() {
                    if i > 0 {
                        widths.between = ui.separator().rect.width() + 2.0 * gap;
                    }
                    let r = ui.scope(|ui| self.status_part(ui, part, &seen));
                    widths.parts.insert(part, r.response.rect.width());
                }
            });
        });
        // Grown past the room they were given (a part shown the first time, or longer): laid
        // out again at once, so as not to run over the text. Narrower, the next frame will do.
        if widths.of(&shown) > parts_wide + 0.5 {
            ui.ctx().request_discard("the status bar's parts grew");
        }
        ui.data_mut(|d| d.insert_temp(widths_id, widths));
    }

    /// What the status bar shows of what other threads change.
    fn status_seen(&self) -> StatusSeen {
        #[cfg(test)]
        if let Some(given) = &self.status_given {
            return given.clone();
        }
        let (clients, steps, shut_out) = match &self.server {
            Some(s) => (s.clients(), s.open_steps(), s.shut_out()),
            None => (0, Vec::new(), false),
        };
        StatusSeen { clients, steps, shut_out, tiles: opal_core::store::status() }
    }

    /// With the image alone, which has no status bar, what the status bar would show, over the
    /// image's foot: a recent notice, and an edit under way, with how far it has got and Cancel.
    fn alone_status(&mut self, ctx: &egui::Context) {
        const SHOWN: std::time::Duration = crate::state::NOTICE_SHOWN;
        let notice = self.st.notice.clone().filter(|(_, at)| at.elapsed() < SHOWN);
        let edit = self.shown_edit().is_some();
        if notice.is_none() && !edit {
            return;
        }
        let seen = self.status_seen();
        egui::Area::new(egui::Id::unique("image-alone-status")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -16.0]).show(
            ctx,
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if let Some((text, at)) = &notice {
                            ui.ctx().request_repaint_after(SHOWN.saturating_sub(at.elapsed()));
                            ui.label(format!("{}  {text}", icons::INFO));
                        }
                        if edit {
                            self.status_part(ui, StatusPart::Edit, &seen);
                        }
                    });
                });
            },
        );
    }

    /// The window as usual, round the panels: the ribbon, the status bar, the tools' strip if
    /// they are at a side, and the panels.
    fn frame_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("ribbon").frame(egui::Frame::NONE).resizable(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            ribbon::ribbon_ui(ui, &mut self.st);
        });
        egui::Panel::bottom("status").resizable(false).show(ui, |ui| self.status_bar(ui));
        let strip = self.st.prefs.choice("ui.tools");
        if strip != "ribbon" {
            let panel =
                if strip == "left" { egui::Panel::left("tools-strip") } else { egui::Panel::right("tools-strip") };
            panel.exact_size(44.0).resizable(false).show(ui, |ui| {
                let mut y = ui.max_rect().top() + 6.0;
                for t in ToolId::ALL {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(ui.max_rect().center().x - 16.0, y),
                        egui::Vec2::splat(32.0),
                    );
                    ribbon::tool_button(ui, &mut self.st, t, r);
                    y += 36.0;
                }
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(ui.visuals().panel_fill)).show(ui, |ui| {
            let mut behavior = Behavior { st: &mut self.st };
            self.tree.ui(&mut behavior, ui);
        });
    }

    /// The image whose edit the status bar shows under way, if one is (see `edits`): the active
    /// image's, else the first busy image's, once it has run long enough to be shown.
    fn shown_edit(&self) -> Option<DocId> {
        let shown = |d: &&crate::doc::DocState| d.busy.as_ref().is_some_and(crate::edits::Busy::shown);
        self.st.active().filter(shown).or_else(|| self.st.docs.iter().find(shown)).map(|d| d.id)
    }

    /// The parts of the status bar there is something for, from its right end, by `seen`.
    fn status_parts(&self, seen: &StatusSeen) -> Vec<StatusPart> {
        let mut parts = Vec::new();
        if seen.clients > 0 || seen.shut_out {
            parts.push(StatusPart::Scripts);
        }
        if self.st.writer.pending().next().is_some() {
            parts.push(StatusPart::Write);
        }
        if self.shown_edit().is_some() {
            parts.push(StatusPart::Edit);
        }
        if seen.tiles.on_disk > 0 {
            parts.push(StatusPart::Disk);
        }
        if self.st.gpu.memory.is_some() {
            parts.push(StatusPart::Gpu);
        }
        if self.st.active().is_some() {
            parts.extend([StatusPart::Zoom, StatusPart::Size, StatusPart::Resolution]);
            if self.st.cursor_shown.is_some() {
                parts.push(StatusPart::Pointer);
            }
        }
        parts
    }

    /// The part `part` of the status bar (one [`OpalApp::status_parts`] gives for `seen`).
    fn status_part(&mut self, ui: &mut egui::Ui, part: StatusPart, seen: &StatusSeen) {
        let weak = self.st.palette.weak_text;
        // A text that could be long, cut short (whole on hover).
        let capped = |ui: &mut egui::Ui, text: RichText| {
            ui.scope(|ui| {
                ui.set_max_width(STATUS_PART_MAX);
                ui.add(egui::Label::new(text).truncate());
            });
        };
        match part {
            // Scripts and assistants connected, and a way to send them off; once sent off, a
            // way to let them in again. Drawn by `seen` alone (the server only told a click).
            StatusPart::Scripts => {
                let n = seen.clients;
                if n > 0 {
                    if ui
                        .small_button("Disconnect")
                        .on_hover_text(
                            "Drop every connected script and assistant (each is cut off at its next \
                             request), and keep them out until you let them in again.",
                        )
                        .clicked()
                        && let Some(server) = &self.server
                    {
                        server.disconnect_all();
                    }
                    let mut who =
                        if n == 1 { "A script is connected".to_owned() } else { format!("{n} scripts are connected") };
                    // A step open is told, so that one a script or assistant forgot is seen.
                    if !seen.steps.is_empty() {
                        let names: Vec<String> = seen.steps.iter().map(|s| format!("\u{201c}{s}\u{201d}")).collect();
                        who += &format!(", in step {}", names.join(", "));
                    }
                    let strong = self.st.palette.strong_text;
                    capped(ui, RichText::new(format!("{}  {who}", crate::icons::INFO)).color(strong));
                } else {
                    if ui.small_button("Let in").on_hover_text("Let scripts and assistants connect again.").clicked()
                        && let Some(server) = &self.server
                    {
                        server.let_in();
                    }
                    ui.label(RichText::new(format!("{}  Scripts are disconnected", crate::icons::INFO)).color(weak));
                }
            }
            // The edit shown under way (see `edits`): how far it has got, and a way to stop it.
            StatusPart::Edit => {
                let Some(id) = self.shown_edit() else { return };
                let d = self.st.doc(id).expect("found");
                let b = d.busy.as_ref().expect("found");
                let label = b.label.clone();
                // Another image's says which.
                let what =
                    if Some(id) == self.st.active_doc { label.clone() } else { format!("{label} in {}", d.title) };
                let fraction = b.progress.fraction();
                let tip = format!("Stop {}; the image stays as it was.", label.to_lowercase());
                if ui.small_button("Cancel").on_hover_text(tip).clicked() {
                    self.st.cancel_edit(id);
                    return;
                }
                match fraction {
                    Some(f) => {
                        let pct = format!("{}%", (f * 100.0).floor());
                        ui.add(egui::ProgressBar::new(f).desired_width(90.0).desired_height(14.0).text(pct));
                    }
                    None => {
                        ui.spinner();
                    }
                }
                capped(ui, RichText::new(format!("{what}\u{2026}")).color(weak));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
            // The write under way: how far it has got, and a way to stop it.
            StatusPart::Write => {
                let Some((w, progress)) = self.st.writer.pending().next() else { return };
                let (doing, path) = match w {
                    crate::writer::Written::Save { path, .. } => ("Saving", path),
                    crate::writer::Written::Export { path } => ("Exporting", path),
                };
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if progress.is_cancelled() {
                    ui.label(RichText::new("Cancelling…").color(weak));
                } else if ui
                    .small_button("Cancel")
                    .on_hover_text(format!("Stop {} {name}; the file is left as it was.", doing.to_lowercase()))
                    .clicked()
                {
                    progress.cancel();
                }
                if let Some(f) = progress.fraction() {
                    // As text of its own, so that it is read out too.
                    let pct = format!("{}%", (f * 100.0).floor());
                    ui.add(egui::ProgressBar::new(f).desired_width(90.0).desired_height(14.0).text(pct));
                }
                capped(ui, RichText::new(format!("{}  {doing} {name}…", crate::icons::SAVE)).color(weak));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
            StatusPart::Disk => {
                let tiles = &seen.tiles;
                ui.label(RichText::new(format!("{} on disk", bytes(tiles.on_disk))).color(weak)).on_hover_text(
                    format!(
                        "Parts of the open images not looked at lately, kept in a scratch file until they are \
                         needed again (Preferences › Memory); {} of them are in memory.",
                        bytes(tiles.resident)
                    ),
                );
            }
            StatusPart::Gpu => {
                let Some((used, budget)) = self.st.gpu.memory else { return };
                ui.label(RichText::new(format!("GPU {} / {}", bytes(used), bytes(budget))).color(weak)).on_hover_text(
                    format!(
                        "{} ({})\nGPU memory this process uses, and the budget the system gives it.",
                        self.st.gpu.adapter, self.st.gpu.backend
                    ),
                );
            }
            StatusPart::Zoom => {
                let unit = self.st.unit();
                let Some(d) = self.st.active_mut() else { return };
                let mut pct = f64::from(d.view.zoom) * 100.0;
                let scope = crate::lengths::image_scope(Some(d.doc()), unit, d.doc().resolution);
                let r = ui.add(
                    crate::number_field::NumberField::number(&mut pct, scope)
                        .range(f64::from(crate::doc::MIN_ZOOM) * 100.0..=f64::from(crate::doc::MAX_ZOOM) * 100.0)
                        .suffix("%")
                        .speed(1.0)
                        .decimals(0, 1)
                        .remember("zoom"),
                );
                if r.changed() {
                    let c = d.view.viewport * 0.5;
                    d.view.zoom_about((pct / 100.0) as f32, c);
                }
                r.on_hover_text("Zoom: drag or type a value");
            }
            StatusPart::Size => {
                let (unit, per) = (self.st.unit(), self.st.resolution_unit());
                let Some(d) = self.st.active() else { return };
                let (c, ppi) = (d.doc().canvas, d.doc().resolution);
                let (w, h) = (f64::from(c.width), f64::from(c.height));
                let mut size = format!("{} × {} px", c.width, c.height);
                if unit != opal_core::units::Unit::Px {
                    size = format!("{} ({size})", crate::lengths::show_size(w, h, unit, ppi));
                }
                let print = crate::lengths::print_note(c.width, c.height, ppi, unit, per);
                ui.label(RichText::new(size).color(weak)).on_hover_text(format!(
                    "The canvas's size. {print} (Preferences › Interface: the unit sizes are shown in.)"
                ));
            }
            StatusPart::Resolution => {
                let per = self.st.resolution_unit();
                let Some(d) = self.st.active() else { return };
                let shown = crate::lengths::show_resolution_short(d.doc().resolution, per);
                let r = ui.add(egui::Button::new(RichText::new(shown).color(weak)).frame(false)).on_hover_text(
                    "The image's resolution: how large its pixels print, which sizes in units other than pixels \
                     are counted at. Click to change it (Image › Resize Image), with or without its pixels.",
                );
                if r.clicked() {
                    self.st.queue.push(Cmd::ResizeImage);
                }
            }
            // Positions count from the canvas's corner, wherever cropping has put it.
            StatusPart::Pointer => {
                let unit = self.st.unit();
                let (Some((x, y)), Some(d)) = (self.st.cursor_shown, self.st.active()) else { return };
                let (c, ppi) = (d.doc().canvas, d.doc().resolution);
                let (x, y) = ((x - c.x) as f64, (y - c.y) as f64);
                let at = if unit == opal_core::units::Unit::Px {
                    format!("{x}, {y}")
                } else {
                    let x = crate::lengths::show(x, unit, ppi);
                    format!(
                        "{}, {}",
                        x.rsplit_once(' ').map_or(x.as_str(), |(n, _)| n),
                        crate::lengths::show(y, unit, ppi)
                    )
                };
                ui.label(RichText::new(at).color(weak));
            }
        }
    }
}

/// A part of the status bar at its right, as they are shown from its right end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum StatusPart {
    Scripts,
    Edit,
    Write,
    Disk,
    Gpu,
    Zoom,
    Size,
    Resolution,
    Pointer,
}

impl StatusPart {
    /// The order they are left out in where the bar is too short for all of them.
    const LEAST_NEEDED_FIRST: [StatusPart; 9] = [
        StatusPart::Disk,
        StatusPart::Gpu,
        StatusPart::Pointer,
        StatusPart::Resolution,
        StatusPart::Size,
        StatusPart::Zoom,
        StatusPart::Scripts,
        StatusPart::Write,
        StatusPart::Edit,
    ];
}

/// What the status bar shows of what other threads change (the server's, the tile store's),
/// read once a frame, so that its parts are chosen and drawn by the same.
#[derive(Clone)]
struct StatusSeen {
    clients: usize,
    steps: Vec<String>,
    shut_out: bool,
    tiles: opal_core::store::Status,
}

/// How wide the status bar's parts were when last shown, in points, and what stands between
/// two (a separator, and the spacing either side of it).
#[derive(Clone, Default)]
struct StatusWidths {
    parts: std::collections::HashMap<StatusPart, f32>,
    between: f32,
}

impl StatusWidths {
    /// How wide `parts` are, shown side by side.
    fn of(&self, parts: &[StatusPart]) -> f32 {
        let each: f32 = parts.iter().map(|p| self.parts.get(p).copied().unwrap_or(0.0)).sum();
        each + parts.len().saturating_sub(1) as f32 * self.between
    }
}

/// The room the status bar keeps for what the canvas does, before its parts, in points.
pub(crate) const STATUS_TEXT_ROOM: f32 = 160.0;

/// The widest a part of the status bar's text may be, in points (the scripts' open steps,
/// the file being written): longer, it is cut short.
const STATUS_PART_MAX: f32 = 260.0;

impl eframe::App for OpalApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::outline_tool::take_reshape_keys(&mut self.st.docs, raw_input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.fitted_to_monitor {
            self.fitted_to_monitor = fit_to_monitor(&ctx, self.work_area);
        }
        let before = self.signature();
        // The scheme can change under us when following the system's.
        self.st.palette = theme::palette(self.st.scheme(&ctx));
        self.st.cursor_shown = self.st.cursor_canvas.take();
        self.st.canvas_held = std::mem::take(&mut self.st.canvas_held_now);
        self.st.leave_undrawn(ctx.cumulative_pass_nr());
        // A rename left open when another document became active is finished, in its own
        // document, as clicking elsewhere would finish it.
        if self.st.renaming.as_ref().is_some_and(|r| Some(r.0) != self.st.active_doc) {
            self.st.finish_rename(true);
        }
        self.handle_keys(&ctx);
        self.handle_dropped_files(&ctx);
        self.poll_gpu();

        // The screen mode: full screen, or not; the image alone, or with the rest. A window
        // seen full screen, then not, left it another way (the system's key): as usual again.
        let shown_full = ctx.input(|i| i.viewport().fullscreen);
        if self.full_screen && shown_full == Some(true) {
            self.full_screen_seen = true;
        } else if self.full_screen && self.full_screen_seen && shown_full == Some(false) {
            self.st.screen = ScreenMode::Standard;
            (self.full_screen, self.full_screen_seen) = (false, false);
        }
        let full = self.st.screen != ScreenMode::Standard;
        if full != self.full_screen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(full));
            (self.full_screen, self.full_screen_seen) = (full, false);
        }
        match (self.st.screen == ScreenMode::ImageAlone).then_some(self.st.active_doc).flatten() {
            Some(id) => {
                let fill = egui::Frame::NONE.fill(ui.visuals().panel_fill);
                egui::CentralPanel::default().frame(fill).show(ui, |ui| canvas::canvas_ui(ui, &mut self.st, id));
                self.alone_status(&ctx);
            }
            None => self.frame_ui(ui),
        }

        dialogs::show_all(&ctx, &mut self.st);
        help::help_window(&ctx, &mut self.st);

        for cmd in std::mem::take(&mut self.st.queue) {
            self.st.run(cmd, &ctx);
        }
        if std::mem::take(&mut self.st.reset_panels) {
            self.tree = default_tree(&self.st);
        }
        self.sync_panes(&ctx);
        self.handle_exit(&ctx);
        self.save_settings_if_due(&ctx);
        self.st.between_frames(ctx.input(|i| i.pointer.any_down()));
        self.st.console.poll(&ctx);
        // After the writes, whose answers it passes on.
        if let Some(server) = &mut self.server {
            if std::mem::take(&mut self.st.let_scripts_in) {
                server.let_in();
            }
            server.serve(&mut self.st, &ctx, ctx.input(|i| i.pointer.any_down()));
        } else if self.server_waits {
            self.take_over_serving(&ctx);
        }
        self.sync_mcp_http();
        if std::mem::take(&mut self.st.raise_window) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.st.thumbs.end_frame();
        // What operations on documents asked to say.
        let notices: Vec<String> = self.st.docs.iter_mut().filter_map(|d| d.notice.take()).collect();
        for n in notices {
            self.st.notify(n);
        }
        // The first drop of history states is said once the status bar is free, so that it
        // neither hides another notice nor is hidden by one (the status bar repaints when its
        // notice expires).
        if self.st.notice_free()
            && let Some(d) = self.st.docs.iter_mut().find(|d| d.drop_note_due())
        {
            d.drop_note_told();
            self.st.notify(crate::doc::DROP_NOTE);
        }
        // Changes made after something was drawn this frame (commands run at the end of
        // the frame, a panel changing a layer after the canvas drew it) need another frame
        // to show.
        if self.signature() != before {
            ctx.request_repaint();
        }
        self.field_had_keys = ctx.egui_wants_keyboard_input();
        // What the fields remembered by name had entered, kept with the settings.
        if let Some(entries) = crate::number_field::changed_entries(&ctx) {
            self.st.field_entries = entries;
            self.st.settings_touched();
        }
        let st = &mut self.st;
        if let Some(rs) = _frame.wgpu_render_state() {
            st.gpu.textures =
                rs.renderer.read().callback_resources.get::<CanvasRenderer>().map_or(0, CanvasRenderer::texture_count);
        }
    }

    fn on_exit(&mut self) {
        if !self.persist || self.st.settings_changed.is_none() {
            return;
        }
        if let Err(e) = config::save(&self.st) {
            eprintln!("Opal Studio: could not save settings: {e}");
        }
    }
}

/// What the status bar says when keys are held back during a drag on the image.
pub const KEYS_WAIT: &str =
    "Keys that would change the image do nothing while a mouse button is held on it: let go first.";

/// Whether `cmd` acts while a mouse button is held on an image: the view's commands, which
/// change nothing a drag works out its edit from. (Not the screen modes: the canvas is drawn
/// elsewhere in another, and would no longer know the button held on it.)
fn acts_during_drag(cmd: Cmd) -> bool {
    matches!(
        cmd,
        Cmd::ZoomIn
            | Cmd::ZoomOut
            | Cmd::ZoomFit
            | Cmd::Zoom100
            | Cmd::ToggleRulers
            | Cmd::ToggleOffCanvas
            | Cmd::ToggleSelectionOverlay
    )
}

/// Takes out of this frame's input, while a drag on an image is under way, every key press but
/// Esc (which gives a drag up), Space (the hand, to scroll) and the view's commands' shortcuts
/// (see [`acts_during_drag`]), and the clipboard's events. Whether any of them would have done
/// something: a command's shortcut, a tool's key, an editing key (arrows, Enter, Delete,
/// Backspace) or a clipboard event; not a key's repeats (R, say, held for R+drag) nor one that
/// does nothing.
fn keys_held_back(ctx: &egui::Context) -> bool {
    let shortcuts = shortcuts_most_specific_first();
    let matches = |s: &egui::KeyboardShortcut, key: Key, modifiers: Modifiers| {
        s.logical_key == key && modifiers.matches_logically(s.modifiers)
    };
    let view = |key: Key, modifiers: Modifiers| {
        shortcuts.iter().any(|(s, c)| acts_during_drag(*c) && matches(s, key, modifiers))
    };
    let acts = |key: Key, modifiers: Modifiers| {
        shortcuts.iter().any(|(s, _)| matches(s, key, modifiers))
            || (modifiers.is_none() && ToolId::ALL.iter().any(|t| t.info().key == key))
            || matches!(
                key,
                Key::ArrowLeft
                    | Key::ArrowRight
                    | Key::ArrowUp
                    | Key::ArrowDown
                    | Key::Enter
                    | Key::Delete
                    | Key::Backspace
            )
    };
    ctx.input_mut(|i| {
        let mut held = false;
        i.events.retain(|e| match e {
            Event::Key { key: Key::Escape | Key::Space, .. } | Event::Key { pressed: false, .. } => true,
            Event::Key { key, modifiers, repeat, .. } => {
                let kept = view(*key, *modifiers);
                held |= !kept && !*repeat && acts(*key, *modifiers);
                kept
            }
            Event::Copy | Event::Cut | Event::Paste(_) | Event::PasteImage(_) => {
                held = true;
                false
            }
            _ => true,
        });
        held
    })
}

/// Every command shortcut with its command, those with more modifiers first. egui matches a
/// shortcut ignoring extra Shift and Alt, so Ctrl+S, tried first, would take Ctrl+Shift+S.
/// Made once.
fn shortcuts_most_specific_first() -> &'static [(egui::KeyboardShortcut, Cmd)] {
    static ALL: std::sync::OnceLock<Vec<(egui::KeyboardShortcut, Cmd)>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all: Vec<(egui::KeyboardShortcut, Cmd)> =
            Cmd::ALL.iter().flat_map(|&c| c.info().shortcuts.iter().map(move |s| (*s, c))).collect();
        let count = |m: Modifiers| u8::from(m.alt) + u8::from(m.shift) + u8::from(m.command || m.ctrl);
        all.sort_by_key(|(s, _)| std::cmp::Reverse(count(s.modifiers)));
        all
    })
}

/// The documents whose tabs are in front, in their containers.
fn shown_docs(tiles: &Tiles<Pane>) -> Vec<DocId> {
    tiles
        .iter()
        .filter_map(|(_, t)| match t {
            egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs)) => tabs.active,
            _ => None,
        })
        .filter_map(|id| match tiles.get_pane(&id) {
            Some(Pane::Document(d)) => Some(*d),
            _ => None,
        })
        .collect()
}

/// Brings `pane`'s tab to the front of its container.
fn show_pane(tiles: &mut Tiles<Pane>, pane: Pane) {
    if let Some(tile) = tiles.find_pane(&pane)
        && let Some(parent) = tiles.parent_of(tile)
        && let Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) = tiles.get_mut(parent)
    {
        tabs.set_active(tile);
    }
}

/// The panel tree's tile for the documents (the tab container holding them or the start
/// page).
fn find_docs_container(tiles: &Tiles<Pane>) -> Option<TileId> {
    let pane = tiles
        .iter()
        .find(|(_, t)| matches!(t, egui_tiles::Tile::Pane(Pane::Document(_) | Pane::Welcome)))
        .map(|(id, _)| *id)?;
    tiles.parent_of(pane)
}

/// Where windows go on a full-HD screen whose taskbar is 48 pixels tall, in physical pixels,
/// at any point, for tests.
#[cfg(test)]
pub(crate) fn full_hd_work_area(_: egui::Pos2) -> Option<egui::Rect> {
    Some(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1920.0, 1032.0)))
}

/// The size the window starts at, in points at 100% interface scale (logical pixels).
pub const START_SIZE: [f32; 2] = [1440.0, 900.0];

/// Room a monitor keeps for a taskbar at its bottom, in logical pixels (Windows 11's), where
/// the system does not say what its docks take.
const TASKBAR: f32 = 48.0;

/// Puts the window all where windows go on its monitor (not under the taskbar or another
/// dock): moved there if it fits, maximized if not, as the starting size does not on a small
/// screen or a full-HD one at 125%. `work_area` says where windows go on the monitor at a
/// point (in physical pixels), where the system can say. Returns whether it could tell: not in
/// the first frame (whose sizes are measured before the interface's zoom applies), nor while
/// the window is minimized, nor where the system does not say where it is (Wayland).
fn fit_to_monitor(ctx: &egui::Context, work_area: fn(egui::Pos2) -> Option<egui::Rect>) -> bool {
    if ctx.cumulative_frame_nr() == 0 {
        return false;
    }
    let (outer, inner, monitor, minimized) = ctx.input(|i| {
        let v = i.viewport();
        (v.outer_rect, v.inner_rect, v.monitor_size, v.minimized)
    });
    let (Some(outer), Some(inner), false) = (outer, inner, minimized == Some(true)) else { return false };
    // What of it shows: its frame, less the borders Windows keeps unseen at its left, right and
    // bottom for resizing (its title bar is its top).
    let shown = egui::Rect::from_min_max(egui::pos2(inner.min.x, outer.min.y), inner.max);
    let ppp = ctx.pixels_per_point();
    let room = match work_area(shown.center() * ppp) {
        Some(px) => egui::Rect::from_min_max(px.min / ppp, px.max / ppp),
        // Where the system does not say, the monitor's size, less a taskbar at the bottom; and
        // not where on it the window is, so it is only ever maximized.
        None => {
            let Some(monitor) = monitor else { return false };
            egui::Rect::from_min_size(shown.min, monitor - egui::vec2(0.0, TASKBAR / ctx.zoom_factor()))
        }
    };
    match fitting(shown, room) {
        // Moved by its frame, the unseen borders with it.
        Some(egui::ViewportCommand::OuterPosition(to)) => {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(to - (shown.min - outer.min)));
        }
        Some(cmd) => ctx.send_viewport_cmd(cmd),
        None => {}
    }
    true
}

/// What puts what shows of the window, `window`, all in `room` (both in points): nothing if it
/// is; moved so that its corner is at the place given, the middle of `room`, if it fits
/// there; else maximized.
fn fitting(window: egui::Rect, room: egui::Rect) -> Option<egui::ViewportCommand> {
    if room.contains_rect(window) {
        None
    } else if window.width() <= room.width() && window.height() <= room.height() {
        Some(egui::ViewportCommand::OuterPosition(room.center() - window.size() / 2.0))
    } else {
        Some(egui::ViewportCommand::Maximized(true))
    }
}

/// Where windows go on the monitor at `point` (in physical pixels): what its taskbar and other
/// docks leave, in physical pixels.
#[cfg(windows)]
fn work_area(point: egui::Pos2) -> Option<egui::Rect> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
    let at = POINT { x: point.x.round() as i32, y: point.y.round() as i32 };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: MonitorFromPoint takes any point (the nearest monitor, for one on none), and
    // GetMonitorInfoW writes only into `info`, whose size it is told.
    let known = unsafe { GetMonitorInfoW(MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool();
    let r = info.rcWork;
    known.then(|| {
        egui::Rect::from_min_max(egui::pos2(r.left as f32, r.top as f32), egui::pos2(r.right as f32, r.bottom as f32))
    })
}

/// Where windows go on the monitor at a point: not known here.
#[cfg(not(windows))]
fn work_area(_point: egui::Pos2) -> Option<egui::Rect> {
    None
}

/// The margin between the side panels' edges and what they show, in points.
const PANE_INSET: i8 = 8;

/// The starting layout: images in tabs on the left; Tool settings above Layers, Selection,
/// History and Console on the right.
fn default_tree(st: &AppState) -> egui_tiles::Tree<Pane> {
    let mut tiles = Tiles::default();
    let mut docs: Vec<TileId> = st.docs.iter().map(|d| tiles.insert_pane(Pane::Document(d.id))).collect();
    if docs.is_empty() {
        docs.push(tiles.insert_pane(Pane::Welcome));
    }
    let active = st.active_doc.and_then(|a| tiles.find_pane(&Pane::Document(a)));
    let doc_tabs = tiles.insert_tab_tile(docs);
    if let (Some(a), Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(t)))) =
        (active, tiles.get_mut(doc_tabs))
    {
        t.set_active(a);
    }
    let tool = tiles.insert_pane(Pane::ToolSettings);
    let tool_tabs = tiles.insert_tab_tile(vec![tool]);
    let layers = tiles.insert_pane(Pane::Layers);
    let selection = tiles.insert_pane(Pane::Selection);
    let history = tiles.insert_pane(Pane::History);
    let console = tiles.insert_pane(Pane::Console);
    let lh = tiles.insert_tab_tile(vec![layers, selection, history, console]);
    // Halves: in a window maximized on a full-HD screen at 125%, so the tool settings show a box's
    // position and size without scrolling (the crop box's after the crop tool's own settings).
    let mut right = egui_tiles::Linear::new(egui_tiles::LinearDir::Vertical, vec![tool_tabs, lh]);
    right.shares.set_share(tool_tabs, 0.5);
    right.shares.set_share(lh, 0.5);
    let right = tiles.insert_container(right);
    let mut root = egui_tiles::Linear::new(egui_tiles::LinearDir::Horizontal, vec![doc_tabs, right]);
    root.shares.set_share(doc_tabs, 0.78);
    root.shares.set_share(right, 0.22);
    let root = tiles.insert_container(root);
    egui_tiles::Tree::new(egui::Id::unique("opal-panels"), root, tiles)
}

struct Behavior<'a> {
    st: &'a mut AppState,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Welcome => "Start".into(),
            Pane::Document(id) => match self.st.doc(*id) {
                Some(d) => {
                    let mut title = d.title.clone();
                    if d.is_dirty() {
                        title.push_str("  ●");
                    }
                    // An edit shown under way on it (see `edits`).
                    if d.busy.as_ref().is_some_and(crate::edits::Busy::shown) {
                        title.push_str(&format!("  {}", icons::HISTORY));
                    }
                    title.into()
                }
                None => "Closed".into(),
            },
            Pane::Layers => format!("{}  Layers", icons::LAYER).into(),
            Pane::Selection => format!("{}  Selection", icons::SELECT_ALL).into(),
            Pane::History => format!("{}  History", icons::HISTORY).into(),
            Pane::ToolSettings => format!("{}  Tool settings", icons::OPTIONS).into(),
            Pane::Console => format!("{}  Console", icons::CONSOLE).into(),
        }
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let inset = |ui: &mut egui::Ui, f: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::NONE.inner_margin(egui::Margin::same(PANE_INSET)).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                f(ui);
            });
        };
        match *pane {
            Pane::Document(id) => canvas::canvas_ui(ui, self.st, id),
            Pane::Welcome => welcome(ui, self.st),
            Pane::Layers => inset(ui, &mut |ui| panels::unless_busy(ui, self.st, panels::layers_panel)),
            Pane::Selection => {
                inset(ui, &mut |ui| panels::unless_busy(ui, self.st, crate::selection_panel::selection_panel))
            }
            Pane::History => inset(ui, &mut |ui| panels::unless_busy(ui, self.st, panels::history_panel)),
            Pane::ToolSettings => inset(ui, &mut |ui| {
                // As wide as the pane, whatever it holds, so that its scroll bar is at its edge.
                let area = egui::ScrollArea::vertical().auto_shrink([false, true]);
                area.show(ui, |ui| panels::tool_settings_panel(ui, self.st));
            }),
            Pane::Console => inset(ui, &mut |ui| crate::console::console_panel(ui, self.st)),
        }
        UiResponse::None
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile_id: TileId) -> bool {
        matches!(tiles.get_pane(&tile_id), Some(Pane::Document(_)))
    }

    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId) -> bool {
        if let Some(Pane::Document(id)) = tiles.get_pane(&tile_id) {
            self.st.request_close(*id);
        }
        // The tab goes once the document has closed (see `retain_pane`).
        false
    }

    fn retain_pane(&mut self, pane: &Pane) -> bool {
        match pane {
            Pane::Document(id) => self.st.doc(*id).is_some(),
            _ => true,
        }
    }

    fn on_tab_button(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId, response: egui::Response) -> egui::Response {
        if (response.clicked() || response.drag_started())
            && let Some(Pane::Document(id)) = tiles.get_pane(&tile_id)
        {
            self.st.active_doc = Some(*id);
        }
        response
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions { all_panes_must_have_tabs: true, ..Default::default() }
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        self.st.palette.tab_bar
    }
}

fn welcome(ui: &mut egui::Ui, st: &mut AppState) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, st.palette.backdrop);
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.25).max(20.0));
        ui.label(RichText::new("Opal Studio").size(32.0).color(st.palette.strong_text));
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let w = 2.0 * 150.0 + ui.spacing().item_spacing.x;
            ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
            let big = |c: char, s: &str| {
                egui::Button::new(RichText::new(format!("{c}  {s}")).size(16.0)).min_size(egui::vec2(150.0, 40.0))
            };
            if ui.add(big(icons::NEW, "New image")).clicked() {
                st.queue.push(Cmd::New);
            }
            if ui.add(big(icons::OPEN, "Open…")).clicked() {
                st.queue.push(Cmd::Open);
            }
        });
        ui.add_space(16.0);
        let weak = st.palette.weak_text;
        for line in [
            "Drop image files here to open them.",
            "Press Ctrl+K to find any command, or Alt to type one from the ribbon.",
            "Press F1 over any button for help.",
        ] {
            ui.label(RichText::new(line).color(weak));
        }
    });
}

/// Opens the API's connection, telling the user if it cannot be; and whether another editor
/// serves scripts already (which they connect to, until it goes).
fn start_server(st: &mut AppState, ctx: &egui::Context) -> (Option<crate::server::Server>, bool) {
    let Some(file) = crate::server::connection_file() else {
        st.scripts_unserved = Some("There is no settings folder to keep the connection's file in.".to_owned());
        return (None, false);
    };
    match crate::server::Server::start(file.clone(), ctx.clone()) {
        Ok(server) => {
            st.scripts_file = Some(file);
            (Some(server), false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            st.notify("Another Opal Studio is open: scripts and assistants connect to that one.");
            (None, true)
        }
        Err(e) => {
            let why = format!("The connection for scripts and assistants could not be opened: {e}");
            st.message(crate::state::MessageKind::Warning, "Scripts cannot connect", why.clone());
            st.scripts_unserved = Some(why);
            (None, false)
        }
    }
}

/// How often an editor waiting to serve scripts looks whether the connection file has gone
/// (the other editor closed), and how often it asks the other editor (which may have ended
/// without removing it).
const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

impl OpalApp {
    /// Serves MCP over HTTP as the preferences say: started, moved to another port, given a
    /// new token or stopped. Not in tests, and only by the editor that serves scripts (its
    /// sessions are that editor's clients).
    fn sync_mcp_http(&mut self) {
        let wanted = self.persist && self.server.is_some() && self.st.prefs.bool("scripting.mcp_http");
        let port = self.st.prefs.f64("scripting.mcp_port") as u16;
        let renew = std::mem::take(&mut self.st.renew_mcp_token);
        // Asked to try again (Connect an Assistant): what failed is tried once more.
        if std::mem::take(&mut self.st.retry_mcp_http) {
            self.mcp_http_failed = None;
        }
        if !wanted {
            (self.mcp_http, self.mcp_http_failed, self.st.mcp_http, self.st.mcp_http_trouble) =
                (None, None, None, None);
            return;
        }
        let serving = self.mcp_http.as_ref().is_some_and(|h| h.port() == port);
        if (serving && !renew) || (!serving && self.mcp_http_failed == Some(port)) {
            return;
        }
        let (Some(token_file), Some(api)) = (crate::mcp_http::token_file(), self.st.scripts_file.clone()) else {
            return;
        };
        // The token first: if a new one cannot be made, the one served stays.
        let token = match crate::mcp_http::token(&token_file, renew) {
            Ok(token) => token,
            Err(e) => {
                self.mcp_http_failed = Some(port);
                let why = format!("Its token could not be kept in {}: {e}", token_file.display());
                let title = if serving { "No new MCP token was made" } else { "MCP over HTTP is not served" };
                self.st.mcp_http_trouble = (!serving).then(|| why.clone());
                return self.st.message(crate::state::MessageKind::Warning, title, why);
            }
        };
        // The old server goes first, freeing its port.
        (self.mcp_http, self.st.mcp_http) = (None, None);
        let new_caller = move || -> Box<dyn crate::mcp::Caller + Send> { Box::new(crate::mcp::pipe_to(api.clone())) };
        match crate::mcp_http::HttpServer::start(port, token.clone(), new_caller) {
            Ok(server) => {
                (self.st.mcp_http, self.st.mcp_http_trouble) = (Some((port, token)), None);
                (self.mcp_http, self.mcp_http_failed) = (Some(server), None);
            }
            Err(e) => {
                self.mcp_http_failed = Some(port);
                let why = format!("Port {port} could not be opened: {e}. Preferences › Scripting chooses another.");
                self.st.mcp_http_trouble = Some(why.clone());
                self.st.message(crate::state::MessageKind::Warning, "MCP over HTTP is not served", why);
            }
        }
    }

    /// Starts serving scripts once the editor that served them has gone.
    fn take_over_serving(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(LOOK_EVERY);
        let now = std::time::Instant::now();
        let (looked, asked) = self.server_looked;
        if now - looked < LOOK_EVERY {
            return;
        }
        let Some(file) = crate::server::connection_file() else { return };
        let ask = now - asked >= ASK_EVERY;
        self.server_looked = (now, if ask { now } else { asked });
        // Starting asks the other editor first (without waiting), and fails while it is there.
        if !ask && file.exists() {
            return;
        }
        match crate::server::Server::start(file.clone(), ctx.clone()) {
            Ok(server) => {
                self.st.scripts_file = Some(file);
                self.server = Some(server);
                self.server_waits = false;
                self.st.notify("The other Opal Studio has closed: scripts and assistants now connect to this one.");
            }
            // Still there.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(e) => {
                self.server_waits = false;
                let why = format!("The connection for scripts and assistants could not be opened: {e}");
                self.st.message(crate::state::MessageKind::Warning, "Scripts cannot connect", why.clone());
                self.st.scripts_unserved = Some(why);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window as it starts, its frame about it (a title bar 31 points tall, borders of 8),
    /// its top-left corner at `at`.
    fn starting(at: [f32; 2]) -> egui::Rect {
        let [w, h] = START_SIZE;
        egui::Rect::from_min_size(at.into(), egui::vec2(w + 16.0, h + 31.0 + 8.0))
    }

    #[test]
    fn the_window_is_moved_or_maximized_into_where_windows_go() {
        // A full-HD screen at 100%, its taskbar 48 points: the window, in its middle, fits.
        let room = full_hd_work_area(egui::Pos2::ZERO).expect("a work area");
        let middle = starting([232.0, 70.0]);
        assert_eq!(fitting(middle, room), None);
        // Cascaded down so that its bottom is under the taskbar: moved up, into the middle.
        let low = starting([300.0, 150.0]);
        let moved = egui::ViewportCommand::OuterPosition(room.center() - low.size() / 2.0);
        assert_eq!(fitting(low, room), Some(moved));
        assert!(room.contains_rect(egui::Rect::from_center_size(room.center(), low.size())));
        // At 125% the same screen is 1536 x 825.6 points where windows go: too short for it.
        let room = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1536.0, 825.6));
        assert_eq!(fitting(starting([40.0, 0.0]), room), Some(egui::ViewportCommand::Maximized(true)));
    }

    /// The interface zoomed (as its size preference does before the first frame), the first
    /// frame's sizes measured as if it were not (as egui-winit measures them then): nothing is
    /// decided by them. In the next frame's, in the zoom's units, a window that fits is left.
    #[test]
    fn the_window_s_fit_waits_for_the_interface_s_zoom() {
        use egui::{Rect, ViewportCommand, ViewportId, pos2, vec2};
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(1.25);
        // In the middle of a full-HD screen at 100%, in physical pixels: it fits.
        let [w, h] = START_SIZE;
        let inner = Rect::from_min_size(pos2(240.0, 97.0), vec2(w, h));
        let outer = Rect::from_min_max(pos2(232.0, 66.0), inner.max + vec2(8.0, 8.0));
        // A frame whose sizes are in points of `scale` physical pixels: whether the fit was
        // decided in it, and the commands it asked for.
        let frame = |scale: f32| {
            let mut input = egui::RawInput::default();
            let v = input.viewports.entry(ViewportId::ROOT).or_default();
            v.native_pixels_per_point = Some(1.0);
            v.outer_rect = Some(Rect::from_min_max(outer.min / scale, outer.max / scale));
            v.inner_rect = Some(Rect::from_min_max(inner.min / scale, inner.max / scale));
            v.monitor_size = Some(vec2(1920.0, 1080.0) / scale);
            let mut decided = false;
            let mut out = ctx.run_ui(input, |ui| decided = fit_to_monitor(ui.ctx(), full_hd_work_area));
            // The fonts' textures, which nothing here draws with.
            out.textures_delta.clear();
            let asked: Vec<ViewportCommand> = out.viewport_output.get(&ViewportId::ROOT).map_or_else(Vec::new, |v| {
                v.commands
                    .iter()
                    .filter(|c| matches!(c, ViewportCommand::Maximized(_) | ViewportCommand::OuterPosition(_)))
                    .cloned()
                    .collect()
            });
            (decided, asked)
        };
        assert_eq!(frame(1.0), (false, vec![]), "decided by the first frame's sizes");
        assert_eq!(frame(1.25), (true, vec![]), "a window that fits was not left");
    }
}
