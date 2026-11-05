//! The application's state, and the actions commands run.

use crate::commands::{Cmd, availability};
use crate::doc::{DocId, DocState};
use crate::prefs::PREFS;
use crate::settings::{Settings, Value};
use crate::theme::{self, Palette, Scheme};
use crate::tools::{self, ToolId};
use eframe::egui::{self, Vec2};
use opal_core::composite::composite_all;
use opal_core::document::{Document, IdGen, Layer, LayerContent, LayerId, Position};
use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::{Tile, TileCoord, pixel_from_f32};
use opal_io::image_files::{self, ExportFormat};
use opal_io::native;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Warning,
    Error,
}

/// Something to tell the user in a dialog.
#[derive(Clone, Debug)]
pub struct Message {
    pub kind: MessageKind,
    pub title: String,
    pub body: String,
}

/// A question about unsaved changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// Closing one document.
    Close(DocId),
    /// Quitting with these documents unsaved.
    Exit(Vec<DocId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    White,
    Transparent,
}

/// The New Image dialog's fields.
#[derive(Clone, Debug)]
pub struct NewImage {
    pub width: u32,
    pub height: u32,
    pub background: Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Png,
    Jpeg,
}

/// The Export dialog's fields.
#[derive(Clone, Debug)]
pub struct ExportDialog {
    pub doc: DocId,
    pub kind: ExportKind,
}

/// What the ribbon remembers between frames.
#[derive(Clone, Debug, Default)]
pub struct RibbonState {
    pub tab: usize,
    pub search: String,
    pub focus_search: bool,
    pub search_selected: usize,
    pub keytips: Keytips,
    /// The command whose button the pointer is over (for F1).
    pub hovered: Option<Cmd>,
    /// The help topic of the group whose button or label the pointer is over (for F1).
    pub hovered_group: Option<(usize, usize)>,
}

/// Keytip mode: letters shown on the ribbon after pressing Alt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keytips {
    #[default]
    Off,
    /// Letters on the tabs (and tools).
    Tabs,
    /// Letters on the buttons of tab `n`.
    Buttons(usize),
}

/// Where the help window is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HelpTopic {
    Start,
    /// Tab and group index into `RIBBON`.
    Group(usize, usize),
    Tools,
    Shortcuts,
}

#[derive(Clone, Debug, Default)]
pub struct GpuStatus {
    pub adapter: String,
    pub backend: String,
    /// Bytes in use by this process, and the budget the system gives it.
    pub memory: Option<(u64, u64)>,
    pub textures: usize,
}

pub struct AppState {
    pub docs: Vec<DocState>,
    /// The document the panels and commands act on: the one last used.
    pub active_doc: Option<DocId>,
    next_doc: u64,
    untitled: u32,
    pub tool: ToolId,
    pub tool_settings: HashMap<ToolId, Settings>,
    /// Named sets of tool settings, per tool.
    pub presets: HashMap<ToolId, BTreeMap<String, BTreeMap<String, Value>>>,
    /// The name being typed for a new preset.
    pub preset_name: Option<String>,
    pub prefs: Settings,
    pub palette: Palette,
    pub ribbon: RibbonState,
    /// Commands to run once this frame's UI is done.
    pub queue: Vec<Cmd>,
    /// Documents whose tab must be created.
    pub opened: Vec<DocId>,
    /// Documents whose tab must be removed.
    pub closed: Vec<DocId>,
    pub messages: VecDeque<Message>,
    pub confirm: Option<Confirm>,
    pub new_image: Option<NewImage>,
    pub last_new: NewImage,
    pub export: Option<ExportDialog>,
    /// The format last exported to, offered first next time.
    pub last_export: ExportKind,
    pub prefs_open: bool,
    pub prefs_section: usize,
    pub about_open: bool,
    pub gallery_open: bool,
    pub gallery: crate::gallery::Gallery,
    pub help: Option<HelpTopic>,
    pub renaming: Option<(LayerId, String)>,
    pub reset_panels: bool,
    /// Set when the window may close (unsaved changes were dealt with).
    pub exit_confirmed: bool,
    pub exit_requested: bool,
    /// When settings last changed and have not been written to disk yet.
    pub settings_changed: Option<Instant>,
    pub cursor_canvas: Option<(i64, i64)>,
    pub gpu: GpuStatus,
}

