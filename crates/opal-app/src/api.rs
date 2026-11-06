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
use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::Pixel;
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
    /// The command it carries out; `None` for a query, or a change the API alone makes.
    pub cmd: Option<Cmd>,
    /// Whether it changes images (a command), rather than asking about them (a query).
    pub changes: bool,
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
        Paste | PasteInPlace | Transform | Preferences | Exit | RunScript | ConnectAssistant | ZoomIn | ZoomOut
        | ZoomFit | Zoom100 | ToggleOffCanvas | ToggleTheme | ToggleRibbon | CycleToolsPlace | ResetPanels | Search
        | Help | Shortcuts | About | WidgetGallery => return None,
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
        "What an image shows (or one of its layers alone), as a PNG, scaled down to fit a size (never \
         enlarged): its bytes in base64, with its width and height.",
        &[
            DOCUMENT,
            Param {
                name: "layer",
                kind: Kind::Layer,
                help: "One layer alone, by id: as if shown, at full opacity.",
                otherwise: Otherwise::Now("the image as shown, all its visible layers together"),
            },
            Param {
                name: "max_side",
                kind: Kind::Int { min: 16, max: 8192 },
                help: "The most pixels it may be across or down.",
                otherwise: Otherwise::Value("1024"),
            },
        ],
    ),
    (
        "pixels",
        "The pixels of an image, as shown, or of one layer alone, within a rectangle of the canvas: in shared \
         memory that the answer names (to be released once opened), or in the answer, in base64.",
        &[
            DOCUMENT,
            Param {
                name: "layer",
                kind: Kind::Layer,
                help: "One layer alone, by id: as if shown, at full opacity.",
                otherwise: Otherwise::Now("the image as shown, all its visible layers together"),
            },
            REGION_X,
            REGION_Y,
            REGION_WIDTH,
            REGION_HEIGHT,
            PIXEL_FORMAT,
            Param {
                name: "shared",
                kind: Kind::Bool,
                help: "In shared memory, which the answer names as shm, rather than in the answer (on Windows: \
                       elsewhere they always come in the answer).",
                otherwise: Otherwise::Value("true"),
            },
        ],
    ),
    (
        "release_pixels",
        "Lets go of the shared memory pixels gave, once it has been opened (it goes by itself a minute after).",
        &[Param {
            name: "shm",
            kind: Kind::Text,
            help: "The shared memory, by the name pixels gave it.",
            otherwise: Otherwise::Required,
        }],
    ),
    ("methods", "Every method of the API, with its parameters: what this list is generated from.", &[]),
];

/// The changes the API alone makes (no command of the interface makes them): their names,
/// what they do, and their parameters.
const CHANGES: &[(&str, &str, &[Param])] = &[(
    "put_pixels",
    "Puts pixels into an image, at a place on its canvas: as a new layer above the active one, or into a pixel \
     layer, replacing its pixels there. They come in shared memory the caller made, or in base64.",
    &[
        DOCUMENT,
        Param {
            name: "layer",
            kind: Kind::Layer,
            help: "A pixel layer, by id, whose pixels they replace there.",
            otherwise: Otherwise::Now("a new layer, above the active one"),
        },
        Param {
            name: "x",
            kind: Kind::Int { min: -SIDE, max: 2 * SIDE },
            help: "Where their left edge goes, in pixels from the canvas's.",
            otherwise: Otherwise::Value("0"),
        },
        Param {
            name: "y",
            kind: Kind::Int { min: -SIDE, max: 2 * SIDE },
            help: "Where their top edge goes, in pixels from the canvas's.",
            otherwise: Otherwise::Value("0"),
        },
        side("width", "How many pixels across.", Otherwise::Required),
        side("height", "How many pixels down.", Otherwise::Required),
        PIXEL_FORMAT,
        Param {
            name: "shm",
            kind: Kind::Text,
            help: "The shared memory holding them, by name: made by the caller, with width × height pixels.",
            otherwise: Otherwise::Now("none: they are in data"),
        },
        Param {
            name: "data",
            kind: Kind::Text,
            help: "The pixels in base64, if not in shared memory (a request is at most 1 MB).",
            otherwise: Otherwise::Now("none: they are in shm"),
        },
        Param {
            name: "name",
            kind: Kind::Text,
            help: "The new layer's name.",
            otherwise: Otherwise::Now("a numbered one"),
        },
    ],
)];

