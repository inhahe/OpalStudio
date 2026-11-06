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
use crate::state::{AppState, Confirm, Keytips};
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
        if persist {
            config::load(&mut st);
        }
        st.apply_prefs(&cc.egui_ctx);
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
            alt_alone: false,
            persist,
            server,
            server_waits,
            server_looked: (std::time::Instant::now(), std::time::Instant::now()),
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
        let typing = ctx.egui_wants_keyboard_input();
        if !typing {
            self.clipboard_events(ctx, paste_presses);
            self.cancel_paste_key(ctx);
            self.editing_keys(ctx);
        }
        // Keytips: Alt pressed and released on its own toggles them.
        let (alt, other_input) = ctx.input(|i| {
            let other = i.events.iter().any(|e| {
                matches!(
                    e,
                    Event::Key { pressed: true, .. }
                        | Event::PointerButton { pressed: true, .. }
                        | Event::MouseWheel { .. }
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
        for (s, cmd) in shortcuts_most_specific_first() {
            if ctx.input_mut(|i| i.consume_shortcut(&s)) {
                self.st.queue.push(cmd);
            }
        }
        for t in ToolId::ALL {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, t.info().key)) {
                self.st.tool = t;
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

    /// Esc cancels a paste still being read, or waiting to be placed, unless a box on the
    /// image shown takes it first.
    fn cancel_paste_key(&mut self, ctx: &egui::Context) {
        if self.st.reading.is_none() {
            return;
        }
        if !self.st.box_takes_escape() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.st.cancel_paste();
        }
    }

    /// Enter, Esc and the arrow keys: applying or cancelling the transform box, and moving
    /// it or the selected layers by a pixel (or, with Shift, by the nudge distance).
    fn editing_keys(&mut self, ctx: &egui::Context) {
        let far = self.st.prefs.f64("nav.nudge").round() as i64;
        let crop_tool = self.st.tool == ToolId::Crop;
        let crop_delete = self.st.tool_settings(ToolId::Crop).bool("crop.delete");
        let sampling = crate::prefs::sampling(&self.st.prefs, "transform");
        let Some(d) = self.st.active_mut() else { return };
        let cropping = crop_tool && d.crop.is_some();
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
                    match d.crop_to(&b, crop_delete, sampling) {
                        Ok(()) => d.crop = None,
                        Err(why) => refused = Some(why),
                    }
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
                d.commit_pending();
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
        if arrows != [0, 0] && !d.selection().is_empty() {
            match crate::move_tool::locked_in_selection(d) {
                Some(name) => self.st.notify(format!("\"{name}\" is locked, so it cannot be moved. Unlock it first.")),
                None => d.nudge_selection(arrows),
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
                    if let Some((cmd, _)) = button_keytips(t).into_iter().find(|(_, k)| *k == c) {
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
        for p in dropped {
            self.st.open_path(&p);
        }
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
        // Files still being written are finished first.
        if self.st.writer.busy() {
            if window_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            self.st.exit_after_writes = true;
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
    fn sync_panes(&mut self) {
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
        if std::mem::take(&mut self.st.show_console) {
            show_pane(&mut self.tree.tiles, Pane::Console);
        }
        let reveal = std::mem::take(&mut self.st.reveal);
        let shown = shown_docs(&self.tree.tiles);
        if let Some(id) = reveal.filter(|id| self.st.doc(*id).is_some()) {
            show_pane(&mut self.tree.tiles, Pane::Document(id));
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
            d.selection().hash(&mut h);
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
        st.tool.hash(&mut h);
        (st.ribbon.tab, &st.ribbon.keytips, st.renaming.is_some(), st.messages.len(), st.confirm.is_some())
            .hash(&mut h);
        (st.new_image.is_some(), st.export.is_some(), st.prefs_open, st.about_open, st.gallery_open, &st.help)
            .hash(&mut h);
        (st.resize_image.is_some(), st.resize_canvas.is_some(), st.rotate_by.is_some()).hash(&mut h);
        st.reading.is_some().hash(&mut h);
        st.writer.busy().hash(&mut h);
        st.notice.as_ref().map(|n| &n.0).hash(&mut h);
        h.finish()
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let weak = self.st.palette.weak_text;
        ui.horizontal(|ui| {
            let tool = self.st.tool.info();
            // A recent notice, for a few seconds, instead of the tool's description.
            const SHOWN: std::time::Duration = crate::state::NOTICE_SHOWN;
            match &self.st.notice {
                Some((text, at)) if at.elapsed() < SHOWN => {
                    ui.label(
                        RichText::new(format!("{}  {text}", crate::icons::INFO)).color(self.st.palette.strong_text),
                    );
                    ui.ctx().request_repaint_after(SHOWN.saturating_sub(at.elapsed()));
                }
                _ => {
                    ui.label(RichText::new(format!("{}  {}", tool.icon, tool.description)).color(weak));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Scripts and assistants connected, and a way to send them off; once sent off,
                // a way to let them in again.
                if let Some(server) = &self.server {
                    let n = server.clients();
                    if n > 0 {
                        if ui
                            .small_button("Disconnect")
                            .on_hover_text(
                                "Drop every connected script and assistant (each is cut off at its next \
                                 request), and keep them out until you let them in again.",
                            )
                            .clicked()
                        {
                            server.disconnect_all();
                        }
                        let who = if n == 1 {
                            "A script is connected".to_owned()
                        } else {
                            format!("{n} scripts are connected")
                        };
                        ui.label(
                            RichText::new(format!("{}  {who}", crate::icons::INFO)).color(self.st.palette.strong_text),
                        );
                        ui.separator();
                    } else if server.shut_out() {
                        if ui
                            .small_button("Let in")
                            .on_hover_text("Let scripts and assistants connect again.")
                            .clicked()
                        {
                            server.let_in();
                        }
                        ui.label(
                            RichText::new(format!("{}  Scripts are disconnected", crate::icons::INFO)).color(weak),
                        );
                        ui.separator();
                    }
                }
                // The write under way: how far it has got, and a way to stop it.
                if let Some((w, progress)) = self.st.writer.pending().next() {
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
                    ui.label(RichText::new(format!("{}  {doing} {name}…", crate::icons::SAVE)).color(weak));
                    ui.separator();
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
                let tiles = opal_core::store::status();
                if tiles.on_disk > 0 {
                    ui.label(RichText::new(format!("{} on disk", bytes(tiles.on_disk))).color(weak)).on_hover_text(
                        format!(
                            "Parts of the open images not looked at lately, kept in a scratch file until they are \
                             needed again (Preferences › Memory); {} of them are in memory.",
                            bytes(tiles.resident)
                        ),
                    );
                    ui.separator();
                }
                if let Some((used, budget)) = self.st.gpu.memory {
                    ui.label(RichText::new(format!("GPU {} / {}", bytes(used), bytes(budget))).color(weak))
                        .on_hover_text(format!(
                            "{} ({})\nGPU memory this process uses, and the budget the system gives it.",
                            self.st.gpu.adapter, self.st.gpu.backend
                        ));
                    ui.separator();
                }
                let Some(d) = self.st.active_mut() else { return };
                let mut pct = d.view.zoom * 100.0;
                let r = ui.add(
                    egui::DragValue::new(&mut pct)
                        .range(crate::doc::MIN_ZOOM * 100.0..=crate::doc::MAX_ZOOM * 100.0)
                        .suffix("%")
                        .speed(1.0)
                        .max_decimals(1),
                );
                if r.changed() {
                    let c = d.view.viewport * 0.5;
                    d.view.zoom_about(pct / 100.0, c);
                }
                r.on_hover_text("Zoom: drag or type a value");
                ui.separator();
                let c = d.doc().canvas;
                ui.label(RichText::new(format!("{} × {} px", c.width, c.height)).color(weak));
                ui.separator();
                // Positions count from the canvas's corner, wherever cropping has put it.
                if let Some((x, y)) = self.st.cursor_shown {
                    ui.label(RichText::new(format!("{}, {}", x - c.x, y - c.y)).color(weak));
                }
            });
        });
    }
}

impl eframe::App for OpalApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let before = self.signature();
        // The scheme can change under us when following the system's.
        self.st.palette = theme::palette(self.st.scheme(&ctx));
        self.st.cursor_shown = self.st.cursor_canvas.take();
        // A rename left open when another document became active is finished, in its own
        // document, as clicking elsewhere would finish it.
        if self.st.renaming.as_ref().is_some_and(|r| Some(r.0) != self.st.active_doc) {
            self.st.finish_rename(true);
        }
        self.handle_keys(&ctx);
        self.handle_dropped_files(&ctx);
        self.poll_gpu();

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

        dialogs::show_all(&ctx, &mut self.st);
        help::help_window(&ctx, &mut self.st);

        for cmd in std::mem::take(&mut self.st.queue) {
            self.st.run(cmd, &ctx);
        }
        if std::mem::take(&mut self.st.reset_panels) {
            self.tree = default_tree(&self.st);
        }
        self.sync_panes();
        self.handle_exit(&ctx);
        self.save_settings_if_due(&ctx);
        self.st.poll_copies();
        self.st.poll_writes(ctx.input(|i| i.pointer.any_down()));
        self.st.console.poll(&ctx);
        // After the writes, whose answers it passes on.
        if let Some(server) = &mut self.server {
            if std::mem::take(&mut self.st.let_scripts_in) {
                server.let_in();
            }
            let busy = self.st.scripts_wait(ctx.input(|i| i.pointer.any_down()));
            server.serve(&mut self.st, &ctx, busy);
        } else if self.server_waits {
            self.take_over_serving(&ctx);
        }
        if std::mem::take(&mut self.st.raise_window) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.st.poll_paste(ctx.input(|i| i.pointer.any_down()));
        self.st.thumbs.end_frame();
        let budget = crate::prefs::bytes(&self.st.prefs, "history.memory");
        for d in &mut self.st.docs {
            d.set_history_budget(budget);
        }
        self.st.apply_tile_budget();
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

/// Every command shortcut with its command, those with more modifiers first. egui matches a
/// shortcut ignoring extra Shift and Alt, so Ctrl+S, tried first, would take Ctrl+Shift+S.
fn shortcuts_most_specific_first() -> Vec<(egui::KeyboardShortcut, Cmd)> {
    let mut all: Vec<(egui::KeyboardShortcut, Cmd)> =
        Cmd::ALL.iter().flat_map(|&c| c.info().shortcuts.iter().map(move |s| (*s, c))).collect();
    let count = |m: Modifiers| u8::from(m.alt) + u8::from(m.shift) + u8::from(m.command || m.ctrl);
    all.sort_by_key(|(s, _)| std::cmp::Reverse(count(s.modifiers)));
    all
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

/// The starting layout: images in tabs on the left; Tool settings above Layers and History
/// on the right.
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
    let history = tiles.insert_pane(Pane::History);
    let console = tiles.insert_pane(Pane::Console);
    let lh = tiles.insert_tab_tile(vec![layers, history, console]);
    let mut right = egui_tiles::Linear::new(egui_tiles::LinearDir::Vertical, vec![tool_tabs, lh]);
    right.shares.set_share(tool_tabs, 0.35);
    right.shares.set_share(lh, 0.65);
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
                Some(d) if d.is_dirty() => format!("{}  ●", d.title).into(),
                Some(d) => d.title.clone().into(),
                None => "Closed".into(),
            },
            Pane::Layers => format!("{}  Layers", icons::LAYER).into(),
            Pane::History => format!("{}  History", icons::HISTORY).into(),
            Pane::ToolSettings => format!("{}  Tool settings", icons::OPTIONS).into(),
            Pane::Console => format!("{}  Console", icons::CONSOLE).into(),
        }
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let inset = |ui: &mut egui::Ui, f: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::NONE.inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                f(ui);
            });
        };
        match *pane {
            Pane::Document(id) => canvas::canvas_ui(ui, self.st, id),
            Pane::Welcome => welcome(ui, self.st),
            Pane::Layers => inset(ui, &mut |ui| panels::layers_panel(ui, self.st)),
            Pane::History => inset(ui, &mut |ui| panels::history_panel(ui, self.st)),
            Pane::ToolSettings => inset(ui, &mut |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| panels::tool_settings_panel(ui, self.st));
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
    let Some(file) = crate::server::connection_file() else { return (None, false) };
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
            st.message(
                crate::state::MessageKind::Warning,
                "Scripts cannot connect",
                format!("The connection for scripts and assistants could not be opened: {e}"),
            );
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
        if (ask || !file.exists())
            && let Ok(server) = crate::server::Server::start(file.clone(), ctx.clone())
        {
            self.st.scripts_file = Some(file);
            self.server = Some(server);
            self.server_waits = false;
            self.st.notify("The other Opal Studio has closed: scripts and assistants now connect to this one.");
        }
    }
}