impl AppState {
    pub fn new() -> AppState {
        let mut tool_settings = HashMap::new();
        for t in ToolId::ALL {
            tool_settings.insert(t, Settings::new(t.info().settings));
        }
        AppState {
            docs: Vec::new(),
            active_doc: None,
            next_doc: 1,
            untitled: 0,
            tool: ToolId::Hand,
            tool_settings,
            presets: HashMap::new(),
            preset_name: None,
            prefs: Settings::new(PREFS),
            palette: theme::palette(Scheme::Dark),
            ribbon: RibbonState::default(),
            queue: Vec::new(),
            opened: Vec::new(),
            closed: Vec::new(),
            messages: VecDeque::new(),
            confirm: None,
            new_image: None,
            last_new: NewImage { width: 1920, height: 1080, background: Background::White },
            export: None,
            last_export: ExportKind::Png,
            prefs_open: false,
            prefs_section: 0,
            about_open: false,
            gallery_open: false,
            gallery: crate::gallery::Gallery::default(),
            help: None,
            renaming: None,
            reset_panels: false,
            exit_confirmed: false,
            exit_requested: false,
            settings_changed: None,
            cursor_canvas: None,
            gpu: GpuStatus::default(),
        }
    }

    /// The color scheme in use: the chosen one, or the system's when following it.
    pub fn scheme(&self, ctx: &egui::Context) -> Scheme {
        match ctx.theme() {
            egui::Theme::Dark => Scheme::Dark,
            egui::Theme::Light => Scheme::Light,
        }
    }

    /// Applies the preferences that live in egui's state: colors, sizes, tooltips.
    pub fn apply_prefs(&mut self, ctx: &egui::Context) {
        let choice = match self.prefs.choice("ui.theme").as_str() {
            "light" => egui::ThemePreference::Light,
            "system" => egui::ThemePreference::System,
            _ => egui::ThemePreference::Dark,
        };
        theme::apply(ctx, choice);
        self.palette = theme::palette(self.scheme(ctx));
        ctx.set_zoom_factor((self.prefs.f64("ui.scale") / 100.0) as f32);
        let tooltips = self.prefs.bool("ui.tooltips");
        let delay = self.prefs.f64("ui.tooltip_delay");
        ctx.global_style_mut(|s| {
            s.interaction.tooltip_delay = if tooltips { delay as f32 } else { f32::INFINITY };
        });
    }

    pub fn settings_touched(&mut self) {
        self.settings_changed = Some(Instant::now());
    }

    pub fn active(&self) -> Option<&DocState> {
        self.active_doc.and_then(|id| self.doc(id))
    }

    pub fn active_mut(&mut self) -> Option<&mut DocState> {
        let id = self.active_doc?;
        self.doc_mut(id)
    }

    pub fn doc(&self, id: DocId) -> Option<&DocState> {
        self.docs.iter().find(|d| d.id == id)
    }

    pub fn doc_mut(&mut self, id: DocId) -> Option<&mut DocState> {
        self.docs.iter_mut().find(|d| d.id == id)
    }

    pub fn tool_settings(&self, t: ToolId) -> &Settings {
        &self.tool_settings[&t]
    }

    pub fn message(&mut self, kind: MessageKind, title: impl Into<String>, body: impl Into<String>) {
        self.messages.push_back(Message { kind, title: title.into(), body: body.into() });
    }

    fn add_doc(&mut self, title: String, doc: Document, ids: IdGen, label: &str) -> DocId {
        let id = DocId(self.next_doc);
        self.next_doc += 1;
        self.docs.push(DocState::new(id, title, doc, ids, label));
        self.opened.push(id);
        self.active_doc = Some(id);
        id
    }