const REGION_X: Param = Param {
    name: "x",
    kind: Kind::Int { min: -SIDE, max: 2 * SIDE },
    help: "The rectangle's left edge, in pixels from the canvas's.",
    otherwise: Otherwise::Value("0"),
};
const REGION_Y: Param = Param {
    name: "y",
    kind: Kind::Int { min: -SIDE, max: 2 * SIDE },
    help: "The rectangle's top edge, in pixels from the canvas's.",
    otherwise: Otherwise::Value("0"),
};
const REGION_WIDTH: Param = side("width", "The rectangle's width, in pixels.", Otherwise::Now("the canvas's"));
const REGION_HEIGHT: Param = side("height", "The rectangle's height, in pixels.", Otherwise::Now("the canvas's"));
const PIXEL_FORMAT: Param = Param {
    name: "format",
    kind: Kind::Choice(&["srgb8", "srgb16", "linear"]),
    help: "How each pixel is given, row by row from the top left, red, green, blue and alpha (straight, not \
           premultiplied): srgb8 as bytes in sRGB; srgb16 as 16-bit values in sRGB, little-endian; linear as \
           32-bit floats in linear light, little-endian.",
    otherwise: Otherwise::Value("\"srgb8\""),
};

/// How pixels are given to and taken from scripts (see [`PIXEL_FORMAT`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum PixelFormat {
    Srgb8,
    Srgb16,
    Linear,
}

impl PixelFormat {
    fn named(name: &str) -> PixelFormat {
        match name {
            "srgb16" => PixelFormat::Srgb16,
            "linear" => PixelFormat::Linear,
            _ => PixelFormat::Srgb8,
        }
    }

    fn name(self) -> &'static str {
        match self {
            PixelFormat::Srgb8 => "srgb8",
            PixelFormat::Srgb16 => "srgb16",
            PixelFormat::Linear => "linear",
        }
    }

    fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Srgb8 => 4,
            PixelFormat::Srgb16 => 8,
            PixelFormat::Linear => 16,
        }
    }

    /// Writes `pixels` into `out` (as many bytes as they take) in this format.
    fn write(self, pixels: &[Pixel], out: &mut [u8]) {
        use rayon::prelude::*;
        let put = |o: &mut [u8], values: &[[u8; 4]]| {
            for (chunk, v) in o.chunks_mut(4).zip(values) {
                chunk.copy_from_slice(v);
            }
        };
        out.par_chunks_mut(self.bytes_per_pixel()).zip(pixels.par_iter()).for_each(|(o, &p)| match self {
            PixelFormat::Srgb8 => o.copy_from_slice(&opal_io::pixels::to_srgb8(p)),
            PixelFormat::Srgb16 => {
                let [r, g, b, a] = opal_io::pixels::to_srgb16(p).map(u16::to_le_bytes);
                o.copy_from_slice(&[r[0], r[1], g[0], g[1], b[0], b[1], a[0], a[1]]);
            }
            PixelFormat::Linear => {
                let [r, g, b, a] = p.map(half::f16::to_f32);
                let k = if a > 0.0 { 1.0 / a } else { 0.0 };
                put(o, &[r * k, g * k, b * k, a].map(f32::to_le_bytes));
            }
        });
    }

    /// The pixels `bytes` holds in this format. A linear value that is not a number is
    /// taken as 0, one past a half float's range as its largest, and alpha is kept within 0
    /// to 1.
    fn read(self, bytes: &[u8]) -> Vec<Pixel> {
        use half::f16;
        use rayon::prelude::*;
        bytes
            .par_chunks_exact(self.bytes_per_pixel())
            .map(|c| match self {
                PixelFormat::Srgb8 => opal_io::pixels::from_srgb8([c[0], c[1], c[2], c[3]]),
                PixelFormat::Srgb16 => {
                    let v = |i: usize| u16::from_le_bytes([c[2 * i], c[2 * i + 1]]);
                    opal_io::pixels::from_srgb16([v(0), v(1), v(2), v(3)])
                }
                PixelFormat::Linear => {
                    let v = |i: usize| {
                        let f = f32::from_le_bytes([c[4 * i], c[4 * i + 1], c[4 * i + 2], c[4 * i + 3]]);
                        if f.is_finite() { f } else { 0.0 }
                    };
                    let a = v(3).clamp(0.0, 1.0);
                    // Within what a half float holds: past it, infinity, which compositing
                    // over turns to NaN (infinity times zero).
                    let channel = |i: usize| f16::from_f32((v(i) * a).clamp(-f16::MAX.to_f32(), f16::MAX.to_f32()));
                    if a == 0.0 { [f16::ZERO; 4] } else { [channel(0), channel(1), channel(2), f16::from_f32(a)] }
                }
            })
            .collect()
    }
}

/// What a method made off the interface thread answers with; the shared memory to keep until
/// the caller has opened it, if any; and, for a change, what is left to do on the interface
/// thread, whose result is the answer instead.
pub struct Made {
    answer: Value,
    keep: Option<(String, crate::shared_memory::Mapping)>,
    finish: Option<Finish>,
}

