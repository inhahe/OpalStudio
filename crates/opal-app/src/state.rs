//! The application's state, and the actions commands run.

use crate::clipboard::Clipboard;
use crate::commands::{Cmd, availability};
use crate::doc::{DocId, DocState};
use crate::image_ops::{ResizeCanvas, ResizeImage, RotateBy, Turn};
use crate::prefs::PREFS;
use crate::settings::{Settings, Value};
use crate::theme::{self, Palette, Scheme};
use crate::tools::{self, ToolId};
use crate::writer::{Failure, Then, Written};
use eframe::egui;
use opal_core::document::{Document, IdGen, Layer, LayerId, Position};
use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::pixel_from_f32;
use opal_io::image_files::{self, ExportFormat};
use opal_io::native;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
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
    /// Pixels per inch.
    pub resolution: f64,
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
    /// The size to export at, the canvas's to begin with.
    pub width: u32,
    pub height: u32,
    pub keep_aspect: bool,
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
    /// The tool whose button the pointer is over (for F1).
    pub hovered_tool: Option<ToolId>,
    /// The help topic of the group whose button or label the pointer is over (for F1).
    pub hovered_group: Option<(usize, usize)>,
}

/// Keytip mode: letters shown on the ribbon after pressing Alt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keytips {
    #[default]
    Off,
    /// Letters on the tabs.
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
    /// The tool chosen: changed by [`AppState::set_tool`].
    tool: ToolId,
    pub tool_settings: HashMap<ToolId, Settings>,
    /// Named sets of tool settings, per tool.
    pub presets: HashMap<ToolId, BTreeMap<String, BTreeMap<String, Value>>>,
    /// The name being typed for a new preset of the chosen tool's settings (let go when
    /// another is chosen).
    pub preset_name: Option<String>,
    pub prefs: Settings,
    pub palette: Palette,
    pub ribbon: RibbonState,
    /// Whether the fields for a box's exact position and size are open beside the transform
    /// or crop box (see `box_fields`).
    pub box_numbers: bool,
    /// Whether this editor has no window (`--headless`, or the one `--mcp` starts): what
    /// would be told in a dialog is printed or answered instead, and nothing waits for one.
    pub windowless: bool,
    /// Commands to run once this frame's UI is done.
    pub queue: Vec<Cmd>,
    /// Documents whose tab must be created.
    pub opened: Vec<DocId>,
    /// A document made active by a command (opened, closed onto, shown again) whose tab must
    /// be brought to the front, so the panels never act on a document that is not on screen.
    pub reveal: Option<DocId>,
    pub messages: VecDeque<Message>,
    /// The last number given an API call answered once its write is done.
    pub next_answer: u64,
    /// The answers to such calls whose writes have finished, by number, until taken.
    pub answered: Vec<(u64, Result<serde_json::Value, String>)>,
    /// The answers being made off this thread for such calls (`image`, `pixels`), by
    /// number: where each will come.
    pub made: Vec<(u64, std::sync::mpsc::Receiver<Result<crate::api::Made, String>>)>,
    /// The shared memory holding pixels a call asked for, by name, until the caller releases
    /// it, and since when.
    pub shared_out: HashMap<String, (crate::shared_memory::Mapping, std::time::Instant)>,
    /// Changes made ready off this thread for such calls (`put_pixels`), by number, waiting
    /// for the user not to be busy to be made.
    pub finishing: VecDeque<(u64, crate::api::Finish)>,
    /// Whether the window should come to the front (a file was handed to it to open).
    pub raise_window: bool,
    /// Scripts run from the editor, and what they printed.
    pub console: crate::console::Console,
    /// What was entered lately in each field remembered by name (see
    /// `NumberField::remember`), the latest first: kept with the settings.
    pub field_entries: std::collections::BTreeMap<String, Vec<String>>,
    /// The connection file of this editor's server, which scripts it runs connect through;
    /// `None` while it serves none (another editor does).
    pub scripts_file: Option<PathBuf>,
    /// Why this editor serves no scripts though no other does (its connection could not be
    /// opened), if that is why.
    pub scripts_unserved: Option<String>,
    /// Whether the Console panel should come to the front (a script was started).
    pub show_console: bool,
    /// Whether scripts shut out should be let in again (the user started one).
    pub let_scripts_in: bool,
    /// Whether Connect an Assistant is open.
    pub assistant_open: bool,
    /// MCP over HTTP while it is served: the port, and the token clients give.
    pub mcp_http: Option<(u16, String)>,
    /// Whether that token should be replaced (clients given it are then refused).
    pub renew_mcp_token: bool,
    /// Why MCP over HTTP is not served though the preference asks for it, if it could not
    /// be; and whether to try again.
    pub mcp_http_trouble: Option<String>,
    pub retry_mcp_http: bool,
    /// The copies on their way to the clipboard, in the order they were started.
    pub copying: VecDeque<crate::paste::Copying>,
    pub confirm: Option<Confirm>,
    pub new_image: Option<NewImage>,
    pub last_new: NewImage,
    pub export: Option<ExportDialog>,
    /// File Info, while it is open.
    pub file_info: Option<crate::file_info::FileInfo>,
    /// File Info's templates, by name.
    pub metadata_templates: std::collections::BTreeMap<String, crate::file_info::Template>,
    /// The format last exported to, offered first next time.
    pub last_export: ExportKind,
    pub prefs_open: bool,
    pub prefs_section: usize,
    pub about_open: bool,
    pub gallery_open: bool,
    pub gallery: crate::gallery::Gallery,
    pub help: Option<HelpTopic>,
    /// A layer being renamed in the Layers panel: its document, the layer, the text so far.
    pub renaming: Option<(DocId, LayerId, String)>,
    pub reset_panels: bool,
    /// The images' panes to be tiled (true) or put back in tabs, as asked (see `compare`).
    pub arrange_images: Option<bool>,
    /// Images opened as references, to be pinned beside the others once their panes exist,
    /// each with the image active when it was asked for.
    pub pin: Vec<(DocId, Option<DocId>)>,
    /// Whether the images shown take the active one's zoom and place as it moves (Lock
    /// Views), and whether they are to now (Match Zoom and Location).
    pub lock_views: bool,
    pub match_views: bool,
    /// The images whose panes are in front, as the panels were last drawn.
    pub shown: Vec<DocId>,
    /// Set when the window may close (unsaved changes were dealt with).
    pub exit_confirmed: bool,
    pub exit_requested: bool,
    /// Saves and exports, written off the interface thread.
    pub writer: crate::writer::Writer,
    /// Quitting waits for files being written ("Save all" in the quit question, or quitting
    /// while they were) and for an edit under way: it goes ahead when they are done, unless a
    /// write fails.
    pub exit_after_work: bool,
    /// Documents to ask about closing once the question on screen is answered.
    pub close_queue: VecDeque<DocId>,
    /// When settings last changed and have not been written to disk yet.
    pub settings_changed: Option<Instant>,
    /// The canvas pixel under the pointer, set by the canvas during the frame.
    pub cursor_canvas: Option<(i64, i64)>,
    /// The position the status bar shows: last frame's `cursor_canvas` (the status bar is
    /// laid out before the canvas reports this frame's).
    pub cursor_shown: Option<(i64, i64)>,
    /// Whether a mouse button pressed on an image is held, set by the canvas during the frame.
    pub canvas_held_now: bool,
    /// Last frame's `canvas_held_now`, which the keys (handled before the canvas is drawn) go
    /// by: while it holds, those that would change the image do nothing.
    pub canvas_held: bool,
    pub gpu: GpuStatus,
    pub clipboard: Box<dyn Clipboard>,
    /// A short note for the status bar, and when it was given (it fades after a while).
    pub notice: Option<(String, Instant)>,
    pub resize_image: Option<ResizeImage>,
    pub resize_canvas: Option<ResizeCanvas>,
    pub rotate_by: Option<RotateBy>,
    /// The Save Selection and Load Selection dialogs, while open.
    pub save_selection: Option<crate::select::SaveSelection>,
    pub load_selection: Option<crate::select::LoadSelection>,
    /// The Grow or Shrink, or Feather, dialog, while open.
    pub modifying: Option<crate::select::Modifying>,
    /// Files dropped on the window and not yet opened, first dropped first (see
    /// [`open_dropped`](Self::open_dropped)).
    pub dropped: VecDeque<PathBuf>,
    /// The window's, which work done elsewhere asks for a frame when it ends (one of its own,
    /// which asks no one, with no window).
    pub repaint: egui::Context,
    /// How the window shows the image (F cycles through them).
    pub screen: ScreenMode,
    /// The script's call being carried out, while one is.
    pub calling: Option<crate::api::Calling>,
    /// The thread the modifier dialogs' previews are worked out on, the latest only: the
    /// editor's own, as its dialogs are (a job given replaces one waiting).
    pub previews: crate::background::LatestJob,
    /// How long an edit is waited for as it begins (see `edits::WAITED_FOR`).
    pub edit_wait: std::time::Duration,
    pub thumbs: crate::thumbs::Thumbnails,
    /// A paste waiting for the clipboard to be read.
    pub reading: Option<Reading>,
    /// Files being read for Open, Place or Open as Layers, or waiting to be used, in the order
    /// asked for (see `opening`).
    pub file_readings: Vec<crate::opening::FileReading>,
    /// The memory images' tiles may take before going to disk, as last given to the store.
    tile_budget: Option<u64>,
    /// The memory this computer has, if known.
    memory_total: Option<u64>,
    /// Whether tiles failing to go to disk has been said.
    told_store_trouble: bool,
}

