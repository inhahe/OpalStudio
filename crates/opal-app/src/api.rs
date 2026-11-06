//! The parameterized forms of the registry's commands, and the queries beside them: what
//! scripts and assistants call (see design.md, *Scripting*). The API's documentation, its
//! Python stubs and the MCP tools are generated from [`methods`], so they cannot drift from
//! what [`AppState::call_from`] accepts.
//!
//! A command called this way does what it does from the interface, but with its parameters
//! given instead of asked for: nothing opens a dialog, and what would have been told in one
//! comes back with the reply (an error as the call's failure, anything else as a note).

use crate::commands::{Cmd, availability};
use crate::dialogs::MAX_SIDE;
use crate::doc::DocId;
use crate::image_ops::{ResizeCanvas, ResizeImage, RotateBy, Turn};
use crate::state::{AppState, Background, MessageKind, NewImage};
use crate::writer::Then;
use eframe::egui;
use opal_core::document::{Layer, LayerContent, LayerId, ObjectKind};
use opal_io::image_files::ExportSize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// What a parameter takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Number {
        min: f64,
        max: f64,
    },
    Text,
    /// A file's path.
    Path,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// An image open in the editor, by id.
    Document,
    /// A layer of the image, by id.
    Layer,
    /// Layers of the image, by id.
    Layers,
}

/// What a parameter left out stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Otherwise {
    /// It must be given.
    Required,
    /// This value (JSON).
    Value(&'static str),
    /// What the editor holds now, as described.
    Now(&'static str),
}

/// A parameter of a method.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub help: &'static str,
    pub otherwise: Otherwise,
}

/// A method of the API: a command of the registry, or a query.
#[derive(Clone, Debug)]
pub struct Method {
    /// What it is called by (the command's name in the registry, in snake case).
    pub name: String,
    pub help: &'static str,
    pub params: &'static [Param],
    /// The command it carries out; `None` for a query.
    pub cmd: Option<Cmd>,
}

const SIDE: i64 = MAX_SIDE as i64;

const DOCUMENT: Param = Param {
    name: "document",
    kind: Kind::Document,
    help: "The image, by id.",
    otherwise: Otherwise::Now("the active image"),
};
const LAYERS: Param = Param {
    name: "layers",
    kind: Kind::Layers,
    help: "The layers, by id.",
    otherwise: Otherwise::Now("the selected layers"),
};
const LAYER: Param = Param {
    name: "layer",
    kind: Kind::Layer,
    help: "The layer, by id.",
    otherwise: Otherwise::Now("the active layer"),
};

const OVERWRITE: Param = Param {
    name: "overwrite",
    kind: Kind::Bool,
    help: "Write over a file already there (otherwise that is refused).",
    otherwise: Otherwise::Value("false"),
};

const fn side(name: &'static str, help: &'static str, otherwise: Otherwise) -> Param {
    Param { name, kind: Kind::Int { min: 1, max: SIDE }, help, otherwise }
}

const NEW_WIDTH: Param = side("width", "Its width, in pixels.", Otherwise::Required);
const NEW_HEIGHT: Param = side("height", "Its height, in pixels.", Otherwise::Required);
const EXPORT_WIDTH: Param = side(
    "width",
    "The width written, in pixels.",
    Otherwise::Now("the canvas's, or in proportion to the height given"),
);
const EXPORT_HEIGHT: Param = side(
    "height",
    "The height written, in pixels.",
    Otherwise::Now("the canvas's, or in proportion to the width given"),
);
const RESIZED_WIDTH: Param = side("width", "The new width, in pixels.", Otherwise::Required);
const RESIZED_HEIGHT: Param = side("height", "The new height, in pixels.", Otherwise::Required);
const CANVAS_WIDTH: Param = side("width", "The new width, in pixels.", Otherwise::Now("the canvas's"));
const CANVAS_HEIGHT: Param = side("height", "The new height, in pixels.", Otherwise::Now("the canvas's"));

/// Where Resize Canvas puts the old canvas in the new, by name, row by row: the one at `i` is
/// at `[i % 3, i / 3]` (left, center, right; top, middle, bottom).
const ANCHOR_NAMES: &[&str] =
    &["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"];

/// The parameters of command `cmd`, or `None` for one of the interface alone (the view,
/// the panels, the windows), or one not callable yet (those that open the transform box).
pub fn params(cmd: Cmd) -> Option<&'static [Param]> {
    use Cmd::*;
    Some(match cmd {
        New => &[
            NEW_WIDTH,
            NEW_HEIGHT,
            Param {
                name: "background",
                kind: Kind::Choice(&["white", "transparent"]),
                help: "What it starts filled with.",
                otherwise: Otherwise::Value("\"white\""),
            },
        ],
        Open => &[Param {
            name: "path",
            kind: Kind::Path,
            help: "The file, by its absolute path: an Opal Studio file, a PNG or a JPEG.",
            otherwise: Otherwise::Required,
        }],
        Save => &[DOCUMENT],
        SaveAs => &[
            DOCUMENT,
            Param {
                name: "path",
                kind: Kind::Path,
                help: "Where: an Opal Studio file (.opal is added unless it is there), by its absolute path.",
                otherwise: Otherwise::Required,
            },
            OVERWRITE,
        ],
        Export => &[
            DOCUMENT,
            Param {
                name: "path",
                kind: Kind::Path,
                help: "Where: a .png, .jpg or .jpeg file (whose extension chooses the format), by its absolute path.",
                otherwise: Otherwise::Required,
            },
            EXPORT_WIDTH,
            EXPORT_HEIGHT,
            OVERWRITE,
            Param {
                name: "sixteen_bit",
                kind: Kind::Bool,
                help: "For a PNG: 16 bits a channel rather than 8.",
                otherwise: Otherwise::Now("the preference"),
            },
            Param {
                name: "quality",
                kind: Kind::Int { min: 1, max: 100 },
                help: "For a JPEG: its quality.",
                otherwise: Otherwise::Now("the preference"),
            },
        ],
        Close => &[
            DOCUMENT,
            Param {
                name: "discard",
                kind: Kind::Bool,
                help: "Close it even with unsaved changes, losing them.",
                otherwise: Otherwise::Value("false"),
            },
        ],
        Undo | Redo | Copy | CopyMerged | MergeVisible | StampVisible | Flatten | CropToContent | RotateCw
        | RotateCcw | Rotate180 | FlipHorizontal | FlipVertical => &[DOCUMENT],
        NewLayer | NewGroup => &[
            DOCUMENT,
            Param { name: "name", kind: Kind::Text, help: "Its name.", otherwise: Otherwise::Now("a numbered one") },
        ],
        Duplicate | GroupLayers | DeleteLayer | MergeSelected | Rasterize => &[DOCUMENT, LAYERS],
        MoveUp | MoveDown | MergeDown | MergeGroup => &[DOCUMENT, LAYER],
        ResizeImage => &[
            DOCUMENT,
            RESIZED_WIDTH,
            RESIZED_HEIGHT,
            Param {
                name: "resample",
                kind: Kind::Bool,
                help: "Resample the pixel layers now (smaller, but enlarging again later cannot bring \
                       their detail back), rather than keep their pixels and draw them at the new size.",
                otherwise: Otherwise::Now("the preference"),
            },
        ],
        ResizeCanvas => &[
            DOCUMENT,
            CANVAS_WIDTH,
            CANVAS_HEIGHT,
            Param {
                name: "anchor",
                kind: Kind::Choice(ANCHOR_NAMES),
                help: "Where the old canvas sits in the new.",
                otherwise: Otherwise::Value("\"center\""),
            },
        ],
        RotateBy => &[
            DOCUMENT,
            Param {
                name: "degrees",
                kind: Kind::Number { min: -360.0, max: 360.0 },
                help: "The angle, clockwise.",
                otherwise: Otherwise::Required,
            },
            Param {
                name: "grow",
                kind: Kind::Bool,
                help: "Grow the canvas so that nothing is cut off.",
                otherwise: Otherwise::Value("true"),
            },
        ],
        ToggleVisible => &[
            DOCUMENT,
            LAYERS,
            Param {
                name: "visible",
                kind: Kind::Bool,
                help: "Shown, or hidden.",
                otherwise: Otherwise::Now("each the opposite of what it is"),
            },
        ],
        ToggleLock => &[
            DOCUMENT,
            LAYERS,
            Param {
                name: "locked",
                kind: Kind::Bool,
                help: "Locked, or not.",
                otherwise: Otherwise::Now("each the opposite of what it is"),
            },
        ],
        Rename => &[
            DOCUMENT,
            LAYER,
            Param { name: "name", kind: Kind::Text, help: "Its new name.", otherwise: Otherwise::Required },
        ],
        Paste | PasteInPlace | Transform | Preferences | Exit | RunScript | ZoomIn | ZoomOut | ZoomFit | Zoom100
        | ToggleOffCanvas | ToggleTheme | ToggleRibbon | CycleToolsPlace | ResetPanels | Search | Help | Shortcuts
        | About | WidgetGallery => return None,
    })
}

