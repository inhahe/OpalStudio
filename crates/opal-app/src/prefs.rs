//! The preferences: every application-wide setting.

use crate::settings::{Def, Init, Kind, Settings};
use opal_core::resample::{Method, Sampling};

pub const SECTIONS: &[&str] = &["Interface", "Canvas", "Navigation", "Transform", "Resize", "Export"];

/// The resampling methods, as a setting's choices.
const METHODS: &[(&str, &str)] =
    &[("nearest", "Nearest"), ("bilinear", "Bilinear"), ("bicubic", "Bicubic"), ("lanczos", "Lanczos")];

/// One of the five settings that make up a [`Sampling`], under the key prefix `$p`.
macro_rules! sampling_def {
    ($p:literal, $section:literal, method) => {
        Def {
            key: concat!($p, ".method"),
            label: "Resampling",
            help: "How pixels are computed at another size or angle. Bicubic suits photos; \
                   Lanczos is sharper still, with more ringing at hard edges; Bilinear is \
                   softer; Nearest keeps hard square pixels, for pixel art.",
            section: $section,
            kind: Kind::Choice(METHODS),
            default: Init::Choice("bicubic"),
            advanced: false,
        }
    };
    ($p:literal, $section:literal, b) => {
        Def {
            key: concat!($p, ".bicubic_b"),
            label: "Bicubic blur (B)",
            help: "The B of the Mitchell–Netravali cubic: 0 is sharpest (Catmull–Rom with C = \
                   0.5); 1/3 with C = 1/3 is Mitchell's softer choice; 1 is a B-spline, smooth \
                   and blurry.",
            section: $section,
            kind: Kind::Float { min: 0.0, max: 1.0, decimals: 3, suffix: "", log: false },
            default: Init::Float(0.0),
            advanced: true,
        }
    };
    ($p:literal, $section:literal, c) => {
        Def {
            key: concat!($p, ".bicubic_c"),
            label: "Bicubic sharpening (C)",
            help: "The C of the Mitchell–Netravali cubic: more sharpens edges, and rings more.",
            section: $section,
            kind: Kind::Float { min: 0.0, max: 1.0, decimals: 3, suffix: "", log: false },
            default: Init::Float(0.5),
            advanced: true,
        }
    };
    ($p:literal, $section:literal, lobes) => {
        Def {
            key: concat!($p, ".lanczos_lobes"),
            label: "Lanczos lobes",
            help: "How far Lanczos reaches: more lobes keep finer detail and ring more.",
            section: $section,
            kind: Kind::Choice(&[("2", "2"), ("3", "3"), ("4", "4")]),
            default: Init::Choice("3"),
            advanced: true,
        }
    };
    ($p:literal, $section:literal, anti_ringing) => {
        Def {
            key: concat!($p, ".anti_ringing"),
            label: "Anti-ringing",
            help: "Keeps each pixel within the range of the source pixels nearest it, which \
                   removes the light and dark halos sharp filters draw along hard edges.",
            section: $section,
            kind: Kind::Bool,
            default: Init::Bool(true),
            advanced: true,
        }
    };
}

/// The sampling the five settings under `prefix` describe.
pub fn sampling(p: &Settings, prefix: &str) -> Sampling {
    let method = match p.choice(&format!("{prefix}.method")).as_str() {
        "nearest" => Method::Nearest,
        "bilinear" => Method::Bilinear,
        "lanczos" => Method::Lanczos { lobes: p.choice(&format!("{prefix}.lanczos_lobes")).parse().unwrap_or(3) },
        _ => Method::Bicubic {
            b: p.f64(&format!("{prefix}.bicubic_b")) as f32,
            c: p.f64(&format!("{prefix}.bicubic_c")) as f32,
        },
    };
    Sampling { method, anti_ringing: p.bool(&format!("{prefix}.anti_ringing")) }
}

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
        key: "ui.layer_thumbs",
        label: "Layer thumbnails",
        help: "A small picture of each layer's content in the Layers panel.",
        section: "Interface",
        kind: Kind::Choice(&[("off", "Off"), ("small", "Small"), ("large", "Large")]),
        default: Init::Choice("small"),
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
    Def {
        key: "nav.nudge",
        label: "Shift+arrow moves by",
        help: "The arrow keys move the selected layers (or the transform box) by one pixel; \
               with Shift, by this many.",
        section: "Navigation",
        kind: Kind::Float { min: 2.0, max: 500.0, decimals: 0, suffix: " px", log: true },
        default: Init::Float(10.0),
        advanced: true,
    },
    // Transform
    sampling_def!("transform", "Transform", method),
    sampling_def!("transform", "Transform", b),
    sampling_def!("transform", "Transform", c),
    sampling_def!("transform", "Transform", lobes),
    sampling_def!("transform", "Transform", anti_ringing),
    Def {
        key: "transform.fit_large",
        label: "Shrink large pastes to fit",
        help: "A pasted image larger than the canvas starts shrunk to fit inside it (it keeps \
               all its pixels, so enlarging it again loses nothing). Off: it starts at its own \
               size.",
        section: "Transform",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "transform.aspect_locked",
        label: "Keep the aspect ratio",
        help: "Scaling with a corner or edge keeps the width and height in proportion; Shift \
               while dragging does the opposite. The lock button on the box switches this too.",
        section: "Transform",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "transform.snapping",
        label: "Snapping",
        help: "While dragging, edges and centers snap to the canvas's and to other layers'. Alt \
               while dragging does the opposite. The snapping button on the box switches this too.",
        section: "Transform",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: false,
    },
    Def {
        key: "transform.snap_distance",
        label: "Snapping distance",
        help: "How close, on screen, an edge must come to snap.",
        section: "Transform",
        kind: Kind::Float { min: 1.0, max: 40.0, decimals: 0, suffix: " px", log: false },
        default: Init::Float(8.0),
        advanced: true,
    },
    Def {
        key: "transform.rotate_step",
        label: "Shift rotates in steps of",
        help: "Holding Shift while rotating turns in steps of this angle.",
        section: "Transform",
        kind: Kind::Float { min: 1.0, max: 90.0, decimals: 1, suffix: "°", log: false },
        default: Init::Float(15.0),
        advanced: true,
    },
    Def {
        key: "transform.draft",
        label: "Faster drawing while dragging",
        help: "While a corner, edge or the box itself is being dragged, draw with Bilinear, \
               which keeps up with large images; the chosen method takes over on release.",
        section: "Transform",
        kind: Kind::Bool,
        default: Init::Bool(true),
        advanced: true,
    },
    // Resize
    sampling_def!("resize", "Resize", method),
    sampling_def!("resize", "Resize", b),
    sampling_def!("resize", "Resize", c),
    sampling_def!("resize", "Resize", lobes),
    sampling_def!("resize", "Resize", anti_ringing),
    Def {
        key: "resize.resample",
        label: "Resample pixels",
        help: "Off: layers keep their pixels and are drawn at the new size, so resizing back \
               later loses nothing (the file stays as large as it was). On: pixel layers are \
               resampled at the new size now, which makes a smaller image smaller on disk too.",
        section: "Resize",
        kind: Kind::Bool,
        default: Init::Bool(false),
        advanced: false,
    },
    // Export
    sampling_def!("export", "Export", method),
    sampling_def!("export", "Export", b),
    sampling_def!("export", "Export", c),
    sampling_def!("export", "Export", lobes),
    sampling_def!("export", "Export", anti_ringing),
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
