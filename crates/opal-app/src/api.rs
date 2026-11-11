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
use opal_core::document::{BoxNumber, Layer, LayerContent, LayerId, ObjectKind};
use opal_core::expr::{Expr, Own, Scope, Want};
use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::Pixel;
use opal_core::units::{DEFAULT_PPI, MAX_PPI, MIN_PPI, Unit, parse_length};
use opal_io::image_files::ExportSize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// What a parameter takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// A whole number, or an expression that comes to one.
    Int {
        min: i64,
        max: i64,
    },
    /// A number, or an expression that comes to one.
    Number {
        min: f64,
        max: f64,
    },
    /// A length: a number in the call's `unit` (pixels unless it says), or an expression
    /// (`opal_core::expr`: "8.5in", "Logo.right + 5mm"), worked out at the image's
    /// resolution; in pixels, from `min` to `max`.
    Length {
        min: f64,
        max: f64,
    },
    /// An angle: a number of degrees, or an expression ("90deg - angle", "1rad").
    Angle {
        min: f64,
        max: f64,
    },
    /// A whole number of pixels: a number, or an expression that comes to a number or a
    /// length (worked out in pixels, and rounded to whole ones).
    Pixels {
        min: i64,
        max: i64,
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
    /// Points, each `[x, y]` in pixels: `min` of them at least.
    Points {
        min: usize,
    },
    /// Any value: one a preference takes.
    Any,
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
    /// What it is.
    pub role: Role,
}

/// What a method of the API is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    /// It changes images.
    Command,
    /// It asks about them.
    Query,
    /// It is about the caller's connection, which the server answers itself.
    Connection,
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
const COMBINE: Param = Param {
    name: "combine",
    kind: Kind::Choice(&opal_core::mask::Combine::NAMES),
    help: "How it combines with what is selected: in its place, added to it, taken away from it, or keeping only \
           what is in both.",
    otherwise: Otherwise::Value("\"replace\""),
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
    Param { name, kind: Kind::Pixels { min: 1, max: SIDE }, help, otherwise }
}

/// A canvas side as a length (rounded to whole pixels).
const fn length_side(name: &'static str, help: &'static str, otherwise: Otherwise) -> Param {
    Param { name, kind: Kind::Length { min: 1.0, max: SIDE as f64 }, help, otherwise }
}

const NEW_WIDTH: Param = length_side("width", "Its width.", Otherwise::Required);
const NEW_HEIGHT: Param = length_side("height", "Its height.", Otherwise::Required);
const EXPORT_WIDTH: Param =
    length_side("width", "The width written.", Otherwise::Now("the canvas's, or in proportion to the height given"));
const EXPORT_HEIGHT: Param =
    length_side("height", "The height written.", Otherwise::Now("the canvas's, or in proportion to the width given"));
const RESIZED_WIDTH: Param =
    length_side("width", "The new width.", Otherwise::Now("the canvas's, or in proportion to the height given"));
const RESIZED_HEIGHT: Param =
    length_side("height", "The new height.", Otherwise::Now("the canvas's, or in proportion to the width given"));
const CANVAS_WIDTH: Param = length_side("width", "The new width.", Otherwise::Now("the canvas's"));
const CANVAS_HEIGHT: Param = length_side("height", "The new height.", Otherwise::Now("the canvas's"));

/// The units' short names, as `unit` takes them: `opal_core`'s, in their order.
const UNIT_IDS: [&str; Unit::ALL.len()] = {
    let mut ids = [""; Unit::ALL.len()];
    let mut i = 0;
    while i < ids.len() {
        ids[i] = Unit::ALL[i].id();
        i += 1;
    }
    ids
};

const UNIT: Param = Param {
    name: "unit",
    kind: Kind::Choice(&UNIT_IDS),
    help: "What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, \
           millimeters, centimeters or meters, converted at the image's resolution. A length can also be \
           given as text with a unit of its own: \"8.5in\", \"21 cm\".",
    otherwise: Otherwise::Value("\"px\""),
};

const NEW_RESOLUTION: Param = Param {
    name: "resolution",
    kind: Kind::Number { min: MIN_PPI, max: MAX_PPI },
    help: "Its resolution, in pixels per inch: how large its pixels print, and what lengths in other units \
           than pixels are converted at.",
    otherwise: Otherwise::Value("72"),
};

use crate::box_fields::FAR;

const BOX_X: Param = Param {
    name: "x",
    kind: Kind::Length { min: -FAR, max: FAR },
    help: "Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point \
           from_layer and from_anchor give (the canvas's top-left corner, unless they say).",
    otherwise: Otherwise::Now("where the command puts it"),
};
const BOX_Y: Param = Param {
    name: "y",
    kind: Kind::Length { min: -FAR, max: FAR },
    help: "Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point \
           from_layer and from_anchor give (the canvas's top-left corner, unless they say).",
    otherwise: Otherwise::Now("where the command puts it"),
};
const BOX_WIDTH: Param = Param {
    name: "width",
    kind: Kind::Length { min: 0.01, max: 1e6 },
    help: "The box's width.",
    otherwise: Otherwise::Now("in proportion to the height given, or as it is"),
};
const BOX_HEIGHT: Param = Param {
    name: "height",
    kind: Kind::Length { min: 0.01, max: 1e6 },
    help: "The box's height.",
    otherwise: Otherwise::Now("in proportion to the width given, or as it is"),
};
const BOX_ANGLE: Param = Param {
    name: "angle",
    kind: Kind::Angle { min: -360.0, max: 360.0 },
    help: "The box's angle, in degrees clockwise, about its anchor point.",
    otherwise: Otherwise::Now("as it is"),
};
const BOX_FIT: Param = Param {
    name: "fit",
    kind: Kind::Choice(&["stretch", "inside", "cover"]),
    help: "With both width and height given: stretch it to them (along its own sides, turned as it is); or keep \
           its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed \
           by its anchor point at x and y), or make it cover that rectangle, centered on it.",
    otherwise: Otherwise::Value("\"stretch\""),
};
const BOX_ANCHOR: Param = Param {
    name: "anchor",
    kind: Kind::Choice(&ANCHOR_NAMES),
    help: "Which point of the box x and y give, and what a size or angle given scales and turns it about (a \
           rectangle given with width and height is placed by its same point).",
    otherwise: Otherwise::Value("\"top-left\""),
};
const FROM_LAYER: Param = Param {
    name: "from_layer",
    kind: Kind::Layer,
    help: "A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), \
           rather than from the canvas. Held where it is when the call begins (a layer moved counts from where \
           it was).",
    otherwise: Otherwise::Now("the canvas"),
};
const FROM_ANCHOR: Param = Param {
    name: "from_anchor",
    kind: Kind::Choice(&ANCHOR_NAMES),
    help: "Which point of the canvas (or of from_layer) x and y count from.",
    otherwise: Otherwise::Value("\"top-left\""),
};
const INTO_ACTIVE: Param = Param {
    name: "into_active",
    kind: Kind::Bool,
    help: "Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer.",
    otherwise: Otherwise::Value("false"),
};

/// Where Resize Canvas puts the old canvas in the new, by name, row by row: the one at `i` is
/// at `[i % 3, i / 3]` (left, center, right; top, middle, bottom).
const ANCHOR_NAMES: [&str; 9] = {
    let mut ids = [""; 9];
    let mut i = 0;
    while i < ids.len() {
        ids[i] = crate::box_fields::POINTS[i].0;
        i += 1;
    }
    ids
};

/// A margin Resize Canvas adds on one side (`side`).
const fn margin(side: &'static str) -> Param {
    Param {
        name: side,
        kind: Kind::Length { min: -(SIDE as f64), max: SIDE as f64 },
        help: "How much to add on that side, rather than give a new size (a negative length cuts it).",
        otherwise: Otherwise::Now("none: the width and height say"),
    }
}

const LEFT: Param = margin("left");
const TOP: Param = margin("top");
const RIGHT: Param = margin("right");
const BOTTOM: Param = margin("bottom");

/// The parameters of command `cmd`, or `None` for one of the interface alone (the view,
/// the panels, the windows). Those that open the transform box take where to put it, and
/// apply it.
pub fn params(cmd: Cmd) -> Option<&'static [Param]> {
    use Cmd::*;
    Some(match cmd {
        New => &[
            NEW_WIDTH,
            NEW_HEIGHT,
            NEW_RESOLUTION,
            UNIT,
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
            UNIT,
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
        | RotateCcw | Rotate180 | FlipHorizontal | FlipVertical | ReapplyAllPlacements | SelectAll | Deselect
        | Reselect | InvertSelection | CropToSelection => &[DOCUMENT],
        SelectLayerPixels => &[DOCUMENT, LAYER, COMBINE],
        Clear | Cut => &[DOCUMENT, LAYERS],
        GrowSelection => &[
            DOCUMENT,
            Param {
                name: "by",
                kind: Kind::Length { min: -opal_core::modify::MAX_DISTANCE, max: opal_core::modify::MAX_DISTANCE },
                help: "How far the outline moves out; less than 0, in.",
                otherwise: Otherwise::Required,
            },
            Param {
                name: "corners",
                kind: Kind::Choice(&opal_core::modify::Reach::NAMES),
                help: "How its corners grow: round, square, or to a point as a diamond's.",
                otherwise: Otherwise::Value("\"round\""),
            },
            UNIT,
        ],
        SmoothSelection => &[
            DOCUMENT,
            Param {
                name: "radius",
                kind: Kind::Length { min: 0.0, max: opal_core::modify::MAX_DISTANCE },
                help: "Pinholes, notches, specks and spurs narrower than twice it go, and corners round to it.",
                otherwise: Otherwise::Required,
            },
            UNIT,
        ],
        BorderSelection => &[
            DOCUMENT,
            Param {
                name: "width",
                kind: Kind::Length { min: 0.0, max: opal_core::modify::MAX_DISTANCE },
                help: "How wide the band along the outline is.",
                otherwise: Otherwise::Required,
            },
            Param {
                name: "side",
                kind: Kind::Choice(&opal_core::modify::Side::NAMES),
                help: "Where the band lies: inside the outline, outside it, or half each way.",
                otherwise: Otherwise::Value("\"centered\""),
            },
            UNIT,
        ],
        FeatherSelection => &[
            DOCUMENT,
            Param {
                name: "width",
                kind: Kind::Length { min: 0.0, max: opal_core::modify::MAX_DISTANCE },
                help: "How wide the falloff from fully selected to not at all is.",
                otherwise: Otherwise::Required,
            },
            Param {
                name: "side",
                kind: Kind::Choice(&opal_core::modify::Side::NAMES),
                help: "Where the falloff lies: inside the outline, outside it, or half each way.",
                otherwise: Otherwise::Value("\"centered\""),
            },
            Param {
                name: "falloff",
                kind: Kind::Choice(&opal_core::modify::Falloff::NAMES),
                help: "Its shape: as a Gaussian blur's edge, or straight.",
                otherwise: Otherwise::Value("\"gaussian\""),
            },
            UNIT,
        ],
        NewLayer | NewGroup => &[
            DOCUMENT,
            Param { name: "name", kind: Kind::Text, help: "Its name.", otherwise: Otherwise::Now("a numbered one") },
        ],
        Duplicate | LayerViaCopy | LayerViaCut | GroupLayers | DeleteLayer | MergeSelected | Rasterize => {
            &[DOCUMENT, LAYERS]
        }
        MoveUp | MoveDown | MergeDown | MergeGroup | ReapplyPlacement => &[DOCUMENT, LAYER],
        ResizeImage => &[
            DOCUMENT,
            RESIZED_WIDTH,
            RESIZED_HEIGHT,
            Param {
                name: "resolution",
                kind: Kind::Number { min: MIN_PPI, max: MAX_PPI },
                help: "The new resolution, in pixels per inch, which lengths in other units than pixels are \
                       converted at. Given without a size, it changes no pixels: only how large they print.",
                otherwise: Otherwise::Now("the image's"),
            },
            UNIT,
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
                kind: Kind::Choice(&ANCHOR_NAMES),
                help: "Where the old canvas sits in the new.",
                otherwise: Otherwise::Now("the center"),
            },
            LEFT,
            TOP,
            RIGHT,
            BOTTOM,
            UNIT,
        ],
        RotateBy => &[
            DOCUMENT,
            Param {
                name: "degrees",
                kind: Kind::Angle { min: -360.0, max: 360.0 },
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
        Paste | PasteInPlace => &[
            Param {
                name: "document",
                kind: Kind::Document,
                help: "The image, by id.",
                otherwise: Otherwise::Now("the active image; with none open, the paste becomes a new image"),
            },
            BOX_X,
            BOX_Y,
            BOX_WIDTH,
            BOX_HEIGHT,
            BOX_ANGLE,
            BOX_FIT,
            BOX_ANCHOR,
            FROM_LAYER,
            FROM_ANCHOR,
            INTO_ACTIVE,
            UNIT,
        ],
        Place => &[
            DOCUMENT,
            Param {
                name: "path",
                kind: Kind::Path,
                help: "The image file, a PNG or a JPEG, by its absolute path.",
                otherwise: Otherwise::Required,
            },
            Param {
                name: "name",
                kind: Kind::Text,
                help: "The new layer's name.",
                otherwise: Otherwise::Now("the file's name"),
            },
            Param {
                name: "print_size",
                kind: Kind::Bool,
                help: "Scale the file by its resolution against the image's, so it prints as large as it did \
                       (before a width or height given), rather than place it pixel for pixel. A file that does \
                       not say its resolution is placed pixel for pixel either way.",
                otherwise: Otherwise::Now("the preference (Transform › Place at the file's print size)"),
            },
            BOX_X,
            BOX_Y,
            BOX_WIDTH,
            BOX_HEIGHT,
            BOX_ANGLE,
            BOX_FIT,
            BOX_ANCHOR,
            FROM_LAYER,
            FROM_ANCHOR,
            INTO_ACTIVE,
            UNIT,
        ],
        SaveSelection => &[
            DOCUMENT,
            Param {
                name: "name",
                kind: Kind::Text,
                help: "The name it is saved under: a selection saved under it already (whatever the case of its \
                       letters) is replaced.",
                otherwise: Otherwise::Required,
            },
        ],
        LoadSelection => &[
            DOCUMENT,
            SAVED_NAME,
            COMBINE,
            Param {
                name: "invert",
                kind: Kind::Bool,
                help: "Invert it first: what it does not select, before it combines.",
                otherwise: Otherwise::Value("false"),
            },
        ],
        TransformSelection => &[
            DOCUMENT,
            BOX_X,
            BOX_Y,
            BOX_WIDTH,
            BOX_HEIGHT,
            BOX_ANGLE,
            BOX_FIT,
            BOX_ANCHOR,
            FROM_LAYER,
            FROM_ANCHOR,
            UNIT,
        ],
        Transform => &[
            DOCUMENT,
            LAYERS,
            BOX_X,
            BOX_Y,
            BOX_WIDTH,
            BOX_HEIGHT,
            BOX_ANGLE,
            BOX_FIT,
            BOX_ANCHOR,
            FROM_LAYER,
            FROM_ANCHOR,
            UNIT,
        ],
        Preferences
        | Exit
        | RunScript
        | ConnectAssistant
        | ZoomIn
        | ZoomOut
        | ZoomFit
        | Zoom100
        | ToggleOffCanvas
        | ToggleRulers
        | ToggleSelectionOverlay
        | ToggleTheme
        | ToggleRibbon
        | CycleToolsPlace
        | ResetPanels
        | ScreenMode
        | Search
        | Help
        | Shortcuts
        | About
        | WidgetGallery => return None,
    })
}

/// The user's view, as a call that changes it found it: the active image, and each image's
/// active and selected layers.
pub type View = (Option<DocId>, Vec<(DocId, (Option<LayerId>, std::collections::HashSet<LayerId>))>);

/// A call being carried out: its name, whether it is heard (answered: not a notification),
/// and the user's view as it found it (for a change), for what it begins that ends later.
#[derive(Clone, Debug)]
pub struct Calling {
    pub name: String,
    pub heard: bool,
    pub view: Option<View>,
}

/// The queries: what the API tells of the editor and its images.
const QUERIES: &[(&str, &str, &[Param])] = &[
    (
        "selection",
        "What is selected in an image: the rectangle round it, as [x, y, width, height] from the canvas's \
         corner (null if nothing is; \"unbounded\" where it reaches past every edge, inverted), and the \
         steps that made it, in order: shapes (rectangles and ellipses, by their middle, size and angle, \
         and how each combines), inversions, grow steps (by, corners), feather steps (width, side, \
         falloff), smooth steps (radius) and border steps (width, side), as grow_selection, \
         feather_selection, smooth_selection and border_selection take them, and masks (a \"mask\" step is, as \
         its of says, a layer's pixels, from select_layer_pixels, or what the steps before it came to, carried \
         pixel by pixel by Rotate, Flip or Resize Image, which could not carry them exactly; it gives no numbers, \
         only how it combines). \
         select_all, deselect, reselect, invert_selection and select_layer_pixels answer with it too.",
        &[DOCUMENT],
    ),
    (
        "saved_selections",
        "The selections saved in an image (save_selection), in the order they were first saved: each one's \
         name, the rectangle round what it selects (as the selection query tells it, carried as the image has \
         been turned, flipped, resized or cropped since it was saved), its steps as they were when it was \
         saved, and whether the image has changed since (load_selection carries it so).",
        &[DOCUMENT],
    ),
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
                kind: Kind::Pixels { min: 16, max: 8192 },
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
    (
        "history",
        "An image's history, a tree: each state kept (its id, what made it, when, the state before it and \
         those after, the one redo goes to), the state it is in, the one last saved, and how many states were \
         dropped to keep within the memory the history may use.",
        &[DOCUMENT],
    ),
    (
        "preferences",
        "Every preference: for each, its key, label, section, help, value and default, and what it takes \
         (its range or choices). set_preference changes them.",
        &[],
    ),
    ("methods", "Every method of the API, with its parameters: what this list is generated from.", &[]),
];

/// The methods about the caller's connection, which the server answers itself (see
/// `server::serve_connection`): their names, what they do, and their parameters.
const CONNECTION: &[(&str, &str, &[Param])] = &[
    (
        "begin_step",
        "Begins a step of history: what the calls until end_step change in each image becomes one state, \
         called name, which one undo takes back. A step begun inside another is part of it. One left open \
         ends when the connection does, or, on a connection that is no script run (an assistant's), before \
         a call that comes 10 minutes or more after the last.",
        &[Param {
            name: "name",
            kind: Kind::Text,
            help: "The step's name, as the History panel shows it.",
            otherwise: Otherwise::Required,
        }],
    ),
    ("end_step", "Ends the step begin_step began (the outermost, if they are inside one another).", &[]),
];

/// Method `name`, if there is one by that name (looked up in an index made once).
pub fn method(name: &str) -> Option<&'static Method> {
    static INDEX: std::sync::OnceLock<HashMap<&'static str, &'static Method>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| methods().iter().map(|m| (m.name.as_str(), m)).collect()).get(name).copied()
}

/// The role of method `name`, if there is one by that name.
pub fn role_of(name: &str) -> Option<Role> {
    method(name).map(|m| m.role)
}

/// Whether `given` are arguments method `name` takes (as every call's are checked): why
/// not, if they are not.
pub fn check_args(name: &str, given: &Map<String, Value>) -> Result<(), String> {
    let method = method(name).ok_or_else(|| format!("There is no method \"{name}\"."))?;
    Args::check(method.params, given).map(|_| ()).map_err(|why| format!("{name}: {why}."))
}

/// What a method's role is called in its description.
fn role_name(role: Role) -> &'static str {
    match role {
        Role::Command => "command",
        Role::Query => "query",
        Role::Connection => "connection",
    }
}

/// The parameters of `select_rectangle` and `select_ellipse`: the shape's box, as the Select
/// tool's box numbers give it, and how it combines.
const SHAPE_PARAMS: &[Param] = &[
    DOCUMENT,
    Param {
        name: "x",
        kind: Kind::Length { min: -FAR, max: FAR },
        help: "Where the shape's anchor point is (of the rectangle round an ellipse), across from the point \
               from_layer and from_anchor give (the canvas's top-left corner, unless they say).",
        otherwise: Otherwise::Now("where that point of the canvas is"),
    },
    Param {
        name: "y",
        kind: Kind::Length { min: -FAR, max: FAR },
        help: "Where the shape's anchor point is, down from the point from_layer and from_anchor give (the \
               canvas's top-left corner, unless they say).",
        otherwise: Otherwise::Now("where that point of the canvas is"),
    },
    Param {
        name: "width",
        kind: Kind::Length { min: 0.01, max: FAR },
        help: "The shape's width (an ellipse's across).",
        otherwise: Otherwise::Now("the canvas's"),
    },
    Param {
        name: "height",
        kind: Kind::Length { min: 0.01, max: FAR },
        help: "The shape's height.",
        otherwise: Otherwise::Now("the canvas's"),
    },
    Param {
        name: "angle",
        kind: Kind::Angle { min: -360.0, max: 360.0 },
        help: "How far the shape is turned, in degrees clockwise, about its anchor point.",
        otherwise: Otherwise::Value("0"),
    },
    COMBINE,
    Param {
        name: "smooth",
        kind: Kind::Bool,
        help: "Select a pixel its edge crosses as much as the shape covers it; else all or nothing, by the \
               pixel's middle.",
        otherwise: Otherwise::Now("the Select tool's Smooth edges"),
    },
    Param {
        name: "anchor",
        kind: Kind::Choice(&ANCHOR_NAMES),
        help: "Which point of the shape (of the rectangle round an ellipse) x and y give, and what the angle \
               turns it about.",
        otherwise: Otherwise::Value("\"top-left\""),
    },
    FROM_LAYER,
    FROM_ANCHOR,
    UNIT,
];

/// A selection saved in the image, by its name.
const SAVED_NAME: Param = Param {
    name: "name",
    kind: Kind::Text,
    help: "The selection saved in the image under this name (whatever the case of its letters).",
    otherwise: Otherwise::Required,
};

