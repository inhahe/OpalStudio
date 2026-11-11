//! Tools: what the pointer does on the canvas.

use crate::icons;
use crate::settings::{Def, Init, Kind};
use eframe::egui::Key;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolId {
    Move,
    Select,
    Crop,
    Hand,
    Zoom,
}

/// What the user needs to know about a tool.
pub struct ToolInfo {
    /// Stable identifier, for saved settings.
    pub id: &'static str,
    pub name: &'static str,
    pub icon: char,
    /// The key that selects it.
    pub key: Key,
    pub description: &'static str,
    /// Longer help, for the help pages.
    pub help: &'static str,
    pub settings: &'static [Def],
    /// Its variants, if it has any: shown at the top of the tool settings, never hidden
    /// behind its button (design.md, *No hidden tool groups*).
    pub variants: Option<Variants>,
}

/// A tool's variants: which of its settings chooses one (a choice, its options the variants),
/// and each one's icon, in the order of the options.
pub struct Variants {
    pub key: &'static str,
    pub icons: &'static [char],
}

impl ToolId {
    /// The icon of the variant `settings` (the tool's) choose, or the tool's own.
    pub fn icon_with(self, settings: &crate::settings::Settings) -> char {
        let info = self.info();
        let Some(v) = &info.variants else { return info.icon };
        let chosen = settings.choice(v.key);
        let options = settings.defs().iter().find(|d| d.key == v.key).map_or(&[][..], |d| match d.kind {
            Kind::Choice(options) => options,
            _ => &[],
        });
        options.iter().position(|(id, _)| *id == chosen).and_then(|i| v.icons.get(i)).copied().unwrap_or(info.icon)
    }

    /// The name of the variant `settings` (the tool's) choose, if the tool has variants.
    pub fn variant_with(self, settings: &crate::settings::Settings) -> Option<&'static str> {
        let v = self.info().variants.as_ref()?;
        let chosen = settings.choice(v.key);
        let d = settings.defs().iter().find(|d| d.key == v.key)?;
        let Kind::Choice(options) = d.kind else { return None };
        options.iter().find(|(id, _)| *id == chosen).map(|(_, label)| *label)
    }

    /// Every tool, in toolbar order.
    pub const ALL: [ToolId; 5] = [ToolId::Move, ToolId::Select, ToolId::Crop, ToolId::Hand, ToolId::Zoom];

    pub fn info(self) -> &'static ToolInfo {
        match self {
            ToolId::Move => &MOVE,
            ToolId::Select => &SELECT,
            ToolId::Crop => &CROP,
            ToolId::Hand => &HAND,
            ToolId::Zoom => &ZOOM,
        }
    }
}

pub static MOVE_SETTINGS: &[Def] = &[
    Def {
        key: "move.pick",
        label: "Click selects the layer",
        help: "Clicking or starting a drag selects the topmost layer under the pointer first \
               (Shift+click adds it to the selection, Alt+click picks its group). Off: drags \
               move the selected layers from anywhere.",
        section: "Move",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "move.pick_opacity",
        label: "Counts as under the pointer from",
        help: "How opaque a layer must be at the point clicked to be picked, so a nearly \
               transparent haze does not catch every click. Ctrl+click with other tools picks \
               layers the same way.",
        section: "Move",
        kind: Kind::Float { min: 1.0, max: 100.0, decimals: 0, suffix: "%", log: false },
        default: Init::Float(10.0),
        advanced: true,
    },
];