    /// Runs `cmd` if it is available now; otherwise explains why not.
    pub fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        if let Err(why) = availability(self, cmd) {
            self.message(MessageKind::Info, cmd.info().name, why);
            return;
        }
        match cmd {
            Cmd::New => self.new_image = Some(self.last_new.clone()),
            Cmd::Open => self.open_dialog(),
            Cmd::Save => self.save_active(false),
            Cmd::SaveAs => self.save_active(true),
            Cmd::Export => {
                let doc = self.active_doc.expect("available");
                self.export = Some(ExportDialog { doc, kind: self.last_export });
            }
            Cmd::Close => {
                let id = self.active_doc.expect("available");
                self.request_close(id);
            }
            Cmd::Preferences => self.prefs_open = true,
            Cmd::Exit => self.exit_requested = true,
            Cmd::Undo => {
                self.active_mut().expect("available").undo();
            }
            Cmd::Redo => {
                self.active_mut().expect("available").redo();
            }
            Cmd::NewLayer => self.new_layer(false),
            Cmd::NewGroup => self.new_layer(true),
            Cmd::Duplicate => self.duplicate_layer(),
            Cmd::DeleteLayer => self.delete_layer(),
            Cmd::MoveUp => self.move_layer(true),
            Cmd::MoveDown => self.move_layer(false),
            Cmd::MergeDown => self.merge_down(),
            Cmd::Flatten => self.flatten(),
            Cmd::ToggleVisible => self.toggle_layer(true),
            Cmd::ToggleLock => self.toggle_layer(false),
            Cmd::Rename => {
                let d = self.active().expect("available");
                let id = d.active_layer.expect("available");
                let name = d.doc().layer(id).map(|l| l.name.clone()).unwrap_or_default();
                self.renaming = Some((id, name));
            }
            Cmd::ZoomIn | Cmd::ZoomOut => {
                let up = cmd == Cmd::ZoomIn;
                let v = &mut self.active_mut().expect("available").view;
                let z = tools::next_preset(v.zoom, up);
                let center = v.viewport * 0.5;
                v.zoom_about(z, center);
            }
            Cmd::ZoomFit => {
                let fit_max = (self.prefs.f64("nav.fit_max") / 100.0) as f32;
                let d = self.active_mut().expect("available");
                let size = Vec2::new(d.doc().canvas.width as f32, d.doc().canvas.height as f32);
                let vp = d.view.viewport;
                d.view.fit(size, vp, fit_max);
            }
            Cmd::Zoom100 => {
                let v = &mut self.active_mut().expect("available").view;
                let vp = v.viewport;
                v.actual_pixels(vp);
            }
            Cmd::ToggleOffCanvas => {
                let shown = self.prefs.f64("canvas.off_canvas") > 0.0;
                let v = if shown { 0.0 } else { 35.0 };
                self.prefs.set("canvas.off_canvas", Value::Float(v));
                self.settings_touched();
            }
            Cmd::ToggleTheme => {
                let next = if self.scheme(ctx) == Scheme::Dark { "light" } else { "dark" };
                self.prefs.set("ui.theme", Value::Choice(next.into()));
                self.apply_prefs(ctx);
                self.settings_touched();
            }
            Cmd::ToggleRibbon => {
                let next = if self.prefs.choice("ui.ribbon") == "full" { "single" } else { "full" };
                self.prefs.set("ui.ribbon", Value::Choice(next.into()));
                self.settings_touched();
            }
            Cmd::CycleToolsPlace => {
                let next = match self.prefs.choice("ui.tools").as_str() {
                    "ribbon" => "left",
                    "left" => "right",
                    _ => "ribbon",
                };
                self.prefs.set("ui.tools", Value::Choice(next.into()));
                self.settings_touched();
            }
            Cmd::ResetPanels => self.reset_panels = true,
            Cmd::Search => self.ribbon.focus_search = true,
            Cmd::Help => self.help = Some(self.help_topic_here()),
            Cmd::Shortcuts => self.help = Some(HelpTopic::Shortcuts),
            Cmd::About => self.about_open = true,
            Cmd::WidgetGallery => self.gallery_open = true,
        }
    }

    /// The help topic for what the pointer is over: a ribbon group, or the start page.
    fn help_topic_here(&self) -> HelpTopic {
        match self.ribbon.hovered_group {
            Some((t, g)) => HelpTopic::Group(t, g),
            None => HelpTopic::Start,
        }
    }

    // ---- Files ------------------------------------------------------------------------

    /// Creates the image described by the New Image dialog.
    pub fn create_image(&mut self, spec: &NewImage) {
        self.last_new = spec.clone();
        let mut ids = IdGen::default();
        let mut doc = Document::new(spec.width, spec.height);
        let image = match spec.background {
            Background::Transparent => RasterImage::new(),
            Background::White => filled(spec.width, spec.height, [1.0, 1.0, 1.0, 1.0]),
        };
        let layer = Layer::new_raster(ids.layer(), ids.object(), "Background", image);
        doc.insert(Position { parent: None, index: 0 }, layer).expect("an empty document takes a layer");
        self.untitled += 1;
        let title = format!("Untitled {}", self.untitled);
        self.add_doc(title, doc, ids, "New image");
    }

    fn open_dialog(&mut self) {
        let files = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("Images and Opal Studio files", &["opal", "png", "jpg", "jpeg"])
            .add_filter("Opal Studio files", &["opal"])
            .add_filter("PNG and JPEG images", &["png", "jpg", "jpeg"])
            .pick_files();
        for path in files.unwrap_or_default() {
            self.open_path(&path);
        }
    }

    /// Opens a file, telling the user if that fails or if anything about it needs saying.
    pub fn open_path(&mut self, path: &Path) {
        // Already open: just show it.
        if let Some(d) = self.docs.iter().find(|d| d.path.as_deref() == Some(path)) {
            self.active_doc = Some(d.id);
            return;
        }
        let title = file_title(path);
        let is_native = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("opal"));
        if is_native {
            match native::load(path) {
                Ok(loaded) => {
                    let id = self.add_doc(title, loaded.doc, loaded.ids, "Open");
                    self.doc_mut(id).expect("just added").path = Some(path.to_owned());
                }
                Err(e) => self.message(MessageKind::Error, "Could not open the file", e.to_string()),
            }
        } else {
            match image_files::import(path) {
                Ok(imported) => {
                    let mut ids = IdGen::default();
                    let mut doc = Document::new(imported.width, imported.height);
                    let layer = Layer::new_raster(ids.layer(), ids.object(), "Background", imported.image);
                    doc.insert(Position { parent: None, index: 0 }, layer).expect("an empty document takes a layer");
                    self.add_doc(title, doc, ids, "Open");
                    for w in imported.warnings {
                        self.message(MessageKind::Warning, format!("Opened {}", file_title(path)), w);
                    }
                }
                Err(e) => self.message(MessageKind::Error, "Could not open the image", e.to_string()),
            }
        }
    }

    fn save_active(&mut self, choose: bool) {
        let id = self.active_doc.expect("available");
        self.save(id, choose);
    }

    /// Saves a document to its file, asking for one if it has none or `choose` is set.
    /// Returns whether it was saved.
    pub fn save(&mut self, id: DocId, choose: bool) -> bool {
        let Some(d) = self.doc_mut(id) else { return false };
        d.commit_pending();
        let path = match (&d.path, choose) {
            (Some(p), false) => p.clone(),
            _ => {
                let mut dialog = rfd::FileDialog::new().set_title("Save As").add_filter("Opal Studio file", &["opal"]);
                dialog = dialog.set_file_name(format!("{}.opal", d.title.trim_end_matches(".opal")));
                match dialog.save_file() {
                    Some(p) => with_extension(p, "opal"),
                    None => return false,
                }
            }
        };
        match native::save(d.history.doc(), &d.ids, &path) {
            Ok(()) => {
                d.saved_at = Some(d.history.current());
                d.title = file_title(&path);
                d.path = Some(path);
                true
            }
            Err(e) => {
                self.message(MessageKind::Error, "Could not save", e.to_string());
                false
            }
        }
    }

    /// Asks where to export, then writes the file.
    pub fn export_to_file(&mut self, spec: &ExportDialog) {
        let Some(d) = self.doc(spec.doc) else { return };
        let stem = d.title.trim_end_matches(".opal").to_owned();
        let (ext, filter) = match spec.kind {
            ExportKind::Png => ("png", "PNG image"),
            ExportKind::Jpeg => ("jpg", "JPEG image"),
        };
        let exts: &[&str] = if spec.kind == ExportKind::Jpeg { &["jpg", "jpeg"] } else { &["png"] };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export")
            .add_filter(filter, exts)
            .set_file_name(format!("{stem}.{ext}"))
            .save_file()
        else {
            return;
        };
        let path = if exts.iter().any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e))) {
            path
        } else {
            with_extension(path, ext)
        };
        let format = match spec.kind {
            ExportKind::Png => ExportFormat::Png { sixteen_bit: self.prefs.bool("export.png_16") },
            ExportKind::Jpeg => {
                let bg = self.prefs.color("export.jpeg_background");
                ExportFormat::Jpeg {
                    quality: self.prefs.f64("export.jpeg_quality").round() as u8,
                    background: [bg.r(), bg.g(), bg.b()],
                }
            }
        };
        let doc = d.doc().clone();
        if let Err(e) = image_files::export(&doc, &path, &format) {
            self.message(MessageKind::Error, "Could not export", e.to_string());
        }
    }

    /// Closes a document, first asking about unsaved changes.
    pub fn request_close(&mut self, id: DocId) {
        match self.doc(id) {
            Some(d) if d.is_dirty() => self.confirm = Some(Confirm::Close(id)),
            Some(_) => self.close_now(id),
            None => {}
        }
    }

    pub fn close_now(&mut self, id: DocId) {
        self.docs.retain(|d| d.id != id);
        self.closed.push(id);
        if self.active_doc == Some(id) {
            self.active_doc = self.docs.last().map(|d| d.id);
        }
    }

    /// The documents with unsaved changes.
    pub fn dirty_docs(&self) -> Vec<DocId> {
        self.docs.iter().filter(|d| d.is_dirty()).map(|d| d.id).collect()
    }

    // ---- Layers -----------------------------------------------------------------------

    /// Where a new layer goes: just above the active layer, in its group.
    fn above_active(d: &DocState) -> Position {
        d.active_layer
            .and_then(|a| d.doc().position(a))
            .map(|p| Position { parent: p.parent, index: p.index + 1 })
            .unwrap_or(Position { parent: None, index: d.doc().layers.len() })
    }

    fn new_layer(&mut self, group: bool) {
        let d = self.active_mut().expect("available");
        let pos = Self::above_active(d);
        let label = if group { "New group" } else { "New layer" };
        let result = d.apply(label, |doc, ids| {
            let id = ids.layer();
            let n = count_layers(doc) + 1;
            let layer = if group {
                Layer::new_group(id, format!("Group {n}"))
            } else {
                Layer::new_empty(id, format!("Layer {n}"))
            };
            doc.insert(pos, layer)
        });
        self.report(result);
    }

    fn duplicate_layer(&mut self) {
        let d = self.active_mut().expect("available");
        let src = d.active_layer.expect("available");
        let result = d.apply("Duplicate layer", |doc, ids| {
            let layer = doc.layer(src).expect("the active layer exists");
            let mut copy = copy_with_new_ids(layer, ids);
            copy.name = format!("{} copy", layer.name);
            let pos = doc.position(src).expect("the active layer exists");
            doc.insert(Position { parent: pos.parent, index: pos.index + 1 }, copy)
        });
        self.report(result);
    }

    fn delete_layer(&mut self) {
        let d = self.active_mut().expect("available");
        let id = d.active_layer.expect("available");
        let result = d.apply("Delete layer", |doc, _| doc.remove(id).map(|_| ()));
        self.report(result);
    }

    fn move_layer(&mut self, up: bool) {
        let d = self.active_mut().expect("available");
        let id = d.active_layer.expect("available");
        let pos = d.doc().position(id).expect("the active layer exists");
        let to = Position { parent: pos.parent, index: if up { pos.index + 1 } else { pos.index - 1 } };
        let result = d.apply(if up { "Move layer up" } else { "Move layer down" }, |doc, _| doc.move_layer(id, to));
        self.report(result);
    }

    /// Moves a layer to `to` (from dragging in the Layers panel).
    pub fn move_layer_to(&mut self, id: LayerId, to: Position) {
        let Some(d) = self.active_mut() else { return };
        let result = d.apply("Move layer", |doc, _| doc.move_layer(id, to));
        self.report(result);
    }

    fn merge_down(&mut self) {
        let d = self.active_mut().expect("available");
        let upper = d.active_layer.expect("available");
        let pos = d.doc().position(upper).expect("the active layer exists");
        let below = d.doc().siblings(pos.parent).expect("its parent exists")[pos.index - 1].id;
        let result = d.apply("Merge down", |doc, ids| {
            let siblings = doc.siblings(pos.parent).expect("its parent exists");
            let pair = [siblings[pos.index - 1].clone(), siblings[pos.index].clone()];
            let image = composite_all(&pair);
            let lower = doc.layer(below).expect("exists");
            let merged = Layer::new_raster(below, ids.object(), lower.name.clone(), image);
            doc.remove(upper)?;
            *doc.layer_mut(below).expect("exists") = merged;
            Ok::<(), opal_core::document::TreeError>(())
        });
        self.report(result);
        if let Some(d) = self.active_mut() {
            d.active_layer = Some(below);
        }
    }

    fn flatten(&mut self) {
        let d = self.active_mut().expect("available");
        let result = d.apply("Flatten image", |doc, ids| {
            let image = composite_all(&doc.layers);
            doc.layers = vec![Arc::new(Layer::new_raster(ids.layer(), ids.object(), "Background", image))];
            Ok::<(), opal_core::document::TreeError>(())
        });
        self.report(result);
    }

    fn toggle_layer(&mut self, visibility: bool) {
        let d = self.active_mut().expect("available");
        let id = d.active_layer.expect("available");
        self.set_layer_flag(id, visibility);
    }

    /// Flips a layer's visibility (`visibility`) or lock.
    pub fn set_layer_flag(&mut self, id: LayerId, visibility: bool) {
        let Some(d) = self.active_mut() else { return };
        let Some(l) = d.doc().layer(id) else { return };
        let label = match (visibility, l.visible, l.locked) {
            (true, true, _) => "Hide layer",
            (true, false, _) => "Show layer",
            (false, _, true) => "Unlock layer",
            (false, _, false) => "Lock layer",
        };
        let result = d.apply(label, |doc, _| {
            let l = doc.layer_mut(id).ok_or(opal_core::document::TreeError::NoSuchLayer(id))?;
            if visibility {
                l.visible = !l.visible;
            } else {
                l.locked = !l.locked;
            }
            Ok::<(), opal_core::document::TreeError>(())
        });
        self.report(result);
    }

    pub fn rename_layer(&mut self, id: LayerId, name: String) {
        let Some(d) = self.active_mut() else { return };
        if d.doc().layer(id).is_none_or(|l| l.name == name) || name.trim().is_empty() {
            return;
        }
        let result = d.apply("Rename layer", |doc, _| {
            doc.layer_mut(id).ok_or(opal_core::document::TreeError::NoSuchLayer(id))?.name = name;
            Ok::<(), opal_core::document::TreeError>(())
        });
        self.report(result);
    }

    fn report<E: std::fmt::Display>(&mut self, r: Result<(), E>) {
        if let Err(e) = r {
            self.message(MessageKind::Error, "That did not work", e.to_string());
        }
    }
}