/// Which step of the selection's recipe: its number among the selection query's steps.
const STEP_INDEX: Param = Param {
    name: "index",
    kind: Kind::Int { min: 0, max: 1 << 20 },
    help: "Which step: its number among the selection query's steps, from 0.",
    otherwise: Otherwise::Required,
};
/// What `edit_selection_step` may change in a step, each kind of step taking its own.
const STEP_SETTINGS: &[Param] = &[
    DOCUMENT,
    STEP_INDEX,
    Param {
        name: "combine",
        kind: Kind::Choice(&opal_core::mask::Combine::NAMES),
        help: "A shape's or a mask's: how it combines with the steps before it.",
        otherwise: Otherwise::Now("as it is"),
    },
    Param {
        name: "by",
        kind: Kind::Length { min: -opal_core::modify::MAX_DISTANCE, max: opal_core::modify::MAX_DISTANCE },
        help: "A grow's: how far it grows the selection (shrinks it, less than 0).",
        otherwise: Otherwise::Now("as it is"),
    },
    Param {
        name: "corners",
        kind: Kind::Choice(&opal_core::modify::Reach::NAMES),
        help: "A grow's: how its corners grow.",
        otherwise: Otherwise::Now("as it is"),
    },
    Param {
        name: "width",
        kind: Kind::Length { min: 0.0, max: opal_core::modify::MAX_DISTANCE },
        help: "A feather's or a border's: how wide its falloff or its band is.",
        otherwise: Otherwise::Now("as it is"),
    },
    Param {
        name: "side",
        kind: Kind::Choice(&opal_core::modify::Side::NAMES),
        help: "A feather's or a border's: on which side of the outline it lies.",
        otherwise: Otherwise::Now("as it is"),
    },
    Param {
        name: "falloff",
        kind: Kind::Choice(&opal_core::modify::Falloff::NAMES),
        help: "A feather's: the shape of its falloff.",
        otherwise: Otherwise::Now("as it is"),
    },
    Param {
        name: "radius",
        kind: Kind::Length { min: 0.0, max: opal_core::modify::MAX_DISTANCE },
        help: "A smooth's: the radius it smooths over.",
        otherwise: Otherwise::Now("as it is"),
    },
    UNIT,
];

/// The changes the API alone makes (no command of the interface makes them): their names,
/// what they do, and their parameters.
const CHANGES: &[(&str, &str, &[Param])] = &[
    (
        "select_outline",
        "Selects what an outline encloses, as the outline tool does: its points joined by straight sides, \
         the last back to the first (a loop crossing itself selects each of its lobes). Answers with what is \
         selected then, as the selection query does.",
        &[
            DOCUMENT,
            Param {
                name: "points",
                kind: Kind::Points { min: 3 },
                help: "The outline's points, in order, each [x, y] in pixels from the canvas's top-left corner.",
                otherwise: Otherwise::Required,
            },
            COMBINE,
            Param {
                name: "smooth",
                kind: Kind::Bool,
                help: "Select a pixel the outline crosses as much as it encloses it; else all or nothing, by the \
                       pixel's middle.",
                otherwise: Otherwise::Now("the outline tool's Smooth edges"),
            },
        ],
    ),
    (
        "delete_saved_selection",
        "Deletes a selection saved in the image (as the Selection panel does), by its name. Answers with the \
         selections saved then, as saved_selections tells them.",
        &[DOCUMENT, SAVED_NAME],
    ),
    (
        "remove_selection_step",
        "Takes a step away from the recipe of what is selected, by its number among the selection query's \
         steps; the selection is worked out again from the rest (as the selection panel does). Answers with \
         what is selected then.",
        &[DOCUMENT, STEP_INDEX],
    ),
    (
        "edit_selection_step",
        "Changes a step of the recipe of what is selected, by its number among the selection query's steps: \
         a shape's or a mask's combine; a grow's by and corners; a feather's width, side and falloff; a \
         smooth's radius; a border's width and side (what is not given stays as it is; what the step does not \
         take is refused). The selection is worked out again with it changed (as the selection panel does). \
         Answers with what is selected then.",
        STEP_SETTINGS,
    ),
    (
        "select_rectangle",
        "Selects a rectangle, which may be turned, as the Select tool's box numbers give it: the canvas, \
         unless the numbers say otherwise. Answers with what is selected then, as the selection query does.",
        SHAPE_PARAMS,
    ),
    (
        "select_ellipse",
        "Selects an ellipse, given by the rectangle round it (which may be turned), as the Select tool's box \
         numbers give it: the one within the canvas, unless the numbers say otherwise. Answers with what is \
         selected then, as the selection query does.",
        SHAPE_PARAMS,
    ),
    (
        "crop",
        "Crops the image to a rectangle of its canvas, which may be turned: the canvas becomes the \
         rectangle, and if it was turned, everything turns the other way, so its contents come out upright. \
         Only the canvas moves (enlarging it again brings back what was cropped away), unless delete says.",
        &[
            DOCUMENT,
            Param {
                name: "x",
                kind: Kind::Length { min: -FAR, max: FAR },
                help: "Where the rectangle's anchor point is, across from the point from_layer and from_anchor give \
                       (the canvas's top-left corner, unless they say).",
                otherwise: Otherwise::Now("where that point of the canvas is"),
            },
            Param {
                name: "y",
                kind: Kind::Length { min: -FAR, max: FAR },
                help: "Where the rectangle's anchor point is, down from the point from_layer and from_anchor give \
                       (the canvas's top-left corner, unless they say).",
                otherwise: Otherwise::Now("where that point of the canvas is"),
            },
            length_side("width", "The rectangle's width.", Otherwise::Now("the canvas's")),
            length_side("height", "The rectangle's height.", Otherwise::Now("the canvas's")),
            Param {
                name: "angle",
                kind: Kind::Angle { min: -360.0, max: 360.0 },
                help: "The rectangle's angle, in degrees clockwise, about its anchor point.",
                otherwise: Otherwise::Value("0"),
            },
            Param {
                name: "delete",
                kind: Kind::Bool,
                help: "Remove the pixels outside the new canvas, rather than keep them for later.",
                otherwise: Otherwise::Now("the Crop tool's Delete cropped pixels"),
            },
            Param {
                name: "anchor",
                kind: Kind::Choice(&ANCHOR_NAMES),
                help: "Which point of the rectangle x and y give, and what the angle turns it about.",
                otherwise: Otherwise::Value("\"top-left\""),
            },
            FROM_LAYER,
            FROM_ANCHOR,
            UNIT,
        ],
    ),
    (
        "set_preference",
        "Changes a preference, as the Preferences window would, or puts it back to its default. Those of \
         scripting (what runs scripts, who may connect) are changed in the Preferences window only.",
        &[
            Param {
                name: "key",
                kind: Kind::Text,
                help: "The preference, by its key (the preferences query lists them).",
                otherwise: Otherwise::Required,
            },
            Param {
                name: "value",
                kind: Kind::Any,
                help: "Its new value: true or false, a number in its range, one of its choices, a color as \
                       [red, green, blue] or [red, green, blue, alpha] (0 to 255, sRGB), or text.",
                otherwise: Otherwise::Now("its default"),
            },
        ],
    ),
    (
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
                kind: Kind::Pixels { min: -SIDE, max: 2 * SIDE },
                help: "Where their left edge goes, in pixels from the canvas's.",
                otherwise: Otherwise::Value("0"),
            },
            Param {
                name: "y",
                kind: Kind::Pixels { min: -SIDE, max: 2 * SIDE },
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
    ),
];

const REGION_X: Param = Param {
    name: "x",
    kind: Kind::Pixels { min: -SIDE, max: 2 * SIDE },
    help: "The rectangle's left edge, in pixels from the canvas's.",
    otherwise: Otherwise::Value("0"),
};
const REGION_Y: Param = Param {
    name: "y",
    kind: Kind::Pixels { min: -SIDE, max: 2 * SIDE },
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

/// What a method made off the interface thread: its answer, or (for a change) what is left
/// to do on the interface thread, which answers; and the shared memory to keep until the
/// caller has opened it, if any.
pub struct Made {
    done: Done,
    keep: Option<(String, crate::shared_memory::Mapping)>,
}

/// What a method made off the interface thread has done.
enum Done {
    /// Answered, with this.
    Answer(Value),
    /// Made a change ready, which this makes, answering.
    Finish(Finish),
}

/// Where `put_pixels` takes its pixels from.
enum Source {
    /// Shared memory the caller made, by name.
    Shared(String),
    /// The request itself, in base64.
    Data(String),
}

/// What a change made ready off the interface thread does on it: its answer.
pub type Finish = Box<dyn FnOnce(&mut AppState) -> Result<Value, String> + Send>;

/// Pixels to put in, made ready: as a new layer's image (at the canvas's origin), or the
/// pixels themselves, to go into a layer.
enum Content {
    Image(Box<RasterImage>),
    Pixels(Vec<Pixel>),
}

/// Whether pixels can go into layer `target` of `doc`: one not locked (nor in a locked
/// group) that holds nothing yet or pixels alone, in one object.
pub(crate) fn pixels_go_into(doc: &opal_core::document::Document, target: LayerId) -> Result<(), String> {
    let layer = doc.layer(target).ok_or_else(|| format!("There is no layer {} in the image.", target.0))?;
    // A group is said to be one first: what is locked inside it is beside the point.
    let LayerContent::Objects(objects) = &layer.content else {
        return Err(format!("\"{}\" is a group: put them in a layer of it.", layer.name));
    };
    if let Some(locked) = crate::commands::locked_for(doc, layer) {
        return Err(format!("\"{}\" is locked. Unlock it first.", locked.name));
    }
    match &objects[..] {
        [] => Ok(()),
        [only] if matches!(only.kind, ObjectKind::Pixels(_)) => Ok(()),
        _ => Err(format!(
            "\"{}\" holds an image or several objects, which pixels do not go into: put them in a new layer.",
            layer.name
        )),
    }
}

/// Puts `content` into image `id` (made ready by `put_pixels`), at `rect` of the canvas: as
/// a new layer above the active one (called `name`), or into layer `target`, replacing its
/// pixels there; as made in `groups`, the call's (a script run, a step). The layer's id.
fn put_pixels_now(
    st: &mut AppState,
    id: DocId,
    target: Option<LayerId>,
    rect: IRect,
    content: Content,
    name: Option<String>,
    groups: Vec<u64>,
) -> Result<Value, String> {
    let d = st.doc_mut(id).ok_or("The image was closed before the pixels could be put in.")?;
    // The call's groups, as when it was carried out (its changes join theirs).
    let before = std::mem::replace(&mut d.groups, groups);
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
    d.groups = before;
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
            role: Role::Command,
        })
    });
    let others = QUERIES.iter().map(|&(name, help, params)| (name, help, params, Role::Query));
    let others = others.chain(CHANGES.iter().map(|&(name, help, params)| (name, help, params, Role::Command)));
    let others = others.chain(CONNECTION.iter().map(|&(name, help, params)| (name, help, params, Role::Connection)));
    commands
        .chain(others.map(|(name, help, params, role)| Method { name: name.to_owned(), help, params, cmd: None, role }))
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
        self.opt_number(name).unwrap_or_else(|| panic!("\"{name}\" was checked to be there"))
    }

    fn opt_number(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(Value::as_f64)
    }

    /// A canvas side, in whole pixels, if given (once [`worked_out`](Self::worked_out)).
    fn side(&self, name: &str) -> Option<u32> {
        self.opt_number(name).map(crate::lengths::whole_side)
    }

    /// These arguments with the numbers of those `params` that `which` takes worked out: a
    /// length in pixels (a number in the call's `unit`, converted at `scope`'s resolution),
    /// an angle in degrees, a number; given as an expression, worked out in `scope`. Each
    /// must then be in its parameter's range. An expression for a parameter named in
    /// `later` is left as it is: a box's, worked out once the box is open (see [`BoxArgs`]).
    fn worked_out(
        mut self,
        params: &[Param],
        scope: &Scope,
        later: &[&str],
        which: impl Fn(&Param) -> bool,
    ) -> Result<Args, String> {
        let unit = self.text("unit").and_then(Unit::from_name).unwrap_or(Unit::Px);
        let ppi = scope.ppi;
        for p in params.iter().filter(|p| which(p)) {
            let Some(v) = self.0.get_mut(p.name) else { continue };
            let text = v.as_str().map(str::to_owned);
            if text.is_some() && later.contains(&p.name) {
                continue;
            }
            let work = |want, unit| -> Result<f64, String> {
                let text = text.as_deref().expect("an expression");
                let scope = Scope { unit, ..*scope };
                Expr::parse(text).and_then(|e| e.eval(&scope, want)).map_err(|e| expr_error(p.name, text, &e))
            };
            // What a value out of range was given as, said after it.
            let given = || text.as_ref().map_or(String::new(), |t| format!(", what \"{t}\" comes to"));
            match p.kind {
                Kind::Length { min, max } => {
                    let px = match &text {
                        Some(_) => work(Want::Length, unit)?,
                        None => unit.to_px(v.as_f64().expect("checked"), ppi),
                    };
                    if !(min..=max).contains(&px) {
                        // A length typed in a unit is told in it too.
                        let typed = match text.as_deref() {
                            Some(t) => parse_length(t),
                            None => Some((v.as_f64().expect("checked"), None)),
                        };
                        let told = match typed {
                            Some((value, u)) if u.unwrap_or(unit) != Unit::Px => {
                                let u = u.unwrap_or(unit);
                                format!("{value} {u}, which is {px} pixels at {ppi} pixels per inch")
                            }
                            Some(_) => format!("{px}"),
                            None => format!("{px}{}", given()),
                        };
                        return Err(format!("\"{}\" must lie between {min} and {max} pixels, not {told}", p.name));
                    }
                    *v = json!(px);
                }
                Kind::Number { min, max } | Kind::Angle { min, max } if text.is_some() => {
                    let want = if matches!(p.kind, Kind::Angle { .. }) { Want::Angle } else { Want::Number };
                    let x = work(want, unit)?;
                    if !(min..=max).contains(&x) {
                        return Err(format!("\"{}\" must lie between {min} and {max}, not {x}{}", p.name, given()));
                    }
                    *v = json!(x);
                }
                Kind::Int { min, max } | Kind::Pixels { min, max } if text.is_some() => {
                    let x = if matches!(p.kind, Kind::Pixels { .. }) {
                        // In whole pixels, whatever the call's unit: a bare number is pixels.
                        work(Want::Pixels, Unit::Px)?.round()
                    } else {
                        let x = work(Want::Number, unit)?;
                        if x.fract() != 0.0 {
                            return Err(format!("\"{}\" must be a whole number, not {x}{}", p.name, given()));
                        }
                        x
                    };
                    if !(min as f64..=max as f64).contains(&x) {
                        return Err(format!("\"{}\" must lie between {min} and {max}, not {x}{}", p.name, given()));
                    }
                    *v = json!(x as i64);
                }
                _ => {}
            }
        }
        Ok(self)
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

/// Where an API call puts a box, in pixels, its anchor point (`at`) from the point `from`:
/// each number given replaces the box's own; a side left out keeps the box's proportions if
/// the other is given; and with both, `fit` says how they are taken.
#[derive(Clone, Debug, Default)]
pub struct BoxArgs {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub angle: Option<f64>,
    pub fit: Fit,
    /// The point of the box (and of a rectangle given) that `x` and `y` give, and that a
    /// size or angle scales and turns about (see [`box_fields::POINTS`]).
    ///
    /// [`box_fields::POINTS`]: crate::box_fields::POINTS
    pub at: [f64; 2],
    /// The point `x` and `y` count from, in the document: located when the call is made,
    /// so a paste whose clipboard is read later, or a box moving the layer counted from,
    /// counts from where it was then.
    pub from: [f64; 2],
    /// What `from` is the point of: kept with a placement, to be counted from again.
    pub origin: crate::box_fields::Origin,
    /// Numbers given as expressions, to be worked out once the box is open (they may name
    /// it: `x + 1cm`), each in place of its number (see [`worked_out`](Self::worked_out)).
    pub typed: Vec<Typed>,
}

/// One of a box's numbers given as an expression.
#[derive(Clone, Debug)]
pub struct Typed {
    pub field: BoxNumber,
    pub expr: Expr,
    /// As it was given, for what is said about it.
    pub text: String,
    /// What a bare number in it is in (the call's `unit`).
    pub unit: Unit,
    /// What it may come to: in pixels, or for the angle, in degrees.
    pub range: (f64, f64),
}

/// What box number `n` may come to, as the calls that place a box take it: in pixels, or
/// for the angle, in degrees.
pub(crate) fn box_range(n: BoxNumber) -> (f64, f64) {
    let p = match n {
        BoxNumber::X => BOX_X,
        BoxNumber::Y => BOX_Y,
        BoxNumber::Width => BOX_WIDTH,
        BoxNumber::Height => BOX_HEIGHT,
        BoxNumber::Angle => BOX_ANGLE,
    };
    match p.kind {
        Kind::Length { min, max } | Kind::Angle { min, max } => (min, max),
        kind => unreachable!("a box's number is a length or an angle, not {kind:?}"),
    }
}

/// The parameters a box's call gives as numbers: worked out once the box is open, if
/// expressions.
const BOXED: [&str; 5] = ["x", "y", "width", "height", "angle"];

/// Whether `method` puts a box somewhere (its numbers counted from a point it is told).
fn takes_box(method: &Method) -> bool {
    method.params.iter().any(|p| p.name == "from_anchor") && method.params.iter().any(|p| p.name == "x")
}

/// How a box given both a width and a height takes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Stretched to them.
    #[default]
    Stretch,
    /// As large as fits inside them, in its own proportions, centered on them.
    Inside,
    /// As small as covers them, in its own proportions, centered on them.
    Cover,
}

impl BoxArgs {
    /// The box `args` give to a method of `params` (once worked out: its expressions kept for
    /// the box), counted from the point their `from_layer` and `from_anchor` give in `doc`
    /// (as it is when the call is made; a new image's canvas corner, with none open), or why
    /// there is no such point.
    fn of(args: &Args, params: &[Param], doc: Option<&opal_core::document::Document>) -> Result<BoxArgs, String> {
        let origin = origin_of(args);
        let from = match doc {
            Some(doc) => located(&origin, doc)?,
            None if args.get("from_layer").is_some() => {
                return Err("No image is open, so there is no layer to count from.".to_owned());
            }
            None => [0.0, 0.0],
        };
        let unit = args.text("unit").and_then(Unit::from_name).unwrap_or(Unit::Px);
        let mut typed = Vec::new();
        for (field, name) in BoxNumber::ALL {
            let Some(text) = args.text(name) else { continue };
            let range = match params.iter().find(|p| p.name == name).map(|p| p.kind) {
                Some(Kind::Length { min, max } | Kind::Angle { min, max }) => (min, max),
                kind => unreachable!("a box's {name} is a length or an angle, not {kind:?}"),
            };
            let expr = Expr::parse(text).expect("checked to read");
            typed.push(Typed { field, expr, text: text.to_owned(), unit, range });
        }
        Ok(BoxArgs {
            x: args.opt_number("x"),
            y: args.opt_number("y"),
            width: args.opt_number("width"),
            height: args.opt_number("height"),
            angle: args.opt_number("angle"),
            typed,
            fit: match args.text("fit") {
                Some("inside") => Fit::Inside,
                Some("cover") => Fit::Cover,
                _ => Fit::Stretch,
            },
            at: crate::box_fields::point(args.text("anchor").unwrap_or("top-left")),
            from,
            origin,
        })
    }

    /// These numbers with the expressions given worked out, in image `doc` (as it was before
    /// the box opened) at `ppi`, for the box `own` (its numbers as these count them).
    pub fn worked_out(&self, doc: &opal_core::document::Document, ppi: f64, own: Own) -> Result<BoxArgs, String> {
        let mut out = BoxArgs { typed: Vec::new(), ..self.clone() };
        for t in &self.typed {
            let want = if t.field == BoxNumber::Angle { Want::Angle } else { Want::Length };
            let scope = Scope { doc: Some(doc), ppi, unit: t.unit, origin: self.from, own: Some(own) };
            let name = t.field.name();
            let v = t.expr.eval(&scope, want).map_err(|e| expr_error(name, &t.text, &e))?;
            let (min, max) = t.range;
            if !(min..=max).contains(&v) {
                let unit = if want == Want::Angle { "" } else { " pixels" };
                return Err(format!(
                    "\"{name}\" must lie between {min} and {max}{unit}, not {v}, what \"{}\" comes to",
                    t.text
                ));
            }
            *match t.field {
                BoxNumber::X => &mut out.x,
                BoxNumber::Y => &mut out.y,
                BoxNumber::Width => &mut out.width,
                BoxNumber::Height => &mut out.height,
                BoxNumber::Angle => &mut out.angle,
            } = Some(v);
        }
        Ok(out)
    }

    /// The box `p` (as the document has it, its position that of the point [`at`](Self::at))
    /// with these numbers put in, counted from [`from`](Self::from); its size as given, or
    /// kept in proportion. (A fit inside or covering a rectangle is [`place_box`]'s.)
    pub fn applied(&self, p: &crate::transform::Params) -> crate::transform::Params {
        let corner = self.from;
        let (w, h) = match (self.width, self.height) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, p.height * w / p.width),
            (None, Some(h)) => (p.width * h / p.height, h),
            (None, None) => (p.width, p.height),
        };
        crate::transform::Params {
            x: self.x.map_or(p.x, |x| corner[0] + x),
            y: self.y.map_or(p.y, |y| corner[1] + y),
            width: w,
            height: h,
            angle: self.angle.unwrap_or(p.angle),
            mirrored: p.mirrored,
        }
    }
}