/// How long the status bar shows a notice.
pub const NOTICE_SHOWN: std::time::Duration = std::time::Duration::from_secs(6);

/// A paste whose clipboard reading runs on a worker thread.
pub struct Reading {
    pub rx: std::sync::mpsc::Receiver<crate::clipboard::ReadResult>,
    /// What the reading gave, once it has come back (at once, for a copy made here), while
    /// the paste waits to be placed.
    pub result: Option<crate::clipboard::ReadResult>,
    pub in_place: bool,
    /// The document it goes to: the one active when it was asked for, as it was then.
    pub target: Target,
    /// The status-bar notice this paste put up, taken down when it is placed.
    pub notice: String,
    /// The API call the paste answers once it is placed, if it was one's.
    pub answer: Option<PasteAnswer>,
}

/// The image what is asked for now goes into later (a paste, files read), as the user had it
/// then: its transform box (its session's serial), and its crop box. A box opened (or a crop
/// box changed) since is the user's newer work, which what goes in waits for rather than
/// ending it (see [`AppState::waits_for`]).
#[derive(Clone, Copy, Debug)]
pub struct Target {
    /// The image; none, a new one.
    pub id: Option<DocId>,
    box_then: Option<u64>,
    crop_then: Option<crate::edit_box::EditBox>,
}

impl AppState {
    /// Image `id` (none: a new one) as the user has it now (see [`Target`]).
    pub(crate) fn target_now(&self, id: Option<DocId>) -> Target {
        let d = id.and_then(|t| self.doc(t));
        Target {
            id,
            box_then: d.and_then(|d| d.transform.as_ref()).map(|s| s.serial),
            crop_then: d.and_then(|d| d.crop),
        }
    }

    /// What something going into `target` must wait for, if anything: a dialog closing, the
    /// edit under way on the image finishing, a box the user opened (or a crop box changed)
    /// since it was asked for.
    pub(crate) fn waits_for(&self, target: &Target) -> Option<&'static str> {
        if self.dialog_open() {
            return Some("the dialog closes");
        }
        let d = target.id.and_then(|t| self.doc(t))?;
        if d.busy.is_some() {
            Some("the edit under way on the image finishes")
        } else if d.transform.as_ref().is_some_and(|s| Some(s.serial) != target.box_then) {
            Some("the transform box closes")
        } else if d.crop.is_some() && d.crop != target.crop_then {
            Some("you crop or leave the Crop tool")
        } else {
            None
        }
    }
}

/// An API call a paste answers: its number, where it puts the box, whether it goes into
/// the active layer, and the call's groups (a script run, a step), which the paste's
/// history state joins.
pub struct PasteAnswer {
    pub n: u64,
    pub place: crate::api::BoxArgs,
    pub into_active: bool,
    pub groups: Vec<u64>,
}

impl AppState {
    /// The tool chosen.
    pub fn tool(&self) -> ToolId {
        self.tool
    }

