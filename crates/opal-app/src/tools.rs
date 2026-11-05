//! Tools: what the pointer does on the canvas.

use crate::icons;
use crate::settings::{Def, Init, Kind};
use eframe::egui::Key;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolId {
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
}

impl ToolId {
    /// Every tool, in toolbar order.
    pub const ALL: [ToolId; 2] = [ToolId::Hand, ToolId::Zoom];

    pub fn info(self) -> &'static ToolInfo {
        match self {
            ToolId::Hand => &HAND,
            ToolId::Zoom => &ZOOM,
        }
    }
}

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