/// Puts the transform box open on `d` where `place` says, or says why it cannot go there,
/// leaving it as it was. Given both sides to fit inside or cover, the box is turned as
/// asked, then fitted, as it is turned, into the rectangle they make on the canvas (placed
/// by its point `at` at `x` and `y`, or at the box's own); otherwise its numbers are put in
/// (see [`BoxArgs::applied`]).
pub(crate) fn place_box(d: &mut crate::doc::DocState, place: &BoxArgs) -> Result<(), String> {
    let corner = place.from;
    let s = d.transform.as_ref().expect("a box is open");
    let at = place.at;
    let p = s.params_at(at);
    // Expressions given are worked out in the image as it was before the box opened, for
    // the box as it is.
    let own = Own { x: p.x - corner[0], y: p.y - corner[1], width: p.width, height: p.height, angle: p.angle };
    let given = place;
    let place = &place.worked_out(d.history.doc(), d.doc().resolution, own)?;
    let s = d.transform.as_ref().expect("a box is open");
    let m = match (place.fit, place.width, place.height) {
        (Fit::Inside | Fit::Cover, Some(w), Some(h)) => {
            let turned = s.edited(&p, &crate::transform::Params { angle: place.angle.unwrap_or(p.angle), ..p }, at);
            let x = place.x.map_or(p.x, |x| corner[0] + x) - at[0] * w;
            let y = place.y.map_or(p.y, |y| corner[1] + y) - at[1] * h;
            let rect = opal_core::geom::FRect::new(x, y, x + w, y + h);
            crate::transform::fitted(turned, &s.frame, &rect, place.fit == Fit::Cover)
        }
        _ => s.edited(&p, &place.applied(&p), at),
    };
    if let Some(why) = d.box_refused(&m) {
        let mut why = why.to_owned();
        why[..1].make_ascii_uppercase();
        return Err(format!("{why}."));
    }
    d.set_box(m, None);
    // What was given as expressions, for the layers to be placed so again: not the sides
    // and position of a rectangle the box was fitted into, which are not the box's.
    let s = d.transform.as_mut().expect("a box is open");
    let anchor = crate::box_fields::index_of(at);
    let fitted = place.fit != Fit::Stretch && place.width.is_some() && place.height.is_some();
    for t in given.typed.iter().filter(|t| !fitted || t.field == BoxNumber::Angle) {
        let side = matches!(t.field, BoxNumber::Width | BoxNumber::Height);
        let keep = side && place.width.is_none() != place.height.is_none();
        s.given.push(crate::transform::Given {
            number: t.field,
            expr: t.expr.clone(),
            unit: t.unit,
            anchor,
            origin: given.origin,
            keep,
        });
    }
    Ok(())
}

/// What is wrong with expression `text`, given for parameter `name`, and where.
fn expr_error(name: &str, text: &str, e: &opal_core::expr::Error) -> String {
    let at = match text.get(e.at.clone()) {
        Some(part) if !part.is_empty() => format!("at \"{part}\" in \"{text}\""),
        _ => format!("at the end of \"{text}\""),
    };
    format!("\"{name}\": {} ({at})", e.message)
}

/// What `args` say x and y count from (`from_layer`, `from_anchor`).
fn origin_of(args: &Args) -> crate::box_fields::Origin {
    use crate::box_fields::{Base, Origin, point_index};
    Origin {
        base: args.opt_int("from_layer").map_or(Base::Canvas, |l| Base::Layer(LayerId(l as u64))),
        point: point_index(args.text("from_anchor").unwrap_or("top-left")),
    }
}

/// Where `origin` is in `doc`, or why it cannot be counted from.
fn located(origin: &crate::box_fields::Origin, doc: &opal_core::document::Document) -> Result<[f64; 2], String> {
    origin.locate(doc).ok_or_else(|| match origin.base {
        crate::box_fields::Base::Layer(id) if doc.layer(id).is_some() => {
            format!("Layer {} holds nothing, so there is nothing of it to count from.", id.0)
        }
        crate::box_fields::Base::Layer(id) => format!("There is no layer {} in the image.", id.0),
        crate::box_fields::Base::Canvas => unreachable!("the canvas is always there"),
    })
}

/// `value` with `notes` added to its "notes", if there are any.
pub(crate) fn with_notes(mut value: Value, notes: Vec<String>) -> Value {
    if !notes.is_empty()
        && let Value::Object(fields) = &mut value
        && let Some(list) = fields.entry("notes").or_insert_with(|| json!([])).as_array_mut()
    {
        list.extend(notes.into_iter().map(Value::from));
    }
    value
}

/// The width and height `args` give (once in pixels) for a canvas `c` wide and tall: as
/// given, or one given and the other in proportion, or the canvas's. Refused past the
/// largest side.
fn sides(args: &Args, c: (u32, u32)) -> Result<(u32, u32), String> {
    let ratio = |given: f64, a: u32, b: u32| (given * f64::from(b) / f64::from(a)).round().max(1.0);
    let (w, h) = match (args.opt_number("width"), args.opt_number("height")) {
        (Some(w), Some(h)) => (w.round(), h.round()),
        (Some(w), None) => (w.round(), ratio(w, c.0, c.1)),
        (None, Some(h)) => (ratio(h, c.1, c.0), h.round()),
        (None, None) => (f64::from(c.0), f64::from(c.1)),
    };
    if w.max(h) > f64::from(MAX_SIDE) {
        return Err(format!(
            "in proportion, that is {w} × {h} pixels, past the most a side can be ({MAX_SIDE}): give both sides."
        ));
    }
    Ok((w.max(1.0) as u32, h.max(1.0) as u32))
}

/// Whether scripts may change preference `d`: not those of scripting (what runs scripts,
/// who may connect, which would let a script or an assistant give itself more than it
/// has), nor curves, which are drawn.
fn settable(d: &crate::settings::Def) -> bool {
    d.section != "Scripting" && !matches!(d.kind, crate::settings::Kind::Curve { .. })
}

/// A setting's value as JSON: a color as [red, green, blue, alpha].
fn setting_json(v: &crate::settings::Value) -> Value {
    use crate::settings::Value as Set;
    match v {
        Set::Bool(b) => json!(b),
        Set::Float(f) => json!(f),
        Set::Choice(c) | Set::Text(c) => json!(c),
        Set::Color(c) => json!(c),
        Set::Curve(c) => serde_json::to_value(c).unwrap_or(Value::Null),
    }
}

/// Whether `value` is of `p`'s kind and in its range.
fn check_value(p: &Param, value: &Value) -> Result<(), String> {
    let name = p.name;
    let id = |v: &Value| v.as_i64().is_some_and(|i| i >= 0);
    let ok = match p.kind {
        Kind::Bool => value.is_boolean(),
        // An expression must read; what it comes to is checked once worked out.
        Kind::Int { .. } | Kind::Pixels { .. } | Kind::Number { .. } | Kind::Angle { .. } | Kind::Length { .. }
            if value.is_string() =>
        {
            let text = value.as_str().expect("checked");
            return Expr::parse(text).map(|_| ()).map_err(|e| expr_error(name, text, &e));
        }
        Kind::Int { min, max } | Kind::Pixels { min, max } => match value.as_i64() {
            Some(i) if (min..=max).contains(&i) => true,
            Some(i) => return Err(format!("\"{name}\" must lie between {min} and {max}, not {i}")),
            None => false,
        },
        Kind::Number { min, max } | Kind::Angle { min, max } => match value.as_f64() {
            Some(x) if (min..=max).contains(&x) => true,
            Some(x) => return Err(format!("\"{name}\" must lie between {min} and {max}, not {x}")),
            None => false,
        },
        Kind::Length { .. } => value.as_f64().is_some_and(f64::is_finite),
        Kind::Any => true,
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
        Kind::Points { min } => value.as_array().is_some_and(|a| {
            let point = |p: &Value| {
                p.as_array()
                    .is_some_and(|xy| xy.len() == 2 && xy.iter().all(|v| v.as_f64().is_some_and(f64::is_finite)))
            };
            a.len() >= min && a.iter().all(point)
        }),
    };
    if ok { Ok(()) } else { Err(format!("\"{name}\" must be {}, not {value}", kind_text(p.kind))) }
}