    /// Chooses `tool`; a preset's name being typed for another is let go.
    pub fn set_tool(&mut self, tool: ToolId) {
        if tool != self.tool {
            self.preset_name = None;
        }
        self.tool = tool;
    }

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
            reveal: None,
            messages: VecDeque::new(),
            next_answer: 0,
            answered: Vec::new(),
            made: Vec::new(),
            shared_out: HashMap::new(),
            finishing: VecDeque::new(),
            raise_window: false,
            console: crate::console::Console::default(),
            field_entries: std::collections::BTreeMap::new(),
            scripts_file: None,
            show_console: false,
            scripts_unserved: None,
            let_scripts_in: false,
            assistant_open: false,
            mcp_http: None,
            renew_mcp_token: false,
            mcp_http_trouble: None,
            retry_mcp_http: false,
            copying: VecDeque::new(),
            confirm: None,
            box_numbers: false,
            windowless: false,
            new_image: None,
            last_new: NewImage {
                width: 1920,
                height: 1080,
                background: Background::White,
                resolution: opal_core::units::DEFAULT_PPI,
            },
            export: None,
            file_info: None,
            metadata_templates: std::collections::BTreeMap::new(),
            last_export: ExportKind::Png,
            prefs_open: false,
            prefs_section: 0,
            about_open: false,
            gallery_open: false,
            gallery: crate::gallery::Gallery::default(),
            help: None,
            renaming: None,
            reset_panels: false,
            arrange_images: None,
            pin: Vec::new(),
            lock_views: false,
            match_views: false,
            shown: Vec::new(),
            exit_confirmed: false,
            exit_requested: false,
            writer: crate::writer::Writer::default(),
            exit_after_work: false,
            close_queue: VecDeque::new(),
            settings_changed: None,
            cursor_canvas: None,
            cursor_shown: None,
            canvas_held_now: false,
            canvas_held: false,
            gpu: GpuStatus::default(),
            // Tests never touch the real clipboard.
            #[cfg(not(test))]
            clipboard: Box::new(crate::clipboard::SystemClipboard::default()),
            #[cfg(test)]
            clipboard: Box::new(crate::clipboard::TestClipboard::default()),
            notice: None,
            resize_image: None,
            resize_canvas: None,
            rotate_by: None,
            save_selection: None,
            load_selection: None,
            modifying: None,
            dropped: VecDeque::new(),
            repaint: egui::Context::default(),
            screen: ScreenMode::Standard,
            calling: None,
            previews: crate::background::LatestJob::new("selection previews"),
            edit_wait: crate::edits::WAITED_FOR,
            thumbs: Default::default(),
            reading: None,
            file_readings: Vec::new(),
            tile_budget: None,
            memory_total: crate::memory::total(),
            told_store_trouble: false,
        }
    }

    /// Shows `text` in the status bar for a while (see [`NOTICE_SHOWN`]).
    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    /// Whether the status bar shows no notice now.
    pub fn notice_free(&self) -> bool {
        self.notice.as_ref().is_none_or(|(_, at)| at.elapsed() >= NOTICE_SHOWN)
    }

    /// Whether work that a call will be answered with once it is done is under way: a
    /// write, a copy, a paste or files being read, an answer being made off this thread, a
    /// change made ready waiting to be made, or an answer made and not yet passed on.
    pub fn work_under_way(&self) -> bool {
        !self.answered.is_empty()
            || self.writer.busy()
            || self.reading.is_some()
            || self.reading_files()
            || !self.copying.is_empty()
            || !self.made.is_empty()
            || !self.finishing.is_empty()
            || self.editing()
    }

    /// Whether a dialog is open, so keys and the canvas belong to it. With no window, none
    /// is ever shown, nor waited for.
    pub fn dialog_open(&self) -> bool {
        !self.windowless
            && (self.confirm.is_some()
                || !self.messages.is_empty()
                || self.new_image.is_some()
                || self.export.is_some()
                || self.resize_image.is_some()
                || self.resize_canvas.is_some()
                || self.rotate_by.is_some()
                || self.save_selection.is_some()
                || self.load_selection.is_some()
                || self.modifying.is_some()
                || self.file_info.is_some())
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

    /// What is done between frames, with or without a window: the edits that have finished
    /// applied (`edits`), the copies and writes that have finished reported, files read used
    /// (`opening`), and a paste whose clipboard has been read placed
    /// (`pointer_held` as for [`poll_writes`](Self::poll_writes) and
    /// [`poll_paste`](Self::poll_paste)), each image's history kept within the memory the preferences give it, and the tiles'
    /// share of memory kept to (and trouble writing them to disk told of).
    pub fn between_frames(&mut self, pointer_held: bool) {
        self.poll_edits();
        self.poll_copies();
        self.poll_writes(pointer_held);
        self.poll_paste(pointer_held);
        self.poll_files(pointer_held);
        let budget = crate::prefs::bytes(&self.prefs, "history.memory");
        for d in &mut self.docs {
            d.set_history_budget(budget);
        }
        self.apply_tile_budget();
    }

    /// Lets images' tiles take the share of memory the preference says, before they go to
    /// disk (work that needs memory caps them lower while it runs: `memory::Allowance`); and
    /// says once if they could not go.
    pub fn apply_tile_budget(&mut self) {
        let budget = tile_budget(self.prefs.f64("memory.tiles"), self.memory_total);
        if budget != self.tile_budget {
            self.tile_budget = budget;
            opal_core::store::set_budget(budget);
        }
        if !self.told_store_trouble
            && let Some(why) = opal_core::store::status().trouble
        {
            self.told_store_trouble = true;
            self.message(
                MessageKind::Warning,
                "Images can no longer go to disk",
                format!(
                    "{why}. Images now stay in memory (but for parts already on disk), so working on more than it \
                     holds will be refused."
                ),
            );
        }
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
        self.show_doc(id);
        id
    }

    /// Makes `id` the active document and brings its tab to the front.
    pub fn show_doc(&mut self, id: DocId) {
        self.active_doc = Some(id);
        self.reveal = Some(id);
    }

    /// Runs `cmd` if it is available now; otherwise explains why not.
    pub fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        if let Err(why) = availability(self, cmd) {
            self.message(MessageKind::Info, cmd.info().name, why);
            return;
        }
        match cmd {
            Cmd::AlignLeft
            | Cmd::AlignHorizontalCenters
            | Cmd::AlignRight
            | Cmd::AlignTop
            | Cmd::AlignVerticalCenters
            | Cmd::AlignBottom
            | Cmd::DistributeHorizontalCenters
            | Cmd::DistributeVerticalCenters
            | Cmd::DistributeHorizontalSpace
            | Cmd::DistributeVerticalSpace => {
                let how = crate::arrange::Arrange::of(cmd).expect("one of them");
                if let Err(why) = self.arrange(how, crate::arrange::To::chosen(self)) {
                    self.message(MessageKind::Info, cmd.info().name, why);
                }
            }
            Cmd::New => self.new_image = Some(self.last_new.clone()),
            Cmd::Open => self.open_dialog(ctx),
            Cmd::Save => self.save_active(false, ctx),
            Cmd::SaveAs => self.save_active(true, ctx),
            Cmd::Export => {
                let doc = self.active_doc.expect("available");
                let c = self.active().expect("available").doc().canvas;
                self.export = Some(ExportDialog {
                    doc,
                    kind: self.last_export,
                    width: c.width,
                    height: c.height,
                    keep_aspect: true,
                });
            }
            Cmd::Close => {
                let id = self.active_doc.expect("available");
                self.request_close(id);
            }
            Cmd::Preferences => self.prefs_open = true,
            Cmd::RunScript => self.run_script_dialog(ctx),
            Cmd::ConnectAssistant => self.assistant_open = true,
            Cmd::Exit => self.exit_requested = true,
            Cmd::Undo => {
                self.active_mut().expect("available").undo();
            }
            Cmd::Copy => self.copy(false, ctx),
            Cmd::CopyMerged => self.copy(true, ctx),
            Cmd::Paste => self.paste(false, ctx),
            Cmd::PasteInPlace => self.paste(true, ctx),
            Cmd::Place => self.place_dialog(ctx),
            Cmd::OpenAsLayers => self.open_as_layers_dialog(ctx),
            Cmd::Transform => self.transform(),
            Cmd::TransformAgain => self.transform_again(false),
            Cmd::TransformAgainCopy => self.transform_again(true),
            Cmd::ReapplyPlacement => {
                let sampling = crate::prefs::sampling(&self.prefs, "transform");
                let d = self.active_mut().expect("available");
                let layer = d.active_layer().expect("available");
                if let Err(why) = d.reapply_placement(layer, sampling) {
                    self.message(MessageKind::Info, "Re-apply Placement", why);
                }
            }
            Cmd::ReapplyAllPlacements => {
                let sampling = crate::prefs::sampling(&self.prefs, "transform");
                let d = self.active_mut().expect("available");
                let done = d.reapply_all_placements(sampling);
                let s = |n: usize| if n == 1 { "" } else { "s" };
                let mut said = vec![format!("{} placement{} re-applied.", done.done, s(done.done))];
                said.extend(done.notes);
                if done.left.is_empty() {
                    self.notify(said.join(" "));
                } else {
                    let name =
                        |l| d.doc().layer(l).map_or_else(|| format!("layer {}", l.0), |x| format!("“{}”", x.name));
                    let left: Vec<String> = done.left.iter().map(|(l, why)| format!("{}: {why}.", name(*l))).collect();
                    let n = left.len();
                    let not = format!("{n} {} not:\n{}", if n == 1 { "was" } else { "were" }, left.join("\n"));
                    self.message(MessageKind::Warning, "Re-apply All Placements", format!("{} {not}", said.join(" ")));
                }
            }
            Cmd::CropToContent => self.crop_to_content(),
            Cmd::SelectAll => self.select_all(),
            Cmd::Deselect => self.deselect(),
            Cmd::Reselect => self.reselect(),
            Cmd::SelectLayerPixels => {
                let layer = self.active().and_then(|d| d.active_layer()).expect("available");
                self.select_layer_pixels(layer, opal_core::mask::Combine::Replace);
            }
            Cmd::InvertSelection => self.invert_selection(),
            Cmd::TransformSelection => self.transform_selection(),
            Cmd::GrowSelection | Cmd::FeatherSelection | Cmd::SmoothSelection | Cmd::BorderSelection => {
                self.modify_selection(cmd, ctx)
            }
            Cmd::Clear => self.clear_selected(),
            Cmd::Cut => {
                self.cut(None, ctx);
            }
            Cmd::CropToSelection => self.crop_to_selection(),
            Cmd::ResizeImage => {
                let doc = self.active().expect("available").doc();
                let (c, resolution) = (doc.canvas, doc.resolution);
                self.resize_image = Some(ResizeImage {
                    width: c.width,
                    height: c.height,
                    keep_aspect: true,
                    resolution,
                    keep_print_size: false,
                });
            }
            Cmd::ResizeCanvas => {
                let c = self.active().expect("available").doc().canvas;
                self.resize_canvas = Some(ResizeCanvas {
                    width: c.width,
                    height: c.height,
                    anchor: [1, 1],
                    by_margins: false,
                    margins: [0; 4],
                });
            }
            Cmd::RotateBy => self.rotate_by = Some(RotateBy { degrees: 0.0, grow: true }),
            Cmd::SaveSelection => {
                let name = crate::select::next_selection_name(self.active().expect("available").doc());
                self.save_selection = Some(crate::select::SaveSelection { name });
            }
            Cmd::LoadSelection => {
                let saved = &self.active().expect("available").doc().saved;
                let name = saved.first().expect("available").name.clone();
                self.load_selection = Some(crate::select::LoadSelection {
                    name,
                    combine: opal_core::mask::Combine::Replace,
                    invert: false,
                });
            }
            Cmd::RotateCw => self.turn(Turn::Clockwise),
            Cmd::RotateCcw => self.turn(Turn::Counterclockwise),
            Cmd::Rotate180 => self.turn(Turn::Half),
            Cmd::FlipHorizontal => self.turn(Turn::FlipHorizontal),
            Cmd::FlipVertical => self.turn(Turn::FlipVertical),
            Cmd::FileInfo => self.open_file_info(),
            Cmd::Redo => {
                self.active_mut().expect("available").redo();
            }
            Cmd::NewLayer => self.new_layer(false),
            Cmd::NewGroup => self.new_layer(true),
            Cmd::Duplicate => {
                self.duplicate_layers();
            }
            Cmd::LayerViaCopy => self.layer_via(false),
            Cmd::LayerViaCut => self.layer_via(true),
            Cmd::GroupLayers => self.group_layers(),
            Cmd::DeleteLayer => self.delete_layers(),
            Cmd::BringToFront => self.order_layers(crate::layer_ops::Order::Front),
            Cmd::SendToBack => self.order_layers(crate::layer_ops::Order::Back),
            Cmd::ReverseLayers => self.order_layers(crate::layer_ops::Order::Reverse),
            Cmd::SelectAllLayers => {
                let d = self.active_mut().expect("available");
                d.select_all_layers();
            }
            Cmd::DeselectLayers => {
                let d = self.active_mut().expect("available");
                d.deselect_layers();
            }
            Cmd::MoveUp => self.move_layer(true),
            Cmd::MoveDown => self.move_layer(false),
            Cmd::MergeDown => self.merge_down(),
            Cmd::MergeSelected => self.merge_selected(),
            Cmd::MergeVisible => self.merge_visible(),
            Cmd::MergeGroup => self.merge_group(),
            Cmd::StampVisible => self.stamp_visible(),
            Cmd::Flatten => self.flatten(),
            Cmd::Rasterize => self.rasterize(),
            Cmd::ToggleVisible => self.toggle_layer(true),
            Cmd::ToggleLock => self.toggle_layer(false),
            Cmd::Rename => {
                let d = self.active().expect("available");
                let id = d.active_layer().expect("available");
                let name = d.doc().layer(id).map(|l| l.name.clone()).unwrap_or_default();
                self.renaming = Some((d.id, id, name));
            }
            Cmd::ZoomIn | Cmd::ZoomOut => {
                let up = cmd == Cmd::ZoomIn;
                let v = &mut self.active_mut().expect("available").view;
                let z = tools::next_preset(v.zoom, up);
                let center = v.viewport * 0.5;
                v.zoom_about(z, center);
            }
            Cmd::RotateViewLeft | Cmd::RotateViewRight => {
                let step = self.tool_settings(ToolId::RotateView).f64("rotate_view.step") as f32;
                let by = if cmd == Cmd::RotateViewLeft { -step } else { step };
                let v = &mut self.active_mut().expect("available").view;
                v.turn_to(v.angle + by);
            }
            Cmd::ResetRotation => self.active_mut().expect("available").view.turn_to(0.0),
            Cmd::ZoomFit => {
                let fit_max = (self.prefs.f64("nav.fit_max") / 100.0) as f32;
                let d = self.active_mut().expect("available");
                let canvas = crate::doc::canvas_rect(&d.doc().canvas);
                let vp = d.view.viewport;
                d.view.fit(canvas, vp, fit_max);
            }
            Cmd::Zoom100 => {
                let v = &mut self.active_mut().expect("available").view;
                let vp = v.viewport;
                v.actual_pixels(vp);
            }
            Cmd::ToggleOffCanvas => {
                let shown = self.prefs.bool("canvas.off_canvas_shown");
                self.prefs.set("canvas.off_canvas_shown", Value::Bool(!shown));
                self.settings_touched();
            }
            Cmd::ToggleRulers => {
                let shown = self.prefs.bool("canvas.rulers");
                self.prefs.set("canvas.rulers", Value::Bool(!shown));
                self.settings_touched();
            }
            Cmd::ToggleSelectionOverlay => {
                let shown = self.prefs.bool("selection.overlay");
                self.prefs.set("selection.overlay", Value::Bool(!shown));
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
            Cmd::TileImages => self.arrange_images = Some(true),
            Cmd::TabImages => self.arrange_images = Some(false),
            Cmd::MatchViews => self.match_views = true,
            Cmd::LockViews => {
                self.lock_views = !self.lock_views;
                // Locked, they are matched at once.
                self.match_views = self.lock_views;
            }
            Cmd::BeforeAfter => {
                let d = self.active_mut().expect("available");
                d.compare = match d.compare.take() {
                    Some(_) => None,
                    None => Some(crate::doc::Compare::new(d.history.root())),
                };
                d.revision += 1;
            }
            Cmd::OpenReference => self.open_reference_dialog(ctx),
            Cmd::ScreenMode => {
                self.screen = self.screen.next();
                match self.screen {
                    ScreenMode::Standard => {}
                    ScreenMode::FullScreen => self.notify("Full screen: F again shows the image alone."),
                    ScreenMode::ImageAlone => self.notify("The image alone: F brings the window back."),
                }
            }
            Cmd::Search => self.ribbon.focus_search = true,
            Cmd::Help => self.help = Some(self.help_topic_here()),
            Cmd::Shortcuts => self.help = Some(HelpTopic::Shortcuts),
            Cmd::About => self.about_open = true,
            Cmd::WidgetGallery => self.gallery_open = true,
        }
    }

    /// The help topic for what the pointer is over: a tool, a command (wherever its button
    /// is), a ribbon group, or else the start page.
    fn help_topic_here(&self) -> HelpTopic {
        if self.ribbon.hovered_tool.is_some() {
            return HelpTopic::Tools;
        }
        if let Some(cmd) = self.ribbon.hovered {
            // Its group on the tab shown, if it is there; else the first group that has it.
            let groups: Vec<(usize, usize)> = crate::commands::RIBBON
                .iter()
                .enumerate()
                .flat_map(|(t, tab)| tab.groups.iter().enumerate().map(move |(g, group)| (t, g, group)))
                .filter(|(_, _, group)| group.cmds.contains(&cmd))
                .map(|(t, g, _)| (t, g))
                .collect();
            if let Some(&(t, g)) = groups.iter().find(|(t, _)| *t == self.ribbon.tab).or(groups.first()) {
                return HelpTopic::Group(t, g);
            }
        }
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
        doc.resolution = spec.resolution;
        let image = match spec.background {
            Background::Transparent => RasterImage::new(),
            Background::White => RasterImage::filled(
                &IRect::from_size(i64::from(spec.width), i64::from(spec.height)),
                pixel_from_f32([1.0; 4]),
            ),
        };
        let layer = Layer::new_raster(ids.layer(), ids.object(), "Background", image);
        doc.insert(Position { parent: None, index: 0 }, layer).expect("an empty document takes a layer");
        self.new_untitled(doc, ids, "New image");
    }

    /// Opens `doc` as a new, untitled image.
    pub(crate) fn new_untitled(&mut self, doc: Document, ids: IdGen, label: &str) {
        self.untitled += 1;
        let title = format!("Untitled {}", self.untitled);
        self.add_doc(title, doc, ids, label);
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        let files = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("Images and Opal Studio files", &["opal", "png", "jpg", "jpeg"])
            .add_filter("Opal Studio files", &["opal"])
            .add_filter("PNG and JPEG images", &["png", "jpg", "jpeg"])
            .pick_files();
        self.open_later(files.unwrap_or_default(), None, ctx);
    }

    /// Opens the files dropped on the window, in the order dropped, while no dialog is open
    /// (one opened behind a dialog would become the image it acts on): read in the
    /// background (see [`open_later`](Self::open_later)).
    pub fn open_dropped(&mut self, ctx: &egui::Context) {
        if self.dialog_open() {
            return;
        }
        let paths: Vec<PathBuf> = self.dropped.drain(..).collect();
        if !paths.is_empty() {
            self.open_later(paths, None, ctx);
        }
    }

    /// Opens `paths` as Open does, each read on a worker thread (see
    /// [`read_then`](Self::read_then)), so the window keeps answering; one open already just
    /// shown. For call `answer` (its number and groups), answers with the image last opened
    /// or shown (and what reading said), or why it could not be opened; else tells the user.
    pub(crate) fn open_later(&mut self, paths: Vec<PathBuf>, answer: Option<(u64, Vec<u64>)>, ctx: &egui::Context) {
        let mut shown = None;
        let mut to_read = Vec::new();
        for path in paths {
            match self.open_already(&path) {
                Some(id) => {
                    self.show_doc(id);
                    shown = Some(id);
                }
                None => to_read.push(path),
            }
        }
        if to_read.is_empty() {
            if let Some((n, _)) = answer {
                self.answered.push((n, Ok(shown.map_or_else(|| json!({}), |id| json!({ "document": id.0 })))));
            }
            return;
        }
        // A notification's answer nobody reads: what opening says goes to the user.
        let scripted = answer.is_some() && self.calling.as_ref().is_none_or(|c| c.heard);
        let then: crate::opening::Then = Box::new(move |st: &mut AppState, read| {
            let (mut last, mut notes, mut failed) = (shown, Vec::new(), Vec::new());
            for (path, file) in read {
                match st.open_read(&path, file) {
                    Ok((id, said)) => {
                        last = Some(id);
                        if scripted {
                            notes.extend(said);
                        } else {
                            for w in said {
                                st.message(MessageKind::Warning, format!("Opened {}", file_title(&path)), w);
                            }
                        }
                    }
                    Err(why) => failed.push(why),
                }
            }
            if !failed.is_empty() && (scripted || last.is_none()) {
                return Err(format!("Could not open the file: {}", failed.join("\n")));
            }
            for why in failed {
                st.message(MessageKind::Error, "Could not open the file", why);
            }
            let value = last.map_or_else(|| json!({}), |id| json!({ "document": id.0 }));
            Ok(crate::api::with_notes(value, notes))
        });
        self.read_then(to_read, None, answer, then, ctx);
    }

    /// The image whose file `path` is, if it is open.
    fn open_already(&self, path: &Path) -> Option<DocId> {
        self.docs.iter().find(|d| d.path.as_deref() == Some(path)).map(|d| d.id)
    }

    /// Opens a file, reading it here (see [`open_later`](Self::open_later) for reading it in
    /// the background), telling the user if that fails or if anything about it needs saying.
    pub fn open_path(&mut self, path: &Path) {
        if let Some(id) = self.open_already(path) {
            self.show_doc(id);
            return;
        }
        match self.open_read(path, crate::opening::read_file(path)) {
            Ok((_, said)) => {
                for w in said {
                    self.message(MessageKind::Warning, format!("Opened {}", file_title(path)), w);
                }
            }
            Err(why) => self.message(MessageKind::Error, "Could not open the file", why),
        }
    }

    /// Opens the file at `path` as `file` (what reading it gave) holds it, as an image of its
    /// own (or shows the image it is open as already): the image, and what should be said of
    /// it; or why it could not be read.
    pub(crate) fn open_read(
        &mut self,
        path: &Path,
        file: Result<crate::opening::ReadFile, String>,
    ) -> Result<(DocId, Vec<String>), String> {
        // Asked for again while it was being read, it is the image it is open as now.
        if let Some(id) = self.open_already(path) {
            self.show_doc(id);
            return Ok((id, Vec::new()));
        }
        let title = file_title(path);
        match file? {
            crate::opening::ReadFile::Native(loaded) => {
                let loaded = *loaded;
                let id = self.add_doc(title, loaded.doc, loaded.ids, "Open");
                self.doc_mut(id).expect("just added").path = Some(path.to_owned());
                Ok((id, Vec::new()))
            }
            crate::opening::ReadFile::Image(imported) => {
                let imported = *imported;
                let mut ids = IdGen::default();
                let (mut doc, said) = self.image_for(imported.width, imported.height, imported.resolution);
                let mut warnings = imported.warnings;
                warnings.extend(said);
                let layer = Layer::new_raster(ids.layer(), ids.object(), "Background", imported.image);
                doc.insert(Position { parent: None, index: 0 }, layer).expect("an empty document takes a layer");
                doc.metadata = imported.metadata;
                let id = self.add_doc(title, doc, ids, "Open");
                Ok((id, warnings))
            }
        }
    }

    /// A new image `width × height` for a file's pixels, at the resolution across the file
    /// gives (`resolution`, across and down, if it gives one that can be), else the default;
    /// and what should be said: that the file's pixels are not square, which Opal Studio's
    /// are.
    pub(crate) fn image_for(&self, width: u32, height: u32, resolution: Option<[f64; 2]>) -> (Document, Vec<String>) {
        let mut doc = Document::new(width, height);
        doc.resolution = self.default_resolution();
        let mut said = Vec::new();
        if let Some([x, y]) = resolution.filter(|&[x, _]| opal_core::units::valid_ppi(x)) {
            doc.resolution = x;
            if (x - y).abs() > x.max(y) * 1e-3 {
                said.push(format!(
                    "This image's pixels are not square ({x} by {y} per inch). Opal Studio's are, so it \
                     takes the resolution across ({x} pixels per inch) for both."
                ));
            }
        }
        (doc, said)
    }

    fn save_active(&mut self, choose: bool, ctx: &egui::Context) {
        let id = self.active_doc.expect("available");
        self.save(id, choose, Then::Nothing, ctx);
    }

    /// Saves a document to its file, asking for one if it has none or `choose` is set; once
    /// it is written (off the interface thread), `then` is done. Returns whether the write
    /// was started (not if no file was chosen).
    pub fn save(&mut self, id: DocId, choose: bool, then: Then, ctx: &egui::Context) -> bool {
        let Some(d) = self.doc_mut(id) else { return false };
        let path = match (&d.path, choose) {
            (Some(p), false) => p.clone(),
            _ => {
                let mut dialog = rfd::FileDialog::new().set_title("Save As").add_filter("Opal Studio file", &["opal"]);
                dialog = dialog.set_file_name(format!("{}.opal", d.stem()));
                match dialog.save_file() {
                    Some(p) => with_extension(p, "opal"),
                    None => return false,
                }
            }
        };
        self.save_to(id, path, then, ctx);
        true
    }

    /// Saves document `id` to `path` (off the interface thread), then does `then`. The image
    /// takes the file at once, so a save asked for before this one is written goes to it too.
    pub fn save_to(&mut self, id: DocId, path: PathBuf, then: Then, ctx: &egui::Context) {
        let Some(d) = self.doc_mut(id) else { return };
        // An edit in progress that changes nothing a file keeps (a selection's, as the
        // modifier dialogs' previews are) stays in progress; an open box is applied.
        if d.transform.is_some() || d.pending.as_ref().is_some_and(|p| !p.doc.saves_as(d.history.doc())) {
            d.commit_pending();
        }
        let before = (d.path.replace(path.clone()), std::mem::replace(&mut d.title, file_title(&path)));
        // The document as it is now: it can be edited while this is written.
        let (doc, ids, at) = (d.history.doc().clone(), d.ids.clone(), d.history.current());
        let written = Written::Save { doc: id, at, path: path.clone(), before };
        let job = Box::new(move |p: &_| native::save(&doc, &ids, &path, p).map_err(|e| e.to_string()));
        self.writer.write(written, then, job, ctx);
    }

    /// Reports the saves and exports done since the last frame, and does what was to follow
    /// them. A close waits for a free moment: no dialog, no box open on the image, and the
    /// button not held (`pointer_held`), so its question never lands in the middle of a drag.
    pub fn poll_writes(&mut self, pointer_held: bool) {
        let finished = self.writer.finished();
        // A failure stops a quit waiting for the writes, so that it is seen (but not one a
        // script asked for, which only the script is told of).
        if finished.iter().any(|f| f.result.is_err() && !matches!(f.then, Then::Answer(_))) {
            self.exit_after_work = false;
        }
        let mut finished = VecDeque::from(finished);
        while let Some(f) = finished.pop_front() {
            match (f.written, f.result) {
                (Written::Save { doc, at, path, .. }, Ok(())) => {
                    // The image may have been closed meanwhile; what was to follow still is.
                    // It is saved if it still goes to this file (not if saved as another since).
                    let title = file_title(&path);
                    if let Some(d) = self.doc_mut(doc)
                        && d.path.as_ref() == Some(&path)
                    {
                        d.saved_at = Some(at);
                    }
                    match f.then {
                        // Changed while it was written: asked about again.
                        Then::Close => self.close_queue.push_back(doc),
                        Then::Nothing => self.notify(format!("Saved \"{title}\".")),
                        Then::Answer(n) => {
                            self.notify(format!("Saved \"{title}\"."));
                            self.answered.push((n, Ok(serde_json::json!({ "path": path.display().to_string() }))));
                        }
                    }
                }
                (Written::Save { doc, path, before, .. }, Err(failure)) => {
                    // The image gets back the file it had before; if it has been given
                    // another since, the save to that one gives this back if it fails too.
                    match self.doc_mut(doc) {
                        Some(d) if d.path.as_ref() == Some(&path) => (d.path, d.title) = before,
                        Some(_) => {
                            crate::writer::pass_on(finished.iter_mut().map(|f| &mut f.written), doc, &path, &before);
                            self.writer.pass_on(doc, &path, &before);
                        }
                        None => {}
                    }
                    match (failure, f.then) {
                        (Failure::Cancelled, Then::Answer(n)) => {
                            self.answered.push((n, Err("The save was cancelled; the file was left as it was.".into())))
                        }
                        (Failure::Error(e), Then::Answer(n)) => {
                            self.answered.push((n, Err(format!("Could not save: {e}"))))
                        }
                        (Failure::Cancelled, _) => self.notify(format!(
                            "Saving \"{}\" was cancelled; the file was left as it was.",
                            file_title(&path)
                        )),
                        (Failure::Error(e), _) => self.message(MessageKind::Error, "Could not save", e),
                    }
                }
                (Written::Export { path, notes }, Ok(())) => {
                    self.notify(format!("Exported \"{}\".", file_title(&path)));
                    match f.then {
                        Then::Answer(n) => {
                            let answer = json!({ "path": path.display().to_string() });
                            self.answered.push((n, Ok(crate::api::with_notes(answer, notes))));
                        }
                        _ => {
                            for note in notes {
                                self.message(MessageKind::Warning, format!("Exported {}", file_title(&path)), note);
                            }
                        }
                    }
                }
                (Written::Export { path, .. }, Err(failure)) => match (failure, f.then) {
                    (Failure::Cancelled, Then::Answer(n)) => {
                        self.answered.push((n, Err("The export was cancelled; the file was left as it was.".into())))
                    }
                    (Failure::Error(e), Then::Answer(n)) => {
                        self.answered.push((n, Err(format!("Could not export: {e}"))))
                    }
                    (Failure::Cancelled, _) => self.notify(format!(
                        "Exporting \"{}\" was cancelled; the file was left as it was.",
                        file_title(&path)
                    )),
                    (Failure::Error(e), _) => self.message(MessageKind::Error, "Could not export", e),
                },
            }
        }
        if self.exit_after_work && !self.writer.busy() && !self.editing() {
            self.exit_after_work = false;
            self.exit_requested = true;
        }
        while !self.dialog_open()
            && !pointer_held
            && let Some(&id) = self.close_queue.front()
            && self.doc(id).is_none_or(|d| d.transform.is_none())
        {
            self.close_queue.pop_front();
            self.request_close(id);
        }
    }

    /// Asks where to export, then writes the file (off the interface thread).
    pub fn export_to_file(&mut self, spec: &ExportDialog, ctx: &egui::Context) {
        let Some(d) = self.doc(spec.doc) else { return };
        let stem = d.stem();
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
        let format = self.export_format(spec.kind, None, None);
        let doc = d.doc().clone();
        let c = doc.canvas;
        let size = (spec.width, spec.height) != (c.width, c.height);
        let size = size.then(|| image_files::ExportSize {
            width: spec.width,
            height: spec.height,
            sampling: crate::prefs::sampling(&self.prefs, "export"),
        });
        let keep = self.export_metadata();
        self.export_to(doc, path, format, size, keep, None, Then::Nothing, ctx);
    }

    /// What of an image's metadata export writes, as the preference says.
    pub(crate) fn export_metadata(&self) -> opal_io::metadata::Keep {
        opal_io::metadata::Keep::named(&self.prefs.choice("export.metadata"))
            .unwrap_or(opal_io::metadata::Keep::NotPlace)
    }

    /// What export writes into a file of `format` of `doc`'s metadata, as `keep` says, made true
    /// of the image exported (at `size`, or the canvas's); and what should be said of it.
    fn embedded(
        doc: &Document,
        format: &ExportFormat,
        size: Option<&image_files::ExportSize>,
        keep: opal_io::metadata::Keep,
    ) -> (image_files::Embed, Vec<String>) {
        use opal_io::metadata;
        let (width, height) = size.map_or((doc.canvas.width, doc.canvas.height), |s| (s.width, s.height));
        let (exif_now, exif_zone, xmp_now) = crate::clock::now();
        let software = format!("Opal Studio {}", env!("CARGO_PKG_VERSION"));
        let out = metadata::Exported { width, height, ppi: doc.resolution, exif_now, exif_zone, xmp_now, software };
        let prepared = metadata::for_export(&doc.metadata, keep, &out);
        let mut notes = Vec::new();
        let embed = match format {
            ExportFormat::Jpeg { .. } => {
                image_files::Embed { jpeg: prepared.jpeg_segments(&mut notes), png: Vec::new() }
            }
            ExportFormat::Png { .. } => image_files::Embed { jpeg: Vec::new(), png: prepared.png_chunks() },
        };
        (embed, notes)
    }

    /// How an export of `kind` is written: as the preferences say, but for `sixteen_bit` (a
    /// PNG's) and `quality` (a JPEG's) where given.
    pub(crate) fn export_format(
        &self,
        kind: ExportKind,
        sixteen_bit: Option<bool>,
        quality: Option<u8>,
    ) -> ExportFormat {
        match kind {
            ExportKind::Png => {
                ExportFormat::Png { sixteen_bit: sixteen_bit.unwrap_or_else(|| self.prefs.bool("export.png_16")) }
            }
            ExportKind::Jpeg => {
                let bg = self.prefs.color("export.jpeg_background");
                ExportFormat::Jpeg {
                    quality: quality.unwrap_or_else(|| self.prefs.f64("export.jpeg_quality").round() as u8),
                    background: [bg.r(), bg.g(), bg.b()],
                }
            }
        }
    }

    /// Writes `doc`'s canvas to `path` (off the interface thread; a failure inside the
    /// encoders is reported like any other), with what of its metadata `keep` keeps, or says
    /// why not if it would need more memory than is free when its turn comes (or than
    /// `memory`, for tests); then does `then`.
    #[allow(clippy::too_many_arguments)]
    pub fn export_to(
        &mut self,
        doc: Document,
        path: PathBuf,
        format: ExportFormat,
        size: Option<image_files::ExportSize>,
        keep: opal_io::metadata::Keep,
        memory: Option<crate::memory::Allowance>,
        then: Then,
        ctx: &egui::Context,
    ) {
        let (embed, notes) = Self::embedded(&doc, &format, size.as_ref(), keep);
        let written = Written::Export { path: path.clone(), notes };
        let job = Box::new(move |p: &_| {
            // Measured now, not when asked for: the write may have waited behind others.
            let mut memory = memory.unwrap_or_else(crate::memory::Allowance::now);
            // Kept while exporting: it caps the tiles' memory.
            memory.take(image_files::export_memory(&doc, size.as_ref()))?;
            let pool = Some(crate::background::pool());
            image_files::export_in(&doc, &path, &format, size.as_ref(), &embed, pool, p).map_err(|e| e.to_string())
        });
        self.writer.write(written, then, job, ctx);
    }

    /// Closes a document, first asking about unsaved changes (after the question on screen,
    /// if one is). One already to close once its save is written just waits.
    pub fn request_close(&mut self, id: DocId) {
        if self.writer.closing(id) {
            let title = self.doc(id).map(|d| d.title.clone()).unwrap_or_default();
            return self.notify(format!("\"{title}\" closes once it is saved."));
        }
        match self.doc(id) {
            Some(d) if d.is_dirty() => {
                if self.confirm.is_some() {
                    self.close_queue.push_back(id);
                } else {
                    self.confirm = Some(Confirm::Close(id));
                }
            }
            Some(_) => self.close_now(id),
            None => {}
        }
    }

    pub fn close_now(&mut self, id: DocId) {
        // An edit under way on it is given up, as it was never part of the image.
        self.give_up_edit(id, "the image was closed");
        // A question about it goes with it.
        match &mut self.confirm {
            Some(Confirm::Close(c)) if *c == id => self.confirm = None,
            Some(Confirm::Exit(ids)) => {
                ids.retain(|d| *d != id);
                if ids.is_empty() {
                    self.confirm = None;
                }
            }
            _ => {}
        }
        self.docs.retain(|d| d.id != id);
        if self.renaming.as_ref().is_some_and(|r| r.0 == id) {
            self.renaming = None;
        }
        if self.active_doc == Some(id) {
            self.active_doc = None;
            if let Some(next) = self.docs.last().map(|d| d.id) {
                self.show_doc(next);
            }
        }
    }

    /// The documents with unsaved changes.
    pub fn dirty_docs(&self) -> Vec<DocId> {
        self.docs.iter().filter(|d| d.is_dirty()).map(|d| d.id).collect()
    }

    pub(crate) fn report<E: std::fmt::Display>(&mut self, r: Result<(), crate::doc::NotMade<E>>) {
        // Refused for an edit under way, the status bar says so already.
        if let Err(crate::doc::NotMade::Failed(e)) = r {
            self.message(MessageKind::Error, "That did not work", e.to_string());
        }
    }
}