/// How many layers the document has, groups included (for naming new ones).
fn count_layers(doc: &Document) -> usize {
    let mut n = 0;
    doc.for_each_layer(&mut |_, _| n += 1);
    n
}

/// A copy of `layer` (and, for a group, everything in it) with fresh identifiers. Pixels are
/// shared until changed.
fn copy_with_new_ids(layer: &Layer, ids: &mut IdGen) -> Layer {
    let mut c = layer.clone();
    c.id = ids.layer();
    match &mut c.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                o.id = ids.object();
            }
        }
        LayerContent::Group(children) => {
            *children = children.iter().map(|ch| Arc::new(copy_with_new_ids(ch, ids))).collect();
        }
    }
    c
}

/// A bitmap covering `width × height` from the origin with one premultiplied linear color.
fn filled(width: u32, height: u32, rgba: [f32; 4]) -> RasterImage {
    let area = IRect::from_size(i64::from(width), i64::from(height));
    let full = Arc::new(Tile::filled(pixel_from_f32(rgba)));
    let mut img = RasterImage::new();
    for c in TileCoord::covering(&area, 0) {
        let r = c.rect(0);
        if area.intersect(&r) == r {
            img.set_tile(c, Some(full.clone()));
        } else {
            let part = area.intersect(&r);
            let tile = img.tile_mut(c);
            for y in part.y0..part.y1 {
                for x in part.x0..part.x1 {
                    tile.set((x - r.x0) as u32, (y - r.y0) as u32, pixel_from_f32(rgba));
                }
            }
        }
    }
    img
}