/// The queries: what the API tells of the editor and its images.
const QUERIES: &[(&str, &str, &[Param])] = &[
    (
        "documents",
        "The images open: for each, its id, title, file, canvas size and whether it has unsaved changes.",
        &[],
    ),
    (
        "layers",
        "An image's layers, top first: for each, its id, name, kind, whether it is shown and locked, its \
         opacity, whether it is selected, and the layers inside a group.",
        &[DOCUMENT],
    ),
    (
        "image",
        "What an image shows, as a PNG, scaled down to fit a size (never enlarged): its bytes in base64, \
         with its width and height.",
        &[
            DOCUMENT,
            Param {
                name: "max_side",
                kind: Kind::Int { min: 16, max: 8192 },
                help: "The most pixels it may be across or down.",
                otherwise: Otherwise::Value("1024"),
            },
        ],
    ),
    ("methods", "Every method of the API, with its parameters: what this list is generated from.", &[]),
];

/// `Name` (a variant's name) in snake case: `SaveAs` → `save_as`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Every method of the API: the commands that have parameterized forms, then the queries
/// (made once).
pub fn methods() -> &'static [Method] {
    static METHODS: std::sync::OnceLock<Vec<Method>> = std::sync::OnceLock::new();
    METHODS.get_or_init(make_methods)
}

fn make_methods() -> Vec<Method> {
    let commands = Cmd::ALL.into_iter().filter_map(|cmd| {
        let params = params(cmd)?;
        Some(Method { name: snake(&format!("{cmd:?}")), help: cmd.info().description, params, cmd: Some(cmd) })
    });
    let queries = QUERIES.iter().map(|&(name, help, params)| Method { name: name.to_owned(), help, params, cmd: None });
    commands.chain(queries).collect()
}

/// The arguments of a call, checked against its method's parameters, with the values that
/// stand for those left out where they are fixed.
#[derive(Debug)]
pub struct Args(Map<String, Value>);

impl Args {
    /// Checks `given` against `params`: no unknown name, each value of its kind and in its
    /// range, each required one given.
    pub fn check(params: &[Param], given: &Map<String, Value>) -> Result<Args, String> {
        if let Some(unknown) = given.keys().find(|k| !params.iter().any(|p| p.name == *k)) {
            let known: Vec<&str> = params.iter().map(|p| p.name).collect();
            return Err(if known.is_empty() {
                format!("\"{unknown}\" is not a parameter: it takes none")
            } else {
                format!("\"{unknown}\" is not a parameter (it takes {})", known.join(", "))
            });
        }
        let mut out = Map::new();
        for p in params {
            let value = match (given.get(p.name), p.otherwise) {
                (Some(Value::Null) | None, Otherwise::Required) => return Err(format!("\"{}\" must be given", p.name)),
                (Some(Value::Null) | None, Otherwise::Now(_)) => continue,
                (Some(Value::Null) | None, Otherwise::Value(json)) => {
                    serde_json::from_str(json).expect("a parameter's default is valid JSON")
                }
                (Some(v), _) => v.clone(),
            };
            check_value(p, &value)?;
            out.insert(p.name.to_owned(), value);
        }
        Ok(Args(out))
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// The value of a parameter that is there (given, or fixed when left out).
    fn int(&self, name: &str) -> i64 {
        self.opt_int(name).unwrap_or_else(|| panic!("\"{name}\" was checked to be there"))
    }

    fn opt_int(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(Value::as_i64)
    }

    fn number(&self, name: &str) -> f64 {
        self.get(name).and_then(Value::as_f64).unwrap_or_else(|| panic!("\"{name}\" was checked to be there"))
    }

    fn flag(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(Value::as_bool)
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(Value::as_str)
    }

    fn path(&self, name: &str) -> PathBuf {
        PathBuf::from(self.text(name).unwrap_or_else(|| panic!("\"{name}\" was checked to be there")))
    }

    fn document(&self) -> Option<DocId> {
        self.opt_int("document").map(|d| DocId(d as u64))
    }

    fn layer(&self) -> Option<LayerId> {
        self.opt_int("layer").map(|l| LayerId(l as u64))
    }

    fn layers(&self) -> Option<Vec<LayerId>> {
        let ids = self.get("layers")?.as_array()?;
        Some(ids.iter().filter_map(Value::as_i64).map(|l| LayerId(l as u64)).collect())
    }
}

/// Whether `value` is of `p`'s kind and in its range.
fn check_value(p: &Param, value: &Value) -> Result<(), String> {
    let name = p.name;
    let id = |v: &Value| v.as_i64().is_some_and(|i| i >= 0);
    let ok = match p.kind {
        Kind::Bool => value.is_boolean(),
        Kind::Int { min, max } => match value.as_i64() {
            Some(i) if (min..=max).contains(&i) => true,
            Some(i) => return Err(format!("\"{name}\" must lie between {min} and {max}, not {i}")),
            None => false,
        },
        Kind::Number { min, max } => match value.as_f64() {
            Some(x) if (min..=max).contains(&x) => true,
            Some(x) => return Err(format!("\"{name}\" must lie between {min} and {max}, not {x}")),
            None => false,
        },
        Kind::Text => value.as_str().is_some_and(|s| !s.trim().is_empty()),
        Kind::Path => match value.as_str() {
            Some(p) if std::path::Path::new(p).is_absolute() => true,
            Some(p) => {
                return Err(format!(
                    "\"{name}\" must be an absolute path (the editor's folder is not the caller's), not \"{p}\""
                ));
            }
            None => false,
        },
        Kind::Choice(words) => match value.as_str() {
            Some(w) if words.contains(&w) => true,
            Some(w) => return Err(format!("\"{name}\" must be one of {}, not \"{w}\"", words.join(", "))),
            None => false,
        },
        Kind::Document | Kind::Layer => id(value),
        Kind::Layers => value.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(id)),
    };
    if ok { Ok(()) } else { Err(format!("\"{name}\" must be {}, not {value}", kind_text(p.kind))) }
}