/// What a value of `kind` is, in words.
pub fn kind_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Bool => "true or false",
        Kind::Int { .. } => "a whole number, or an expression",
        Kind::Number { .. } => "a number, or an expression",
        Kind::Length { .. } => "a number, or an expression: a length with its unit (\"8.5in\"), or arithmetic",
        Kind::Angle { .. } => "a number of degrees, or an expression",
        Kind::Pixels { .. } => "a whole number of pixels, or an expression",
        Kind::Any => "a value",
        Kind::Text => "text that is not blank",
        Kind::Path => "a file's path",
        Kind::Choice(_) => "one of its words",
        Kind::Document => "an image's id",
        Kind::Layer => "a layer's id",
        Kind::Layers => "a list of layers' ids, not empty",
        Kind::Points { .. } => "a list of points, each [x, y] in pixels, enough of them",
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
        let Some(method) = method(name) else {
            return Reply::Now(Err(format!("There is no method \"{name}\".")));
        };
        if method.role == Role::Connection {
            return Reply::Now(Err(format!("{name} is about a connection to the editor, which answers it.")));
        }
        let args = match Args::check(method.params, given) {
            Ok(args) => args,
            Err(why) => return Reply::Now(Err(format!("{name}: {why}."))),
        };
        let args = match self.worked_out_args(method, args) {
            Ok(args) => args,
            Err(why) => return Reply::Now(Err(format!("{name}: {why}."))),
        };
        let told = if heard { self.messages.len() } else { usize::MAX };
        let notice = self.notice.clone();
        // The image and layers the user had, to put back if the change is refused (it makes
        // those it names the active ones).
        let view = (method.role == Role::Command).then(|| {
            (self.active_doc, self.docs.iter().map(|d| (d.id, d.layer_selection_state())).collect::<Vec<_>>())
        });
        self.calling = Some(Calling { name: name.to_owned(), heard, view });
        let reply = match method.cmd {
            Some(cmd) => self.carry_out(cmd, method.params, &args, ctx),
            None if method.name == "image" => self.image_by_args(&args, ctx),
            None if method.name == "pixels" => self.pixels_by_args(&args, ctx),
            None if method.name == "put_pixels" => self.put_pixels_by_args(&args, ctx),
            None if method.name == "crop" => match self.crop_by_args(&args) {
                Ok(()) => self.reply_when_done(|st, id| st.canvas_json_of(id)),
                Err(why) => Reply::Now(Err(why)),
            },
            None if method.name == "select_rectangle" => {
                Reply::Now(self.select_by_args(opal_core::mask::ShapeKind::Rectangle, &args))
            }
            None if method.name == "select_ellipse" => {
                Reply::Now(self.select_by_args(opal_core::mask::ShapeKind::Ellipse, &args))
            }
            None if method.name == "select_outline" => Reply::Now(self.select_outline_by_args(&args)),
            None if method.name == "remove_selection_step" => self.selection_step_by_args(true, &args),
            None if method.name == "edit_selection_step" => self.selection_step_by_args(false, &args),
            None if method.name == "delete_saved_selection" => Reply::Now(self.delete_saved_by_args(&args)),
            None if method.name == "set_preference" => Reply::Now(self.set_preference(&args, ctx)),
            None => Reply::Now(self.answer(&method.name, &args)),
        };
        let view = self.calling.take().expect("set above").view;
        let (refusals, notes) = self.told_since(told, notice);
        let reply = match reply {
            Reply::Now(Ok(_)) if !refusals.is_empty() => Reply::Now(Err(refusals.join("\n"))),
            Reply::Now(Ok(value)) if !notes.is_empty() => Reply::Now(Ok(with_notes(value, notes))),
            other => other,
        };
        if let (Some(view), Reply::Now(Err(_))) = (view, &reply) {
            self.restore_view(view);
        }
        reply
    }

    /// Puts the user's view back as `view` had it: the active image, and each image's
    /// selected and active layers.
    pub(crate) fn restore_view(&mut self, (active, selections): View) {
        for (id, s) in selections {
            if let Some(d) = self.doc_mut(id) {
                d.set_layer_selection_state(s);
            }
        }
        if let Some(id) = active.filter(|id| self.doc(*id).is_some()) {
            self.show_doc(id);
        }
    }

    /// [`call`](Self::call), made in `groups` (the script run the connection is, the step
    /// the call is in: what it changes in each image is joined into one history state when
    /// each ends: see [`group_ended`](Self::group_ended)), and `heard` unless it is a
    /// notification. A file opened by a notification (one handed over by an editor started
    /// to open it) brings the window to the front.
    pub fn call_from(
        &mut self,
        groups: &[u64],
        heard: bool,
        name: &str,
        given: &Map<String, Value>,
        ctx: &egui::Context,
    ) -> Reply {
        self.docs.iter_mut().for_each(|d| d.groups = groups.to_vec());
        let reply = self.call_heard(heard, name, given, ctx);
        self.docs.iter_mut().for_each(|d| d.groups.clear());
        if !heard && name == "open" {
            self.raise_window = true;
        }
        reply
    }

    /// Group `group` (a script run, or a step a client began: `step`) has ended: in each
    /// image, the states it made one after another become one, labeled `label`, so that one
    /// undo takes back what it did.
    pub fn group_ended(&mut self, group: u64, label: &str, step: bool) {
        self.docs.iter_mut().for_each(|d| d.group_ended(group, label, step));
    }

    /// What would have been told in a dialog since `told` messages were, while the status bar
    /// said `notice`, which comes back with a call's reply instead (the messages taken): the
    /// errors, and refusals (told as information), as the call's failure; the warnings, and a
    /// notice the status bar would have shown, as notes.
    pub(crate) fn told_since(
        &mut self,
        told: usize,
        notice: Option<(String, std::time::Instant)>,
    ) -> (Vec<String>, Vec<String>) {
        let messages: Vec<_> = self.messages.drain(told.min(self.messages.len())..).collect();
        let text = |m: &crate::state::Message| format!("{}: {}", m.title, m.body);
        let refusals: Vec<String> = messages.iter().filter(|m| m.kind != MessageKind::Warning).map(text).collect();
        let mut notes: Vec<String> = messages.iter().filter(|m| m.kind == MessageKind::Warning).map(text).collect();
        if self.notice != notice
            && let Some((said, _)) = &self.notice
        {
            notes.push(said.clone());
        }
        (refusals, notes)
    }

    /// The reply to the call being carried out (`calling`), which may have begun an edit of the
    /// active image (see `edits`): once that is applied, what `answer` gives of that image then;
    /// at once, if it began none (nothing, with no image open).
    fn reply_when_done(&mut self, answer: impl FnOnce(&AppState, DocId) -> Value + 'static) -> Reply {
        let Some(id) = self.active_doc else { return Reply::Now(Ok(json!({}))) };
        let begun = self.doc(id).and_then(|d| d.busy.as_ref()).is_some_and(|b| b.answer.is_none());
        if !begun {
            return Reply::Now(Ok(answer(self, id)));
        }
        self.next_answer += 1;
        let n = self.next_answer;
        let call = self.calling.clone().expect("a call's reply");
        let b = self.doc_mut(id).and_then(|d| d.busy.as_mut()).expect("checked above");
        b.answer = Some(crate::edits::Answer { n, call, with: Box::new(answer) });
        Reply::Later(n)
    }

    /// Whether scripts' calls wait now: while the user has a dialog open, holds a button
    /// (`pointer_down`), or has a transform box open, an edit in progress that a command
    /// would commit under them; or while an edit is under way (`edits`), which the script
    /// may have begun and would go on from.
    pub fn scripts_wait(&self, pointer_down: bool) -> bool {
        self.dialog_open() || pointer_down || self.docs.iter().any(|d| d.transform.is_some()) || self.editing()
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
                            "resolution": d.doc().resolution,
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
            "history" => {
                let d = self.doc(self.which(args)?).expect("found");
                Ok(history_json(d))
            }
            "selection" => {
                let d = self.doc(self.which(args)?).expect("found");
                Ok(selection_json(d))
            }
            "saved_selections" => {
                let d = self.doc(self.which(args)?).expect("found");
                Ok(saved_json(d.doc()))
            }
            "methods" => Ok(describe()),
            "preferences" => Ok(self.preferences_json()),
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
        // (A paste shows its image once it is placed, which may be refused after the call has
        // returned, when nothing would put the user's back.)
        let pastes = matches!(cmd, Cmd::Paste | Cmd::PasteInPlace);
        if params.iter().any(|p| p.kind == Kind::Document) && !pastes {
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
                let (width, height) = (args.side("width").expect("required"), args.side("height").expect("required"));
                let resolution = args.number("resolution");
                self.create_image(&NewImage { width, height, background, resolution });
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
                let all = crate::memory::Allowance::now();
                if self.copy_within(cmd == Cmd::CopyMerged, all, Some(n), ctx) {
                    Reply::Later(n)
                } else {
                    now(Ok(json!({})))
                }
            }
            Cmd::Cut => {
                self.next_answer += 1;
                let n = self.next_answer;
                if self.cut(Some(n), ctx) { Reply::Later(n) } else { now(Ok(json!({}))) }
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
                let d = self.active().expect("available").doc();
                let (c, resolution) = (d.canvas, args.opt_number("resolution").unwrap_or(d.resolution));
                let (width, height) = match sides(args, (c.width, c.height)) {
                    Ok(sides) => sides,
                    Err(why) => return now(Err(format!("resize_image: {why}"))),
                };
                let spec = ResizeImage { width, height, keep_aspect: false, resolution, keep_print_size: false };
                let resample = args.flag("resample").unwrap_or_else(|| self.prefs.bool("resize.resample"));
                self.resize_image(&spec, resample);
                self.reply_when_done(|st, id| st.canvas_json_of(id))
            }
            Cmd::ResizeCanvas => {
                let c = self.active().expect("available").doc().canvas;
                let anchor = args.text("anchor").map(|a| crate::box_fields::point(a).map(|f| (f * 2.0) as u8));
                let margins = ["left", "top", "right", "bottom"].map(|s| args.opt_number(s).map(|v| v.round() as i64));
                let by_margins = margins.iter().any(Option::is_some);
                if by_margins && ["width", "height", "anchor"].iter().any(|n| args.get(n).is_some()) {
                    return now(Err(
                        "resize_canvas: give margins (left, top, right, bottom), or a size and anchor, not both."
                            .to_owned(),
                    ));
                }
                let spec = ResizeCanvas {
                    width: args.side("width").unwrap_or(c.width),
                    height: args.side("height").unwrap_or(c.height),
                    anchor: anchor.unwrap_or([1, 1]),
                    by_margins,
                    margins: margins.map(|m| m.unwrap_or(0)),
                };
                self.resize_canvas(&spec);
                now(Ok(self.canvas_json()))
            }
            Cmd::RotateBy => {
                let degrees = args.number("degrees");
                if degrees != 0.0 {
                    self.rotate_by(&RotateBy { degrees, grow: args.flag("grow").unwrap_or(true) });
                }
                self.reply_when_done(|st, id| st.canvas_json_of(id))
            }
            Cmd::RotateCw | Cmd::RotateCcw | Cmd::Rotate180 | Cmd::FlipHorizontal | Cmd::FlipVertical => {
                self.turn(match cmd {
                    Cmd::RotateCw => Turn::Clockwise,
                    Cmd::RotateCcw => Turn::Counterclockwise,
                    Cmd::Rotate180 => Turn::Half,
                    Cmd::FlipHorizontal => Turn::FlipHorizontal,
                    _ => Turn::FlipVertical,
                });
                self.reply_when_done(|st, id| st.canvas_json_of(id))
            }
            Cmd::ToggleVisible | Cmd::ToggleLock => {
                let visibility = cmd == Cmd::ToggleVisible;
                let wanted = args.flag(if visibility { "visible" } else { "locked" });
                let d = self.active_mut().expect("available");
                let ids = d.selected_layers();
                let flag = |l: &Layer| if visibility { l.visible } else { l.locked };
                // Already as asked: nothing to record.
                if wanted.is_some_and(|w| ids.iter().all(|&id| d.doc().layer(id).is_none_or(|l| flag(l) == w))) {
                    return now(Ok(self.selected_layers_json()));
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
                now(Ok(self.selected_layers_json()))
            }
            Cmd::Paste | Cmd::PasteInPlace => {
                // The image named, or the active one; with none open, and none named, the
                // paste makes one.
                let target = match (args.document(), self.active_doc) {
                    (None, None) => None,
                    _ => match self.which(args) {
                        Ok(id) => Some(id),
                        Err(why) => return now(Err(why)),
                    },
                };
                let groups = target.and_then(|t| self.doc(t)).map(|d| d.groups.clone()).unwrap_or_default();
                let place = match BoxArgs::of(args, params, target.and_then(|t| self.doc(t)).map(|d| d.history.doc())) {
                    Ok(place) => place,
                    Err(why) => return now(Err(format!("{}: {why}", snake(&format!("{cmd:?}"))))),
                };
                self.next_answer += 1;
                let answer = crate::state::PasteAnswer {
                    n: self.next_answer,
                    place,
                    into_active: args.flag("into_active") == Some(true),
                    groups,
                };
                let n = answer.n;
                self.paste_answering(cmd == Cmd::PasteInPlace, target, Some(answer), ctx);
                // Answered at once if the clipboard was read at once, else once it is.
                Reply::Later(n)
            }
            Cmd::Place => {
                let place = match BoxArgs::of(args, params, self.active().map(|d| d.history.doc())) {
                    Ok(place) => place,
                    Err(why) => return now(Err(format!("place: {why}"))),
                };
                let name = args.text("name");
                let print_size =
                    args.flag("print_size").unwrap_or_else(|| self.prefs.bool("transform.place_print_size"));
                match self.place_file(&args.path("path"), name, print_size) {
                    Ok(warnings) => {
                        let notes = warnings.into_iter().map(|w| format!("Placed: {w}")).collect();
                        let into = args.flag("into_active") == Some(true);
                        now(self.apply_box(&place, into).map(|v| with_notes(v, notes)))
                    }
                    Err(why) => now(Err(format!("place: {why}"))),
                }
            }
            Cmd::Transform => {
                let place = match BoxArgs::of(args, params, self.active().map(|d| d.history.doc())) {
                    Ok(place) => place,
                    Err(why) => return now(Err(format!("transform: {why}"))),
                };
                self.transform();
                if self.active().is_some_and(|d| d.transform.is_none()) {
                    // Refused, and said why.
                    return now(Ok(json!({})));
                }
                now(self.apply_box(&place, false))
            }
            Cmd::SaveSelection => {
                let name = args.text("name").expect("required");
                if let Some(why) = crate::select::save_name_refused(name) {
                    return now(Err(format!("save_selection: {why}")));
                }
                self.save_selection(name);
                now(Ok(saved_json(self.active().expect("available").doc())))
            }
            Cmd::LoadSelection => {
                let name = args.text("name").expect("required");
                let combine = args.text("combine").and_then(opal_core::mask::Combine::named).unwrap_or_default();
                self.load_selection(name, combine, args.flag("invert").unwrap_or(false));
                self.reply_when_done(|st, id| selection_json(st.doc(id).expect("open")))
            }
            Cmd::TransformSelection => {
                let place = match BoxArgs::of(args, params, self.active().map(|d| d.history.doc())) {
                    Ok(place) => place,
                    Err(why) => return now(Err(format!("transform_selection: {why}"))),
                };
                self.transform_selection();
                let Some(d) = self.active_mut().filter(|d| d.transform.is_some()) else {
                    // Refused, and said why.
                    return now(Ok(json!({})));
                };
                if let Err(why) = place_box(d, &place) {
                    d.cancel_transform();
                    return now(Err(why));
                }
                let id = d.id;
                self.commit_box(id);
                self.reply_when_done(|st, id| selection_json(st.doc(id).expect("its image")))
            }
            Cmd::Rename => {
                let id = self.active().and_then(|d| d.active_layer()).expect("available");
                self.rename_layer(doc.expect("available"), id, args.text("name").expect("required").to_owned());
                now(Ok(json!({ "layer": id.0 })))
            }
            Cmd::GrowSelection | Cmd::FeatherSelection | Cmd::SmoothSelection | Cmd::BorderSelection => {
                use opal_core::modify::{Falloff, Reach, Side};
                use opal_core::selection::Step;
                // Each a name its choice allows (checked), or its default.
                let step = match cmd {
                    Cmd::GrowSelection => Step::Grow {
                        by: args.number("by"),
                        corners: args.text("corners").and_then(Reach::named).unwrap_or_default(),
                    },
                    Cmd::SmoothSelection => Step::Smooth { radius: args.number("radius") },
                    Cmd::BorderSelection => Step::Border {
                        width: args.number("width"),
                        side: args.text("side").and_then(Side::named).unwrap_or_default(),
                    },
                    _ => Step::Feather {
                        width: args.number("width"),
                        side: args.text("side").and_then(Side::named).unwrap_or_default(),
                        falloff: args.text("falloff").and_then(Falloff::named).unwrap_or_default(),
                    },
                };
                self.add_modifier(step);
                self.reply_when_done(|st, id| selection_json(st.doc(id).expect("its edit was applied")))
            }
            Cmd::SelectLayerPixels => {
                let combine = args.text("combine").and_then(opal_core::mask::Combine::named).unwrap_or_default();
                let layer = self.active().and_then(|d| d.active_layer()).expect("available");
                self.select_layer_pixels(layer, combine);
                self.reply_when_done(|st, id| selection_json(st.doc(id).expect("open")))
            }
            Cmd::SelectAll | Cmd::Deselect | Cmd::InvertSelection => {
                self.run(cmd, ctx);
                // What is selected now, as the `selection` query tells it.
                now(Ok(selection_json(self.active().expect("available"))))
            }
            // Carried as the image has changed since, which may be worked out off the interface
            // thread.
            Cmd::Reselect => {
                self.run(cmd, ctx);
                self.reply_when_done(|st, id| selection_json(st.doc(id).expect("open")))
            }
            _ => {
                // The rest take no more than the image and layers set above.
                self.run(cmd, ctx);
                self.reply_when_done(|st, id| st.selected_layers_json_of(id))
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
        let (width, height) = match sides(args, (c.width, c.height)) {
            Ok(sides) => sides,
            Err(why) => return Reply::Now(Err(format!("export: {why}"))),
        };
        let size = ((width, height) != (c.width, c.height)).then(|| ExportSize {
            width,
            height,
            sampling: crate::prefs::sampling(&self.prefs, "export"),
        });
        self.reply_after_write(|st, then| st.export_to(doc, path, format, size, None, then, ctx))
    }

    /// The resolution lengths given to `method` with `args` are converted at: the one they
    /// give (a new image's, a resize's), else that of the image the method acts on, else
    /// the default.
    fn resolution_for(&self, method: &Method, args: &Args) -> f64 {
        if let Some(ppi) = args.opt_number("resolution") {
            return ppi;
        }
        let acts_on_one = method.params.iter().any(|p| p.kind == Kind::Document);
        let doc = acts_on_one.then(|| self.which(args).ok()).flatten().and_then(|id| self.doc(id));
        doc.map_or(DEFAULT_PPI, |d| d.doc().resolution)
    }

    /// `args` (checked) with their numbers worked out: expressions in the image the method
    /// acts on (else the active one), positions from its canvas's top-left corner. The
    /// resolution first, as the lengths are converted at it; a box's numbers given as
    /// expressions are left for the box (see [`BoxArgs::worked_out`]).
    fn worked_out_args(&self, method: &Method, args: Args) -> Result<Args, String> {
        let acts_on_one = method.params.iter().any(|p| p.kind == Kind::Document);
        let id = if acts_on_one { self.which(&args).ok() } else { self.active_doc };
        let doc = id.and_then(|id| self.doc(id)).map(|d| d.doc());
        let origin = doc.map_or([0.0, 0.0], |d| [d.canvas.x as f64, d.canvas.y as f64]);
        let scope = |ppi| Scope { doc, ppi, unit: Unit::Px, origin, own: None };
        let ppi = doc.map_or(DEFAULT_PPI, |d| d.resolution);
        let args = args.worked_out(method.params, &scope(ppi), &[], |p| p.name == "resolution")?;
        let ppi = self.resolution_for(method, &args);
        let later: &[&str] = if takes_box(method) { &BOXED } else { &[] };
        args.worked_out(method.params, &scope(ppi), later, |p| p.name != "resolution")
    }

    /// Puts the transform box open on the active image where `place` says, and applies it
    /// (into the active layer, for a paste, if `into_active`). Answers with the layer it
    /// ends on (or the selected layers, for a transform) and the box: in pixels, its anchor
    /// point from the point `place` counts from. Refused, the box cancelled, if it would go
    /// past what an image file can hold, or there is no layer to go into.
    pub(crate) fn apply_box(&mut self, place: &BoxArgs, into_active: bool) -> Result<Value, String> {
        let d = self.active_mut().expect("a box is open");
        let corner = place.from;
        let paste = d.transform.as_ref().expect("a box is open").paste.clone();
        if let Err(why) = place_box(d, place) {
            d.cancel_transform();
            return Err(why);
        }
        if into_active {
            match paste {
                Some(p) if p.active.is_some() => d.toggle_paste_into_active(),
                Some(_) => {
                    d.cancel_transform();
                    return Err("There is no plain, unlocked active layer to put it into.".to_owned());
                }
                None => {}
            }
        }
        let s = d.transform.as_ref().expect("still open");
        let b = s.params_at(place.at);
        let (kind, layer) = (s.kind, s.targets[0].layer);
        d.notice = None;
        d.commit_pending();
        let boxed = json!({
            "x": b.x - corner[0],
            "y": b.y - corner[1],
            "width": b.width,
            "height": b.height,
            "angle": b.angle,
        });
        let out = if kind == crate::transform::Kind::Transform {
            json!({ "layers": d.selected_layers().iter().map(|l| l.0).collect::<Vec<_>>(), "box": boxed })
        } else {
            json!({ "layer": layer.0, "box": boxed })
        };
        // What applying it said (it stays an image, say), the status bar says too.
        Ok(with_notes(out, d.notice.clone().into_iter().collect()))
    }

    /// Crops the image `args` names to the rectangle they give (see the `crop` method).
    fn crop_by_args(&mut self, args: &Args) -> Result<(), String> {
        let id = self.which(args)?;
        self.show_doc(id);
        let delete =
            args.flag("delete").unwrap_or_else(|| self.tool_settings(crate::tools::ToolId::Crop).bool("crop.delete"));
        let sampling = crate::prefs::sampling(&self.prefs, "transform");
        let b = self.box_by_args(id, args, "crop", true)?;
        let d = self.doc_mut(id).expect("found");
        let c = d.doc().canvas;
        // The Crop tool's box, if it had one, goes back around the new canvas (once cropped).
        if b.canvas() != c || b.angle.abs() > 1e-9 || delete {
            self.crop_with(id, b, delete, sampling).map_err(|why| format!("crop: {why}"))?;
        } else {
            d.crop = None;
        }
        Ok(())
    }

    /// The box call `name` (`crop`, or a select call) gives in `args` for image `id`, as its
    /// box numbers measure it: the canvas's where they leave numbers out (x and y where the
    /// anchor's point of it is, so too for an expression naming the box), its sides whole
    /// pixels if `whole`.
    fn box_by_args(&self, id: DocId, args: &Args, name: &str, whole: bool) -> Result<crate::edit_box::EditBox, String> {
        let params = method(name).expect("a method").params;
        let d = self.doc(id).expect("found");
        let (c, ppi) = (d.doc().canvas, d.doc().resolution);
        let doc = d.history.doc();
        let place = BoxArgs::of(args, params, Some(doc)).map_err(|why| format!("{name}: {why}"))?;
        let (at, from) = (place.at, place.from);
        let (w, h) = (f64::from(c.width), f64::from(c.height));
        let own = Own {
            x: c.x as f64 + at[0] * w - from[0],
            y: c.y as f64 + at[1] * h - from[1],
            width: w,
            height: h,
            angle: 0.0,
        };
        let b = place.worked_out(doc, ppi, own).map_err(|why| format!("{name}: {why}"))?;
        let side = |v: Option<f64>, canvas: f64| v.map_or(canvas, |v| if whole { v.round() } else { v });
        let p = crate::transform::Params {
            x: b.x.unwrap_or(own.x),
            y: b.y.unwrap_or(own.y),
            width: side(b.width, w),
            height: side(b.height, h),
            angle: b.angle.unwrap_or(0.0),
            mirrored: false,
        };
        Ok(crate::edit_box::EditBox::from_numbers(&p, from, at))
    }

    /// Selects a shape of `kind` as `args` give its box (the canvas's, unless they say), as
    /// `select_rectangle` and `select_ellipse` do; what is selected then, as the `selection`
    /// query tells it.
    fn select_by_args(&mut self, kind: opal_core::mask::ShapeKind, args: &Args) -> Result<Value, String> {
        use opal_core::mask::{Combine, Shape, ShapeKind};
        let name = match kind {
            ShapeKind::Rectangle => "select_rectangle",
            ShapeKind::Ellipse => "select_ellipse",
        };
        let id = self.which(args)?;
        self.show_doc(id);
        let smooth = args
            .flag("smooth")
            .unwrap_or_else(|| self.tool_settings(crate::tools::ToolId::Select).bool("select.smooth"));
        let combine = args.text("combine").and_then(Combine::named).unwrap_or_default();
        let b = self.box_by_args(id, args, name, false)?;
        let like = Shape { kind, center: [0.0; 2], size: [0.0; 2], angle: 0.0, smooth };
        let d = self.doc_mut(id).expect("found");
        let step = opal_core::selection::Step::Shape { shape: b.shape(&like), combine };
        let made = opal_core::selection::with_maker(d.pixel_selection().map(|s| &**s), step);
        d.set_pixel_selection(Some(made), crate::select_tool::label(crate::select_tool::Draws::Shape(kind)));
        Ok(selection_json(d))
    }

    /// Deletes the saved selection `args` name (`delete_saved_selection`): the selections saved
    /// then, as `saved_selections` tells them; or why not.
    fn delete_saved_by_args(&mut self, args: &Args) -> Result<Value, String> {
        let id = self.which(args)?;
        self.show_doc(id);
        let name = args.text("name").expect("required");
        if self.doc(id).and_then(|d| d.doc().saved_selection(name)).is_none() {
            return Err(format!("delete_saved_selection: No selection is saved as \"{name}\"."));
        }
        self.delete_saved_selection(name);
        Ok(saved_json(self.doc(id).expect("open").doc()))
    }

    /// Takes away (`remove`), or changes as `args` say, the step of the selection they name
    /// (`remove_selection_step`, `edit_selection_step`): answered once the selection is worked
    /// out again, with what is selected then.
    fn selection_step_by_args(&mut self, remove: bool, args: &Args) -> Reply {
        let name = if remove { "remove_selection_step" } else { "edit_selection_step" };
        let id = match self.which(args) {
            Ok(id) => id,
            Err(why) => return Reply::Now(Err(why)),
        };
        self.show_doc(id);
        let Some(selection) = self.doc(id).and_then(|d| d.pixel_selection()).cloned() else {
            return Reply::Now(Err(format!("{name}: Nothing is selected.")));
        };
        let index = args.int("index") as usize;
        let Some(step) = selection.steps().get(index).cloned() else {
            let n = selection.steps().len();
            return Reply::Now(Err(format!(
                "{name}: The selection has {n} steps, numbered from 0: there is no step {index}."
            )));
        };
        if remove {
            self.remove_selection_step(index);
        } else {
            let changed = match changed_step(step, args) {
                Ok(changed) => changed,
                Err(why) => return Reply::Now(Err(format!("{name}: {why}"))),
            };
            let mut steps = selection.steps().to_vec();
            steps[index] = changed;
            self.rework_selection(steps, crate::select::STEP_CHANGED);
        }
        self.reply_when_done(|st, id| selection_json(st.doc(id).expect("its image")))
    }

    /// Selects what the outline `args` give encloses (`select_outline`); what is selected then,
    /// as the `selection` query tells it.
    fn select_outline_by_args(&mut self, args: &Args) -> Result<Value, String> {
        use opal_core::outline::{Outline, Segment};
        let id = self.which(args)?;
        self.show_doc(id);
        let smooth = args
            .flag("smooth")
            .unwrap_or_else(|| self.tool_settings(crate::tools::ToolId::Outline).bool("outline.smooth"));
        let combine = args.text("combine").and_then(opal_core::mask::Combine::named).unwrap_or_default();
        let d = self.doc_mut(id).expect("found");
        let c = d.doc().canvas;
        // From the canvas's corner, as everywhere in the API.
        let points: Vec<[f64; 2]> = args
            .get("points")
            .and_then(Value::as_array)
            .expect("checked")
            .iter()
            .map(|p| [p[0].as_f64().expect("checked") + c.x as f64, p[1].as_f64().expect("checked") + c.y as f64])
            .collect();
        let outline = Outline {
            start: points[0],
            segments: points[1..].iter().map(|&to| Segment::Line { to }).collect(),
            smooth,
        };
        let step = opal_core::selection::Step::Outline { outline, combine };
        let made = opal_core::selection::with_maker(d.pixel_selection().map(|s| &**s), step);
        d.set_pixel_selection(Some(made), crate::outline_tool::LABEL);
        Ok(selection_json(d))
    }

    /// Every preference, as the `preferences` query tells them.
    fn preferences_json(&self) -> Value {
        use crate::settings::Kind as Setting;
        let prefs: Vec<Value> = crate::prefs::PREFS
            .iter()
            .map(|d| {
                let mut v = json!({
                    "key": d.key,
                    "label": d.label,
                    "section": d.section,
                    "help": d.help,
                    "value": setting_json(&self.prefs.get(d.key)),
                    "default": setting_json(&d.default_value()),
                });
                v["kind"] = json!(match d.kind {
                    Setting::Bool => "bool",
                    Setting::Float { min, max, .. } => {
                        (v["min"], v["max"]) = (json!(min), json!(max));
                        "number"
                    }
                    Setting::Choice(options) => {
                        v["choices"] = json!(options.iter().map(|(id, _)| *id).collect::<Vec<_>>());
                        "choice"
                    }
                    Setting::Color => "color",
                    Setting::Curve { .. } => "curve",
                    Setting::Text { .. } => "text",
                });
                if !settable(d) {
                    v["settable"] = json!(false);
                }
                v
            })
            .collect();
        json!(prefs)
    }

    /// Changes the preference `args` names, or puts it back to its default (see the
    /// `set_preference` method), applying it to `ctx` as the Preferences window does.
    fn set_preference(&mut self, args: &Args, ctx: &egui::Context) -> Result<Value, String> {
        use crate::settings::{Kind as Setting, Value as Set};
        let key = args.text("key").expect("required");
        let Some(def) = crate::prefs::PREFS.iter().find(|d| d.key == key) else {
            return Err(format!(
                "set_preference: there is no preference \"{key}\" (the preferences query lists them)."
            ));
        };
        if !settable(def) {
            return Err(format!(
                "set_preference: {key} is changed in the Preferences window only{}.",
                if def.section == "Scripting" {
                    ": it says what runs scripts and who may connect, which no script or assistant may change"
                } else {
                    ""
                }
            ));
        }
        let Some(given) = args.get("value") else {
            self.prefs.reset(key);
            self.settings_touched();
            self.apply_prefs(ctx);
            return Ok(json!({ "key": key, "value": setting_json(&self.prefs.get(key)) }));
        };
        let value = match (def.kind, given) {
            (Setting::Bool, Value::Bool(b)) => Some(Set::Bool(*b)),
            (Setting::Float { .. }, Value::Number(n)) => n.as_f64().map(Set::Float),
            (Setting::Choice(_), Value::String(s)) => Some(Set::Choice(s.clone())),
            (Setting::Text { .. }, Value::String(s)) => Some(Set::Text(s.clone())),
            (Setting::Color, Value::Array(a)) if matches!(a.len(), 3 | 4) => {
                let c: Option<Vec<u8>> = a.iter().map(|x| x.as_u64().and_then(|x| u8::try_from(x).ok())).collect();
                c.map(|c| Set::Color([c[0], c[1], c[2], c.get(3).copied().unwrap_or(255)]))
            }
            _ => None,
        };
        let Some(value) = value.filter(|v| def.accepts(v)) else {
            let takes = match def.kind {
                Setting::Bool => "true or false".to_owned(),
                Setting::Float { min, max, .. } => format!("a number from {min} to {max}"),
                Setting::Choice(options) => {
                    format!(
                        "one of {}",
                        options.iter().map(|(id, _)| format!("\"{id}\"")).collect::<Vec<_>>().join(", ")
                    )
                }
                Setting::Color => "[red, green, blue] or [red, green, blue, alpha], each from 0 to 255".to_owned(),
                Setting::Text { .. } => "text that is not blank and has no spaces at either end".to_owned(),
                Setting::Curve { .. } => unreachable!("not settable"),
            };
            return Err(format!("set_preference: {key} takes {takes}, not {given}."));
        };
        self.prefs.set(key, value);
        self.settings_touched();
        self.apply_prefs(ctx);
        Ok(json!({ "key": key, "value": setting_json(&self.prefs.get(key)) }))
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
                done: Done::Answer(json!({ "png": base64(&png), "width": width, "height": height })),
                keep: None,
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
                let name = crate::shared_memory::new_name().map_err(|e| format!("pixels: {e}"))?;
                let mut mapping = crate::shared_memory::Mapping::create(&name, len)?;
                pool.install(|| format.write(&pixels, mapping.bytes_mut()));
                answer["shm"] = json!(name);
                answer["bytes"] = json!(len);
                Ok(Made { done: Done::Answer(answer), keep: Some((name, mapping)) })
            } else {
                let mut out = vec![0u8; len];
                pool.install(|| format.write(&pixels, &mut out));
                answer["data"] = json!(base64(&out));
                Ok(Made { done: Done::Answer(answer), keep: None })
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
            (Some(name), None) => Source::Shared(name.to_owned()),
            (None, Some(data)) => Source::Data(data.to_owned()),
            _ => return now(Err("put_pixels: give the pixels in shm or in data, one of them.".to_owned())),
        };
        let canvas = d.doc().canvas.rect();
        let (x0, y0) = (canvas.x0 + args.int("x"), canvas.y0 + args.int("y"));
        let rect = IRect::new(x0, y0, x0 + width, y0 + height);
        let name = args.text("name").map(str::to_owned);
        let groups = d.groups.clone();
        let (w, h) = (width as u32, height as u32);
        self.off_thread("put", ctx, move || {
            // What they are read into, and the tiles they become.
            let mut memory = crate::memory::Allowance::now();
            memory.take(crate::memory::Need::at_once(count * 16))?;
            let pool = crate::background::pool();
            // All their parallel work in the background pool, not the interface thread's.
            let made = |bytes: &[u8]| {
                pool.install(|| match (target, format) {
                    // Kept at 8 bits, as an 8-bit image file's pixels are.
                    (None, PixelFormat::Srgb8) => {
                        Content::Image(Box::new(RasterImage::from_srgb8(w, h, bytemuck::cast_slice(bytes))))
                    }
                    (None, _) => Content::Image(Box::new(RasterImage::from_pixels(w, h, &format.read(bytes)))),
                    (Some(_), _) => Content::Pixels(format.read(bytes)),
                })
            };
            let content = match source {
                Source::Shared(shm) => made(&crate::shared_memory::Mapping::open(&shm, len)?.bytes()[..len]),
                Source::Data(data) => {
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
            let finish: Finish = Box::new(move |st| put_pixels_now(st, id, target, rect, content, name, groups));
            Ok(Made { done: Done::Finish(finish), keep: None })
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
                match made.done {
                    Done::Finish(finish) => finishing.push_back((*n, finish)),
                    Done::Answer(answer) => answered.push((*n, Ok(answer))),
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
        self.canvas_json_of(self.active_doc.expect("available"))
    }

    /// Image `id`'s canvas and resolution, as the calls that change them answer.
    fn canvas_json_of(&self, id: DocId) -> Value {
        let d = self.doc(id).expect("open").doc();
        json!({ "width": d.canvas.width, "height": d.canvas.height, "resolution": d.resolution })
    }

    /// The active image's selected layers, and its active layer, as many calls answer.
    fn selected_layers_json(&self) -> Value {
        self.active_doc.map_or_else(|| json!({}), |id| self.selected_layers_json_of(id))
    }

    /// Image `id`'s selected layers and its active layer (nothing, if it is closed).
    fn selected_layers_json_of(&self, id: DocId) -> Value {
        match self.doc(id) {
            Some(d) => json!({
                "layers": d.selected_layers().iter().map(|l| l.0).collect::<Vec<_>>(),
                "active": d.active_layer().map(|l| l.0),
            }),
            None => json!({}),
        }
    }
}

/// `step` changed as `args` say (see `edit_selection_step`), or why it cannot be: it does not
/// take a setting they give.
fn changed_step(step: opal_core::selection::Step, args: &Args) -> Result<opal_core::selection::Step, String> {
    use opal_core::mask::Combine;
    use opal_core::modify::{Falloff, Reach, Side};
    use opal_core::selection::Step;
    let (kind, takes): (&str, &[&str]) = match &step {
        Step::Shape { .. } => ("A shape", &["combine"]),
        Step::Outline { .. } => ("An outline", &["combine"]),
        Step::Mask { .. } => ("A mask", &["combine"]),
        Step::Invert => ("An inverting step", &[]),
        Step::Grow { .. } => ("A grow", &["by", "corners"]),
        Step::Feather { .. } => ("A feather", &["width", "side", "falloff"]),
        Step::Smooth { .. } => ("A smooth", &["radius"]),
        Step::Border { .. } => ("A border", &["width", "side"]),
    };
    let given = ["combine", "by", "corners", "width", "side", "falloff", "radius"];
    if let Some(wrong) = given.iter().find(|&&g| args.get(g).is_some() && !takes.contains(&g)) {
        return Err(match takes {
            [] => format!("{kind} has nothing to change."),
            _ => format!("{kind} takes {}, not {wrong}.", takes.join(" and ")),
        });
    }
    let combine = |c: Combine| args.text("combine").and_then(Combine::named).unwrap_or(c);
    let side = |s: Side| args.text("side").and_then(Side::named).unwrap_or(s);
    Ok(match step {
        Step::Shape { shape, combine: c } => Step::Shape { shape, combine: combine(c) },
        Step::Outline { outline, combine: c } => Step::Outline { outline, combine: combine(c) },
        Step::Mask { mask, combine: c, of } => Step::Mask { mask, combine: combine(c), of },
        Step::Invert => Step::Invert,
        Step::Grow { by, corners } => Step::Grow {
            by: args.opt_number("by").unwrap_or(by),
            corners: args.text("corners").and_then(Reach::named).unwrap_or(corners),
        },
        Step::Feather { width, side: s, falloff } => Step::Feather {
            width: args.opt_number("width").unwrap_or(width),
            side: side(s),
            falloff: args.text("falloff").and_then(Falloff::named).unwrap_or(falloff),
        },
        Step::Smooth { radius } => Step::Smooth { radius: args.opt_number("radius").unwrap_or(radius) },
        Step::Border { width, side: s } => {
            Step::Border { width: args.opt_number("width").unwrap_or(width), side: side(s) }
        }
    })
}

/// What image `d` has selected, as the `selection` query gives it: the rectangle round it and
/// the steps that made it, positions counted from the canvas's corner, as everywhere in the
/// API.
fn selection_json(d: &crate::doc::DocState) -> Value {
    match d.pixel_selection() {
        Some(s) => selection_value(s, &d.doc().canvas, &opal_core::geom::Affine::IDENTITY),
        None => json!({ "bounds": Value::Null, "steps": [] }),
    }
}

/// The selections saved in `doc`, as the `saved_selections` query gives them.
fn saved_json(doc: &opal_core::document::Document) -> Value {
    let saved: Vec<Value> = doc
        .saved
        .iter()
        .map(|s| {
            let mut v = selection_value(&s.kept.selection, &doc.canvas, &s.kept.since);
            v["name"] = s.name.clone().into();
            v["changed_since"] = (s.kept.since != opal_core::geom::Affine::IDENTITY).into();
            v
        })
        .collect();
    json!(saved)
}

/// Selection `s` as the `selection` query tells it: the rectangle round what it selects,
/// carried by `since` (the box round its corners so carried), from the corner of `c`; and its
/// steps, shapes by their middles from that corner.
fn selection_value(
    s: &opal_core::selection::Selection,
    c: &opal_core::document::Canvas,
    since: &opal_core::geom::Affine,
) -> Value {
    use opal_core::selection::Step;
    let bounds = match s.mask().extent() {
        opal_core::mask::Extent::Nothing => Value::Null,
        opal_core::mask::Extent::Within(r) => {
            let r = opal_core::geom::IRect::covering(&since.bounds(&r.to_f()));
            json!([r.x0 - c.x, r.y0 - c.y, r.width(), r.height()])
        }
        opal_core::mask::Extent::Unbounded => json!("unbounded"),
    };
    let steps: Vec<Step> = s
        .steps()
        .iter()
        .map(|step| match step {
            Step::Shape { shape, combine } => {
                let center = [shape.center[0] - c.x as f64, shape.center[1] - c.y as f64];
                Step::Shape { shape: opal_core::mask::Shape { center, ..*shape }, combine: *combine }
            }
            Step::Outline { outline, combine } => {
                let from_corner = opal_core::geom::Affine::translate(-c.x as f64, -c.y as f64);
                Step::Outline { outline: outline.transformed(&from_corner), combine: *combine }
            }
            other => other.clone(),
        })
        .collect();
    json!({ "bounds": bounds, "steps": steps })
}

/// Image `d`'s history, as the `history` query gives it.
fn history_json(d: &crate::doc::DocState) -> Value {
    let h = &d.history;
    let id = |n: opal_core::history::NodeId| n.0;
    let states: Vec<Value> = h
        .nodes()
        .map(|(n, node)| {
            let time = node.created.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |t| t.as_secs_f64());
            json!({
                "state": id(n),
                "label": node.label,
                "time": time,
                "parent": node.parent.map(id),
                "children": node.children.iter().copied().map(id).collect::<Vec<_>>(),
                "redo": h.redo_target_of(n).map(id),
            })
        })
        .collect();
    // Saved at a state no longer kept: as good as at none.
    let saved = d.saved_at.filter(|&s| h.contains(s)).map(id);
    json!({ "current": id(h.current()), "saved": saved, "dropped": h.dropped(), "states": states })
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
                            Kind::Int { min, max } | Kind::Pixels { min, max } => {
                                (v["min"], v["max"]) = (json!(min), json!(max))
                            }
                            Kind::Number { min, max } | Kind::Length { min, max } | Kind::Angle { min, max } => {
                                (v["min"], v["max"]) = (json!(min), json!(max))
                            }
                            Kind::Choice(words) => v["choices"] = json!(words),
                            Kind::Points { min } => v["min"] = json!(min),
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
                json!({ "name": m.name, "help": m.help, "kind": role_name(m.role), "params": params })
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
        Kind::Length { .. } => "length",
        Kind::Angle { .. } => "angle",
        Kind::Pixels { .. } => "pixels",
        Kind::Any => "any",
        Kind::Text => "text",
        Kind::Path => "path",
        Kind::Choice(_) => "choice",
        Kind::Document => "document",
        Kind::Layer => "layer",
        Kind::Layers => "layers",
        Kind::Points { .. } => "points",
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
            "document" | "layer" => "int".into(),
            // An expression is text, or built by the package's own (Expr).
            "int" | "pixels" => "int | str | Expr".into(),
            "number" | "length" | "angle" => "float | str | Expr".into(),
            "any" => "Any".into(),
            "text" => "str".into(),
            "path" => "str | os.PathLike[str]".into(),
            "layers" => "list[int]".into(),
            "points" => "list[tuple[float, float]]".into(),
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
            let pixels = match p["kind"].as_str() {
                Some("length" | "pixels") => " pixels",
                Some("angle") => " degrees",
                _ => "",
            };
            text += &format!(" From {min} to {max}{pixels}.");
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
             import contextlib\nimport os\nfrom pathlib import Path\nfrom typing import Any, Literal\n\n\
             from opal.expr import Expr as Expr, box as box, canvas as canvas, cm as cm, deg as deg, fn as fn, \
             ft as ft, inch as inch, layer as layer, m as m, mm as mm, pc as pc, pi as pi, ppi as ppi, pt as pt, \
             px as px, rad as rad, yd as yd\n\n\
             PROTOCOL: int\n\n\
             class OpalError(Exception):\n    code: int | None\n    def __init__(self, message: str, code: int | None = None) -> None: ...\n\n\
             def connection_file() -> Path: ...\n\
             def editor_program() -> str | None: ...\n\
             def connect(file: str | os.PathLike[str] | None = None, *, script: str | None = ..., \
             windowless: bool | None = None) -> Editor: ...\n\n\
             class Editor:\n    version: str | None\n\
             \x20   def __init__(self, pipe: str, token: str, script: str | None = None) -> None: ...\n\
             \x20   def call(self, method: str, **params: Any) -> Any: ...\n\
             \x20   def methods(self) -> list[dict[str, Any]]: ...\n\
             \x20   def step(self, name: str) -> contextlib.AbstractContextManager[Editor]: ...\n\
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
             becomes one history state, `Script: <name>`. Without it, each call is a state of its own.\n\n\
             ## Expressions\n\n\
             Every number, length, angle and pixel count a method takes may be an expression instead, as\n\
             text (`\"Logo.right + 5mm\"`), worked out by the editor when the call runs, against the image as\n\
             it is then: the language its fields take (see `design.md`, *Expressions*). It has numbers in\n\
             any unit, mixed (`2in + 3mm`; a bare number added to a length is in the call's `unit`, one\n\
             multiplying it a plain factor); `+ - * / ^` and parentheses; `min`, `max`, `abs`, `round`,\n\
             `floor`, `ceil` (to whole pixels, or a step: `round(x, 1mm)`), `sqrt`, `sin`, `cos`, `tan`,\n\
             `asin`, `acos`, `atan`, `atan2`, `pi`; the canvas and layers, by name or `layer(id)`, as bounds\n\
             (`left`, `right`, `top`, `bottom`, `width`, `height`, `x`, `y`, and the points `top_left`,\n\
             `top_right`, `bottom_left`, `bottom_right`, `center`, each with `.x` and `.y`), positions from\n\
             what the call counts from (the canvas's top-left corner, unless `from_layer` and `from_anchor`\n\
             say); points (`mid`, `lerp`, `dist`, `point(x, y)`); the image's `ppi`; and for a call that\n\
             places a box, the box as it was (`x`, `y`, `width`, `height`, `angle`): `transform(x=\"x + 1cm\")`\n\
             moves the selected layers 1 cm right. What cannot be worked out is refused, saying where. The\n\
             Python package builds the same text with operators: `opal.layer(logo).right + opal.mm(5)`,\n\
             `opal.box.width / 2`, `opal.fn.mid(a, b)` (`opal.expr`).\n",
        );
        for (role, heading) in
            [(Role::Command, "Commands"), (Role::Query, "Queries"), (Role::Connection, "The connection")]
        {
            out += &format!("\n## {heading}\n");
            for m in describe().as_array().unwrap().iter().filter(|m| m["kind"] == json!(role_name(role))) {
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
    fn selections_are_made_inverted_cleared_and_told_by_calls() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 300, "height": 200 })).unwrap();
        let selection = |st: &mut AppState| now(st, "selection", json!({})).unwrap();
        assert_eq!(selection(&mut st), json!({ "bounds": null, "steps": [] }));
        let refused = now(&mut st, "clear", json!({})).unwrap_err();
        assert!(refused.contains("Select an area first"), "{refused}");
        // Selecting answers with what is selected, as the query does.
        let s = now(&mut st, "select_all", json!({})).unwrap();
        assert_eq!(s, selection(&mut st));
        assert_eq!(s["bounds"], json!([0, 0, 300, 200]));
        assert_eq!(s["steps"][0]["step"], json!("shape"));
        assert_eq!(s["steps"][0]["shape"]["kind"], json!("rectangle"));
        now(&mut st, "invert_selection", json!({})).unwrap();
        assert_eq!(selection(&mut st)["bounds"], json!("unbounded"));
        assert_eq!(selection(&mut st)["steps"][1]["step"], json!("invert"));
        // Inverted back, then cleared: the background is gone within the canvas.
        now(&mut st, "invert_selection", json!({})).unwrap();
        now(&mut st, "clear", json!({})).unwrap();
        let d = st.active().unwrap();
        let flat = opal_core::composite::composite_within(&d.doc().layers, &d.doc().canvas.rect());
        assert!(flat.content_bounds().is_none(), "the canvas was not cleared");
        now(&mut st, "deselect", json!({})).unwrap();
        assert_eq!(selection(&mut st)["bounds"], Value::Null);
        // Positions count from the canvas's corner, wherever it is: cropped to a selection,
        // the selection is the canvas, at (0, 0).
        let shape = opal_core::mask::Shape {
            kind: opal_core::mask::ShapeKind::Rectangle,
            center: [60.0, 45.0],
            size: [100.0, 50.0],
            angle: 0.0,
            smooth: true,
        };
        let step = opal_core::selection::Step::Shape { shape, combine: opal_core::mask::Combine::Replace };
        let s = opal_core::selection::Selection::new(step);
        st.active_mut().unwrap().set_pixel_selection(Some(s), "Select");
        assert_eq!(selection(&mut st)["bounds"], json!([10, 20, 100, 50]));
        now(&mut st, "crop_to_selection", json!({})).unwrap();
        let s = selection(&mut st);
        assert_eq!(s["bounds"], json!([0, 0, 100, 50]));
        assert_eq!(s["steps"][0]["shape"]["center"], json!([50.0, 25.0]));
    }

    #[test]
    fn shapes_are_selected_by_their_box_numbers() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 300, "height": 200 })).unwrap();
        // Left out, the box is the canvas.
        let s = now(&mut st, "select_rectangle", json!({})).unwrap();
        assert_eq!(s["bounds"], json!([0, 0, 300, 200]));
        assert_eq!(s["steps"][0]["shape"]["kind"], json!("rectangle"));
        // An ellipse by its middle, added; its numbers expressions.
        let s = now(
            &mut st,
            "select_ellipse",
            json!({ "x": "canvas.width + 50", "y": 100, "width": 40, "height": "20", "anchor": "center", "combine": "add" }),
        )
        .unwrap();
        assert_eq!(s["bounds"], json!([0, 0, 370, 200]), "{s}");
        assert_eq!(s["steps"][1]["shape"]["center"], json!([350.0, 100.0]));
        assert_eq!(s["steps"][1]["combine"], json!("add"));
        // Taken away, sharp, in another unit (72 pixels an inch).
        let s = now(
            &mut st,
            "select_rectangle",
            json!({ "x": 0, "y": 0, "width": "1in", "height": 200, "combine": "subtract", "smooth": false }),
        )
        .unwrap();
        assert_eq!(s["bounds"], json!([72, 0, 298, 200]));
        assert_eq!(s["steps"][2]["shape"]["smooth"], json!(false));
        // Turned, about its anchor.
        let s = now(&mut st, "select_rectangle", json!({ "x": 10, "y": 10, "width": 100, "height": 1, "angle": 90 }))
            .unwrap();
        assert_eq!(s["bounds"], json!([9, 10, 1, 100]), "{s}");
        // A box made of nothing is refused.
        assert!(now(&mut st, "select_ellipse", json!({ "width": 0 })).is_err());
    }

    #[test]
    fn selections_are_grown_shrunk_and_feathered_by_calls() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 300, "height": 200 })).unwrap();
        // Nothing selected: refused, saying why.
        assert!(now(&mut st, "grow_selection", json!({ "by": 5 })).unwrap_err().contains("Nothing is selected"));
        now(&mut st, "select_rectangle", json!({ "x": 100, "y": 50, "width": 100, "height": 100, "smooth": false }))
            .unwrap();
        let s = now(&mut st, "grow_selection", json!({ "by": "10px", "corners": "square" })).unwrap();
        assert_eq!(s["bounds"], json!([90, 40, 120, 120]));
        assert_eq!(s["steps"][1], json!({ "step": "grow", "by": 10.0, "corners": "square" }));
        let s = now(&mut st, "grow_selection", json!({ "by": -20 })).unwrap();
        assert_eq!(s["bounds"], json!([110, 60, 80, 80]));
        let s =
            now(&mut st, "feather_selection", json!({ "width": 8, "side": "outward", "falloff": "linear" })).unwrap();
        // Falling off outward over 8: pixel 102, whose middle is 7.5 out of the side at 110,
        // still a little selected; 101 not.
        assert_eq!(s["bounds"], json!([102, 52, 96, 96]), "{s}");
        assert_eq!(s["steps"][3]["step"], json!("feather"));
        let label = st.active().unwrap().history.undo_label().map(str::to_owned);
        assert_eq!(label.as_deref(), Some("Feather selection"));
        // Smoothed and bordered: steps of the recipe too; the border a band along the outline,
        // here inward, 10 pixels wide.
        now(&mut st, "select_rectangle", json!({ "x": 100, "y": 50, "width": 100, "height": 100, "smooth": false }))
            .unwrap();
        let s = now(&mut st, "smooth_selection", json!({ "radius": 3 })).unwrap();
        assert_eq!(s["steps"][1], json!({ "step": "smooth", "radius": 3.0 }));
        let s = now(&mut st, "border_selection", json!({ "width": 10, "side": "inward" })).unwrap();
        assert_eq!(s["steps"][2], json!({ "step": "border", "width": 10.0, "side": "inward" }));
        let d = st.active().unwrap();
        let level = |x, y| d.pixel_selection().unwrap().mask().level_at(x, y);
        assert_eq!([level(105, 100), level(150, 100), level(95, 100)], [opal_core::mask::FULL, 0, 0]);
        assert_eq!(d.history.undo_label(), Some("Border selection"));
        // Not memory enough: refused, saying why, the selection as it was.
        let states = st.active().unwrap().history.nodes().count();
        let _pretend = crate::memory::testing::pretend_free(Some(1000));
        let why = now(&mut st, "grow_selection", json!({ "by": 5 })).unwrap_err();
        assert!(why.starts_with("grow_selection: This would need about"), "{why}");
        assert_eq!(st.active().unwrap().history.nodes().count(), states);
    }

    #[test]
    fn a_call_that_begins_an_edit_is_answered_once_it_is_applied() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 40, "height": 20 })).unwrap();
        now(&mut st, "new_layer", json!({})).unwrap();
        st.edit_wait = std::time::Duration::ZERO;
        // The edits' threads held, so that the merge is still under way when answered.
        let held = crate::background::testing::hold(Some(crate::edits::pool()));
        let Reply::Later(n) = st.call("merge_down", &args(json!({})), &egui::Context::default()) else {
            panic!("answered before the merge was done")
        };
        assert!(st.scripts_wait(false), "the next call would not wait for it");
        // Another image made active meanwhile: the answer is still about the merged one.
        let merged = st.active_doc.unwrap();
        st.create_image(&crate::state::NewImage {
            width: 10,
            height: 10,
            background: crate::state::Background::White,
            resolution: 72.0,
        });
        // (Its selected layer's id another than the merged image's: ids are each image's own.)
        for _ in 0..2 {
            now(&mut st, "new_layer", json!({})).unwrap();
        }
        drop(held);
        let layers = later(&mut st, n).unwrap();
        let d = st.doc(merged).unwrap();
        let ids: Vec<u64> = d.selected_layers().iter().map(|l| l.0).collect();
        assert_eq!(layers["layers"], json!(ids), "answered about the image active then");
        assert_eq!(d.history.undo_label(), Some("Merge down"));
    }

    #[test]
    fn a_script_transforms_the_selection_by_its_box_numbers() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 300, "height": 200 })).unwrap();
        let rect = json!({ "x": 10, "y": 20, "width": 40, "height": 30, "smooth": false });
        now(&mut st, "select_rectangle", rect).unwrap();
        let s = now(&mut st, "transform_selection", json!({ "x": 100, "y": 50 })).unwrap();
        assert_eq!(s["bounds"], json!([100, 50, 40, 30]), "{s}");
        // A quarter turn about its middle: 30 wide and 40 tall about the same middle.
        let s = now(&mut st, "transform_selection", json!({ "angle": 90, "anchor": "center" })).unwrap();
        assert_eq!(s["bounds"], json!([105, 45, 30, 40]), "{s}");
        let d = st.active().unwrap();
        assert_eq!(d.history.undo_label(), Some("Transform selection"));
        assert!(d.transform.is_none(), "the box was left open");
        // Inverted: refused, saying why.
        now(&mut st, "invert_selection", json!({})).unwrap();
        let why = now(&mut st, "transform_selection", json!({ "x": 0 })).unwrap_err();
        assert!(why.contains("no box"), "{why}");
    }

    #[test]
    fn what_reselect_brings_back_turns_and_resizes_with_the_image() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 40, "height": 20 })).unwrap();
        let rect = json!({ "x": 0, "y": 0, "width": 10, "height": 5, "smooth": false });
        now(&mut st, "select_rectangle", rect).unwrap();
        now(&mut st, "deselect", json!({})).unwrap();
        let kept = st.active().unwrap().doc().deselected.clone().unwrap().selection;
        now(&mut st, "rotate_cw", json!({})).unwrap();
        now(&mut st, "resize_image", json!({ "width": 40, "height": 80 })).unwrap();
        // Not worked out as the image turned and grew: kept as it was, with how.
        let d = st.active().unwrap().doc().deselected.clone().unwrap();
        assert!(Arc::ptr_eq(&d.selection, &kept), "carried as the image changed");
        assert_ne!(d.since, opal_core::geom::Affine::IDENTITY);
        // A quarter turn clockwise: the top-left corner's 10 x 5 is the top-right's 5 x 10 of
        // the 20 x 40 canvas; twice the size, 10 x 20.
        let s = now(&mut st, "reselect", json!({})).unwrap();
        assert_eq!(s["bounds"], json!([30, 0, 10, 20]), "{s}");
    }

    #[test]
    fn a_script_saves_lists_loads_and_deletes_selections() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 40, "height": 20 })).unwrap();
        let rect = json!({ "x": 1, "y": 1, "width": 10, "height": 5, "smooth": false });
        now(&mut st, "select_rectangle", rect).unwrap();
        now(&mut st, "grow_selection", json!({ "by": 1, "corners": "square" })).unwrap();
        let before = st.active().unwrap().doc().clone();
        let listed = now(&mut st, "save_selection", json!({ "name": "Corner" })).unwrap();
        assert_eq!(listed[0]["name"], json!("Corner"));
        assert_eq!(listed[0]["bounds"], json!([0, 0, 12, 7]), "{listed}");
        assert!(!st.active().unwrap().doc().saves_as(&before), "a saved selection is no change to save");
        // Saved again under the same name, whatever its case: replaced.
        now(&mut st, "select_rectangle", json!({ "x": 30, "y": 10, "width": 5, "height": 5 })).unwrap();
        now(&mut st, "save_selection", json!({ "name": "Other" })).unwrap();
        now(&mut st, "deselect", json!({})).unwrap();
        // Loaded in place of nothing: its steps come back.
        let s = now(&mut st, "load_selection", json!({ "name": "corner" })).unwrap();
        assert_eq!((s["bounds"].clone(), s["steps"].as_array().unwrap().len()), (json!([0, 0, 12, 7]), 2), "{s}");
        // Added to it, inverted: everything but the other, so the rectangle round it is unbounded.
        let s = now(&mut st, "load_selection", json!({ "name": "Other", "combine": "add", "invert": true })).unwrap();
        assert_eq!(s["bounds"], json!("unbounded"), "{s}");
        assert_eq!(s["steps"][2]["of"], json!("saved"), "{s}");
        // The image turned a quarter: loaded, carried so; listed, said so.
        now(&mut st, "deselect", json!({})).unwrap();
        now(&mut st, "rotate_cw", json!({})).unwrap();
        let listed = now(&mut st, "saved_selections", json!({})).unwrap();
        assert_eq!(
            (listed[0]["changed_since"].clone(), listed[0]["bounds"].clone()),
            (json!(true), json!([13, 0, 7, 12])),
            "{listed}"
        );
        let s = now(&mut st, "load_selection", json!({ "name": "Corner" })).unwrap();
        assert_eq!(s["bounds"], json!([13, 0, 7, 12]), "{s}");
        // Refused: a name not saved, an empty one; deleted, gone.
        let why = now(&mut st, "load_selection", json!({ "name": "Nowhere" })).unwrap_err();
        assert!(why.contains("No selection is saved as \"Nowhere\""), "{why}");
        let why = now(&mut st, "save_selection", json!({ "name": "  " })).unwrap_err();
        assert!(why.contains("not blank"), "{why}");
        let left = now(&mut st, "delete_saved_selection", json!({ "name": "CORNER" })).unwrap();
        assert_eq!(left.as_array().unwrap().len(), 1);
        assert_eq!(left[0]["name"], json!("Other"));
    }

    #[test]
    fn a_script_selects_what_an_outline_encloses() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 300, "height": 200 })).unwrap();
        let triangle = json!({ "points": [[10, 10], [110, 10], [10, 90]] });
        let s = now(&mut st, "select_outline", triangle).unwrap();
        assert_eq!(s["bounds"], json!([10, 10, 100, 80]), "{s}");
        assert_eq!(s["steps"][0]["step"], json!("outline"));
        assert_eq!(s["steps"][0]["outline"]["start"], json!([10.0, 10.0]));
        // Added: the two together; and fewer than three points refused.
        let more = json!({ "points": [[200, 100], [250, 100], [250, 150], [200, 150]], "combine": "add" });
        let s = now(&mut st, "select_outline", more).unwrap();
        assert_eq!(s["bounds"], json!([10, 10, 240, 140]), "{s}");
        let why = now(&mut st, "select_outline", json!({ "points": [[0, 0], [5, 5]] })).unwrap_err();
        assert!(why.contains("a list of points"), "{why}");
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Outline selection"));
    }

    #[test]
    fn a_script_changes_and_takes_away_the_selection_s_steps() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 300, "height": 200 })).unwrap();
        let rect = json!({ "x": 10, "y": 10, "width": 40, "height": 30, "smooth": false });
        now(&mut st, "select_rectangle", rect).unwrap();
        now(&mut st, "grow_selection", json!({ "by": 5, "corners": "square" })).unwrap();
        let s = now(&mut st, "edit_selection_step", json!({ "index": 1, "by": 10 })).unwrap();
        assert_eq!((s["bounds"].clone(), s["steps"][1]["by"].clone()), (json!([0, 0, 60, 50]), json!(10.0)), "{s}");
        // A grow takes no radius; there is no step 5.
        let wrong = now(&mut st, "edit_selection_step", json!({ "index": 1, "radius": 3 })).unwrap_err();
        assert!(wrong.contains("A grow takes by and corners, not radius"), "{wrong}");
        let missing = now(&mut st, "remove_selection_step", json!({ "index": 5 })).unwrap_err();
        assert!(missing.contains("there is no step 5"), "{missing}");
        // The first rectangle subtracted from nothing: nothing selected.
        let s = now(&mut st, "edit_selection_step", json!({ "index": 0, "combine": "subtract" })).unwrap();
        assert_eq!(s["bounds"], Value::Null, "{s}");
        now(&mut st, "undo", json!({})).unwrap();
        let s = now(&mut st, "remove_selection_step", json!({ "index": 1 })).unwrap();
        assert_eq!(s["bounds"], json!([10, 10, 40, 30]), "{s}");
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Remove selection step"));
    }

    #[test]
    fn a_notification_whose_edit_fails_once_under_way_tells_the_user() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 40, "height": 20 })).unwrap();
        now(&mut st, "select_rectangle", json!({ "x": 5, "y": 5, "width": 10, "height": 10 })).unwrap();
        st.edit_wait = std::time::Duration::ZERO;
        let held = crate::background::testing::hold(Some(crate::edits::pool()));
        let _pretend = crate::memory::testing::pretend_free(Some(1000));
        let reply = st.call_from(&[], false, "grow_selection", &args(json!({ "by": 5 })), &egui::Context::default());
        assert!(matches!(reply, Reply::Later(_)), "answered before the grow was done");
        drop(held);
        let mut waited = 0;
        while st.editing() {
            assert!(waited < 1000, "the grow never ended");
            st.poll_edits();
            std::thread::sleep(std::time::Duration::from_millis(5));
            waited += 1;
        }
        // Told as a refusal, as it would be at once.
        let said = st.messages.back().map(|m| (m.kind, m.title.clone(), m.body.clone()));
        let refused = |(k, t, b): &(MessageKind, String, String)| {
            *k == MessageKind::Info && t == "grow_selection" && b.contains("would need")
        };
        assert!(said.as_ref().is_some_and(refused), "{said:?}");
    }

    #[test]
    fn a_call_whose_edit_fails_once_under_way_puts_the_view_back() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 40, "height": 20 })).unwrap();
        let first = st.active_doc.unwrap();
        now(&mut st, "select_rectangle", json!({ "x": 5, "y": 5, "width": 10, "height": 10 })).unwrap();
        now(&mut st, "new", json!({ "width": 30, "height": 30 })).unwrap();
        let second = st.active_doc.unwrap();
        st.edit_wait = std::time::Duration::ZERO;
        // Under way when answered, and refused memory once it runs.
        let held = crate::background::testing::hold(Some(crate::edits::pool()));
        let _pretend = crate::memory::testing::pretend_free(Some(1000));
        let call = args(json!({ "document": first.0, "by": 5 }));
        let Reply::Later(n) = st.call("grow_selection", &call, &egui::Context::default()) else {
            panic!("answered before the grow was done")
        };
        assert_eq!(st.active_doc, Some(first), "the call did not make its image active");
        drop(held);
        let why = later(&mut st, n).unwrap_err();
        assert!(why.starts_with("grow_selection: This would need about"), "{why}");
        assert_eq!(st.active_doc, Some(second), "the view was not put back");
    }

    #[test]
    fn a_cut_erases_what_it_copied_and_nothing_once_the_copy_fails() {
        use crate::clipboard::TestClipboard;
        use opal_core::document::{LayerContent, ObjectKind};
        use opal_core::mask::{Combine, Shape, ShapeKind};
        use opal_core::selection::{Selection, Step};
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        st.clipboard = Box::new(TestClipboard::default());
        // A white background reaching 100 pixels past the canvas's right edge.
        now(&mut st, "new", json!({ "width": 400, "height": 200 })).unwrap();
        let narrow = |doc: &mut opal_core::document::Document, _: &mut _| {
            doc.canvas.width = 300;
            Ok::<(), ()>(())
        };
        st.active_mut().unwrap().apply("Narrow the canvas", narrow).unwrap();
        let select = |st: &mut AppState, [x0, y0, x1, y1]: [f64; 4]| {
            let shape = Shape {
                kind: ShapeKind::Rectangle,
                center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
                size: [x1 - x0, y1 - y0],
                angle: 0.0,
                smooth: false,
            };
            let s = Selection::new(Step::Shape { shape, combine: Combine::Replace });
            st.active_mut().unwrap().set_pixel_selection(Some(s), "Select");
        };
        let alpha = |st: &AppState, x: i64, y: i64| {
            let LayerContent::Objects(o) = &st.active().unwrap().doc().layers[0].content else { panic!("a group") };
            let ObjectKind::Pixels(p) = &o[0].kind else { panic!("not pixels") };
            opal_core::tile::pixel_to_f32(p.image.pixel(x - p.offset[0], y - p.offset[1]))[3]
        };
        let cut = |st: &mut AppState| match st.call("cut", &args(json!({})), &egui::Context::default()) {
            Reply::Later(n) => later(st, n),
            Reply::Now(r) => panic!("answered at once: {r:?}"),
        };
        // Selected across the canvas's edge: copied, and erased, only within the canvas.
        select(&mut st, [250.0, 50.0, 400.0, 150.0]);
        assert_eq!(cut(&mut st).unwrap(), json!({ "width": 50, "height": 100 }));
        assert_eq!([alpha(&st, 275, 100), alpha(&st, 350, 100)], [0.0, 1.0], "erased what it did not copy");
        // A copy the clipboard refuses: the cut is taken back, and cannot be redone.
        st.clipboard =
            Box::new(TestClipboard { refuses: Some("The clipboard is in use.".into()), ..Default::default() });
        select(&mut st, [0.0, 0.0, 100.0, 100.0]);
        let said = cut(&mut st).unwrap_err();
        assert!(said.contains("in use, so nothing was cut"), "{said}");
        assert_eq!(alpha(&st, 50, 50), 1.0, "erased what was not copied");
        let d = st.active().unwrap();
        assert_eq!((d.history.undo_label(), d.history.can_redo()), (Some("Select"), false));
        // Something done since, it stays (in history), and the user is told how to get it back.
        let n = match st.call("cut", &args(json!({})), &egui::Context::default()) {
            Reply::Later(n) => n,
            Reply::Now(r) => panic!("answered at once: {r:?}"),
        };
        now(&mut st, "deselect", json!({})).unwrap();
        let said = later(&mut st, n).unwrap_err();
        assert!(said.contains("undo it to get it back"), "{said}");
        assert_eq!(alpha(&st, 50, 50), 0.0);
        // Undone before the copy fails: nothing is cut, and no redo cuts it without the copy.
        select(&mut st, [100.0, 100.0, 150.0, 150.0]);
        let n = match st.call("cut", &args(json!({})), &egui::Context::default()) {
            Reply::Later(n) => n,
            Reply::Now(r) => panic!("answered at once: {r:?}"),
        };
        st.active_mut().unwrap().undo();
        let said = later(&mut st, n).unwrap_err();
        assert!(said.contains("so nothing was cut"), "{said}");
        let d = st.active().unwrap();
        assert!(!d.history.can_redo(), "the cut is still there to redo: {:?}", d.history.redo_label());
        // Its image closed meanwhile: nothing is said of the cut.
        let n = match st.call("cut", &args(json!({})), &egui::Context::default()) {
            Reply::Later(n) => n,
            Reply::Now(r) => panic!("answered at once: {r:?}"),
        };
        now(&mut st, "close", json!({ "discard": true })).unwrap();
        assert_eq!(later(&mut st, n).unwrap_err(), "Could not copy: The clipboard is in use.");
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
        // Lengths are checked once in pixels, at the image's resolution.
        let worked = |a: Args, ppi| a.worked_out(p, &Scope::bare(Unit::Px, ppi), &[], |_| true);
        let err = |v| Args::check(p, &args(v)).and_then(|a| worked(a, 72.0)).unwrap_err();
        assert!(err(json!({ "width": 10 })).contains("\"height\" must be given"));
        assert!(err(json!({ "width": 0, "height": 5 })).contains("between 1 and"));
        // An expression that names nothing there is, or does not read, is told where.
        let ten = err(json!({ "width": "ten", "height": 5 }));
        assert!(ten.contains("\"width\": “ten” is not a word the language has") && ten.contains("at \"ten\""), "{ten}");
        let furlongs = err(json!({ "width": "5 furlongs", "height": 5 }));
        assert!(furlongs.contains("An operator") && furlongs.contains("at \"furlongs\""), "{furlongs}");
        let sum = err(json!({ "width": "2in * 1in", "height": 5 }));
        assert!(sum.contains("an area, not a length"), "{sum}");
        let past = err(json!({ "width": "100000in + 1px", "height": 5 }));
        assert!(past.contains("not 7200001, what \"100000in + 1px\" comes to"), "{past}");
        let far = err(json!({ "width": "400 ft", "height": 5 }));
        assert!(far.contains("400 ft, which is 345600 pixels at 72 pixels per inch"), "{far}");
        let px = |v| Args::check(p, &args(v)).and_then(|a| worked(a, 300.0)).unwrap();
        let sized = px(json!({ "width": "8.5in", "height": 11, "unit": "in" }));
        assert_eq!((sized.side("width"), sized.side("height")), (Some(2550), Some(3300)));
        let mixed = px(json!({ "width": "25.4 mm", "height": "1 pc", "unit": "in" }));
        assert_eq!((mixed.side("width"), mixed.side("height")), (Some(300), Some(50)));
        // Arithmetic, a bare number in the call's unit.
        let sum = px(json!({ "width": "1 + 1in / 2", "height": "2cm * 3", "unit": "in" }));
        assert_eq!((sum.side("width"), sum.side("height")), (Some(450), Some(709)));
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
        let given = crate::shared_memory::new_name().unwrap();
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
    fn the_history_is_told_as_a_tree() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 8, "height": 8 })).unwrap();
        now(&mut st, "new_layer", json!({ "name": "A" })).unwrap();
        now(&mut st, "undo", json!({})).unwrap();
        now(&mut st, "new_layer", json!({ "name": "B" })).unwrap();
        let h = now(&mut st, "history", json!({})).unwrap();
        let states = h["states"].as_array().unwrap();
        let labels: Vec<&str> = states.iter().map(|s| s["label"].as_str().unwrap()).collect();
        assert_eq!(labels, ["New image", "New layer", "New layer"]);
        // The first has the other two after it, a branch; the image is in the last.
        let (root, a, b) = (&states[0], &states[1], &states[2]);
        assert_eq!(root["children"], json!([a["state"], b["state"]]));
        assert_eq!((a["parent"].clone(), root["redo"].clone()), (root["state"].clone(), b["state"].clone()));
        assert_eq!(h["current"], b["state"]);
        // New, it is as saved where it began.
        assert_eq!(h["saved"], root["state"]);
        assert!(root["time"].as_f64().unwrap() > 1.7e9 && h["dropped"] == json!(0));
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
        st.call_from(&[], false, "open", &given, &egui::Context::default());
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
        assert_eq!(canvas, json!({ "width": 100, "height": 32, "resolution": 72.0 }));
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

    /// Waits for the answer to call `n`, polling the copies, writes and pastes (not all that is
    /// done between frames, which would give the tiles of every test in this process a
    /// budget).
    fn later(st: &mut AppState, n: u64) -> Result<Value, String> {
        for _ in 0..1000 {
            st.poll_edits();
            st.poll_copies();
            st.poll_writes(false);
            st.poll_paste(false);
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
        assert_eq!(d.selected_layers(), [LayerId(a), LayerId(b)], "the selection changed");
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

    #[test]
    fn sizes_are_given_in_any_unit_at_the_image_s_resolution() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 8.5, "height": "11in", "unit": "in", "resolution": 300 })).unwrap();
        let docs = now(&mut st, "documents", json!({})).unwrap();
        let d = &docs[0];
        assert_eq!(
            (d["width"].as_u64(), d["height"].as_u64(), d["resolution"].as_f64()),
            (Some(2550), Some(3300), Some(300.0))
        );
        // A resolution alone changes how large the pixels print, not them.
        let c = now(&mut st, "resize_image", json!({ "resolution": 150 })).unwrap();
        assert_eq!(c, json!({ "width": 2550, "height": 3300, "resolution": 150.0 }));
        // Lengths are converted at the new resolution; one side keeps the proportions.
        let c = now(&mut st, "resize_image", json!({ "width": "4in", "resolution": 100 })).unwrap();
        assert_eq!(c, json!({ "width": 400, "height": 518, "resolution": 100.0 }));
        // Margins, in any unit, a negative one cutting.
        let c = now(&mut st, "resize_canvas", json!({ "left": "1in", "right": -50, "top": "0.5 in" })).unwrap();
        assert_eq!((c["width"].as_u64(), c["height"].as_u64()), (Some(450), Some(568)));
        assert_eq!(st.active().unwrap().doc().canvas.x, -100, "the left margin went on the left");
        let both = now(&mut st, "resize_canvas", json!({ "left": 1, "anchor": "top" })).unwrap_err();
        assert!(both.contains("not both"), "{both}");
        let far = now(&mut st, "resize_image", json!({ "width": "100 yd" })).unwrap_err();
        assert!(far.contains("100 yd, which is 360000 pixels at 100 pixels per inch"), "{far}");
    }

    /// Where a file called `name` that a test places is written.
    fn place_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("opal-api-place-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A `w × h` PNG of one color, written to a file of its own; its path.
    fn png_file(name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let path = place_path(name);
        image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255])).save(&path).unwrap();
        path
    }

    #[test]
    fn a_file_is_placed_into_a_box_given_in_any_unit_and_fitted_to_it() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 200, "height": 100, "resolution": 100 })).unwrap();
        let path = png_file("strip.png", 40, 20).display().to_string();
        // Fitted inside a 1-inch square, centered on it: plain numbers in the call's unit,
        // the others in their own.
        let given =
            json!({ "path": path, "x": "0.1in", "y": "10px", "width": 1, "height": 1, "unit": "in", "fit": "inside" });
        let placed = now(&mut st, "place", given).unwrap();
        assert_eq!(placed["box"], json!({ "x": 10.0, "y": 35.0, "width": 100.0, "height": 50.0, "angle": 0.0 }));
        let layer = placed["layer"].as_u64().unwrap();
        let d = st.active().unwrap();
        assert!(d.transform.is_none(), "the box was left open");
        assert_eq!(d.history.undo_label(), Some("Place"));
        assert_eq!(now(&mut st, "layers", json!({})).unwrap()[0]["name"], json!("strip"), "named after its file");
        // Covering a rectangle, and then turned, by numbers.
        let cover = json!({ "layers": [layer], "x": 0, "y": 0, "width": 100, "height": 100, "fit": "cover" });
        let t = now(&mut st, "transform", cover).unwrap();
        assert_eq!(t["box"], json!({ "x": -50.0, "y": 0.0, "width": 200.0, "height": 100.0, "angle": 0.0 }));
        let t = now(&mut st, "transform", json!({ "angle": 90 })).unwrap();
        assert_eq!((t["box"]["x"].clone(), t["box"]["angle"].clone()), (json!(-50.0), json!(90.0)));
        assert_eq!(t["layers"], json!([layer]));
        // Turned, it is fitted as turned into the rectangle as it lies on the canvas: a
        // quarter turn makes the 40 × 20 image 20 wide and 40 tall, 7.5 times larger inside
        // 400 × 300, centered; its own top-left corner is then at the right.
        let turned =
            json!({ "layers": [layer], "x": 0, "y": 0, "width": 400, "height": 300, "angle": 90, "fit": "inside" });
        let t = now(&mut st, "transform", turned).unwrap();
        let b = &t["box"];
        let close = |v: &Value, want: f64| (v.as_f64().unwrap() - want).abs() < 1e-6;
        assert!(close(&b["x"], 275.0) && close(&b["y"], 0.0) && close(&b["width"], 300.0), "{t}");
        assert!(close(&b["height"], 150.0) && close(&b["angle"], 90.0), "{t}");
        // Given at its center, a box is placed by its center, and turns about it.
        let centered = json!({ "layers": [layer], "x": 50, "y": 50, "width": 40, "anchor": "center" });
        let t = now(&mut st, "transform", centered).unwrap();
        assert_eq!((t["box"]["x"].clone(), t["box"]["y"].clone()), (json!(50.0), json!(50.0)), "{t}");
        let t = now(&mut st, "transform", json!({ "angle": 45, "anchor": "center" })).unwrap();
        let (x, y) = (t["box"]["x"].as_f64().unwrap(), t["box"]["y"].as_f64().unwrap());
        assert!((x - 50.0).abs() < 1e-9 && (y - 50.0).abs() < 1e-9, "it did not turn about its center: {t}");
        // Into the active layer, if it can take it; else refused, and no box left open.
        now(&mut st, "new_group", json!({})).unwrap();
        let refused = now(&mut st, "place", json!({ "path": path, "into_active": true })).unwrap_err();
        assert!(refused.contains("no plain, unlocked active layer"), "{refused}");
        assert!(st.active().unwrap().transform.is_none(), "the refused box was left open");
        let missing = now(&mut st, "place", json!({ "path": format!("{path}.gone") })).unwrap_err();
        assert!(missing.starts_with("place:"), "{missing}");
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).ok();
    }

    #[test]
    fn a_paste_read_off_the_interface_thread_is_placed_with_no_window() {
        // As --headless and the editor --mcp starts run it: between frames, nothing else, and
        // with a message no one will ever dismiss.
        use crate::clipboard::{Pasted, TestClipboard};
        let mut st = AppState::new();
        st.windowless = true;
        now(&mut st, "new", json!({ "width": 50, "height": 50 })).unwrap();
        st.message(MessageKind::Warning, "Settings not loaded", "the file could not be read");
        let px = vec![opal_core::tile::pixel_from_f32([0.0, 1.0, 0.0, 1.0]); 4];
        let image = Pasted {
            width: 2,
            height: 2,
            image: RasterImage::from_pixels(2, 2, &px),
            from: None,
            resolution: None,
            warnings: Vec::new(),
        };
        st.clipboard = Box::new(TestClipboard { image: Some(image), slow: true, ..Default::default() });
        let Reply::Later(n) = st.call("paste", &args(json!({ "x": 1, "y": 2 })), &egui::Context::default()) else {
            panic!("answered at once")
        };
        for _ in 0..1000 {
            st.between_frames(false);
            if let Some((_, placed)) = st.answers().into_iter().find(|(m, _)| *m == n) {
                assert_eq!(
                    (placed.clone().unwrap()["box"]["x"].clone(), placed.unwrap()["box"]["y"].clone()),
                    (json!(1.0), json!(2.0))
                );
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the paste was never placed");
    }

    #[test]
    fn an_image_opened_has_its_file_s_resolution() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        // A file that does not say has the preference's.
        now(&mut st, "set_preference", json!({ "key": "ui.default_resolution", "value": 96 })).unwrap();
        let plain = std::env::temp_dir().join(format!("opal-api-no-ppi-{}.png", std::process::id()));
        image::RgbaImage::new(4, 4).save(&plain).unwrap();
        now(&mut st, "open", json!({ "path": plain.display().to_string() })).unwrap();
        std::fs::remove_file(&plain).ok();
        assert_eq!(st.active().unwrap().doc().resolution, 96.0);
        now(&mut st, "close", json!({ "discard": true })).unwrap();
        now(&mut st, "new", json!({ "width": 30, "height": 20, "resolution": 240 })).unwrap();
        let path = std::env::temp_dir().join(format!("opal-api-ppi-{}.png", std::process::id()));
        let doc = st.active().unwrap().doc().clone();
        opal_io::image_files::export(
            &doc,
            &path,
            &opal_io::image_files::ExportFormat::Png { sixteen_bit: false },
            None,
        )
        .unwrap();
        now(&mut st, "open", json!({ "path": path.display().to_string() })).unwrap();
        std::fs::remove_file(&path).ok();
        let docs = now(&mut st, "documents", json!({})).unwrap();
        assert_eq!(docs[1]["resolution"], json!(240.0), "{docs}");
        // Pixels that are not square: the resolution across, and a note that says so.
        use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
        let tall = std::env::temp_dir().join(format!("opal-api-tall-{}.jpg", std::process::id()));
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 90);
        encoder.set_pixel_density(PixelDensity { density: (100, 200), unit: PixelDensityUnit::Inches });
        image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8)).write_with_encoder(encoder).unwrap();
        std::fs::write(&tall, jpeg).unwrap();
        let opened = now(&mut st, "open", json!({ "path": tall.display().to_string() })).unwrap();
        std::fs::remove_file(&tall).ok();
        assert!(opened["notes"][0].as_str().is_some_and(|n| n.contains("not square")), "{opened}");
        assert_eq!(now(&mut st, "documents", json!({})).unwrap()[2]["resolution"], json!(100.0));
    }

    #[test]
    fn what_a_script_places_transforms_and_pastes_is_part_of_its_run() {
        use crate::clipboard::{Pasted, TestClipboard};
        let _turn = crate::background::testing::turn();
        let ctx = egui::Context::default();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 100, "height": 100 })).unwrap();
        let path = png_file("run.png", 10, 10).display().to_string();
        let states = st.active().unwrap().history.nodes().count();
        // Each call made in the run's group, as the server makes a script's.
        let call = |st: &mut AppState, name: &str, given: Value| st.call_from(&[7], true, name, &args(given), &ctx);
        let Reply::Now(Ok(placed)) = call(&mut st, "place", json!({ "path": path, "x": 5 })) else {
            panic!("not placed")
        };
        let Reply::Now(Ok(_)) = call(&mut st, "transform", json!({ "layers": [placed["layer"]], "angle": 30 })) else {
            panic!("not transformed")
        };
        // A paste read off the interface thread, placed after its call has returned.
        let px = vec![opal_core::tile::pixel_from_f32([1.0; 4]); 4];
        let image = Pasted {
            width: 2,
            height: 2,
            image: RasterImage::from_pixels(2, 2, &px),
            from: None,
            resolution: None,
            warnings: Vec::new(),
        };
        st.clipboard = Box::new(TestClipboard { image: Some(image), slow: true, ..Default::default() });
        let Reply::Later(n) = call(&mut st, "paste", json!({})) else { panic!("answered at once") };
        later(&mut st, n).unwrap();
        st.group_ended(7, "Script: run", false);
        let d = st.active().unwrap();
        assert_eq!(d.history.undo_label(), Some("Script: run"));
        assert_eq!(d.history.nodes().count(), states + 1, "the run is not one state");
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).ok();
    }

    #[test]
    fn a_box_takes_the_numbers_given_and_keeps_its_proportions_for_the_rest() {
        let p = crate::transform::Params { x: 10.0, y: 20.0, width: 40.0, height: 20.0, angle: 0.0, mirrored: false };
        let corner = [100.0, 200.0];
        let applied = |b: BoxArgs| {
            let q = BoxArgs { from: corner, ..b }.applied(&p);
            (q.x, q.y, q.width, q.height)
        };
        assert_eq!(applied(BoxArgs::default()), (10.0, 20.0, 40.0, 20.0));
        assert_eq!(
            applied(BoxArgs { x: Some(5.0), width: Some(80.0), ..BoxArgs::default() }),
            (105.0, 20.0, 80.0, 40.0)
        );
        assert_eq!(applied(BoxArgs { height: Some(10.0), ..BoxArgs::default() }), (10.0, 20.0, 20.0, 10.0));
        let both = BoxArgs { x: Some(0.0), y: Some(0.0), width: Some(60.0), height: Some(60.0), ..BoxArgs::default() };
        assert_eq!(applied(both), (100.0, 200.0, 60.0, 60.0));
    }

    #[test]
    fn the_image_is_cropped_to_a_rectangle_given_in_any_unit() {
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 200, "height": 100, "resolution": 100 })).unwrap();
        let c = now(&mut st, "crop", json!({ "x": "0.5in", "width": "1in", "height": 50 })).unwrap();
        assert_eq!(c, json!({ "width": 100, "height": 50, "resolution": 100.0 }));
        let d = st.active().unwrap();
        assert_eq!((d.doc().canvas.x, d.doc().canvas.y), (50, 0));
        assert_eq!(d.history.undo_label(), Some("Crop"));
        // The same rectangle again: nothing to do, no state made.
        let before = st.active().unwrap().history.current();
        now(&mut st, "crop", json!({})).unwrap();
        assert_eq!(st.active().unwrap().history.current(), before, "an empty crop made a state");
        // Refused (a locked layer and a turn), the user's image stays the one shown.
        let other = now(&mut st, "new", json!({ "width": 10, "height": 10 })).unwrap()["document"].clone();
        let first = now(&mut st, "documents", json!({})).unwrap()[0]["document"].clone();
        let bg = now(&mut st, "layers", json!({ "document": first })).unwrap()[0]["layer"].clone();
        now(&mut st, "toggle_lock", json!({ "document": first, "layers": [bg], "locked": true })).unwrap();
        now(&mut st, "new_layer", json!({ "document": other })).unwrap();
        let refused = now(&mut st, "crop", json!({ "document": first, "angle": 10 })).unwrap_err();
        assert!(refused.contains("locked"), "{refused}");
        assert_eq!(st.active_doc.map(|d| d.0), other.as_u64(), "a refused crop left its image shown");
        now(&mut st, "toggle_lock", json!({ "document": first, "layers": [bg], "locked": false })).unwrap();
        let turned = now(
            &mut st,
            "crop",
            json!({ "document": first, "x": 10, "y": 10, "width": 20, "height": 20, "angle": 30 }),
        )
        .unwrap();
        assert_eq!((turned["width"].as_u64(), turned["height"].as_u64()), (Some(20), Some(20)));
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Crop and straighten"));
        // By its center, with no place given: the canvas's center, so a centered crop.
        now(&mut st, "new", json!({ "width": 200, "height": 100 })).unwrap();
        now(&mut st, "crop", json!({ "width": 100, "height": 50, "anchor": "center" })).unwrap();
        let c = st.active().unwrap().doc().canvas;
        assert_eq!((c.x, c.y, c.width, c.height), (50, 25, 100, 50));
    }

    #[test]
    fn preferences_are_told_and_changed_but_not_those_of_scripting() {
        use crate::settings::Value as Set;
        let mut st = AppState::new();
        let prefs = now(&mut st, "preferences", json!({})).unwrap();
        let find = |key: &str| prefs.as_array().unwrap().iter().find(|p| p["key"] == key).unwrap().clone();
        let unit = find("ui.unit");
        assert_eq!((unit["value"].clone(), unit["kind"].clone()), (json!("px"), json!("choice")));
        assert!(unit["choices"].as_array().unwrap().contains(&json!("mm")));
        assert_eq!(find("scripting.python")["settable"], json!(false));
        assert_eq!(find("export.jpeg_quality")["max"], json!(100.0));
        now(&mut st, "set_preference", json!({ "key": "ui.unit", "value": "mm" })).unwrap();
        assert_eq!(st.unit(), Unit::Mm);
        // Those egui keeps take effect at once, as from the Preferences window.
        let ctx = egui::Context::default();
        let scaled = st.call("set_preference", &args(json!({ "key": "ui.scale", "value": 150 })), &ctx);
        assert!(matches!(scaled, Reply::Now(Ok(_))), "{scaled:?}");
        // (egui takes a new zoom at its next frame.)
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        assert_eq!(ctx.zoom_factor(), 1.5, "the interface size was not applied");
        now(&mut st, "set_preference", json!({ "key": "export.jpeg_quality", "value": 77 })).unwrap();
        assert_eq!(st.prefs.f64("export.jpeg_quality"), 77.0);
        now(&mut st, "set_preference", json!({ "key": "canvas.checker_a", "value": [10, 20, 30] })).unwrap();
        assert_eq!(st.prefs.get("canvas.checker_a"), Set::Color([10, 20, 30, 255]));
        let set = |st: &mut AppState, key: &str, value: Value| {
            now(st, "set_preference", json!({ "key": key, "value": value })).unwrap_err()
        };
        assert!(set(&mut st, "export.jpeg_quality", json!(500)).contains("a number from 1 to 100"));
        assert!(set(&mut st, "ui.unit", json!("furlongs")).contains("one of \"px\""));
        assert!(set(&mut st, "ui.tooltips", json!("yes")).contains("true or false"));
        assert!(set(&mut st, "canvas.checker_a", json!([1, 2, 300])).contains("0 to 255"));
        // Left out, a value goes back to its default.
        now(&mut st, "set_preference", json!({ "key": "ui.unit" })).unwrap();
        assert_eq!(st.unit(), Unit::Px);
        // What runs scripts, and who may connect, no script or assistant may change.
        let refused = set(&mut st, "scripting.python", json!("calc.exe"));
        assert!(refused.contains("Preferences window only"), "{refused}");
        assert_ne!(st.prefs.text("scripting.python"), "calc.exe");
        assert!(set(&mut st, "scripting.mcp_http", json!(true)).contains("Preferences window only"));
        let none = now(&mut st, "set_preference", json!({ "key": "no.such" })).unwrap_err();
        assert!(none.contains("no preference"), "{none}");
    }

    #[test]
    fn a_paste_is_placed_where_the_call_says_and_applied() {
        use crate::clipboard::{Pasted, TestClipboard};
        let ctx = egui::Context::default();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 100, "height": 100 })).unwrap();
        let image = || {
            let px = vec![opal_core::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]); 200];
            Pasted {
                width: 20,
                height: 10,
                image: RasterImage::from_pixels(20, 10, &px),
                from: None,
                resolution: None,
                warnings: Vec::new(),
            }
        };
        let clipboard = |slow| Box::new(TestClipboard { image: Some(image()), slow, ..Default::default() });
        let call = |st: &mut AppState, name: &str, given: Value| match st.call(name, &args(given), &ctx) {
            Reply::Later(n) => n,
            Reply::Now(r) => panic!("{name} answered at once: {r:?}"),
        };
        st.clipboard = clipboard(false);
        let n = call(&mut st, "paste", json!({ "x": 5, "y": 6, "width": 40 }));
        let placed = later(&mut st, n).unwrap();
        assert_eq!(placed["box"], json!({ "x": 5.0, "y": 6.0, "width": 40.0, "height": 20.0, "angle": 0.0 }));
        assert!(st.active().unwrap().transform.is_none(), "the box was left open");
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Paste"));
        // Read off the interface thread, it is placed once read; asked for again before
        // then, the first is told it was replaced.
        st.clipboard = clipboard(true);
        let first = call(&mut st, "paste_in_place", json!({}));
        let second = call(&mut st, "paste_in_place", json!({ "angle": 45 }));
        assert!(later(&mut st, first).unwrap_err().contains("Another paste"));
        assert_eq!(later(&mut st, second).unwrap()["box"]["angle"], json!(45.0));
        // Nothing on the clipboard: the call is told so, no dialog is left, and the image
        // the user had stays shown, though the paste named another.
        let first = st.active_doc.unwrap().0;
        let second = now(&mut st, "new", json!({ "width": 10, "height": 10 })).unwrap()["document"].as_u64();
        st.clipboard = Box::new(TestClipboard::default());
        let empty = call(&mut st, "paste", json!({ "document": first }));
        assert!(later(&mut st, empty).unwrap_err().contains("holds no image"));
        assert!(st.messages.is_empty(), "{:?}", st.messages);
        assert_eq!(st.active_doc.map(|d| d.0), second, "a refused paste left its image shown");
        now(&mut st, "close", json!({ "discard": true })).unwrap();
        // With no image open, a paste is a new image.
        now(&mut st, "close", json!({ "discard": true })).unwrap();
        st.clipboard = clipboard(false);
        let anew = call(&mut st, "paste", json!({}));
        let made = later(&mut st, anew).unwrap();
        assert_eq!(made["document"].as_u64(), st.active_doc.map(|d| d.0));
        assert_eq!(st.active().unwrap().doc().canvas.width, 20);
        // A copy from an image at 300 ppi, pasted as a new image, is at 300 ppi too.
        now(&mut st, "resize_image", json!({ "resolution": 300 })).unwrap();
        let copied = call(&mut st, "copy_merged", json!({}));
        later(&mut st, copied).unwrap();
        now(&mut st, "close", json!({ "discard": true })).unwrap();
        let anew = call(&mut st, "paste", json!({}));
        later(&mut st, anew).unwrap();
        assert_eq!(st.active().unwrap().doc().resolution, 300.0, "the copy's resolution was lost");
    }

    /// A white JPEG `w × h` pixels at `ppi` pixels per inch across and down (none recorded
    /// if `None`), written to a file of the test's own; its path.
    fn jpeg_at(name: &str, w: u32, h: u32, ppi: Option<[u16; 2]>) -> String {
        let path = place_path(name);
        crate::paste::testing::white_jpeg(&path, w, h, ppi);
        path.display().to_string()
    }

    /// The box of what a call answered: its position, width and height.
    fn box_of(v: &Value) -> [f64; 4] {
        ["x", "y", "width", "height"].map(|k| v["box"][k].as_f64().unwrap())
    }

    #[test]
    fn a_file_is_placed_at_its_print_size_unless_asked_otherwise() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 400, "height": 300, "resolution": 100 })).unwrap();
        // 90 pixels at 300 ppi are 0.3 in: 30 pixels at 100 ppi.
        let logo = jpeg_at("logo.jpg", 90, 60, Some([300, 300]));
        let width = |v: Value| box_of(&v)[2];
        assert_eq!(width(now(&mut st, "place", json!({ "path": logo })).unwrap()), 30.0);
        assert_eq!(width(now(&mut st, "place", json!({ "path": logo, "print_size": false })).unwrap()), 90.0);
        // So too from the preference.
        now(&mut st, "set_preference", json!({ "key": "transform.place_print_size", "value": false })).unwrap();
        assert_eq!(width(now(&mut st, "place", json!({ "path": logo })).unwrap()), 90.0);
        // A file that does not say its resolution: pixel for pixel either way.
        let shot = jpeg_at("shot.jpg", 90, 60, None);
        assert_eq!(width(now(&mut st, "place", json!({ "path": shot, "print_size": true })).unwrap()), 90.0);
        // Pixels that are not square: 200 × 100 at 200 ppi across and 100 down is an inch
        // each way, so 100 pixels each way here.
        let fax = jpeg_at("fax.jpg", 200, 100, Some([200, 100]));
        let b = box_of(&now(&mut st, "place", json!({ "path": fax, "print_size": true })).unwrap());
        assert_eq!(b, [150.0, 100.0, 100.0, 100.0]);
        // The box switches between the two, about its center, and back.
        st.place_file(std::path::Path::new(&logo), None, true).unwrap();
        let d = st.active_mut().unwrap();
        let center = |d: &crate::doc::DocState| {
            let s = d.transform.as_ref().unwrap();
            (s.current.apply(s.frame.center()), s.params().width)
        };
        let (c0, w0) = center(d);
        d.toggle_print_size().unwrap();
        let (c1, w1) = center(d);
        assert!((w0, w1) == (30.0, 90.0) && (c0[0] - c1[0]).abs() < 1e-9 && (c0[1] - c1[1]).abs() < 1e-9);
        d.toggle_print_size().unwrap();
        assert_eq!(center(d).1, 30.0);
        d.cancel_transform();
        // A file at the image's own resolution has nothing to switch.
        let same = jpeg_at("same.jpg", 90, 60, Some([100, 100]));
        st.place_file(std::path::Path::new(&same), None, true).unwrap();
        let d = st.active_mut().unwrap();
        assert!(d.toggle_print_size().is_err() && d.transform.as_ref().unwrap().params().width == 90.0);
        d.cancel_transform();
        std::fs::remove_dir_all(std::path::Path::new(&logo).parent().unwrap()).ok();
    }

    #[test]
    fn a_placed_file_s_switch_gives_what_placing_it_so_gives() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 400, "height": 300, "resolution": 200 })).unwrap();
        let place = |st: &mut AppState, path: &str, on: bool| {
            box_of(&now(st, "place", json!({ "path": path, "print_size": on })).unwrap())
        };
        // The canvas's pixels at half its resolution print twice its size, and are fitted
        // back to exactly the canvas.
        let half = jpeg_at("half.jpg", 400, 300, Some([100, 100]));
        assert_eq!(place(&mut st, &half, true), [0.0, 0.0, 400.0, 300.0]);
        // Larger than the canvas at its print size only: fitted to it there, its own size
        // pixel for pixel, and the switch gives each, either way. (The call commits its
        // box; this leaves it open.)
        let open = |st: &mut AppState, path: &str, on: bool| {
            st.place_file(std::path::Path::new(path), None, on).unwrap();
        };
        let cancel = |st: &mut AppState| st.active_mut().unwrap().cancel_transform();
        now(&mut st, "resize_image", json!({ "resolution": 100 })).unwrap();
        let wide = jpeg_at("wide.jpg", 300, 200, Some([50, 50]));
        let width = |st: &AppState| st.active().unwrap().transform.as_ref().unwrap().params().width;
        for on in [true, false] {
            let [w0, w1] = if on { [400.0, 300.0] } else { [300.0, 400.0] };
            open(&mut st, &wide, on);
            assert!((width(&st) - w0).abs() < 1e-9, "placed at {} rather than {w0}", width(&st));
            st.active_mut().unwrap().toggle_print_size().unwrap();
            assert!((width(&st) - w1).abs() < 1e-9, "switched to {} rather than {w1}", width(&st));
            st.active_mut().unwrap().toggle_print_size().unwrap();
            assert!((width(&st) - w0).abs() < 1e-9, "switched back to {} rather than {w0}", width(&st));
            cancel(&mut st);
        }
        // Switched to its own pixels, upright, it is on whole pixels, as placing it so is:
        // 91 × 61 pixels at 300 ppi are 30.33 × 20.33 here, centered at (200, 150).
        let odd = jpeg_at("odd.jpg", 91, 61, Some([300, 300]));
        open(&mut st, &odd, true);
        let d = st.active_mut().unwrap();
        d.toggle_print_size().unwrap();
        let m = d.transform.as_ref().unwrap().current;
        assert_eq!([m.a, m.b, m.c, m.d], [1.0, 0.0, 0.0, 1.0]);
        let whole = |v: f64, near: f64| v.fract() == 0.0 && (v - near).abs() <= 0.5;
        assert!(whole(m.e, 154.5) && whole(m.f, 119.5), "at ({}, {})", m.e, m.f);
        cancel(&mut st);
        // A resolution too far from the image's to scale by: pixel for pixel, and said.
        now(&mut st, "resize_image", json!({ "resolution": 0.01 })).unwrap();
        let fine = jpeg_at("fine.jpg", 10, 10, Some([65000, 65000]));
        let v = now(&mut st, "place", json!({ "path": fine, "print_size": true })).unwrap();
        assert_eq!(box_of(&v)[2], 10.0);
        assert!(v["notes"].to_string().contains("too far from the image's"), "{v}");
        open(&mut st, &fine, true);
        let why = st.active_mut().unwrap().toggle_print_size().unwrap_err();
        assert!(why.contains("no print size to switch to"), "{why}");
        cancel(&mut st);
        std::fs::remove_dir_all(std::path::Path::new(&half).parent().unwrap()).ok();
    }

    #[test]
    fn a_call_s_numbers_may_be_expressions_naming_the_image_and_the_box() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        // At 144 ppi (worked out first: the lengths are converted at it), 2 in by 1 in.
        now(&mut st, "new", json!({ "width": "2in", "height": "1in", "resolution": "72 * 2" })).unwrap();
        let c = st.active().unwrap().doc().canvas;
        assert_eq!((c.width, c.height, st.active().unwrap().doc().resolution), (288, 144, 144.0));
        let path = png_file("logo.png", 40, 20).display().to_string();
        let logo = now(&mut st, "place", json!({ "path": path, "name": "Logo", "x": 10, "y": 20 })).unwrap();
        let logo = logo["layer"].clone();
        let bounds = |st: &AppState, layer: &Value| {
            let id = LayerId(layer.as_u64().unwrap());
            let b = st.active().unwrap().doc().layer(id).unwrap().bounds().unwrap();
            [b.x0, b.y0, b.x1, b.y1]
        };
        // Another layer's edge, a length in a unit, and a bare number in the call's unit.
        let given =
            json!({ "path": path, "x": "Logo.right + 0.25in", "y": "layer(\"Logo\").bottom + 1", "unit": "in" });
        let mark = now(&mut st, "place", given).unwrap()["layer"].clone();
        assert_eq!(bounds(&st, &mark), [86.0, 184.0, 126.0, 204.0]);
        // The box's own numbers, as it was: moved 1 in right, then half as wide (its
        // proportions kept), and turned.
        now(&mut st, "transform", json!({ "layers": [mark], "x": "x + 1in" })).unwrap();
        assert_eq!(bounds(&st, &mark), [230.0, 184.0, 270.0, 204.0]);
        let v = now(&mut st, "transform", json!({ "layers": [mark], "width": "width / 2", "angle": "90deg - 60" }))
            .unwrap();
        let (w, angle) = (v["box"]["width"].as_f64().unwrap(), v["box"]["angle"].as_f64().unwrap());
        assert!((w - 20.0).abs() < 1e-9 && (angle - 30.0).abs() < 1e-9, "{v}");
        // A crop's rectangle, the canvas less an inch.
        let c = now(&mut st, "crop", json!({ "width": "canvas.width - 1in", "height": "height" })).unwrap();
        assert_eq!((c["width"].as_u64(), c["height"].as_u64()), (Some(144), Some(144)), "{c}");
        // A whole number of pixels may be a length.
        let image = method("image").unwrap().params;
        let doc = st.active().unwrap().doc().clone();
        let scope = Scope { doc: Some(&doc), ppi: 144.0, unit: Unit::Px, origin: [0.0, 0.0], own: None };
        // (Rounded, not cut short.)
        let a = Args::check(image, &args(json!({ "max_side": "canvas.width / 4 + 0.6" }))).unwrap();
        assert_eq!(a.worked_out(image, &scope, &[], |_| true).unwrap().int("max_side"), 37);
        // What cannot be worked out says what, and where; nothing is done.
        let wrong = now(&mut st, "transform", json!({ "layers": [logo], "x": "Lgo.right" })).unwrap_err();
        assert!(wrong.contains("\"x\": There is no layer called “Lgo”") && wrong.contains("at \"Lgo\""), "{wrong}");
        assert!(st.active().unwrap().transform.is_none(), "the box was left open");
        let turned = now(&mut st, "transform", json!({ "layers": [logo], "angle": "400" })).unwrap_err();
        assert!(turned.contains("between -360 and 360, not 400, what \"400\" comes to"), "{turned}");
        let kind = now(&mut st, "transform", json!({ "layers": [logo], "x": "30deg" })).unwrap_err();
        assert!(kind.contains("an angle, not a length"), "{kind}");
        let unread = now(&mut st, "transform", json!({ "layers": [logo], "x": "1 +" })).unwrap_err();
        assert!(unread.contains("Something is missing at the end"), "{unread}");
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).ok();
    }

    #[test]
    fn a_layer_placed_by_expressions_naming_the_image_is_placed_so_again() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 200, "height": 100, "resolution": 100 })).unwrap();
        let path = png_file("piece.png", 10, 10).display().to_string();
        let id = |v: &Value| LayerId(v["layer"].as_u64().unwrap());
        let logo = id(&now(&mut st, "place", json!({ "path": path, "name": "Logo", "x": 20, "y": 30 })).unwrap());
        let placed = |st: &AppState, l: LayerId| st.active().unwrap().doc().layer(l).unwrap().placed.clone();
        let left = |st: &AppState, l: LayerId| st.active().unwrap().doc().layer(l).unwrap().bounds().unwrap().x0;
        // x names another layer, so it is kept, the layer by id; y is a length alone, which
        // working out again would not change.
        let mark = id(&now(&mut st, "place", json!({ "path": path, "x": "Logo.right + 5", "y": "0.1in" })).unwrap());
        let p = placed(&st, mark).expect("its placement was not kept");
        assert_eq!((p.layers.clone(), p.numbers.len()), (vec![mark], 1));
        assert_eq!(
            (p.numbers[0].number, p.numbers[0].expr.clone()),
            (BoxNumber::X, format!("layer({}).right + 5", logo.0))
        );
        // The logo moved, Re-apply follows it, as one history state.
        now(&mut st, "transform", json!({ "layers": [logo.0], "x": 50 })).unwrap();
        let states = st.active().unwrap().history.nodes().count();
        now(&mut st, "reapply_placement", json!({ "layer": mark.0 })).unwrap();
        assert_eq!(left(&st, mark), 65.0);
        assert_eq!(st.active().unwrap().history.nodes().count(), states + 1);
        assert_eq!(placed(&st, mark), Some(p.clone()), "re-applied, it is kept as it was");
        // A side given with the other, not in proportion with it: re-applied, the other
        // stays as it is.
        let wide = id(&now(&mut st, "place", json!({ "path": path, "width": "Logo.width * 2", "height": 7 })).unwrap());
        now(&mut st, "transform", json!({ "layers": [logo.0], "width": 20 })).unwrap();
        now(&mut st, "reapply_placement", json!({ "layer": wide.0 })).unwrap();
        let b = st.active().unwrap().doc().layer(wide).unwrap().bounds().unwrap();
        assert_eq!((b.width(), b.height()), (40.0, 7.0));
        now(&mut st, "transform", json!({ "layers": [logo.0], "width": 10 })).unwrap();
        // Renamed, the logo is still the layer it names, shown by its new name.
        now(&mut st, "rename", json!({ "layer": logo.0, "name": "Brand mark" })).unwrap();
        now(&mut st, "transform", json!({ "layers": [logo.0], "x": 0 })).unwrap();
        now(&mut st, "reapply_placement", json!({ "layer": mark.0 })).unwrap();
        assert_eq!(left(&st, mark), 15.0);
        let doc = st.active().unwrap().doc().clone();
        let shown = Expr::parse(&p.numbers[0].expr).unwrap().text(&|l| doc.layer(l).map(|l| l.name.clone()));
        assert_eq!(shown, "layer(\"Brand mark\").right + 5");
        // Saved and opened again, it is kept.
        let file = place_path("placed.opal");
        call(&mut st, "save_as", json!({ "path": file.display().to_string() })).unwrap();
        call(&mut st, "close", json!({})).unwrap();
        call(&mut st, "open", json!({ "path": file.display().to_string() })).unwrap();
        assert_eq!(placed(&st, mark), Some(p.clone()), "the placement was not saved");
        // A copy has not been placed.
        now(&mut st, "duplicate", json!({ "layers": [mark.0] })).unwrap();
        let copy = st.active().unwrap().active_layer().unwrap();
        assert_ne!(copy, mark);
        assert_eq!(placed(&st, copy), None, "a copy kept a placement");
        // Placed by the box again, by numbers that name the box itself or nothing of the
        // image, it has no placement to re-apply.
        now(&mut st, "transform", json!({ "layers": [mark.0], "x": "x + 1" })).unwrap();
        assert_eq!(placed(&st, mark), None, "the box named itself, so working it out again would move it again");
        let refused = now(&mut st, "reapply_placement", json!({ "layer": mark.0 })).unwrap_err();
        assert!(refused.contains("not placed by expressions"), "{refused}");
        // The layer it names gone, Re-apply says so, and leaves it as it was.
        now(&mut st, "transform", json!({ "layers": [mark.0], "x": "Brand mark.right" })).unwrap_err();
        now(&mut st, "transform", json!({ "layers": [mark.0], "x": "layer(\"Brand mark\").right" })).unwrap();
        now(&mut st, "delete_layer", json!({ "layers": [logo.0] })).unwrap();
        let before = left(&st, mark);
        // (From the ribbon, with another layer selected: the selection is put back.)
        let sampling = crate::prefs::sampling(&st.prefs, "transform");
        let d = st.active_mut().unwrap();
        d.select_only(copy);
        d.make_active(copy);
        let gone = d.reapply_placement(mark, sampling).unwrap_err();
        assert!(gone.contains("names a layer that is no longer in the image"), "{gone}");
        let d = st.active().unwrap();
        assert!(d.transform.is_none() && left(&st, mark) == before);
        assert!(d.is_selected(copy) && !d.is_selected(mark), "the selection was not put back");
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
    }

    #[test]
    fn a_placement_keeps_only_what_working_out_again_would_not_move_by_itself() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 200, "height": 100, "resolution": 100 })).unwrap();
        let path = png_file("piece.png", 10, 10).display().to_string();
        let id = |v: &Value| LayerId(v["layer"].as_u64().unwrap());
        let logo = id(&now(&mut st, "place", json!({ "path": path, "name": "Logo", "x": 20, "y": 30 })).unwrap());
        let placed = |st: &AppState, l: LayerId| st.active().unwrap().doc().layer(l).unwrap().placed.clone();
        // Naming the layer it moves, or counting from it, it would move again each time.
        now(&mut st, "transform", json!({ "layers": [logo.0], "x": "Logo.left + 10" })).unwrap();
        assert_eq!(placed(&st, logo), None, "it named the layer it moved");
        let from_itself = json!({ "layers": [logo.0], "x": "canvas.width - 100", "from_layer": logo.0 });
        now(&mut st, "transform", from_itself).unwrap();
        assert_eq!(placed(&st, logo), None, "it counted from the layer it moved");
        // Fitted inside a rectangle, its sides are the rectangle's, not the box's (though
        // its height comes out the rectangle's too).
        let given = json!({ "path": path, "width": "Logo.width * 3", "height": "Logo.height", "fit": "inside" });
        let fitted = id(&now(&mut st, "place", given).unwrap());
        assert_eq!(placed(&st, fitted), None, "a rectangle's sides were kept as the box's");
        // An angle past half a turn is the same angle, and kept.
        let turned =
            id(&now(&mut st, "place", json!({ "path": path, "angle": "180 + atan2(Logo.height, Logo.width)" }))
                .unwrap());
        let p = placed(&st, turned).expect("the angle was not kept");
        assert_eq!(p.numbers.iter().map(|n| n.number).collect::<Vec<_>>(), vec![BoxNumber::Angle]);
        // Only drawn another way where it is, it keeps its placement.
        let sampling = crate::prefs::sampling(&st.prefs, "transform");
        let d = st.active_mut().unwrap();
        d.select_only(turned);
        d.make_active(turned);
        assert!(d.begin_transform(sampling));
        d.set_box_sampling(opal_core::resample::Sampling { method: opal_core::resample::Method::Nearest, ..sampling });
        d.commit_pending();
        assert_eq!(placed(&st, turned), Some(p), "drawing it another way lost its placement");
        // Given what leaves it where it is, the box is kept as given all the same.
        let mark = id(&now(&mut st, "place", json!({ "path": path, "x": 5 })).unwrap());
        let left = |st: &AppState, l: LayerId| st.active().unwrap().doc().layer(l).unwrap().bounds().unwrap().x0;
        let there = format!("Logo.left - {}", left(&st, logo) - left(&st, mark));
        now(&mut st, "transform", json!({ "layers": [mark.0], "x": there })).unwrap();
        assert_eq!(left(&st, mark), 5.0);
        assert!(placed(&st, mark).is_some(), "a placement that moved nothing was dropped");
        // Placed together, they are re-applied together, or not at all: not one of them
        // again by itself once the other has been placed otherwise, and none if one is locked.
        let other = id(&now(&mut st, "place", json!({ "path": path, "x": 100 })).unwrap());
        now(&mut st, "transform", json!({ "layers": [mark.0, other.0], "x": "Logo.right" })).unwrap();
        assert_eq!(placed(&st, mark).unwrap().layers.len(), 2);
        // Open on one of them, the box is not what they were placed as.
        let sampling = crate::prefs::sampling(&st.prefs, "transform");
        let d = st.active_mut().unwrap();
        d.select_only(mark);
        d.make_active(mark);
        assert!(d.begin_transform(sampling));
        assert!(d.box_placement().is_none(), "one of two placed together was offered their placement");
        d.cancel_transform();
        now(&mut st, "toggle_lock", json!({ "layers": [other.0], "locked": true })).unwrap();
        let locked = now(&mut st, "reapply_placement", json!({ "layer": mark.0 })).unwrap_err();
        assert!(locked.contains("is locked"), "{locked}");
        now(&mut st, "toggle_lock", json!({ "layers": [other.0], "locked": false })).unwrap();
        now(&mut st, "transform", json!({ "layers": [other.0], "x": 150 })).unwrap();
        let apart = now(&mut st, "reapply_placement", json!({ "layer": mark.0 })).unwrap_err();
        assert!(apart.contains("placed again since"), "{apart}");
        // A group transformed keeps its placement itself.
        now(&mut st, "group_layers", json!({ "layers": [mark.0, other.0] })).unwrap();
        let group = st.active().unwrap().active_layer().unwrap();
        assert!(st.active().unwrap().doc().layer(group).unwrap().is_group());
        now(&mut st, "transform", json!({ "layers": [group.0], "x": "Logo.right" })).unwrap();
        assert!(placed(&st, group).is_some(), "the group has no placement of its own");
        now(&mut st, "transform", json!({ "layers": [logo.0], "x": 0 })).unwrap();
        now(&mut st, "reapply_placement", json!({ "layer": group.0 })).unwrap();
        let b = st.active().unwrap().doc().layer(group).unwrap().bounds().unwrap();
        assert_eq!(b.x0, 10.0, "the group was not placed so again");
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).ok();
    }

    #[test]
    fn re_applying_every_placement_follows_what_each_is_placed_by_and_leaves_loops() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 400, "height": 100, "resolution": 100 })).unwrap();
        let path = png_file("piece.png", 10, 10).display().to_string();
        let id = |v: &Value| LayerId(v["layer"].as_u64().unwrap());
        let place = |st: &mut AppState, name: &str, x: &str| {
            id(&now(st, "place", json!({ "path": path, "name": name, "x": x })).unwrap())
        };
        let left = |st: &AppState, l: LayerId| st.active().unwrap().doc().layer(l).unwrap().bounds().unwrap().x0;
        // A chain: B is placed by A, and C by B; C lies below B, so found first.
        let a = place(&mut st, "A", "0");
        let b = place(&mut st, "B", "A.right + 5");
        let c = place(&mut st, "C", "B.right + 5");
        now(&mut st, "move_down", json!({ "layer": c.0 })).unwrap();
        // A loop: E placed by D, then D by E; and F placed by D.
        let d = place(&mut st, "D", "200");
        let e = place(&mut st, "E", "D.right + 5");
        now(&mut st, "transform", json!({ "layers": [d.0], "x": "E.right + 5" })).unwrap();
        let f = place(&mut st, "F", "D.right + 20");
        now(&mut st, "transform", json!({ "layers": [a.0], "x": 100 })).unwrap();
        now(&mut st, "transform", json!({ "layers": [d.0], "x": 300 })).unwrap();
        // (D placed again by a number, the loop is gone: placed by E alone, then D, again.)
        now(&mut st, "transform", json!({ "layers": [d.0], "x": "E.right + 5" })).unwrap();
        let (d_at, e_at) = (left(&st, d), left(&st, e));
        let states = st.active().unwrap().history.nodes().count();
        let v = now(&mut st, "reapply_all_placements", json!({})).unwrap();
        // B after A, C after B, as one history state.
        assert_eq!([left(&st, b), left(&st, c)], [115.0, 130.0], "not placed in the order they depend on");
        assert_eq!(st.active().unwrap().history.nodes().count(), states + 1);
        // D and E left as they were, and said; F placed by D as D is.
        assert_eq!([left(&st, d), left(&st, e)], [d_at, e_at], "a loop was re-applied");
        assert_eq!(left(&st, f), d_at + 10.0 + 20.0);
        let said = v["notes"].to_string();
        assert!(said.contains("placed relative to one another"), "{said}");
        assert!(said.contains("3 placements re-applied") && said.contains("2 were not"), "{said}");
        // A layer placed in a group that is placed too: placing either moves the other, a
        // loop of its own kind.
        let title = place(&mut st, "Title", "0");
        let logo = place(&mut st, "Logo", "Title.right + 5");
        now(&mut st, "group_layers", json!({ "layers": [logo.0] })).unwrap();
        let card = st.active().unwrap().active_layer().unwrap();
        now(&mut st, "transform", json!({ "layers": [card.0], "x": "canvas.width / 2" })).unwrap();
        now(&mut st, "transform", json!({ "layers": [title.0], "x": 50 })).unwrap();
        let (logo_at, card_at) = (left(&st, logo), left(&st, card));
        let v = now(&mut st, "reapply_all_placements", json!({})).unwrap();
        assert_eq!(
            [left(&st, logo), left(&st, card)],
            [logo_at, card_at],
            "a layer and the group it is in were re-applied"
        );
        assert!(v["notes"].to_string().contains("one holds the other"), "{v}");
        // A placement naming a layer since put into what it places would move it again:
        // refused, not re-applied.
        let pic = place(&mut st, "Pic", "0");
        let row = place(&mut st, "Row", "Pic.right + 10");
        now(&mut st, "group_layers", json!({ "layers": [row.0] })).unwrap();
        let group = st.active().unwrap().active_layer().unwrap();
        now(&mut st, "transform", json!({ "layers": [group.0], "x": "Pic.right + 10" })).unwrap();
        let d = st.active_mut().unwrap();
        d.apply("Move", |doc, _| doc.move_layer(pic, opal_core::document::Position { parent: Some(group), index: 0 }))
            .unwrap();
        let refused = now(&mut st, "reapply_placement", json!({ "layer": group.0 })).unwrap_err();
        assert!(refused.contains("moves with it now"), "{refused}");
        // With nothing selected, nothing is selected after.
        let sampling = crate::prefs::sampling(&st.prefs, "transform");
        let d = st.active_mut().unwrap();
        d.set_layer_selection_state((None, std::collections::HashSet::new()));
        d.reapply_all_placements(sampling);
        assert_eq!(
            d.layer_selection_state(),
            (None, std::collections::HashSet::new()),
            "the selection was not left as it was"
        );
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).ok();
    }

    #[test]
    fn a_box_is_put_a_length_from_a_point_of_another_layer() {
        let _turn = crate::background::testing::turn();
        let mut st = AppState::new();
        now(&mut st, "new", json!({ "width": 200, "height": 100, "resolution": 100 })).unwrap();
        let path = png_file("mark.png", 10, 10).display().to_string();
        let mark = now(&mut st, "place", json!({ "path": path, "x": 50, "y": 20 })).unwrap()["layer"].clone();
        let bounds = |st: &AppState, layer: &Value| {
            let id = LayerId(layer.as_u64().unwrap());
            let b = st.active().unwrap().doc().layer(id).unwrap().bounds().unwrap();
            (b.x0, b.y0, b.x1, b.y1)
        };
        // Its top-left corner 0.05 in (5 px) right of the mark's bottom-right corner.
        let given = json!({ "path": path, "x": "0.05in", "y": 0, "from_layer": mark, "from_anchor": "bottom-right" });
        let next = now(&mut st, "place", given).unwrap();
        assert_eq!((next["box"]["x"].clone(), next["box"]["y"].clone()), (json!(5.0), json!(0.0)), "{next}");
        let layer = next["layer"].clone();
        assert_eq!(bounds(&st, &layer), (65.0, 30.0, 75.0, 40.0));
        // Its center on the canvas's, by numbers from the canvas's center.
        let centered = json!({ "layers": [layer], "x": 0, "y": 0, "anchor": "center", "from_anchor": "center" });
        now(&mut st, "transform", centered).unwrap();
        assert_eq!(bounds(&st, &layer), (95.0, 45.0, 105.0, 55.0));
        // Cropped to the mark, by numbers from it.
        let crop = json!({ "from_layer": mark, "x": -1, "y": -1, "width": 12, "height": 12 });
        let c = now(&mut st, "crop", crop).unwrap();
        assert_eq!((c["width"].as_u64(), c["height"].as_u64()), (Some(12), Some(12)));
        let canvas = st.active().unwrap().doc().canvas;
        assert_eq!((canvas.x, canvas.y), (49, 19));
        // A paste read later counts from where the layer was when it was asked for, though
        // the layer has moved since.
        use crate::clipboard::{Pasted, TestClipboard};
        let px = vec![opal_core::tile::pixel_from_f32([1.0; 4]); 4];
        let image = Pasted {
            width: 2,
            height: 2,
            image: RasterImage::from_pixels(2, 2, &px),
            from: None,
            resolution: None,
            warnings: Vec::new(),
        };
        st.clipboard = Box::new(TestClipboard { image: Some(image), slow: true, ..Default::default() });
        let ctx = egui::Context::default();
        let asked = json!({ "from_layer": mark, "x": 0, "y": 0 });
        let Reply::Later(n) = st.call("paste", &args(asked), &ctx) else { panic!("answered at once") };
        now(&mut st, "transform", json!({ "layers": [mark], "x": 150 })).unwrap();
        let pasted = later(&mut st, n).unwrap()["layer"].clone();
        let (x0, y0, _, _) = bounds(&st, &pasted);
        let canvas = st.active().unwrap().doc().canvas;
        assert_eq!((x0 - canvas.x as f64, y0 - canvas.y as f64), (1.0, 1.0), "not from where the mark was");
        // A layer that is not there, or holds nothing, is nothing to count from.
        let none = now(&mut st, "place", json!({ "path": path, "from_layer": 999 })).unwrap_err();
        assert!(none.contains("no layer 999"), "{none}");
        let empty = now(&mut st, "new_layer", json!({})).unwrap()["layer"].clone();
        let nothing = now(&mut st, "place", json!({ "path": path, "from_layer": empty })).unwrap_err();
        assert!(nothing.contains("holds nothing"), "{nothing}");
        assert!(st.active().unwrap().transform.is_none(), "a refused box was left open");
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).ok();
    }
}