/// The name a file is shown under.
fn file_title(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn with_extension(mut path: PathBuf, ext: &str) -> PathBuf {
    let has = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext));
    if !has {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".");
        name.push(ext);
        path.set_file_name(name);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_image() -> AppState {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White });
        st
    }

    fn ctx() -> egui::Context {
        egui::Context::default()
    }

    #[test]
    fn a_new_white_image_is_white_inside_and_empty_outside() {
        let st = app_with_image();
        let d = st.active().unwrap();
        let img = d.doc().layers[0].rasters().next().unwrap().clone();
        assert_eq!(img.pixel(299, 199), pixel_from_f32([1.0; 4]));
        assert_eq!(img.pixel(300, 0)[3].to_f32(), 0.0, "nothing beyond the canvas");
        assert!(!d.history.can_undo());
    }

    #[test]
    fn layer_commands_build_and_undo() {
        let mut st = app_with_image();
        let c = ctx();
        st.run(Cmd::NewLayer, &c);
        st.run(Cmd::Duplicate, &c);
        let d = st.active().unwrap();
        assert_eq!(d.doc().layers.len(), 3);
        let names: Vec<_> = d.doc().layers.iter().map(|l| l.name.clone()).collect();
        assert_eq!(names, ["Background", "Layer 2", "Layer 2 copy"]);
        assert_eq!(d.active_layer, Some(d.doc().layers[2].id), "the copy is active");
        st.run(Cmd::MoveDown, &c);
        assert_eq!(st.active().unwrap().doc().layers[1].name, "Layer 2 copy");
        st.run(Cmd::Undo, &c);
        st.run(Cmd::Undo, &c);
        assert_eq!(st.active().unwrap().doc().layers.len(), 2);
    }

    #[test]
    fn merge_down_bakes_both_layers_into_the_lower() {
        let mut st = app_with_image();
        let c = ctx();
        st.run(Cmd::NewLayer, &c);
        let d = st.active_mut().unwrap();
        let top = d.active_layer.unwrap();
        d.apply::<()>("paint", |doc, ids| {
            let mut img = RasterImage::new();
            img.set_pixel(0, 0, pixel_from_f32([0.0, 0.0, 0.5, 0.5]));
            *doc.layer_mut(top).unwrap() = Layer::new_raster(top, ids.object(), "Layer 2", img);
            Ok(())
        })
        .unwrap();
        let bottom = d.doc().layers[0].id;
        st.run(Cmd::MergeDown, &c);
        let d = st.active().unwrap();
        assert_eq!(d.doc().layers.len(), 1);
        assert_eq!(d.doc().layers[0].id, bottom);
        assert_eq!(d.active_layer, Some(bottom));
        let px = d.doc().layers[0].rasters().next().unwrap().pixel(0, 0);
        // Half-transparent blue over white.
        let px = opal_core::tile::pixel_to_f32(px);
        let want = [0.5, 0.5, 1.0, 1.0];
        assert!(px.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3), "{px:?}");
    }

    #[test]
    fn unavailable_commands_explain_themselves() {
        let mut st = AppState::new();
        assert_eq!(availability(&st, Cmd::Save), Err("Open or create an image first.".into()));
        st.create_image(&NewImage { width: 10, height: 10, background: Background::Transparent });
        assert!(availability(&st, Cmd::Undo).unwrap_err().contains("Nothing to undo"));
        assert!(availability(&st, Cmd::MergeDown).unwrap_err().contains("no layer below"));
        st.run(Cmd::ToggleLock, &ctx());
        assert!(availability(&st, Cmd::DeleteLayer).unwrap_err().contains("locked"));
        // Running an unavailable command tells the user rather than doing nothing.
        st.run(Cmd::DeleteLayer, &ctx());
        assert_eq!(st.active().unwrap().doc().layers.len(), 1);
        assert!(st.messages.back().unwrap().body.contains("locked"));
    }

    #[test]
    fn duplicates_get_fresh_ids_throughout() {
        let mut ids = IdGen::default();
        let mut g = Layer::new_group(ids.layer(), "g");
        if let LayerContent::Group(c) = &mut g.content {
            c.push(Arc::new(Layer::new_raster(ids.layer(), ids.object(), "x", RasterImage::new())));
        }
        let copy = copy_with_new_ids(&g, &mut ids);
        let child = &copy.children().unwrap()[0];
        assert_ne!(copy.id, g.id);
        assert_ne!(child.id, g.children().unwrap()[0].id);
    }

    #[test]
    fn extensions_are_added_only_when_missing() {
        assert_eq!(with_extension(PathBuf::from("a/b"), "opal"), PathBuf::from("a/b.opal"));
        assert_eq!(with_extension(PathBuf::from("a/b.OPAL"), "opal"), PathBuf::from("a/b.OPAL"));
        assert_eq!(with_extension(PathBuf::from("a/b.png"), "opal"), PathBuf::from("a/b.png.opal"));
    }
}