pub static SELECT_SETTINGS: &[Def] = &[
    Def {
        key: "select.shape",
        label: "Shape",
        help: "What a drag draws.",
        section: "Select",
        kind: Kind::Choice(&[("rectangle", "Rectangle"), ("ellipse", "Ellipse")]),
        default: Init::Choice("rectangle"),
        advanced: false,
    },
    Def {
        key: "select.combine",
        label: "Combine",
        help: "How what a drag draws combines with what is selected: replacing it, added to it, \
               taken away from it, or keeping only what is in both. Keys held as a drag starts \
               choose for that drag: Shift adds, Alt takes away, both keep only what is in both.",
        section: "Select",
        kind: Kind::Choice(&[
            ("replace", "Replace"),
            ("add", "Add"),
            ("subtract", "Subtract"),
            ("intersect", "Intersect"),
        ]),
        default: Init::Choice("replace"),
        advanced: false,
    },
    Def {
        key: "select.smooth",
        label: "Smooth edges",
        help: "A pixel the edge crosses is selected as much of it as the shape covers. Off: each \
               pixel is selected or not, by its middle.",
        section: "Select",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
];

static SELECT: ToolInfo = ToolInfo {
    id: "select",
    name: "Select",
    icon: icons::RECTANGLE,
    key: Key::M,
    description: "Drag to select a rectangle or an ellipse; Shift adds to the selection, Alt takes away.",
    help: "Drag across the canvas to select a rectangle, or an ellipse (choose which at the top of \
           the tool settings). Edges fall on whole pixels. Holding Shift as the drag starts adds \
           to what is selected, Alt takes away, and both keep only what is in both (or choose \
           with Combine); pressing Shift once dragging makes it a square or a circle, and Alt \
           draws it from its middle. Esc while dragging gives up the drag; a click without a \
           drag selects nothing.",
    settings: SELECT_SETTINGS,
    variants: Some(Variants { key: "select.shape", icons: &[icons::RECTANGLE, icons::ELLIPSE] }),
};

pub static CROP_SETTINGS: &[Def] = &[
    Def {
        key: "crop.aspect",
        label: "Aspect ratio",
        help: "Free lets the box take any shape (Shift while dragging keeps its current one); \
               the others keep it in proportion.",
        section: "Crop",
        kind: Kind::Choice(&[
            ("free", "Free"),
            ("original", "As the image"),
            ("1:1", "1:1 (square)"),
            ("4:3", "4:3"),
            ("3:2", "3:2"),
            ("16:9", "16:9"),
            ("5:4", "5:4"),
            ("custom", "Custom"),
        ]),
        default: Init::Choice("free"),
        advanced: false,
    },
    Def {
        key: "crop.portrait",
        label: "Portrait",
        help: "Turn the ratio so the box is taller than it is wide.",
        section: "Crop",
        kind: Kind::Bool,
        default: Init::Bool(false),
        advanced: false,
    },
    Def {
        key: "crop.custom_w",
        label: "Custom ratio width",
        help: "With a custom aspect ratio: its width part (for 7:5, the 7).",
        section: "Crop",
        kind: Kind::Float { min: 0.01, max: 10_000.0, decimals: 2, suffix: "", log: true },
        default: Init::Float(7.0),
        advanced: true,
    },
    Def {
        key: "crop.custom_h",
        label: "Custom ratio height",
        help: "With a custom aspect ratio: its height part (for 7:5, the 5).",
        section: "Crop",
        kind: Kind::Float { min: 0.01, max: 10_000.0, decimals: 2, suffix: "", log: true },
        default: Init::Float(5.0),
        advanced: true,
    },
    Def {
        key: "crop.guides",
        label: "Guides",
        help: "Lines across the box to compose by.",
        section: "Crop",
        kind: Kind::Choice(&[("thirds", "Rule of thirds"), ("grid", "Grid"), ("none", "None")]),
        default: Init::Choice("thirds"),
        advanced: false,
    },
    Def {
        key: "crop.delete",
        label: "Delete cropped pixels",
        help: "Off: cropping only moves the canvas, so enlarging it again brings back what was \
               cropped away. On: pixels outside the new canvas are removed (images and other \
               editable content keep theirs).",
        section: "Crop",
        kind: Kind::Bool,
        default: Init::Bool(false),
        advanced: false,
    },
    Def {
        key: "crop.shade",
        label: "Shade outside",
        help: "How dark what falls outside the box is drawn.",
        section: "Crop",
        kind: Kind::Float { min: 0.0, max: 100.0, decimals: 0, suffix: "%", log: false },
        default: Init::Float(60.0),
        advanced: true,
    },
];

static CROP: ToolInfo = ToolInfo {
    id: "crop",
    name: "Crop",
    icon: icons::CROP,
    key: Key::C,
    description: "Drag the box to choose what to keep; drag outside a corner to straighten. Enter crops.",
    help: "A box covers the canvas: drag its corners or edges to resize it, inside it to move it, \
           or elsewhere to draw a new one. Dragging just outside a corner turns it, to \
           straighten a tilted photo: the image is turned to match when you crop. Enter, the \
           Crop button or a double-click inside the box crops; Esc puts the box back. Cropping \
           only moves the canvas, so nothing is lost unless Delete cropped pixels is set; a box \
           larger than the canvas enlarges it.",
    settings: CROP_SETTINGS,
    variants: None,
};

static MOVE: ToolInfo = ToolInfo {
    id: "move",
    name: "Move",
    icon: icons::MOVE,
    key: Key::V,
    description: "Drag to move the selected layers; click to select the layer under the pointer.",
    help: "Dragging moves the selected layers by whole pixels, snapping to the canvas's and \
           other layers' edges and centers (Alt while dragging turns snapping off or on; Shift \
           keeps the move horizontal, vertical or diagonal). Clicking selects the topmost \
           layer under the pointer; Shift+click adds it to the selection, Alt+click selects its \
           group, and right-clicking lists every layer there. With any tool, the arrow keys \
           move the selected layers by a pixel, and Ctrl+click selects the layer under the \
           pointer. To scale or rotate, use Transform (Ctrl+T).",
    settings: MOVE_SETTINGS,
    variants: None,
};

static HAND: ToolInfo = ToolInfo {
    id: "hand",
    name: "Hand",
    icon: icons::HAND,
    key: Key::H,
    description: "Drag to move the view around the image.",
    help: "Drag anywhere on the canvas to scroll the view. You can also scroll the view with \
           any tool by holding Space and dragging, or by dragging with the middle mouse \
           button.",
    settings: &[],
    variants: None,
};

pub static ZOOM_SETTINGS: &[Def] = &[
    Def {
        key: "zoom.drag",
        label: "Drag to zoom",
        help: "Dragging right zooms in and dragging left zooms out, around the point where the \
               drag started. Off: dragging does nothing; click to zoom.",
        section: "Zoom",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "zoom.drag_speed",
        label: "Drag zoom speed",
        help: "How much the zoom doubles per 100 points of dragging.",
        section: "Zoom",
        kind: Kind::Float { min: 10.0, max: 400.0, decimals: 0, suffix: "%", log: true },
        default: Init::Float(100.0),
        advanced: true,
    },
    Def {
        key: "zoom.steps",
        label: "Clicks zoom by",
        help: "Preset levels: each click moves to the next of 100%, 150%, 200%, 300% and so on \
               (and the same steps below 100%). Factor: each click multiplies or divides the \
               zoom by the factor below.",
        section: "Zoom",
        kind: Kind::Choice(&[("presets", "Preset levels"), ("factor", "Factor")]),
        default: Init::Choice("presets"),
        advanced: true,
    },
    Def {
        key: "zoom.factor",
        label: "Zoom factor",
        help: "How much one click zooms when clicks zoom by a factor.",
        section: "Zoom",
        kind: Kind::Float { min: 1.05, max: 4.0, decimals: 2, suffix: "×", log: false },
        default: Init::Float(2.0),
        advanced: true,
    },
];

static ZOOM: ToolInfo = ToolInfo {
    id: "zoom",
    name: "Zoom",
    icon: icons::ZOOM_IN,
    key: Key::Z,
    description: "Click to zoom in, Alt+click to zoom out, or drag to zoom smoothly.",
    help: "Click to zoom in around the point clicked; hold Alt and click to zoom out. Drag \
           right or left to zoom in or out smoothly around where the drag started. With any \
           tool, the mouse wheel zooms around the pointer (see Preferences › Navigation).",
    settings: ZOOM_SETTINGS,
    variants: None,
};

/// The preset zoom levels the zoom commands and clicks step through.
pub const ZOOM_PRESETS: &[f32] = &[
    1.0 / 128.0,
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 12.0,
    1.0 / 8.0,
    1.0 / 6.0,
    1.0 / 4.0,
    1.0 / 3.0,
    1.0 / 2.0,
    2.0 / 3.0,
    1.0,
    1.5,
    2.0,
    3.0,
    4.0,
    6.0,
    8.0,
    12.0,
    16.0,
    24.0,
    32.0,
    48.0,
    64.0,
    96.0,
    128.0,
];

/// The next preset zoom level above (`up`) or below `zoom`.
pub fn next_preset(zoom: f32, up: bool) -> f32 {
    let eps = 1e-4;
    if up {
        ZOOM_PRESETS.iter().copied().find(|&z| z > zoom * (1.0 + eps)).unwrap_or(zoom)
    } else {
        ZOOM_PRESETS.iter().rev().copied().find(|&z| z < zoom * (1.0 - eps)).unwrap_or(zoom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_step_in_both_directions() {
        assert_eq!(next_preset(1.0, true), 1.5);
        assert_eq!(next_preset(1.0, false), 2.0 / 3.0);
        assert_eq!(next_preset(1.2, true), 1.5);
        assert_eq!(next_preset(1.2, false), 1.0);
        assert_eq!(next_preset(128.0, true), 128.0);
        assert_eq!(next_preset(1.0 / 128.0, false), 1.0 / 128.0);
    }

    #[test]
    fn tool_keys_are_unique() {
        let mut keys: Vec<Key> = ToolId::ALL.iter().map(|t| t.info().key).collect();
        keys.sort_by_key(|k| format!("{k:?}"));
        keys.dedup();
        assert_eq!(keys.len(), ToolId::ALL.len());
    }

    #[test]
    fn tool_setting_defaults_fit_their_kinds() {
        for t in ToolId::ALL {
            for d in t.info().settings {
                assert!(d.accepts(&d.default_value()), "{}'s default does not fit its kind", d.key);
            }
        }
    }
}