/// What a change made ready off the interface thread does on it: its answer.
pub type Finish = Box<dyn FnOnce(&mut AppState) -> Result<Value, String> + Send>;

/// Pixels to put in, made ready: as a new layer's image (at the canvas's origin), or the
/// pixels themselves, to go into a layer.
enum Content {
    Image(Box<RasterImage>),
    Pixels(Vec<Pixel>),
}

/// Whether pixels can go into layer `target` of `doc`: one that is not locked and holds
/// nothing yet or pixels alone, in one object.
fn pixels_go_into(doc: &opal_core::document::Document, target: LayerId) -> Result<(), String> {
    let layer = doc.layer(target).ok_or_else(|| format!("There is no layer {} in the image.", target.0))?;
    if layer.locked {
        return Err(format!("\"{}\" is locked. Unlock it first.", layer.name));
    }
    match &layer.content {
        LayerContent::Objects(objects) if objects.is_empty() => Ok(()),
        LayerContent::Objects(objects) if objects.len() == 1 && matches!(objects[0].kind, ObjectKind::Pixels(_)) => {
            Ok(())
        }
        LayerContent::Group(_) => Err(format!("\"{}\" is a group: put them in a layer of it.", layer.name)),
        LayerContent::Objects(_) => Err(format!(
            "\"{}\" holds an image or several objects, which pixels do not go into: put them in a new layer.",
            layer.name
        )),
    }
}

/// Puts `content` into image `id` (made ready by `put_pixels`), at `rect` of the canvas: as
/// a new layer above the active one (called `name`), or into layer `target`, replacing its
/// pixels there; as made by script run `script`, if the call was. The layer's id.
fn put_pixels_now(
    st: &mut AppState,
    id: DocId,
    target: Option<LayerId>,
    rect: IRect,
    content: Content,
    name: Option<String>,
    script: Option<u64>,
) -> Result<Value, String> {
    let d = st.doc_mut(id).ok_or("The image was closed before the pixels could be put in.")?;
    // The call's run, as when it was carried out (its changes join the run's).
    let before = std::mem::replace(&mut d.script, script);
    let into = (|| match (target, content) {
        (None, Content::Image(image)) => {
            let pos = AppState::above_active(d);
            let mut made = None;
            d.apply("Put pixels", |doc, ids| {
                let name = name.unwrap_or_else(|| format!("Layer {}", crate::layer_ops::count_layers(doc) + 1));
                let mut layer = Layer::new_raster(ids.layer(), ids.object(), name, *image);
                if let LayerContent::Objects(objects) = &mut layer.content
                    && let ObjectKind::Pixels(p) = &mut objects[0].kind
                {
                    p.offset = [rect.x0, rect.y0];
                }
                made = Some(layer.id);
                doc.insert(pos, layer)
            })
            .map_err(|e| e.to_string())?;
            Ok(made.expect("added"))
        }
        (Some(target), Content::Pixels(pixels)) => {
            pixels_go_into(d.doc(), target)?;
            d.apply("Put pixels", |doc, ids| {
                let l = doc.layer_mut(target).ok_or(opal_core::document::TreeError::NoSuchLayer(target))?;
                let LayerContent::Objects(objects) = &mut l.content else { unreachable!("checked") };
                if objects.is_empty() {
                    let mut image = RasterImage::new();
                    image.write_rect(&rect, &pixels);
                    objects.push(opal_core::document::Object::pixels(ids.object(), image));
                } else if let ObjectKind::Pixels(p) = &mut objects[0].kind {
                    let local = rect.translate(-p.offset[0], -p.offset[1]);
                    Arc::make_mut(&mut p.image).write_rect(&local, &pixels);
                }
                Ok::<(), opal_core::document::TreeError>(())
            })
            .map_err(|e| e.to_string())?;
            Ok(target)
        }
        _ => unreachable!("an image is made for a new layer, pixels for one there"),
    })();
    d.script = before;
    into.map(|layer| json!({ "layer": layer.0 }))
}

/// The methods clients give in their own way, not as they are: the list of methods itself,
/// and those of pixels, which pass through shared memory (the Python package makes them
/// numpy arrays; an assistant sees an image as `image` gives it).
pub const OWN_WAY: &[&str] = &["methods", "pixels", "put_pixels", "release_pixels"];