/// What a value of `kind` is, in words.
pub fn kind_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Bool => "true or false",
        Kind::Int { .. } => "a whole number",
        Kind::Number { .. } => "a number",
        Kind::Text => "text that is not blank",
        Kind::Path => "a file's path",
        Kind::Choice(_) => "one of its words",
        Kind::Document => "an image's id",
        Kind::Layer => "a layer's id",
        Kind::Layers => "a list of layers' ids, not empty",
    }
}

/// What a call answers.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Done: what it gives back, or why it could not be done.
    Now(Result<Value, String>),
    /// Started (a file being written): its answer comes as [`AppState::answers`] does
    /// this number's.
    Later(u64),
}

impl AppState {
    /// Calls method `name` of the API with the arguments `given`: carries out the command
    /// (or answers the query) as the interface would, without asking anything.
    #[cfg(test)]
    pub fn call(&mut self, name: &str, given: &Map<String, Value>, ctx: &egui::Context) -> Reply {
        self.call_heard(true, name, given, ctx)
    }

    /// [`call`](Self::call); but unless `heard`, no one reads the answer (the call was a
    /// notification), so what would have been told in a dialog is told in one after all.
    fn call_heard(&mut self, heard: bool, name: &str, given: &Map<String, Value>, ctx: &egui::Context) -> Reply {
        let Some(method) = methods().iter().find(|m| m.name == name) else {
            return Reply::Now(Err(format!("There is no method \"{name}\".")));
        };
        let args = match Args::check(method.params, given) {
            Ok(args) => args,
            Err(why) => return Reply::Now(Err(format!("{name}: {why}."))),
        };
        let told = if heard { self.messages.len() } else { usize::MAX };
        let notice = self.notice.clone();
        // The image and layers the user had, to put back if the command is refused (it
        // makes those it names the active ones).
        let view = (self.active_doc, self.docs.iter().map(|d| (d.id, d.selection_state())).collect::<Vec<_>>());
        let reply = match method.cmd {
            Some(cmd) => self.carry_out(cmd, method.params, &args, ctx),
            None if method.name == "image" => self.image_by_args(&args, ctx),
            None => Reply::Now(self.answer(&method.name, &args)),
        };
        // What would have been told in a dialog comes back with the reply instead: an error,
        // or a refusal (told as information), as the call's failure; a warning, or a notice
        // the status bar would have shown, as a note.
        let messages: Vec<_> = self.messages.drain(told.min(self.messages.len())..).collect();
        let text = |m: &crate::state::Message| format!("{}: {}", m.title, m.body);
        let refusals: Vec<String> = messages.iter().filter(|m| m.kind != MessageKind::Warning).map(text).collect();
        let mut notes: Vec<String> = messages.iter().filter(|m| m.kind == MessageKind::Warning).map(text).collect();
        if self.notice != notice
            && let Some((said, _)) = &self.notice
        {
            notes.push(said.clone());
        }
        let reply = match reply {
            Reply::Now(Ok(_)) if !refusals.is_empty() => Reply::Now(Err(refusals.join("\n"))),
            Reply::Now(Ok(mut value)) if !notes.is_empty() => {
                if let Value::Object(fields) = &mut value {
                    fields.insert("notes".to_owned(), json!(notes));
                }
                Reply::Now(Ok(value))
            }
            other => other,
        };
        if method.cmd.is_some() && matches!(reply, Reply::Now(Err(_))) {
            let (active, selections) = view;
            for (id, s) in selections {
                if let Some(d) = self.doc_mut(id) {
                    d.set_selection_state(s);
                }
            }
            if let Some(id) = active.filter(|id| self.doc(*id).is_some()) {
                self.show_doc(id);
            }
        }
        reply
    }

    /// [`call`](Self::call), made by script run `script` if it is one (its changes to each
    /// image are joined into one history state when the run ends: see
    /// [`script_ended`](Self::script_ended)), and `heard` unless it is a notification. A
    /// file opened by a notification (one handed over by an editor started to open it)
    /// brings the window to the front.
    pub fn call_from(
        &mut self,
        script: Option<u64>,
        heard: bool,
        name: &str,
        given: &Map<String, Value>,
        ctx: &egui::Context,
    ) -> Reply {
        self.docs.iter_mut().for_each(|d| d.script = script);
        let reply = self.call_heard(heard, name, given, ctx);
        self.docs.iter_mut().for_each(|d| d.script = None);
        if !heard && name == "open" {
            self.raise_window = true;
        }
        reply
    }

    /// Script run `run`, named `name` (its file's), has ended: in each image, the states it
    /// made one after another become one, so that one undo takes back what it did.
    pub fn script_ended(&mut self, run: u64, name: &str) {
        let label = format!("Script: {name}");
        self.docs.iter_mut().for_each(|d| d.script_ended(run, &label));
    }

    /// Whether scripts' calls wait now: while the user has a dialog open, holds a button
    /// (`pointer_down`), or has a transform box open, an edit in progress that a command
    /// would commit under them.
    pub fn scripts_wait(&self, pointer_down: bool) -> bool {
        self.dialog_open() || pointer_down || self.docs.iter().any(|d| d.transform.is_some())
    }