/// How the window shows the image (design.md, *The window*).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScreenMode {
    /// As usual.
    #[default]
    Standard,
    /// The window full screen, as it is otherwise.
    FullScreen,
    /// The window full screen, with the active image alone in it: no ribbon, panels or status
    /// bar (as usual while no image is open).
    ImageAlone,
}

impl ScreenMode {
    /// The one F goes to from this one.
    pub fn next(self) -> ScreenMode {
        match self {
            ScreenMode::Standard => ScreenMode::FullScreen,
            ScreenMode::FullScreen => ScreenMode::ImageAlone,
            ScreenMode::ImageAlone => ScreenMode::Standard,
        }
    }
}

/// The memory images' tiles may take before going to disk: `percent` of the computer's
/// `total` memory; none set (they never go) if the total is not known.
fn tile_budget(percent: f64, total: Option<u64>) -> Option<u64> {
    total.map(|t| (t as f64 * percent.clamp(0.0, 100.0) / 100.0) as u64)
}

/// The name a file is shown under.
pub(crate) fn file_title(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub(crate) fn with_extension(mut path: PathBuf, ext: &str) -> PathBuf {
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
mod budget_tests {
    use super::*;

    #[test]
    fn between_frames_each_image_s_history_keeps_to_the_preference() {
        let mut st = AppState::new();
        st.create_image(&NewImage {
            width: 8,
            height: 8,
            background: Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        assert_eq!(st.active().unwrap().history.budget(), u64::MAX);
        st.prefs.set("history.memory", crate::settings::Value::Float(0.5));
        // As a run with no window has it (see `headless`), as well as the window.
        st.between_frames(false);
        assert_eq!(st.active().unwrap().history.budget(), 500_000_000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn app_with_image() -> AppState {
        let mut st = AppState::new();
        st.create_image(&NewImage {
            width: 300,
            height: 200,
            background: Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        st
    }

    fn ctx() -> egui::Context {
        egui::Context::default()
    }

    #[test]
    fn files_dropped_while_a_dialog_is_open_wait_until_it_closes() {
        let mut st = app_with_image();
        let first = st.active_doc;
        let path = std::env::temp_dir().join(format!("opal-dropped-{}.png", std::process::id()));
        image::RgbaImage::new(4, 4).save(&path).unwrap();
        // Behind a dialog, it would become the image the dialog acts on: it waits.
        st.rotate_by = Some(RotateBy { degrees: 30.0, grow: true });
        st.dropped.push_back(path.clone());
        let ctx = egui::Context::default();
        st.open_dropped(&ctx);
        assert_eq!(st.active_doc, first, "opened behind the dialog");
        assert_eq!(st.dropped.len(), 1);
        st.rotate_by = None;
        st.open_dropped(&ctx);
        crate::opening::testing::wait(&mut st);
        let _ = std::fs::remove_file(&path);
        assert!(st.dropped.is_empty());
        assert_eq!(st.docs.len(), 2);
        assert_ne!(st.active_doc, first, "not opened once the dialog closed");
    }

    #[test]
    fn an_export_needing_more_memory_than_is_free_is_refused() {
        let mut st = app_with_image();
        let doc = st.active().unwrap().doc().clone();
        let path = std::env::temp_dir().join(format!("opal-refused-{}.png", std::process::id()));
        let png = ExportFormat::Png { sixteen_bit: false };
        st.export_to(
            doc,
            path.clone(),
            png,
            None,
            opal_io::metadata::Keep::All,
            Some(crate::memory::Allowance::memory(Some(1000))),
            Then::Nothing,
            &ctx(),
        );
        for _ in 0..1000 {
            st.poll_writes(false);
            if !st.writer.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let exists = path.exists();
        let _ = std::fs::remove_file(&path);
        assert!(!exists, "written all the same");
        let said = st.messages.back().map(|m| (m.title.as_str(), m.body.as_str()));
        assert!(said.is_some_and(|(t, b)| t == "Could not export" && b.contains("memory")), "{said:?}");
    }

    #[test]
    fn a_copy_needing_more_memory_than_is_free_is_refused() {
        let mut st = app_with_image();
        st.copy_within(true, crate::memory::Allowance::memory(Some(1000)), None, &ctx());
        let said = st.messages.back().map(|m| (m.title.as_str(), m.body.as_str()));
        assert!(said.is_some_and(|(t, b)| t == "Copy" && b.contains("memory")), "{said:?}");
        st.poll_copies();
        assert!(st.notice.is_none(), "copied all the same");
        st.copy_within(true, crate::memory::Allowance::memory(None), None, &ctx());
        st.poll_copies();
        assert!(st.notice.as_ref().is_some_and(|n| n.0.starts_with("Copied")), "{:?}", st.notice);
    }

    #[test]
    fn images_may_keep_the_share_of_memory_the_preference_says() {
        assert_eq!(tile_budget(60.0, Some(32_000_000_000)), Some(19_200_000_000));
        assert_eq!(tile_budget(100.0, Some(8_000_000_000)), Some(8_000_000_000));
        assert_eq!(tile_budget(60.0, None), None, "not knowing the memory, nothing goes to disk");
        assert_eq!(tile_budget(AppState::new().prefs.f64("memory.tiles"), Some(10_000)), Some(6_000), "60% by default");
    }

    #[test]
    fn a_new_white_image_shares_its_tiles_alike() {
        // Tiles inside, along the right edge, along the bottom, and the corner.
        let img = RasterImage::filled(&IRect::from_size(1000, 600), pixel_from_f32([1.0; 4]));
        let distinct: std::collections::HashSet<*const opal_core::tile::Tile> =
            img.tiles().map(|(_, t)| Arc::as_ptr(t)).collect();
        assert_eq!((img.tile_count(), distinct.len()), (12, 4));
        assert_eq!(img.pixel(999, 599), pixel_from_f32([1.0; 4]));
        assert_eq!(img.pixel(1000, 599)[3].to_f32(), 0.0);
        assert_eq!(img.pixel(999, 600)[3].to_f32(), 0.0);
    }

    #[test]
    fn a_new_white_image_is_white_inside_and_empty_outside() {
        let st = app_with_image();
        let d = st.active().unwrap();
        let img = d.doc().layers[0].objects()[0].image().clone();
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
        assert_eq!(d.active_layer(), Some(d.doc().layers[2].id), "the copy is active");
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
        let top = d.active_layer().unwrap();
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
        assert_eq!(d.active_layer(), Some(bottom));
        let px = d.doc().layers[0].objects()[0].image().pixel(0, 0);
        // Half-transparent blue over white.
        let px = opal_core::tile::pixel_to_f32(px);
        let want = [0.5, 0.5, 1.0, 1.0];
        assert!(px.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3), "{px:?}");
    }

    #[test]
    fn unavailable_commands_explain_themselves() {
        let mut st = AppState::new();
        assert_eq!(availability(&st, Cmd::Save), Err("Open or create an image first.".into()));
        st.create_image(&NewImage {
            width: 10,
            height: 10,
            background: Background::Transparent,
            resolution: opal_core::units::DEFAULT_PPI,
        });
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
        if let opal_core::document::LayerContent::Group(c) = &mut g.content {
            c.push(Arc::new(Layer::new_raster(ids.layer(), ids.object(), "x", RasterImage::new())));
        }
        let copy = crate::layer_ops::copy_with_new_ids(&g, &mut ids);
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

    #[test]
    fn f1_over_a_command_opens_its_group_s_help() {
        let mut st = app_with_image();
        st.ribbon.hovered = Some(Cmd::MergeDown);
        st.run(Cmd::Help, &ctx());
        let (t, g) = crate::commands::RIBBON
            .iter()
            .enumerate()
            .find_map(|(t, tab)| tab.groups.iter().position(|g| g.cmds.contains(&Cmd::MergeDown)).map(|g| (t, g)))
            .unwrap();
        assert_eq!(st.help, Some(HelpTopic::Group(t, g)));
    }

    #[test]
    fn locked_layers_stop_flatten_and_deleting_their_group() {
        let mut st = app_with_image();
        let c = ctx();
        st.run(Cmd::ToggleLock, &c);
        assert!(availability(&st, Cmd::Flatten).unwrap_err().contains("locked"));
        st.run(Cmd::ToggleLock, &c);
        // A locked layer inside an unlocked group.
        st.run(Cmd::NewGroup, &c);
        let group = st.active().unwrap().active_layer().unwrap();
        let bg = st.active().unwrap().doc().layers[0].id;
        st.move_layer_to(bg, Position { parent: Some(group), index: 0 });
        st.active_mut().unwrap().select_only(bg);
        st.run(Cmd::ToggleLock, &c);
        st.active_mut().unwrap().select_only(group);
        assert!(availability(&st, Cmd::DeleteLayer).unwrap_err().contains("locked"));
    }

    #[test]
    fn a_rename_finishes_in_the_document_it_began_in() {
        let mut st = app_with_image();
        let a = st.active_doc.unwrap();
        let layer = st.active().unwrap().active_layer().unwrap();
        st.run(Cmd::Rename, &ctx());
        st.renaming.as_mut().unwrap().2 = "Sky".into();
        // Another image opens, with a layer of the same identifier, before Enter.
        st.create_image(&NewImage {
            width: 10,
            height: 10,
            background: Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        let b = st.active_doc.unwrap();
        assert_ne!(a, b);
        st.finish_rename(true);
        assert_eq!(st.doc(a).unwrap().doc().layer(layer).unwrap().name, "Sky");
        assert_eq!(st.doc(b).unwrap().doc().layers[0].name, "Background");
    }

    #[test]
    fn suggested_names_drop_the_old_extension() {
        let mut st = AppState::new();
        st.create_image(&NewImage {
            width: 10,
            height: 10,
            background: Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        let d = st.active_mut().unwrap();
        d.title = "photo.jpg".into();
        assert_eq!(d.stem(), "photo");
        d.title = "Untitled 3".into();
        assert_eq!(d.stem(), "Untitled 3");
    }

    #[test]
    fn hiding_content_beyond_the_canvas_keeps_its_strength() {
        let mut st = AppState::new();
        st.prefs.set("canvas.off_canvas", Value::Float(60.0));
        st.run(Cmd::ToggleOffCanvas, &ctx());
        st.run(Cmd::ToggleOffCanvas, &ctx());
        assert_eq!(st.prefs.f64("canvas.off_canvas"), 60.0);
        assert_eq!(crate::commands::checked(&st, Cmd::ToggleOffCanvas), Some(true));
    }
}