/// How long shared memory pixels gave is kept, if the caller never releases it.
const KEEP_SHARED: std::time::Duration = std::time::Duration::from_secs(60);

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
        Some(Method {
            name: snake(&format!("{cmd:?}")),
            help: cmd.info().description,
            params,
            cmd: Some(cmd),
            changes: true,
        })
    });
    let others = QUERIES.iter().map(|&(name, help, params)| (name, help, params, false));
    let others = others.chain(CHANGES.iter().map(|&(name, help, params)| (name, help, params, true)));
    commands
        .chain(others.map(|(name, help, params, changes)| Method {
            name: name.to_owned(),
            help,
            params,
            cmd: None,
            changes,
        }))
        .collect()
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
        let view = method
            .cmd
            .map(|_| (self.active_doc, self.docs.iter().map(|d| (d.id, d.selection_state())).collect::<Vec<_>>()));
        let reply = match method.cmd {
            Some(cmd) => self.carry_out(cmd, method.params, &args, ctx),
            None if method.name == "image" => self.image_by_args(&args, ctx),
            None if method.name == "pixels" => self.pixels_by_args(&args, ctx),
            None if method.name == "put_pixels" => self.put_pixels_by_args(&args, ctx),
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
        if let Some((active, selections)) = view
            && matches!(reply, Reply::Now(Err(_)))
        {
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
    fn answer(&mut self, name: &str, args: &Args) -> Result<Value, String> {
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
            "release_pixels" => {
                // Whether or not it is still kept (it goes by itself after a while).
                self.shared_out.remove(args.text("shm").expect("required"));
                Ok(json!({}))
            }
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
        let mut doc = self.doc(id).expect("found").doc().clone();
        doc.layers = match shown_layers(&doc, args) {
            Ok(layers) => layers,
            Err(why) => return Reply::Now(Err(why)),
        };
        let c = doc.canvas;
        let k = (args.int("max_side") as f64 / f64::from(c.width.max(c.height))).min(1.0);
        let side = |s: u32| ((f64::from(s) * k).round() as u32).max(1);
        let (width, height) = (side(c.width), side(c.height));
        let size = ((width, height) != (c.width, c.height)).then(|| ExportSize {
            width,
            height,
            sampling: crate::prefs::sampling(&self.prefs, "export"),
        });
        self.off_thread("image", ctx, move || {
            // Kept while it is made, as an export's: it caps the tiles' memory.
            let mut memory = crate::memory::Allowance::now();
            memory.take(opal_io::image_files::export_memory(&doc, size.as_ref()))?;
            let png = opal_io::image_files::png_in_memory(&doc, size.as_ref(), Some(crate::background::pool()))?;
            Ok(Made {
                answer: json!({ "png": base64(&png), "width": width, "height": height }),
                keep: None,
                finish: None,
            })
        })
    }

    /// Reads the pixels `args` asks for (the `pixels` query) on a thread of its own.
    fn pixels_by_args(&mut self, args: &Args, ctx: &egui::Context) -> Reply {
        let id = match self.which(args) {
            Ok(id) => id,
            Err(why) => return Reply::Now(Err(why)),
        };
        let doc = self.doc(id).expect("found").doc();
        let layers = match shown_layers(doc, args) {
            Ok(layers) => layers,
            Err(why) => return Reply::Now(Err(why)),
        };
        let canvas = doc.canvas.rect();
        let (x0, y0) = (canvas.x0 + args.int("x"), canvas.y0 + args.int("y"));
        let width = args.opt_int("width").unwrap_or(canvas.width());
        let height = args.opt_int("height").unwrap_or(canvas.height());
        let rect = IRect::new(x0, y0, x0 + width, y0 + height);
        let format = PixelFormat::named(args.text("format").expect("it has a default"));
        // Elsewhere, there is no shared memory yet: always in the answer.
        let shared = cfg!(windows) && args.flag("shared") == Some(true);
        let count = rect.width() as u64 * rect.height() as u64;
        let bytes = count * format.bytes_per_pixel() as u64;
        self.off_thread("pixels", ctx, move || {
            let len =
                usize::try_from(bytes).map_err(|_| "pixels: that is more than this computer can hold.".to_owned())?;
            let mut memory = crate::memory::Allowance::now();
            memory.take_tiles(opal_core::composite::composite_cost(&layers, Some(&rect)))?;
            // The pixels read, and what they are written to (in base64, a third more again).
            let written = if shared { bytes } else { bytes + bytes.div_ceil(3) * 4 };
            memory.take(crate::memory::Need::at_once(count * 8 + written))?;
            // The work in parallel goes to the background pool, not the interface thread's.
            let pool = crate::background::pool();
            let pixels = pool.install(|| opal_core::composite::flatten(&layers, &rect));
            let mut answer = json!({ "width": rect.width(), "height": rect.height(), "format": format.name() });
            if shared {
                let name = crate::shared_memory::new_name();
                let mut mapping = crate::shared_memory::Mapping::create(&name, len)?;
                pool.install(|| format.write(&pixels, mapping.bytes_mut()));
                answer["shm"] = json!(name);
                answer["bytes"] = json!(len);
                Ok(Made { answer, keep: Some((name, mapping)), finish: None })
            } else {
                let mut out = vec![0u8; len];
                pool.install(|| format.write(&pixels, &mut out));
                answer["data"] = json!(base64(&out));
                Ok(Made { answer, keep: None, finish: None })
            }
        })
    }

    /// Puts the pixels `args` gives into an image (the `put_pixels` change): read and made
    /// tiles on a thread of its own, then put in on this one, when the user is not busy (see
    /// [`answers_while`](Self::answers_while)); its answer, the layer they went into.
    fn put_pixels_by_args(&mut self, args: &Args, ctx: &egui::Context) -> Reply {
        let now = |r: Result<Value, String>| Reply::Now(r);
        let id = match self.which(args) {
            Ok(id) => id,
            Err(why) => return now(Err(why)),
        };
        let d = self.doc(id).expect("found");
        let target = args.layer();
        // Asked again once the pixels are ready: the layer may have changed meanwhile.
        if let Some(t) = target
            && let Err(why) = pixels_go_into(d.doc(), t)
        {
            return now(Err(why));
        }
        let (width, height) = (args.int("width"), args.int("height"));
        let format = PixelFormat::named(args.text("format").expect("it has a default"));
        let count = width as u64 * height as u64;
        let Ok(len) = usize::try_from(count * format.bytes_per_pixel() as u64) else {
            return now(Err("put_pixels: that is more than this computer can hold.".to_owned()));
        };
        let source = match (args.text("shm"), args.text("data")) {
            (Some(name), None) => Ok(name.to_owned()),
            (None, Some(data)) => Err(data.to_owned()),
            _ => return now(Err("put_pixels: give the pixels in shm or in data, one of them.".to_owned())),
        };
        let canvas = d.doc().canvas.rect();
        let rect = IRect::new(canvas.x0 + args.int("x"), canvas.y0 + args.int("y"), 0, 0);
        let rect = IRect::new(rect.x0, rect.y0, rect.x0 + width, rect.y0 + height);
        let name = args.text("name").map(str::to_owned);
        let script = d.script;
        let (w, h) = (width as u32, height as u32);
        self.off_thread("put", ctx, move || {
            // What they are read into, and the tiles they become.
            let mut memory = crate::memory::Allowance::now();
            memory.take(crate::memory::Need::at_once(count * 16))?;
            let pool = crate::background::pool();
            let made = |bytes: &[u8]| match (target, format) {
                // Kept at 8 bits, as an 8-bit image file's pixels are.
                (None, PixelFormat::Srgb8) => {
                    Content::Image(Box::new(RasterImage::from_srgb8(w, h, bytemuck::cast_slice(bytes))))
                }
                (None, _) => {
                    Content::Image(Box::new(RasterImage::from_pixels(w, h, &pool.install(|| format.read(bytes)))))
                }
                (Some(_), _) => Content::Pixels(pool.install(|| format.read(bytes))),
            };
            let content = match source {
                Ok(shm) => made(&crate::shared_memory::Mapping::open(&shm, len)?.bytes()[..len]),
                Err(data) => {
                    let raw = unbase64(&data).ok_or("put_pixels: data is not base64.")?;
                    if raw.len() != len {
                        return Err(format!(
                            "put_pixels: data holds {} bytes, not the {len} of {width} × {height} {} pixels.",
                            raw.len(),
                            format.name()
                        ));
                    }
                    made(&raw)
                }
            };
            let finish: Finish = Box::new(move |st| put_pixels_now(st, id, target, rect, content, name, script));
            Ok(Made { answer: Value::Null, keep: None, finish: Some(finish) })
        })
    }

    /// Runs `make` (making the answer to a call that does `what`) on a thread of its own:
    /// its answer comes as [`answers`](Self::answers) does the number this replies with.
    fn off_thread(
        &mut self,
        what: &str,
        ctx: &egui::Context,
        make: impl FnOnce() -> Result<Made, String> + Send + 'static,
    ) -> Reply {
        self.next_answer += 1;
        let n = self.next_answer;
        let (send, made) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let run = move || {
            let _ = send.send(make());
            ctx.request_repaint();
        };
        if let Err(e) = std::thread::Builder::new().name(format!("api {what}")).spawn(run) {
            return Reply::Now(Err(format!("The {what} could not be made: {e}")));
        }
        self.made.push((n, made));
        Reply::Later(n)
    }

    /// [`answers_while`](Self::answers_while) the user is not busy.
    #[cfg(test)]
    pub fn answers(&mut self) -> Vec<(u64, Result<Value, String>)> {
        self.answers_while(false, false)
    }

    /// The answers to calls whose writes, copies to the clipboard, images or pixels have
    /// finished since asked last, by number. Shared memory pixels were put in is kept until
    /// the caller releases it, or for a minute. A change made ready off this thread is made
    /// once the user is not `busy` (as a call waits then), and refused once `quitting`.
    pub fn answers_while(&mut self, busy: bool, quitting: bool) -> Vec<(u64, Result<Value, String>)> {
        let mut answered = std::mem::take(&mut self.answered);
        let shared_out = &mut self.shared_out;
        let finishing = &mut self.finishing;
        shared_out.retain(|_, (_, since)| since.elapsed() < KEEP_SHARED);
        self.made.retain_mut(|(n, made)| match made.try_recv() {
            Ok(Ok(made)) => {
                if let Some((name, mapping)) = made.keep {
                    shared_out.insert(name, (mapping, std::time::Instant::now()));
                }
                match made.finish {
                    Some(finish) => finishing.push_back((*n, finish)),
                    None => answered.push((*n, Ok(made.answer))),
                }
                false
            }
            Ok(Err(why)) => {
                answered.push((*n, Err(why)));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                answered.push((*n, Err("The answer could not be made.".to_owned())));
                false
            }
        });
        while (quitting || !busy)
            && let Some((n, finish)) = self.finishing.pop_front()
        {
            let result = if quitting { Err("The editor is closing.".to_owned()) } else { finish(self) };
            answered.push((n, result));
        }
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

/// The layers a query about `doc` shows: the one `args` names (alone, shown, at full
/// opacity), or all of them.
fn shown_layers(doc: &opal_core::document::Document, args: &Args) -> Result<Vec<Arc<Layer>>, String> {
    match args.layer() {
        None => Ok(doc.layers.clone()),
        Some(id) => doc
            .layer(id)
            .map(|l| vec![Arc::new(Layer { visible: true, opacity: 1.0, ..l.clone() })])
            .ok_or_else(|| format!("There is no layer {} in the image.", id.0)),
    }
}

/// The bytes `text` holds in base64 (the standard alphabet; padding optional), or `None` if
/// it is not base64.
pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let text = text.trim_end_matches('=').as_bytes();
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 2);
    for chunk in text.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= u32::from(digit(c)?) << (18 - 6 * i);
        }
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    Some(out)
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
                json!({ "name": m.name, "help": m.help, "kind": if m.changes { "command" } else { "query" }, "params": params })
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
             \x20   def pixels(self, document: int | None = None, layer: int | None = None, x: int = 0, y: int = 0, \
             width: int | None = None, height: int | None = None, \
             format: Literal[\"srgb8\", \"srgb16\", \"linear\"] = \"srgb8\") -> Any: ...\n\
             \x20   def put_pixels(self, pixels: Any, document: int | None = None, layer: int | None = None, \
             x: int = 0, y: int = 0, name: str | None = None) -> int: ...\n\
             \x20   def disconnect(self) -> None: ...\n\
             \x20   def __enter__(self) -> Editor: ...\n\
             \x20   def __exit__(self, *exc: Any) -> None: ...\n",
        );
        // The client gives these itself (`methods`, the list it fetched on connecting).
        for m in describe().as_array().unwrap().iter().filter(|m| !OWN_WAY.contains(&m["name"].as_str().unwrap())) {
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
        assert!(st.made.is_empty());
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

    /// The answer to call `reply`, waited for if it comes later.
    fn answered(st: &mut AppState, reply: Reply) -> Result<Value, String> {
        let n = match reply {
            Reply::Now(r) => return r,
            Reply::Later(n) => n,
        };
        for _ in 0..1000 {
            st.poll_writes(false);
            if let Some((_, r)) = st.answers().into_iter().find(|(m, _)| *m == n) {
                return r;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("call {n} was never answered")
    }

    fn call(st: &mut AppState, name: &str, given: Value) -> Result<Value, String> {
        let reply = st.call(name, &args(given), &egui::Context::default());
        answered(st, reply)
    }

    #[test]
    fn base64_comes_back_as_it_went() {
        for bytes in [&b""[..], b"M", b"Ma", b"Man", &[0xfb, 0xff, 0xbf, 0, 1]] {
            assert_eq!(unbase64(&base64(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(unbase64("TWE").as_deref(), Some(&b"Ma"[..]), "unpadded");
        assert!(unbase64("T").is_none() && unbase64("TW!=").is_none());
    }

    #[test]
    fn pixels_go_in_and_come_out_in_each_format() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 3, "height": 2, "background": "transparent" })).unwrap();
        // Two rows of three: red, half-transparent blue, nothing; then green, white, black.
        let rgba: Vec<u8> =
            [[255, 0, 0, 255], [0, 0, 255, 128], [0, 0, 0, 0], [0, 255, 0, 255], [255, 255, 255, 255], [0, 0, 0, 255]]
                .concat();
        let put = json!({ "width": 3, "height": 2, "data": base64(&rgba), "name": "Given" });
        let layer = call(&mut st, "put_pixels", put).unwrap()["layer"].as_u64().unwrap();
        assert_eq!(st.active().unwrap().active_layer().map(|l| l.0), Some(layer), "the new layer is the active one");
        let got = call(&mut st, "pixels", json!({ "shared": false })).unwrap();
        assert_eq!(
            (got["width"].as_u64(), got["height"].as_u64(), got["format"].as_str()),
            (Some(3), Some(2), Some("srgb8"))
        );
        assert_eq!(unbase64(got["data"].as_str().unwrap()).unwrap(), rgba);
        // Linear light, straight alpha; and 16 bits.
        let linear = call(&mut st, "pixels", json!({ "layer": layer, "format": "linear", "shared": false })).unwrap();
        let bytes = unbase64(linear["data"].as_str().unwrap()).unwrap();
        let f = |i: usize| f32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap());
        assert!(
            (f(0) - 1.0).abs() < 1e-3 && f(1) == 0.0 && (f(7) - 128.0 / 255.0).abs() < 1e-2,
            "{:?}",
            [f(0), f(6), f(7)]
        );
        assert!((f(6) - 1.0).abs() < 1e-3, "blue, straight: {}", f(6));
        let sixteen = call(
            &mut st,
            "pixels",
            json!({ "format": "srgb16", "shared": false, "x": 1, "y": 1, "width": 1, "height": 1 }),
        );
        assert_eq!(unbase64(sixteen.unwrap()["data"].as_str().unwrap()).unwrap(), [255u8; 8], "white");
        // Back in as linear floats, into the layer, replacing its pixels there.
        let black: Vec<u8> = [0f32, 0.0, 0.0, 1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let into = json!({ "layer": layer, "x": 2, "y": 0, "width": 1, "height": 1, "format": "linear", "data": base64(&black) });
        assert_eq!(call(&mut st, "put_pixels", into).unwrap()["layer"].as_u64(), Some(layer));
        let corner =
            call(&mut st, "pixels", json!({ "x": 2, "y": 0, "width": 1, "height": 1, "shared": false })).unwrap();
        assert_eq!(unbase64(corner["data"].as_str().unwrap()).unwrap(), [0, 0, 0, 255]);
        // A layer hidden still gives its own pixels.
        now(&mut st, "toggle_visible", json!({ "layers": [layer], "visible": false })).unwrap();
        let alone =
            call(&mut st, "pixels", json!({ "layer": layer, "x": 2, "width": 1, "height": 1, "shared": false }));
        assert_eq!(unbase64(alone.unwrap()["data"].as_str().unwrap()).unwrap(), [0, 0, 0, 255]);
        now(&mut st, "toggle_visible", json!({ "layers": [layer], "visible": true })).unwrap();
        // Into a layer placed away from the canvas's corner: where it is on the canvas.
        let one = |p: [u8; 4]| base64(&p);
        let placed = json!({ "x": 1, "y": 1, "width": 1, "height": 1, "data": one([9, 9, 9, 255]) });
        let dot = call(&mut st, "put_pixels", placed).unwrap()["layer"].as_u64().unwrap();
        let again = json!({ "layer": dot, "x": 1, "y": 1, "width": 1, "height": 1, "data": one([7, 7, 7, 255]) });
        call(&mut st, "put_pixels", again).unwrap();
        let at =
            call(&mut st, "pixels", json!({ "layer": dot, "x": 1, "y": 1, "width": 1, "height": 1, "shared": false }));
        assert_eq!(unbase64(at.unwrap()["data"].as_str().unwrap()).unwrap(), [7, 7, 7, 255]);
        // Refused: pixels said to be more than they are; and into a layer that is not of pixels.
        let short = json!({ "width": 3, "height": 3, "data": base64(&rgba) });
        assert!(call(&mut st, "put_pixels", short).unwrap_err().contains("not the 36"));
        let group = now(&mut st, "new_group", json!({})).unwrap()["layer"].as_u64().unwrap();
        let err =
            call(&mut st, "put_pixels", json!({ "layer": group, "width": 1, "height": 1, "data": base64(&[0; 4]) }));
        assert!(err.unwrap_err().contains("is a group"));
        // Into a layer with nothing in it yet, as new_layer makes one.
        let empty = now(&mut st, "new_layer", json!({ "name": "Empty" })).unwrap()["layer"].as_u64().unwrap();
        let fill = json!({ "layer": empty, "x": 1, "y": 0, "width": 1, "height": 1, "data": one([5, 6, 7, 255]) });
        assert_eq!(call(&mut st, "put_pixels", fill).unwrap()["layer"].as_u64(), Some(empty));
        let got = call(&mut st, "pixels", json!({ "layer": empty, "x": 1, "width": 1, "height": 1, "shared": false }));
        assert_eq!(unbase64(got.unwrap()["data"].as_str().unwrap()).unwrap(), [5, 6, 7, 255]);
        // Linear light past a half float's range: its largest, not infinity.
        let bright: Vec<u8> = [1e5f32, 0.5, 0.5, 1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let put = json!({ "width": 1, "height": 1, "format": "linear", "data": base64(&bright) });
        let hdr = call(&mut st, "put_pixels", put).unwrap()["layer"].as_u64().unwrap();
        let back = call(
            &mut st,
            "pixels",
            json!({ "layer": hdr, "width": 1, "height": 1, "format": "linear", "shared": false }),
        );
        let back = unbase64(back.unwrap()["data"].as_str().unwrap()).unwrap();
        let red = f32::from_le_bytes(back[0..4].try_into().unwrap());
        assert_eq!(red, half::f16::MAX.to_f32(), "{red}");
        let neither = call(&mut st, "put_pixels", json!({ "width": 1, "height": 1 }));
        assert!(neither.unwrap_err().contains("one of them"));
        // A layer alone, as an image.
        assert!(call(&mut st, "image", json!({ "layer": layer })).unwrap()["png"].is_string());
        assert!(call(&mut st, "image", json!({ "layer": 999 })).unwrap_err().contains("no layer 999"));
    }

    #[cfg(windows)]
    #[test]
    fn pixels_asked_for_wait_in_shared_memory_until_released() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 5, "height": 4 })).unwrap();
        let got = call(&mut st, "pixels", json!({})).unwrap();
        let (name, bytes) = (got["shm"].as_str().unwrap().to_owned(), got["bytes"].as_u64().unwrap() as usize);
        assert_eq!(bytes, 5 * 4 * 4);
        let opened = crate::shared_memory::Mapping::open(&name, bytes).unwrap();
        assert!(opened.bytes().iter().all(|&b| b == 255), "a white canvas");
        now(&mut st, "release_pixels", json!({ "shm": name })).unwrap();
        assert!(st.shared_out.is_empty());
        // Again, or after it went by itself: no harm.
        assert!(now(&mut st, "release_pixels", json!({ "shm": name })).is_ok());
        // Given through shared memory the caller made.
        let given = crate::shared_memory::new_name();
        let mut made = crate::shared_memory::Mapping::create(&given, 4).unwrap();
        made.bytes_mut().copy_from_slice(&[10, 20, 30, 255]);
        let put = json!({ "width": 1, "height": 1, "shm": given });
        let layer = call(&mut st, "put_pixels", put).unwrap()["layer"].as_u64().unwrap();
        let back =
            call(&mut st, "pixels", json!({ "layer": layer, "width": 1, "height": 1, "shared": false })).unwrap();
        assert_eq!(unbase64(back["data"].as_str().unwrap()).unwrap(), [10, 20, 30, 255]);
        let missing = json!({ "width": 1, "height": 1, "shm": "Local\\opal-no-such-memory" });
        assert!(call(&mut st, "put_pixels", missing).unwrap_err().contains("no shared memory"));
    }

    #[test]
    fn pixels_made_ready_wait_to_go_in_while_the_user_is_busy() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 4, "height": 4 })).unwrap();
        let layers = |st: &AppState| st.active().unwrap().doc().layers.len();
        let before = layers(&st);
        let put = |st: &mut AppState| {
            let given = args(json!({ "width": 1, "height": 1, "data": base64(&[1, 2, 3, 255]) }));
            match st.call("put_pixels", &given, &egui::Context::default()) {
                Reply::Later(n) => n,
                Reply::Now(r) => panic!("answered at once: {r:?}"),
            }
        };
        let n = put(&mut st);
        // Made ready, but the user busy: held, not put in.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while st.finishing.is_empty() && std::time::Instant::now() < until {
            assert!(st.answers_while(true, false).is_empty());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!((st.finishing.len(), layers(&st)), (1, before), "put in while the user was busy");
        let answered = st.answers_while(false, false);
        assert!(matches!(answered.as_slice(), [(m, Ok(_))] if *m == n), "{answered:?}");
        assert_eq!(layers(&st), before + 1);
        // Once the user has asked to quit: refused.
        let n = put(&mut st);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut answered = Vec::new();
        while answered.is_empty() && std::time::Instant::now() < until {
            answered = st.answers_while(true, true);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(matches!(answered.as_slice(), [(m, Err(why))] if *m == n && why.contains("closing")), "{answered:?}");
        assert_eq!(layers(&st), before + 1);
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