    /// Answers query `name`.
    fn answer(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "documents" => Ok(json!(
                self.docs
                    .iter()
                    .map(|d| {
                        let c = d.doc().canvas;
                        json!({
                            "document": d.id.0,
                            "title": d.title,
                            "path": d.path.as_ref().map(|p| p.display().to_string()),
                            "width": c.width,
                            "height": c.height,
                            "unsaved": d.is_dirty(),
                            "active": self.active_doc == Some(d.id),
                        })
                    })
                    .collect::<Vec<_>>()
            )),
            "layers" => {
                let d = self.doc(self.which(args)?).expect("found");
                Ok(json!(d.doc().layers.iter().rev().map(|l| layer_json(l, d)).collect::<Vec<_>>()))
            }
            "methods" => Ok(describe()),
            _ => unreachable!("every query is answered (but `image`, by image_by_args)"),
        }
    }

    /// The image `args` names, or the active one; it must be open.
    fn which(&self, args: &Args) -> Result<DocId, String> {
        match args.document() {
            Some(id) if self.doc(id).is_some() => Ok(id),
            Some(id) => Err(format!("There is no image {}.", id.0)),
            None => self.active_doc.ok_or_else(|| "No image is open.".to_owned()),
        }
    }

    /// Carries out `cmd` with `args`.
    fn carry_out(&mut self, cmd: Cmd, params: &[Param], args: &Args, ctx: &egui::Context) -> Reply {
        let now = |r: Result<Value, String>| Reply::Now(r);
        // The image and layers named become the active ones, as clicking them would make
        // them, so that the command acts on them.
        if params.iter().any(|p| p.kind == Kind::Document) {
            match self.which(args) {
                Ok(id) => self.show_doc(id),
                Err(why) => return now(Err(why)),
            }
        }
        if let Err(why) = self.target(args) {
            return now(Err(why));
        }
        if let Err(why) = availability(self, cmd) {
            return now(Err(format!("{}: {why}", cmd.info().name)));
        }
        let doc = self.active_doc;
        match cmd {
            Cmd::New => {
                let background = match args.text("background") {
                    Some("transparent") => Background::Transparent,
                    _ => Background::White,
                };
                let (width, height) = (args.int("width") as u32, args.int("height") as u32);
                self.create_image(&NewImage { width, height, background });
                now(Ok(json!({ "document": self.active_doc.expect("just made").0 })))
            }
            Cmd::Open => {
                let before = self.docs.len();
                self.open_path(&args.path("path"));
                // Opened, or shown if it already was.
                match self.active_doc {
                    Some(id) if self.docs.len() > before || self.doc(id).is_some_and(|d| d.path.is_some()) => {
                        now(Ok(json!({ "document": id.0 })))
                    }
                    _ => now(Ok(json!({}))),
                }
            }
            Cmd::Save => {
                let id = doc.expect("available");
                let Some(path) = self.doc(id).and_then(|d| d.path.clone()) else {
                    return now(Err("The image has no file yet: save it with save_as.".to_owned()));
                };
                self.reply_after_write(|st, then| st.save_to(id, path, then, ctx))
            }
            Cmd::SaveAs => {
                let id = doc.expect("available");
                let path = crate::state::with_extension(args.path("path"), "opal");
                let own = self.doc(id).and_then(|d| d.path.as_ref()) == Some(&path);
                if !own && path.exists() && args.flag("overwrite") != Some(true) {
                    return now(Err(format!("{} is there already: give overwrite to write over it.", path.display())));
                }
                self.reply_after_write(|st, then| st.save_to(id, path, then, ctx))
            }
            Cmd::Export => self.export_by_args(args, ctx),
            Cmd::Copy | Cmd::CopyMerged => {
                // Answered once on the clipboard (a failure there is the call's).
                self.next_answer += 1;
                let n = self.next_answer;
                if self.copy_answering(cmd == Cmd::CopyMerged, n, ctx) { Reply::Later(n) } else { now(Ok(json!({}))) }
            }
            Cmd::Close => {
                let id = doc.expect("available");
                let unsaved = self.doc(id).is_some_and(|d| d.is_dirty());
                if unsaved && args.flag("discard") != Some(true) {
                    return now(Err(
                        "The image has unsaved changes: save it first, or close it with discard.".to_owned()
                    ));
                }
                self.close_now(id);
                now(Ok(json!({})))
            }
            Cmd::NewLayer | Cmd::NewGroup => match self.new_layer_named(cmd == Cmd::NewGroup, args.text("name")) {
                Some(id) => now(Ok(json!({ "layer": id.0 }))),
                // Refused, and said why.
                None => now(Ok(json!({}))),
            },
            Cmd::ResizeImage => {
                let spec = ResizeImage {
                    width: args.int("width") as u32,
                    height: args.int("height") as u32,
                    keep_aspect: false,
                };
                let resample = args.flag("resample").unwrap_or_else(|| self.prefs.bool("resize.resample"));
                self.resize_image(&spec, resample);
                now(Ok(self.canvas_json()))
            }
            Cmd::ResizeCanvas => {
                let c = self.active().expect("available").doc().canvas;
                let anchor = args
                    .text("anchor")
                    .and_then(|a| ANCHOR_NAMES.iter().position(|n| *n == a))
                    .map(|i| [(i % 3) as u8, (i / 3) as u8]);
                let spec = ResizeCanvas {
                    width: args.opt_int("width").map_or(c.width, |w| w as u32),
                    height: args.opt_int("height").map_or(c.height, |h| h as u32),
                    anchor: anchor.unwrap_or([1, 1]),
                    by_margins: false,
                    margins: [0; 4],
                };
                self.resize_canvas(&spec);
                now(Ok(self.canvas_json()))
            }
            Cmd::RotateBy => {
                let degrees = args.number("degrees");
                if degrees != 0.0 {
                    self.rotate_by(&RotateBy { degrees, grow: args.flag("grow").unwrap_or(true) });
                }
                now(Ok(self.canvas_json()))
            }
            Cmd::RotateCw | Cmd::RotateCcw | Cmd::Rotate180 | Cmd::FlipHorizontal | Cmd::FlipVertical => {
                self.turn(match cmd {
                    Cmd::RotateCw => Turn::Clockwise,
                    Cmd::RotateCcw => Turn::Counterclockwise,
                    Cmd::Rotate180 => Turn::Half,
                    Cmd::FlipHorizontal => Turn::FlipHorizontal,
                    _ => Turn::FlipVertical,
                });
                now(Ok(self.canvas_json()))
            }
            Cmd::ToggleVisible | Cmd::ToggleLock => {
                let visibility = cmd == Cmd::ToggleVisible;
                let wanted = args.flag(if visibility { "visible" } else { "locked" });
                let d = self.active_mut().expect("available");
                let ids = d.selection();
                let flag = |l: &Layer| if visibility { l.visible } else { l.locked };
                // Already as asked: nothing to record.
                if wanted.is_some_and(|w| ids.iter().all(|&id| d.doc().layer(id).is_none_or(|l| flag(l) == w))) {
                    return now(Ok(self.selection_json()));
                }
                let label = match (visibility, wanted) {
                    (true, Some(true)) => "Show layers",
                    (true, Some(false)) => "Hide layers",
                    (true, None) => "Show or hide layers",
                    (false, Some(true)) => "Lock layers",
                    (false, Some(false)) => "Unlock layers",
                    (false, None) => "Lock or unlock layers",
                };
                // One step of history for all of them.
                let result = d.apply(label, |doc, _| {
                    for id in ids {
                        let l = doc.layer_mut(id).ok_or(opal_core::document::TreeError::NoSuchLayer(id))?;
                        let flag = if visibility { &mut l.visible } else { &mut l.locked };
                        *flag = wanted.unwrap_or(!*flag);
                    }
                    Ok::<(), opal_core::document::TreeError>(())
                });
                self.report(result);
                now(Ok(self.selection_json()))
            }
            Cmd::Rename => {
                let id = self.active().and_then(|d| d.active_layer()).expect("available");
                self.rename_layer(doc.expect("available"), id, args.text("name").expect("required").to_owned());
                now(Ok(json!({ "layer": id.0 })))
            }
            _ => {
                // The rest take no more than the image and layers set above.
                self.run(cmd, ctx);
                now(Ok(self.selection_json()))
            }
        }
    }

    /// Makes the layers `args` names the selected ones (the first named the active one, or
    /// the one `layer` names): each must be in the active image.
    fn target(&mut self, args: &Args) -> Result<(), String> {
        let mut ids = match (args.layer(), args.layers()) {
            (Some(one), _) => vec![one],
            (None, Some(many)) => many,
            (None, None) => return Ok(()),
        };
        // Each once: a repeated one would be toggled back out of the selection.
        let mut seen = std::collections::HashSet::new();
        ids.retain(|id| seen.insert(*id));
        let d = self.active_mut().ok_or("No image is open.")?;
        if let Some(missing) = ids.iter().find(|id| d.doc().layer(**id).is_none()) {
            return Err(format!("There is no layer {} in the image.", missing.0));
        }
        d.select_only(ids[0]);
        for &id in &ids[1..] {
            d.toggle_selected(id);
        }
        d.make_active(ids[0]);
        Ok(())
    }

    /// Exports the active image as `args` say.
    fn export_by_args(&mut self, args: &Args, ctx: &egui::Context) -> Reply {
        let path = args.path("path");
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        let kind = match ext.as_deref() {
            Some("png") => crate::state::ExportKind::Png,
            Some("jpg" | "jpeg") => crate::state::ExportKind::Jpeg,
            _ => return Reply::Now(Err("export: \"path\" must end in .png, .jpg or .jpeg.".to_owned())),
        };
        if path.exists() && args.flag("overwrite") != Some(true) {
            return Reply::Now(Err(format!("{} is there already: give overwrite to write over it.", path.display())));
        }
        let format = self.export_format(kind, args.flag("sixteen_bit"), args.opt_int("quality").map(|q| q as u8));
        let doc = self.active().expect("available").doc().clone();
        let c = doc.canvas;
        // One side given: the other in proportion, as the dialog keeps it.
        let ratio = |a: u32, b: u32, given: i64| ((given as f64 * f64::from(b) / f64::from(a)).round() as u32).max(1);
        let (width, height) = match (args.opt_int("width"), args.opt_int("height")) {
            (Some(w), Some(h)) => (w as u32, h as u32),
            (Some(w), None) => (w as u32, ratio(c.width, c.height, w)),
            (None, Some(h)) => (ratio(c.height, c.width, h), h as u32),
            (None, None) => (c.width, c.height),
        };
        if width.max(height) > MAX_SIDE {
            return Reply::Now(Err(format!(
                "export: in proportion, that is {width} × {height} pixels, past the most a side can be \
                 ({MAX_SIDE}): give both sides."
            )));
        }
        let size = ((width, height) != (c.width, c.height)).then(|| ExportSize {
            width,
            height,
            sampling: crate::prefs::sampling(&self.prefs, "export"),
        });
        self.reply_after_write(|st, then| st.export_to(doc, path, format, size, None, then, ctx))
    }

    /// Starts a write with `start`, which is given what to do once it is written: answer
    /// this call.
    fn reply_after_write(&mut self, start: impl FnOnce(&mut AppState, Then)) -> Reply {
        self.next_answer += 1;
        let n = self.next_answer;
        start(self, Then::Answer(n));
        Reply::Later(n)
    }

    /// Makes the image `args` names, scaled to fit `max_side`, as a PNG (in memory, on a
    /// thread of its own: nothing the user sees, unlike an export); the answer, once made,
    /// is its bytes in base64, with its size (see [`answers`]).
    ///
    /// [`answers`]: Self::answers
    fn image_by_args(&mut self, args: &Args, ctx: &egui::Context) -> Reply {
        let id = match self.which(args) {
            Ok(id) => id,
            Err(why) => return Reply::Now(Err(why)),
        };
        let doc = self.doc(id).expect("found").doc().clone();
        let c = doc.canvas;
        let k = (args.int("max_side") as f64 / f64::from(c.width.max(c.height))).min(1.0);
        let side = |s: u32| ((f64::from(s) * k).round() as u32).max(1);
        let (width, height) = (side(c.width), side(c.height));
        let size = ((width, height) != (c.width, c.height)).then(|| ExportSize {
            width,
            height,
            sampling: crate::prefs::sampling(&self.prefs, "export"),
        });
        self.next_answer += 1;
        let n = self.next_answer;
        let (send, made) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let make = move || {
            // Kept while it is made, as an export's: it caps the tiles' memory.
            let mut memory = crate::memory::Allowance::now();
            let result = memory
                .take(opal_io::image_files::export_memory(&doc, size.as_ref()))
                .and_then(|()| {
                    opal_io::image_files::png_in_memory(&doc, size.as_ref(), Some(crate::background::pool()))
                })
                .map(|png| json!({ "png": base64(&png), "width": width, "height": height }));
            drop(memory);
            let _ = send.send(result);
            ctx.request_repaint();
        };
        if let Err(e) = std::thread::Builder::new().name("api image".into()).spawn(make) {
            return Reply::Now(Err(format!("The image could not be made: {e}")));
        }
        self.images_made.push((n, made));
        Reply::Later(n)
    }

    /// The answers to calls whose writes, copies to the clipboard or images have finished
    /// since asked last, by number.
    pub fn answers(&mut self) -> Vec<(u64, Result<Value, String>)> {
        let mut answered = std::mem::take(&mut self.answered);
        self.images_made.retain(|(n, made)| match made.try_recv() {
            Ok(result) => {
                answered.push((*n, result));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                answered.push((*n, Err("The image could not be made.".to_owned())));
                false
            }
        });
        answered
    }

    fn canvas_json(&self) -> Value {
        let c = self.active().expect("available").doc().canvas;
        json!({ "width": c.width, "height": c.height })
    }

    fn selection_json(&self) -> Value {
        match self.active() {
            Some(d) => json!({
                "layers": d.selection().iter().map(|l| l.0).collect::<Vec<_>>(),
                "active": d.active_layer().map(|l| l.0),
            }),
            None => json!({}),
        }
    }
}

