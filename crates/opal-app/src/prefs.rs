//! The preferences: every application-wide setting.

use crate::settings::{Def, Init, Kind};

pub const SECTIONS: &[&str] = &["Interface", "Canvas", "Navigation", "Export"];

pub static PREFS: &[Def] = &[
    // Interface
    Def {
        key: "ui.theme",
        label: "Color scheme",
        help: "Dark is neutral gray, so the interface does not skew how you judge the image's \
               colors, as in most image editors. Following the system switches along with \
               the operating system's light or dark mode.",
        section: "Interface",
        kind: Kind::Choice(&[("dark", "Dark"), ("light", "Light"), ("system", "Follow the system")]),
        default: Init::Choice("dark"),
        advanced: false,
    },
    Def {
        key: "ui.ribbon",
        label: "Ribbon",
        help: "Full shows large buttons with labels, for learning; single line takes less room.",
        section: "Interface",
        kind: Kind::Choice(&[("full", "Full"), ("single", "Single line")]),
        default: Init::Choice("full"),
        advanced: false,
    },
    Def {
        key: "ui.tools",
        label: "Tools",
        help: "Where the tool buttons are: pinned at the left end of the ribbon, or in a strip \
               along a side of the window (for narrow screens).",
        section: "Interface",
        kind: Kind::Choice(&[("ribbon", "In the ribbon"), ("left", "Left strip"), ("right", "Right strip")]),
        default: Init::Choice("ribbon"),
        advanced: false,
    },
    Def {
        key: "ui.scale",
        label: "Interface size",
        help: "Scales all text and controls, on top of the system's display scaling.",
        section: "Interface",
        kind: Kind::Float { min: 75.0, max: 200.0, decimals: 0, suffix: "%", log: false },
        default: Init::Float(100.0),
        advanced: false,
    },
    Def {
        key: "ui.tooltips",
        label: "Tooltips",
        help: "Show a description when the pointer rests on a button or setting.",
        section: "Interface",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "ui.tooltip_delay",
        label: "Tooltip delay",
        help: "How long the pointer must rest before a tooltip appears.",
        section: "Interface",
        kind: Kind::Float { min: 0.0, max: 3.0, decimals: 2, suffix: " s", log: false },
        default: Init::Float(0.5),
        advanced: true,
    },
    // Canvas
    Def {
        key: "canvas.checker_size",
        label: "Checkerboard squares",
        help: "The size of the squares that show transparency.",
        section: "Canvas",
        kind: Kind::Float { min: 2.0, max: 64.0, decimals: 0, suffix: " px", log: true },
        default: Init::Float(8.0),
        advanced: false,
    },
    Def {
        key: "canvas.checker_a",
        label: "Checkerboard color 1",
        help: "One of the two colors of the transparency checkerboard.",
        section: "Canvas",
        kind: Kind::Color,
        default: Init::Color([255, 255, 255, 255]),
        advanced: false,
    },
    Def {
        key: "canvas.checker_b",
        label: "Checkerboard color 2",
        help: "The other color of the transparency checkerboard.",
        section: "Canvas",
        kind: Kind::Color,
        default: Init::Color([204, 204, 204, 255]),
        advanced: false,
    },
    Def {
        key: "canvas.off_canvas_shown",
        label: "Show content beyond the canvas",
        help: "Whether the parts of layers that lie outside the canvas are shown. They are kept \
               either way, and never exported.",
        section: "Canvas",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "canvas.off_canvas",
        label: "Content beyond the canvas",
        help: "How visible the parts of layers that lie outside the canvas are, when they are \
               shown.",
        section: "Canvas",
        kind: Kind::Float { min: 0.0, max: 100.0, decimals: 0, suffix: "%", log: false },
        default: Init::Float(35.0),
        advanced: false,
    },
    Def {
        key: "canvas.border",
        label: "Canvas outline",
        help: "Draw a thin line around the canvas.",
        section: "Canvas",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: true,
    },
    Def {
        key: "canvas.sharp_from",
        label: "Sharp pixels from",
        help: "At this zoom and above, pixels are drawn as sharp squares; below it, smoothly.",
        section: "Canvas",
        kind: Kind::Float { min: 100.0, max: 1600.0, decimals: 0, suffix: "%", log: true },
        default: Init::Float(200.0),
        advanced: true,
    },
    Def {
        key: "canvas.zoomed_out",
        label: "Zoomed out",
        help: "Below 100%, the image is shown from reduced copies. Sharper uses the copy just \
               larger than the screen needs (crisper, slightly more shimmer when panning); \
               smoother uses the one just smaller (softer).",
        section: "Canvas",
        kind: Kind::Choice(&[("sharper", "Sharper"), ("smoother", "Smoother")]),
        default: Init::Choice("sharper"),
        advanced: true,
    },
    // Navigation
    Def {
        key: "nav.wheel",
        label: "Mouse wheel",
        help: "What the mouse wheel does over the canvas. Touchpad scrolling always scrolls the \
               view, and pinching zooms.",
        section: "Navigation",
        kind: Kind::Choice(&[("zoom", "Zooms around the pointer"), ("scroll", "Scrolls the view")]),
        default: Init::Choice("zoom"),
        advanced: false,
    },
    Def {
        key: "nav.wheel_step",
        label: "Wheel zoom step",
        help: "How much one notch of the mouse wheel zooms.",
        section: "Navigation",
        kind: Kind::Float { min: 1.0, max: 50.0, decimals: 0, suffix: "%", log: false },
        default: Init::Float(15.0),
        advanced: true,
    },
    Def {
        key: "nav.fit_max",
        label: "Fit enlarges up to",
        help: "Fitting the image to the window never zooms past this, so small images are not \
               blown up.",
        section: "Navigation",
        kind: Kind::Float { min: 100.0, max: 1600.0, decimals: 0, suffix: "%", log: true },
        default: Init::Float(100.0),
        advanced: true,
    },
    // Export
    Def {
        key: "export.jpeg_quality",
        label: "JPEG quality",
        help: "Higher keeps more detail and makes larger files.",
        section: "Export",
        kind: Kind::Float { min: 1.0, max: 100.0, decimals: 0, suffix: "", log: false },
        default: Init::Float(92.0),
        advanced: false,
    },
    Def {
        key: "export.jpeg_background",
        label: "JPEG background",
        help: "JPEG has no transparency: transparent areas are filled with this color.",
        section: "Export",
        kind: Kind::Color,
        default: Init::Color([255, 255, 255, 255]),
        advanced: false,
    },
    Def {
        key: "export.png_16",
        label: "16-bit PNG",
        help: "Write 16 bits per channel instead of 8: smoother gradients for further editing, \
               files about twice as large.",
        section: "Export",
        kind: Kind::Bool,
        default: Init::Bool(false),
        advanced: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_sections_known() {
        let mut keys: Vec<&str> = PREFS.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n);
        for d in PREFS {
            assert!(SECTIONS.contains(&d.section), "{} has unknown section {}", d.key, d.section);
            assert!(d.accepts(&d.default_value()), "{}'s default does not fit its kind", d.key);
        }
    }
}