/// `bytes` in base64 (the standard alphabet, padded).
pub fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            out.push(if i <= chunk.len() { DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Layer `l` of image `d`, as the `layers` query tells it.
fn layer_json(l: &Arc<Layer>, d: &crate::doc::DocState) -> Value {
    let kind = match &l.content {
        LayerContent::Group(_) => "group",
        LayerContent::Objects(objects) if objects.iter().all(|o| matches!(o.kind, ObjectKind::Pixels(_))) => "pixels",
        LayerContent::Objects(_) => "objects",
    };
    let mut out = json!({
        "layer": l.id.0,
        "name": l.name,
        "kind": kind,
        "visible": l.visible,
        "locked": l.locked,
        "opacity": l.opacity,
        "selected": d.is_selected(l.id),
    });
    if let Some(children) = l.children() {
        out["layers"] = json!(children.iter().rev().map(|c| layer_json(c, d)).collect::<Vec<_>>());
    }
    out
}

/// The whole API as data: each method's name, help and parameters (each with its kind,
/// range or words, help, and what leaving it out stands for).
pub fn describe() -> Value {
    json!(
        methods()
            .iter()
            .map(|m| {
                let params: Vec<Value> = m
                    .params
                    .iter()
                    .map(|p| {
                        let mut v = json!({ "name": p.name, "kind": kind_name(p.kind), "help": p.help });
                        match p.kind {
                            Kind::Int { min, max } => (v["min"], v["max"]) = (json!(min), json!(max)),
                            Kind::Number { min, max } => (v["min"], v["max"]) = (json!(min), json!(max)),
                            Kind::Choice(words) => v["choices"] = json!(words),
                            _ => {}
                        }
                        match p.otherwise {
                            Otherwise::Required => v["required"] = json!(true),
                            Otherwise::Value(json) => {
                                v["default"] = serde_json::from_str(json).expect("a parameter's default is valid JSON")
                            }
                            Otherwise::Now(what) => v["otherwise"] = json!(what),
                        }
                        v
                    })
                    .collect();
                json!({ "name": m.name, "help": m.help, "kind": if m.cmd.is_some() { "command" } else { "query" }, "params": params })
            })
            .collect::<Vec<_>>()
    )
}

/// A kind's name in [`describe`].
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Bool => "bool",
        Kind::Int { .. } => "int",
        Kind::Number { .. } => "number",
        Kind::Text => "text",
        Kind::Path => "path",
        Kind::Choice(_) => "choice",
        Kind::Document => "document",
        Kind::Layer => "layer",
        Kind::Layers => "layers",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().expect("an object").clone()
    }

    /// A parameter's Python type, as the stubs give it.
    fn py_type(p: &Value) -> String {
        match p["kind"].as_str().unwrap() {
            "bool" => "bool".into(),
            "int" | "document" | "layer" => "int".into(),
            "number" => "float".into(),
            "text" => "str".into(),
            "path" => "str | os.PathLike[str]".into(),
            "layers" => "list[int]".into(),
            "choice" => {
                let words: Vec<String> = p["choices"].as_array().unwrap().iter().map(|w| format!("{w}")).collect();
                format!("Literal[{}]", words.join(", "))
            }
            other => panic!("no Python type for {other}"),
        }
    }

    /// What a parameter is, in words: its help, its range or words, and what leaving it out
    /// stands for.
    fn param_text(p: &Value) -> String {
        let mut text = p["help"].as_str().unwrap().to_owned();
        if let (Some(min), Some(max)) = (p.get("min"), p.get("max")) {
            text += &format!(" From {min} to {max}.");
        }
        if let Some(d) = p.get("default") {
            text += &format!(" Default: {d}.");
        } else if let Some(o) = p.get("otherwise") {
            text += &format!(" Default: {}.", o.as_str().unwrap());
        }
        text
    }

    /// The Python stubs for the methods: the `Editor` class's methods, with their types
    /// and help, for editors and type checkers.
    fn python_stubs() -> String {
        let mut out = String::from(
            "# The editor's methods, for editors and type checkers. Made from its command\n\
             # registry by the test `api::tests::the_python_stubs_and_api_page_are_current`: run\n\
             # it with OPAL_WRITE_STUBS=1 to make this again. Do not edit by hand.\n\n\
             import os\nfrom pathlib import Path\nfrom typing import Any, Literal\n\n\
             PROTOCOL: int\n\n\
             class OpalError(Exception):\n    code: int | None\n    def __init__(self, message: str, code: int | None = None) -> None: ...\n\n\
             def connection_file() -> Path: ...\n\
             def connect(file: str | os.PathLike[str] | None = None, *, script: str | None = ...) -> Editor: ...\n\n\
             class Editor:\n    version: str | None\n\
             \x20   def __init__(self, pipe: str, token: str, script: str | None = None) -> None: ...\n\
             \x20   def call(self, method: str, **params: Any) -> Any: ...\n\
             \x20   def methods(self) -> list[dict[str, Any]]: ...\n\
             \x20   def disconnect(self) -> None: ...\n\
             \x20   def __enter__(self) -> Editor: ...\n\
             \x20   def __exit__(self, *exc: Any) -> None: ...\n",
        );
        // The client gives `methods` itself (the list it fetched on connecting).
        for m in describe().as_array().unwrap().iter().filter(|m| m["name"] != json!("methods")) {
            let params = m["params"].as_array().unwrap();
            let mut sig: Vec<String> = Vec::new();
            for p in params {
                let name = p["name"].as_str().unwrap();
                let ty = py_type(p);
                sig.push(match (p.get("required"), p.get("default")) {
                    (Some(_), _) => format!("{name}: {ty}"),
                    (None, Some(d)) => format!("{name}: {ty} = {}", py_value(d)),
                    (None, None) => format!("{name}: {ty} | None = None"),
                });
            }
            let star = if sig.is_empty() { String::new() } else { format!(", *, {}", sig.join(", ")) };
            out += &format!(
                "    def {}(self{star}) -> Any:\n        \"\"\"{}",
                m["name"].as_str().unwrap(),
                m["help"].as_str().unwrap()
            );
            if !params.is_empty() {
                out += "\n\n";
                for p in params {
                    out += &format!("        {}: {}\n", p["name"].as_str().unwrap(), param_text(p));
                }
                out += "        ";
            }
            out += "\"\"\"\n";
        }
        out
    }

    /// A JSON value as Python writes it.
    fn py_value(v: &Value) -> String {
        match v {
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Null => "None".into(),
            other => other.to_string(),
        }
    }

    /// The API's reference page: each method with its help and parameters.
    fn api_page() -> String {
        let mut out = String::from(
            "# The Opal Studio API\n\n\
             Every method a script or assistant can call (see `design.md`, *Scripting*), made from the\n\
             editor's command registry by the test `api::tests::the_python_stubs_and_api_page_are_current`:\n\
             run it with `OPAL_WRITE_STUBS=1` to make this page again; do not edit it by hand. From Python,\n\
             each is a method of the editor (`editor.new(width=640, height=480)`), taking its parameters by\n\
             name; over the pipe, a JSON-RPC method of the same name.\n\n\
             ## Connecting\n\n\
             The pipe's name and a token are in `api.json` in the user's settings folder\n\
             (`%APPDATA%\\Opal Studio\\config` on Windows), or in the file `OPAL_STUDIO_API` names. Each\n\
             request and each response is one line of JSON-RPC 2.0. A connection's first request must be\n\
             `hello`, with `token`; anything else is refused and the connection dropped. `hello` may also\n\
             name a `script` (the Python package gives the running script's file name): the connection is\n\
             then a script run, and once it closes, what it changed in each image one change after another\n\
             becomes one history state, `Script: <name>`. Without it, each call is a state of its own.\n",
        );
        for kind in ["command", "query"] {
            out += if kind == "command" { "\n## Commands\n" } else { "\n## Queries\n" };
            for m in describe().as_array().unwrap().iter().filter(|m| m["kind"] == json!(kind)) {
                out += &format!("\n### `{}`\n\n{}\n", m["name"].as_str().unwrap(), m["help"].as_str().unwrap());
                let params = m["params"].as_array().unwrap();
                if !params.is_empty() {
                    out += "\n| Parameter | Kind | |\n|---|---|---|\n";
                    for p in params {
                        let kind = match p["kind"].as_str().unwrap() {
                            "choice" => {
                                let words: Vec<String> = p["choices"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|w| format!("`{}`", w.as_str().unwrap()))
                                    .collect();
                                words.join(", ")
                            }
                            k => k.to_owned(),
                        };
                        let required = if p.get("required").is_some() { " (required)" } else { "" };
                        out +=
                            &format!("| `{}`{required} | {kind} | {} |\n", p["name"].as_str().unwrap(), param_text(p));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn the_python_stubs_and_api_page_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python");
        for (file, made) in [("opal/__init__.pyi", python_stubs()), ("API.md", api_page())] {
            let path = root.join(file);
            if std::env::var_os("OPAL_WRITE_STUBS").is_some() {
                std::fs::write(&path, &made).unwrap();
            }
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
            assert!(on_disk == made, "python/{file} is out of date: run this test with OPAL_WRITE_STUBS=1");
        }
    }

    fn now(st: &mut AppState, name: &str, given: Value) -> Result<Value, String> {
        match st.call(name, &args(given), &egui::Context::default()) {
            Reply::Now(r) => r,
            Reply::Later(n) => panic!("{name} answers later ({n})"),
        }
    }

    #[test]
    fn every_parameter_default_is_valid_json_of_its_kind_and_names_are_unique() {
        let methods = methods();
        let mut names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), methods.len(), "two methods share a name");
        for m in methods {
            for p in m.params {
                if let Otherwise::Value(json) = p.otherwise {
                    let v: Value = serde_json::from_str(json).unwrap_or_else(|_| panic!("{}.{}", m.name, p.name));
                    check_value(p, &v).unwrap_or_else(|e| panic!("{}.{}: {e}", m.name, p.name));
                }
            }
        }
        assert!(names.contains(&"save_as") && names.contains(&"merge_down") && names.contains(&"layers"));
        assert!(!names.contains(&"zoom_in"), "the view is the interface's alone");
    }

    #[test]
    fn base64_is_the_standard_padded_alphabet() {
        assert_eq!([base64(b""), base64(b"M"), base64(b"Ma"), base64(b"Man")], ["", "TQ==", "TWE=", "TWFu"]);
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
    }

    #[test]
    fn an_image_is_answered_as_a_png_scaled_to_fit() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 400, "height": 100 })).unwrap();
        let Reply::Later(n) = st.call("image", &args(json!({ "max_side": 100 })), &egui::Context::default()) else {
            panic!("answered at once")
        };
        assert!(st.writer.pending().next().is_none(), "made as an export is, in the status bar");
        let mut answers = Vec::new();
        for _ in 0..500 {
            st.poll_writes(false);
            answers.extend(st.answers());
            if !answers.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(answers.len(), 1);
        let (got, image) = answers.pop().unwrap();
        let image = image.unwrap();
        assert_eq!((got, image["width"].as_u64(), image["height"].as_u64()), (n, Some(100), Some(25)));
        // A PNG's signature, in base64.
        assert!(image["png"].as_str().unwrap().starts_with("iVBORw0KGgo"), "{image}");
        assert!(st.images_made.is_empty());
    }

    #[test]
    fn arguments_are_checked_against_the_parameters() {
        let p = params(Cmd::New).unwrap();
        assert!(Args::check(p, &args(json!({ "width": 10, "height": 20 }))).is_ok());
        let err = |v| Args::check(p, &args(v)).unwrap_err();
        assert!(err(json!({ "width": 10 })).contains("\"height\" must be given"));
        assert!(err(json!({ "width": 0, "height": 5 })).contains("between 1 and"));
        assert!(err(json!({ "width": "ten", "height": 5 })).contains("a whole number"));
        assert!(err(json!({ "width": 1, "height": 5, "colour": 1 })).contains("not a parameter"));
        assert!(err(json!({ "width": 1, "height": 5, "background": "blue" })).contains("one of white, transparent"));
        let fixed = Args::check(p, &args(json!({ "width": 1, "height": 5 }))).unwrap();
        assert_eq!(fixed.text("background"), Some("white"), "the default stands in");
    }

    #[test]
    fn a_call_that_changes_nothing_records_nothing() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 64, "height": 32 })).unwrap();
        let layer = now(&mut st, "new_layer", json!({ "name": "Sky" })).unwrap()["layer"].as_u64().unwrap();
        let at = st.active().unwrap().history.current();
        // Already visible, and not locked: no step of history, and nothing to save.
        now(&mut st, "toggle_visible", json!({ "layers": [layer], "visible": true })).unwrap();
        now(&mut st, "toggle_lock", json!({ "layers": [layer], "locked": false })).unwrap();
        assert_eq!(st.active().unwrap().history.current(), at, "a step that changed nothing");
        now(&mut st, "toggle_visible", json!({ "layers": [layer], "visible": false })).unwrap();
        assert_ne!(st.active().unwrap().history.current(), at);
    }

    #[test]
    fn what_cannot_be_done_as_asked_is_refused() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 10, "height": 30000 })).unwrap();
        // The other side, in proportion, would be far past the most a side can be.
        // (In a folder that is not there: were it not refused, the write would fail at once.)
        let path = std::env::temp_dir().join("opal-no-such-folder").join("strip.png").display().to_string();
        let err = now(&mut st, "export", json!({ "path": path, "width": 3000 })).unwrap_err();
        assert!(err.contains("give both sides"), "{err}");
        // A blank name, which the interface would never give a layer.
        let err = now(&mut st, "new_layer", json!({ "name": "  " })).unwrap_err();
        assert!(err.contains("not blank"), "{err}");
        let layer = now(&mut st, "new_layer", json!({})).unwrap()["layer"].as_u64().unwrap();
        let err = now(&mut st, "rename", json!({ "layer": layer, "name": "" })).unwrap_err();
        assert!(err.contains("not blank"), "{err}");
    }

    #[test]
    fn a_file_handed_over_to_open_is_told_of_here_and_brings_the_window_up() {
        let mut st = AppState::new();
        let missing = std::env::temp_dir().join("opal-api-no-such-file.png");
        let given = args(json!({ "path": missing.display().to_string() }));
        // A notification: no one hears its answer, so the user is told, as by a dialog.
        st.call_from(None, false, "open", &given, &egui::Context::default());
        assert!(st.messages.iter().any(|m| m.kind == MessageKind::Error), "what opening it said was lost");
        assert!(st.raise_window);
    }

    #[test]
    fn scripts_wait_while_a_transform_box_is_open() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 64, "height": 32 })).unwrap();
        assert!(!st.scripts_wait(false));
        assert!(st.scripts_wait(true), "a button held");
        assert!(st.active_mut().unwrap().begin_transform(opal_core::resample::Sampling::default()));
        assert!(st.scripts_wait(false), "the box is an edit in progress");
    }

    #[test]
    fn a_script_makes_an_image_and_edits_its_layers() {
        let mut st = AppState::new();
        let made = now(&mut st, "new", json!({ "width": 64, "height": 32, "background": "transparent" })).unwrap();
        let doc = made["document"].as_u64().unwrap();
        let docs = now(&mut st, "documents", json!({})).unwrap();
        assert_eq!((docs[0]["document"].as_u64(), docs[0]["width"].as_u64()), (Some(doc), Some(64)));
        let layer =
            now(&mut st, "new_layer", json!({ "document": doc, "name": "Sky" })).unwrap()["layer"].as_u64().unwrap();
        let layers = now(&mut st, "layers", json!({ "document": doc })).unwrap();
        assert_eq!((layers[0]["layer"].as_u64(), layers[0]["name"].as_str()), (Some(layer), Some("Sky")), "{layers}");
        now(&mut st, "toggle_visible", json!({ "layers": [layer], "visible": false })).unwrap();
        now(&mut st, "toggle_visible", json!({ "layers": [layer], "visible": false })).unwrap();
        assert_eq!(now(&mut st, "layers", json!({})).unwrap()[0]["visible"], json!(false), "a second hide showed it");
        now(&mut st, "rename", json!({ "layer": layer, "name": "Clouds" })).unwrap();
        now(&mut st, "undo", json!({})).unwrap();
        assert_eq!(now(&mut st, "layers", json!({})).unwrap()[0]["name"], json!("Sky"));
        let canvas = now(&mut st, "resize_canvas", json!({ "width": 100, "anchor": "top-left" })).unwrap();
        assert_eq!(canvas, json!({ "width": 100, "height": 32 }));
        // Refused, with the reason, and nothing left to show in a dialog.
        let err = now(&mut st, "rename", json!({ "layer": 999, "name": "x" })).unwrap_err();
        assert!(err.contains("no layer 999"), "{err}");
        let err = now(&mut st, "close", json!({})).unwrap_err();
        assert!(err.contains("unsaved"), "{err}");
        now(&mut st, "close", json!({ "discard": true })).unwrap();
        assert!(now(&mut st, "documents", json!({})).unwrap().as_array().unwrap().is_empty());
        assert!(st.messages.is_empty(), "{:?}", st.messages);
        let err = now(&mut st, "zoom_in", json!({})).unwrap_err();
        assert!(err.contains("no method"), "{err}");
    }

    /// Waits for the answer to call `n`, polling the writes.
    fn later(st: &mut AppState, n: u64) -> Result<Value, String> {
        for _ in 0..1000 {
            st.poll_writes(false);
            if let Some((_, r)) = st.answers().into_iter().find(|(m, _)| *m == n) {
                return r;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("call {n} was never answered");
    }

    #[test]
    fn files_are_named_as_the_interface_names_them_and_not_written_over() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        now(&mut st, "new", json!({ "width": 40, "height": 20 })).unwrap();
        let dir = std::env::temp_dir().join(format!("opal-api-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file of another kind, there already: saving "as" it adds .opal, as the dialog does.
        let photo = dir.join("photo.jpg");
        std::fs::write(&photo, b"not to be lost").unwrap();
        let Reply::Later(n) = st.call("save_as", &args(json!({ "path": photo.display().to_string() })), &ctx) else {
            panic!("answered at once")
        };
        let saved = later(&mut st, n).unwrap();
        assert_eq!(saved["path"], json!(dir.join("photo.jpg.opal").display().to_string()));
        assert_eq!(std::fs::read(&photo).unwrap(), b"not to be lost");
        // Over a file there already, only when told to; its own file, always.
        let other = dir.join("other.opal");
        std::fs::write(&other, b"x").unwrap();
        let refused = now(&mut st, "save_as", json!({ "path": other.display().to_string() })).unwrap_err();
        assert!(refused.contains("there already"), "{refused}");
        let Reply::Later(n) = st.call("save", &args(json!({})), &ctx) else { panic!("refused its own file") };
        later(&mut st, n).unwrap();
        // An export of one side given keeps the proportions.
        let png = dir.join("half.png");
        let Reply::Later(n) = st.call("export", &args(json!({ "path": png.display().to_string(), "width": 20 })), &ctx)
        else {
            panic!("answered at once")
        };
        later(&mut st, n).unwrap();
        let header = std::fs::read(&png).unwrap();
        let side = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
        assert_eq!((side(16), side(20)), (20, 10), "not in proportion");
        let again = now(&mut st, "export", json!({ "path": png.display().to_string() })).unwrap_err();
        assert!(again.contains("overwrite"), "{again}");
        let Reply::Later(n) =
            st.call("export", &args(json!({ "path": png.display().to_string(), "overwrite": true })), &ctx)
        else {
            panic!("refused with overwrite")
        };
        later(&mut st, n).unwrap();
        // A relative path: the editor's folder is not the caller's.
        let relative = now(&mut st, "export", json!({ "path": "sky.png" })).unwrap_err();
        assert!(relative.contains("absolute"), "{relative}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_refusal_fails_the_call_and_leaves_the_users_view_as_it_was() {
        let mut st = AppState::new();
        let first = now(&mut st, "new", json!({ "width": 30, "height": 30, "background": "transparent" })).unwrap();
        let first = first["document"].as_u64().unwrap();
        let a = now(&mut st, "new_layer", json!({ "name": "a" })).unwrap()["layer"].as_u64().unwrap();
        let b = now(&mut st, "new_layer", json!({ "name": "b" })).unwrap()["layer"].as_u64().unwrap();
        now(&mut st, "toggle_visible", json!({ "layers": [a, b], "visible": true })).unwrap();
        let second = now(&mut st, "new", json!({ "width": 10, "height": 10 })).unwrap()["document"].as_u64().unwrap();
        // The user is on the second image; a script's call on the first is refused (its
        // layers are all empty: told as information, a refusal).
        let refused = now(&mut st, "crop_to_content", json!({ "document": first })).unwrap_err();
        assert!(refused.contains("empty"), "{refused}");
        assert_eq!(st.active_doc.map(|d| d.0), Some(second), "the active image changed");
        let d = st.doc(DocId(first)).unwrap();
        assert_eq!(d.selection(), [LayerId(a), LayerId(b)], "the selection changed");
        // Ids named twice, each once.
        let both = now(&mut st, "toggle_visible", json!({ "document": first, "layers": [a, b, a], "visible": false }))
            .unwrap();
        assert_eq!(both["layers"], json!([a, b]));
        let layers = now(&mut st, "layers", json!({ "document": first })).unwrap();
        assert!(
            layers.as_array().unwrap().iter().filter(|l| l["layer"] != json!(1)).all(|l| l["visible"] == json!(false))
        );
        // One step of history for both.
        now(&mut st, "undo", json!({ "document": first })).unwrap();
        let layers = now(&mut st, "layers", json!({ "document": first })).unwrap();
        assert!(layers.as_array().unwrap().iter().all(|l| l["visible"] == json!(true)), "{layers}");
        assert!(st.messages.is_empty());
    }

    #[test]
    fn a_layer_made_with_a_name_is_one_step_of_history() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 10, "height": 10 })).unwrap();
        let before = now(&mut st, "layers", json!({})).unwrap().as_array().unwrap().len();
        now(&mut st, "new_layer", json!({ "name": "Sky" })).unwrap();
        now(&mut st, "undo", json!({})).unwrap();
        let after = now(&mut st, "layers", json!({})).unwrap();
        assert_eq!(after.as_array().unwrap().len(), before, "one undo left a layer: {after}");
    }

    #[test]
    fn a_copy_is_answered_once_on_the_clipboard() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 30, "height": 20 })).unwrap();
        let Reply::Later(n) = st.call("copy_merged", &args(json!({})), &egui::Context::default()) else {
            panic!("answered before the clipboard had it")
        };
        let mut answers = Vec::new();
        for _ in 0..500 {
            st.poll_copies();
            answers.extend(st.answers());
            if !answers.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(answers, vec![(n, Ok(json!({ "width": 30, "height": 20 })))]);
        assert!(st.messages.is_empty());
    }

    #[test]
    fn what_a_dialog_would_have_said_comes_back_with_the_call() {
        let mut st = AppState::new();
        let missing = std::env::temp_dir().join("opal-no-such-file.png");
        let err = now(&mut st, "open", json!({ "path": missing.display().to_string() })).unwrap_err();
        assert!(err.contains("Could not open"), "{err}");
        assert!(st.messages.is_empty(), "the error was left to a dialog too");
    }

    #[test]
    fn a_save_is_answered_once_written() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        let ctx = egui::Context::default();
        now(&mut st, "new", json!({ "width": 8, "height": 8 })).unwrap();
        let path = std::env::temp_dir().join(format!("opal-api-{}.opal", std::process::id()));
        let Reply::Later(n) = st.call("save_as", &args(json!({ "path": path.display().to_string() })), &ctx) else {
            panic!("answered at once")
        };
        let mut answers = Vec::new();
        for _ in 0..500 {
            st.poll_writes(false);
            answers.extend(st.answers());
            if !answers.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::fs::remove_file(&path).ok();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].0, n);
        assert!(answers[0].1.is_ok(), "{:?}", answers[0].1);
        assert!(!now(&mut st, "documents", json!({})).unwrap()[0]["unsaved"].as_bool().unwrap());
    }
}
