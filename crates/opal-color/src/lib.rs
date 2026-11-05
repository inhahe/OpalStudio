//! Color models and conversions.
//!
//! A port of the color engine of the author's colorpicker
//! (`D:\visual studio projects\colorpicker\js\color-engine.js`), with OKLab, OKLCh and
//! linear sRGB added. Two deliberate differences from the original:
//!
//! - Values are never rounded or clamped. The colorpicker rounds sRGB to whole numbers and
//!   clamps to the gamut because it only displays colors; an image editor must round-trip
//!   pixels exactly, including colors outside the sRGB gamut (stored as negative or >1
//!   linear values).
//! - The hub is linear sRGB, the editor's working space, rather than XYZ. The two are
//!   related by one matrix, so this only saves work on the common path.
//!
//! Every model's values are in its display units (sRGB 0–255, Lab L* 0–100, hue 0–360°,
//! and so on), the units its channel ranges are given in.

pub mod matrix;

use matrix::{Mat3, invert, mul, mul_vec};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------
// Constants (the same values the colorpicker uses)
// ---------------------------------------------------------------------------------------

/// Linear sRGB (IEC 61966-2-1, D65) to CIE XYZ.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];
/// CIE XYZ to linear sRGB.
pub const XYZ_TO_SRGB: Mat3 = invert(&SRGB_TO_XYZ);

/// Linear Adobe RGB (1998), D65, to CIE XYZ.
const ADOBE_TO_XYZ: Mat3 = [
    [0.5767309, 0.1855540, 0.1881852],
    [0.2973769, 0.6273491, 0.0752741],
    [0.0270343, 0.0706872, 0.9911085],
];
const ADOBE_TO_SRGB: Mat3 = mul(&XYZ_TO_SRGB, &ADOBE_TO_XYZ);
const SRGB_TO_ADOBE: Mat3 = invert(&ADOBE_TO_SRGB);
/// Adobe RGB's transfer exponent, 563/256.
const ADOBE_GAMMA: f64 = 2.19921875;

/// CIE XYZ to LMS cone responses (Hunt–Pointer–Estevez).
const XYZ_TO_LMS: Mat3 = [
    [0.4002400, 0.7076000, -0.0808100],
    [-0.2263000, 1.1653200, 0.0457000],
    [0.0000000, 0.0000000, 0.9182200],
];
const SRGB_TO_LMS: Mat3 = mul(&XYZ_TO_LMS, &SRGB_TO_XYZ);
const LMS_TO_SRGB: Mat3 = invert(&SRGB_TO_LMS);

/// The D65 reference white in XYZ, which CIE L*a*b* is referenced to here (as in the
/// colorpicker; Photoshop and ICC profiles use D50 instead).
pub const D65: [f64; 3] = [0.95047, 1.0, 1.08883];

/// Linear sRGB to OKLab's cone-like space (Björn Ottosson).
const OKLAB_M1: Mat3 = [
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
];
const OKLAB_M2: Mat3 = [
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
];
const OKLAB_M1_INV: Mat3 = invert(&OKLAB_M1);
const OKLAB_M2_INV: Mat3 = invert(&OKLAB_M2);

// ---------------------------------------------------------------------------------------
// Transfer functions
// ---------------------------------------------------------------------------------------

/// sRGB-encoded (0–1) to linear light. Negative values mirror the curve, as extended-range
/// sRGB (scRGB, CSS Color 4) does, so out-of-gamut colors round-trip.
pub fn srgb_decode(v: f64) -> f64 {
    let a = v.abs();
    let lin = if a <= 0.04045 { a / 12.92 } else { ((a + 0.055) / 1.055).powf(2.4) };
    lin.copysign(v)
}

/// Linear light to sRGB-encoded (0–1); the inverse of [`srgb_decode`].
pub fn srgb_encode(v: f64) -> f64 {
    let a = v.abs();
    let enc = if a <= 0.0031308 { a * 12.92 } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
    enc.copysign(v)
}

fn adobe_decode(v: f64) -> f64 {
    v.abs().powf(ADOBE_GAMMA).copysign(v)
}

fn adobe_encode(v: f64) -> f64 {
    v.abs().powf(1.0 / ADOBE_GAMMA).copysign(v)
}

// ---------------------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------------------

/// A color model the user can pick wherever a setting is defined over color channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorModel {
    #[serde(rename = "srgb")]
    Srgb,
    #[serde(rename = "linear-srgb")]
    LinearSrgb,
    #[serde(rename = "adobergb")]
    AdobeRgb,
    #[serde(rename = "xyz")]
    Xyz,
    #[serde(rename = "lab")]
    Lab,
    #[serde(rename = "lch")]
    Lch,
    #[serde(rename = "oklab")]
    Oklab,
    #[serde(rename = "oklch")]
    Oklch,
    #[serde(rename = "hsb")]
    Hsb,
    #[serde(rename = "hsl")]
    Hsl,
    #[serde(rename = "cmy")]
    Cmy,
    #[serde(rename = "lms")]
    Lms,
    #[serde(rename = "opponent")]
    Opponent,
}

/// How a channel's values behave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// An ordinary range, from `min` (none) to `max`.
    Bounded,
    /// A range centered on zero, such as Lab's a* (green to red).
    Signed,
    /// An angle in degrees, 0–360, that wraps around.
    Hue,
}

/// One of a model's three channels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    /// Stable identifier, unique within the model.
    pub id: &'static str,
    /// Name shown to the user.
    pub name: &'static str,
    /// The usual range of values, in the model's display units. Colors outside the sRGB
    /// gamut can fall outside it.
    pub min: f64,
    pub max: f64,
    pub kind: ChannelKind,
}

const fn ch(id: &'static str, name: &'static str, min: f64, max: f64, kind: ChannelKind) -> Channel {
    Channel { id, name, min, max, kind }
}

use ChannelKind::{Bounded, Hue, Signed};

impl ColorModel {
    /// Every model, in the order menus list them.
    pub const ALL: [ColorModel; 13] = [
        ColorModel::Srgb,
        ColorModel::LinearSrgb,
        ColorModel::AdobeRgb,
        ColorModel::Hsb,
        ColorModel::Hsl,
        ColorModel::Cmy,
        ColorModel::Lab,
        ColorModel::Lch,
        ColorModel::Oklab,
        ColorModel::Oklch,
        ColorModel::Xyz,
        ColorModel::Lms,
        ColorModel::Opponent,
    ];

    /// Stable identifier, as used in files.
    pub fn id(self) -> &'static str {
        match self {
            ColorModel::Srgb => "srgb",
            ColorModel::LinearSrgb => "linear-srgb",
            ColorModel::AdobeRgb => "adobergb",
            ColorModel::Xyz => "xyz",
            ColorModel::Lab => "lab",
            ColorModel::Lch => "lch",
            ColorModel::Oklab => "oklab",
            ColorModel::Oklch => "oklch",
            ColorModel::Hsb => "hsb",
            ColorModel::Hsl => "hsl",
            ColorModel::Cmy => "cmy",
            ColorModel::Lms => "lms",
            ColorModel::Opponent => "opponent",
        }
    }

    /// The model whose [`id`](Self::id) is `id`.
    pub fn from_id(id: &str) -> Option<ColorModel> {
        ColorModel::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Name shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            ColorModel::Srgb => "sRGB",
            ColorModel::LinearSrgb => "Linear sRGB",
            ColorModel::AdobeRgb => "Adobe RGB (1998)",
            ColorModel::Xyz => "CIE XYZ",
            ColorModel::Lab => "CIE L*a*b*",
            ColorModel::Lch => "CIE LCh",
            ColorModel::Oklab => "OKLab",
            ColorModel::Oklch => "OKLCh",
            ColorModel::Hsb => "HSB / HSV",
            ColorModel::Hsl => "HSL",
            ColorModel::Cmy => "CMY",
            ColorModel::Lms => "LMS (cone response)",
            ColorModel::Opponent => "Color opponent",
        }
    }

    /// One or two sentences on what the model is and what it is good for.
    pub fn description(self) -> &'static str {
        match self {
            ColorModel::Srgb => {
                "The standard RGB of the web and of most monitors and photos, with its usual \
                 gamma encoding. Not perceptually uniform."
            }
            ColorModel::LinearSrgb => {
                "sRGB's primaries without gamma encoding: values proportional to light. \
                 Opal Studio's working space, where mixing light behaves physically."
            }
            ColorModel::AdobeRgb => {
                "A wider gamut than sRGB, especially in cyan-green, common in print and \
                 photography. Gamma 563/256."
            }
            ColorModel::Xyz => {
                "CIE 1931 XYZ tristimulus values, the device-independent space most others \
                 are defined from. Y is luminance."
            }
            ColorModel::Lab => {
                "CIE 1976 L*a*b*, roughly perceptually uniform. L* is lightness, a* green to \
                 red, b* blue to yellow. Referenced to D65."
            }
            ColorModel::Lch => {
                "L*a*b* in polar form: lightness, chroma (colorfulness) and hue angle."
            }
            ColorModel::Oklab => {
                "A modern perceptually uniform space (Björn Ottosson, 2020), more even in hue \
                 than L*a*b*. L is lightness, a green to red, b blue to yellow."
            }
            ColorModel::Oklch => {
                "OKLab in polar form: lightness, chroma and hue. Changing hue here keeps \
                 perceived lightness, unlike HSL or HSB."
            }
            ColorModel::Hsb => {
                "Hue, saturation and brightness: a cylindrical rearrangement of sRGB. \
                 Brightness 100 is the most intense color of that hue, not white."
            }
            ColorModel::Hsl => {
                "Hue, saturation and lightness: like HSB, but lightness 100 is white and 50 \
                 is the most intense color."
            }
            ColorModel::Cmy => {
                "Cyan, magenta and yellow, the complements of red, green and blue \
                 (C = 255 − R and so on)."
            }
            ColorModel::Lms => {
                "The responses of the eye's long, medium and short-wavelength cones \
                 (Hunt–Pointer–Estevez)."
            }
            ColorModel::Opponent => {
                "How the visual system encodes color after the cones: yellow-blue, \
                 red-green and brightness."
            }
        }
    }

    /// The model's three channels.
    pub fn channels(self) -> [Channel; 3] {
        match self {
            ColorModel::Srgb | ColorModel::AdobeRgb => [
                ch("r", "Red", 0.0, 255.0, Bounded),
                ch("g", "Green", 0.0, 255.0, Bounded),
                ch("b", "Blue", 0.0, 255.0, Bounded),
            ],
            ColorModel::LinearSrgb => [
                ch("r", "Red", 0.0, 1.0, Bounded),
                ch("g", "Green", 0.0, 1.0, Bounded),
                ch("b", "Blue", 0.0, 1.0, Bounded),
            ],
            ColorModel::Xyz => [
                ch("x", "X", 0.0, 1.1, Bounded),
                ch("y", "Y", 0.0, 1.1, Bounded),
                ch("z", "Z", 0.0, 1.1, Bounded),
            ],
            ColorModel::Lab => [
                ch("l", "L*", 0.0, 100.0, Bounded),
                ch("a", "a*", -128.0, 127.0, Signed),
                ch("b", "b*", -128.0, 127.0, Signed),
            ],
            ColorModel::Lch => [
                ch("l", "L*", 0.0, 100.0, Bounded),
                ch("c", "C*", 0.0, 150.0, Bounded),
                ch("h", "h", 0.0, 360.0, Hue),
            ],
            ColorModel::Oklab => [
                ch("l", "L", 0.0, 1.0, Bounded),
                ch("a", "a", -0.4, 0.4, Signed),
                ch("b", "b", -0.4, 0.4, Signed),
            ],
            ColorModel::Oklch => [
                ch("l", "L", 0.0, 1.0, Bounded),
                ch("c", "C", 0.0, 0.4, Bounded),
                ch("h", "h", 0.0, 360.0, Hue),
            ],
            ColorModel::Hsb => [
                ch("h", "Hue", 0.0, 360.0, Hue),
                ch("s", "Saturation", 0.0, 100.0, Bounded),
                ch("b", "Brightness", 0.0, 100.0, Bounded),
            ],
            ColorModel::Hsl => [
                ch("h", "Hue", 0.0, 360.0, Hue),
                ch("s", "Saturation", 0.0, 100.0, Bounded),
                ch("l", "Lightness", 0.0, 100.0, Bounded),
            ],
            ColorModel::Cmy => [
                ch("c", "Cyan", 0.0, 255.0, Bounded),
                ch("m", "Magenta", 0.0, 255.0, Bounded),
                ch("y", "Yellow", 0.0, 255.0, Bounded),
            ],
            ColorModel::Lms => [
                ch("l", "L (long)", 0.0, 1.0, Bounded),
                ch("m", "M (medium)", 0.0, 1.0, Bounded),
                ch("s", "S (short)", 0.0, 1.0, Bounded),
            ],
            ColorModel::Opponent => [
                ch("yb", "Yellow-Blue", -50.0, 50.0, Signed),
                ch("rg", "Red-Green", -30.0, 30.0, Signed),
                ch("br", "Brightness", 0.0, 100.0, Bounded),
            ],
        }
    }

    /// Converts this model's values to linear sRGB (unclamped).
    pub fn to_linear_srgb(self, v: [f64; 3]) -> [f64; 3] {
        match self {
            ColorModel::LinearSrgb => v,
            ColorModel::Srgb => v.map(|c| srgb_decode(c / 255.0)),
            ColorModel::AdobeRgb => mul_vec(&ADOBE_TO_SRGB, v.map(|c| adobe_decode(c / 255.0))),
            ColorModel::Xyz => mul_vec(&XYZ_TO_SRGB, v),
            ColorModel::Lab => mul_vec(&XYZ_TO_SRGB, lab_to_xyz(v)),
            ColorModel::Lch => ColorModel::Lab.to_linear_srgb(polar_to_rect(v)),
            ColorModel::Oklab => oklab_to_linear_srgb(v),
            ColorModel::Oklch => oklab_to_linear_srgb(polar_to_rect(v)),
            ColorModel::Hsb => hsb_to_srgb_encoded(v).map(srgb_decode),
            ColorModel::Hsl => hsl_to_srgb_encoded(v).map(srgb_decode),
            ColorModel::Cmy => v.map(|c| srgb_decode((255.0 - c) / 255.0)),
            ColorModel::Lms => mul_vec(&LMS_TO_SRGB, v),
            ColorModel::Opponent => mul_vec(&LMS_TO_SRGB, opponent_to_lms(v)),
        }
    }

    /// Converts linear sRGB to this model's values (unclamped).
    pub fn from_linear_srgb(self, rgb: [f64; 3]) -> [f64; 3] {
        match self {
            ColorModel::LinearSrgb => rgb,
            ColorModel::Srgb => rgb.map(|c| srgb_encode(c) * 255.0),
            ColorModel::AdobeRgb => mul_vec(&SRGB_TO_ADOBE, rgb).map(|c| adobe_encode(c) * 255.0),
            ColorModel::Xyz => mul_vec(&SRGB_TO_XYZ, rgb),
            ColorModel::Lab => xyz_to_lab(mul_vec(&SRGB_TO_XYZ, rgb)),
            ColorModel::Lch => rect_to_polar(ColorModel::Lab.from_linear_srgb(rgb)),
            ColorModel::Oklab => linear_srgb_to_oklab(rgb),
            ColorModel::Oklch => rect_to_polar(linear_srgb_to_oklab(rgb)),
            ColorModel::Hsb => srgb_encoded_to_hsb(rgb.map(srgb_encode)),
            ColorModel::Hsl => srgb_encoded_to_hsl(rgb.map(srgb_encode)),
            ColorModel::Cmy => rgb.map(|c| 255.0 - srgb_encode(c) * 255.0),
            ColorModel::Lms => mul_vec(&SRGB_TO_LMS, rgb),
            ColorModel::Opponent => lms_to_opponent(mul_vec(&SRGB_TO_LMS, rgb)),
        }
    }

    /// Converts this model's values to CIE XYZ.
    pub fn to_xyz(self, v: [f64; 3]) -> [f64; 3] {
        match self {
            ColorModel::Xyz => v,
            _ => mul_vec(&SRGB_TO_XYZ, self.to_linear_srgb(v)),
        }
    }

    /// Converts CIE XYZ to this model's values.
    pub fn from_xyz(self, xyz: [f64; 3]) -> [f64; 3] {
        match self {
            ColorModel::Xyz => xyz,
            _ => self.from_linear_srgb(mul_vec(&XYZ_TO_SRGB, xyz)),
        }
    }

    /// Maps this model's values onto a common scale: bounded channels and hue to 0–1, signed
    /// channels to −1–1 (dividing by the larger of the range's two ends). Used wherever
    /// weights must mean the same for channels whose units differ.
    pub fn normalize(self, v: [f64; 3]) -> [f64; 3] {
        let chans = self.channels();
        std::array::from_fn(|i| normalize_channel(&chans[i], v[i]))
    }

    /// The inverse of [`normalize`](Self::normalize).
    pub fn denormalize(self, n: [f64; 3]) -> [f64; 3] {
        let chans = self.channels();
        std::array::from_fn(|i| denormalize_channel(&chans[i], n[i]))
    }
}

impl std::fmt::Display for ColorModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

fn normalize_channel(c: &Channel, v: f64) -> f64 {
    match c.kind {
        Bounded => (v - c.min) / (c.max - c.min),
        Signed => v / c.min.abs().max(c.max.abs()),
        Hue => v / 360.0,
    }
}

fn denormalize_channel(c: &Channel, n: f64) -> f64 {
    match c.kind {
        Bounded => c.min + n * (c.max - c.min),
        Signed => n * c.min.abs().max(c.max.abs()),
        Hue => n * 360.0,
    }
}

/// Converts `v` from one model to another.
pub fn convert(v: [f64; 3], from: ColorModel, to: ColorModel) -> [f64; 3] {
    if from == to {
        return v;
    }
    to.from_linear_srgb(from.to_linear_srgb(v))
}

// ---------------------------------------------------------------------------------------
// Per-model helpers
// ---------------------------------------------------------------------------------------

const LAB_EPSILON: f64 = 216.0 / 24389.0; // (6/29)^3
const LAB_KAPPA: f64 = 24389.0 / 27.0;

fn lab_f(t: f64) -> f64 {
    if t > LAB_EPSILON { t.cbrt() } else { (LAB_KAPPA * t + 16.0) / 116.0 }
}

fn lab_f_inv(t: f64) -> f64 {
    if t > 6.0 / 29.0 { t * t * t } else { (116.0 * t - 16.0) / LAB_KAPPA }
}

fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let fx = lab_f(xyz[0] / D65[0]);
    let fy = lab_f(xyz[1] / D65[1]);
    let fz = lab_f(xyz[2] / D65[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_to_xyz([l, a, b]: [f64; 3]) -> [f64; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;
    [D65[0] * lab_f_inv(fx), D65[1] * lab_f_inv(fy), D65[2] * lab_f_inv(fz)]
}

/// `[L, C, h°]` to `[L, a, b]`.
fn polar_to_rect([l, c, h]: [f64; 3]) -> [f64; 3] {
    let (s, co) = h.to_radians().sin_cos();
    [l, c * co, c * s]
}

/// `[L, a, b]` to `[L, C, h°]`, with h in 0–360 (0 for neutral colors).
fn rect_to_polar([l, a, b]: [f64; 3]) -> [f64; 3] {
    let c = (a * a + b * b).sqrt();
    let mut h = b.atan2(a).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    [l, c, h]
}

fn linear_srgb_to_oklab(rgb: [f64; 3]) -> [f64; 3] {
    let lms = mul_vec(&OKLAB_M1, rgb).map(f64::cbrt);
    mul_vec(&OKLAB_M2, lms)
}

fn oklab_to_linear_srgb(lab: [f64; 3]) -> [f64; 3] {
    let lms = mul_vec(&OKLAB_M2_INV, lab).map(|c| c * c * c);
    mul_vec(&OKLAB_M1_INV, lms)
}

/// The red, green and blue of a hue on the hexagonal HSB/HSL color wheel, before scaling.
fn hue_sector(h: f64, chroma: f64) -> [f64; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - ((h % 2.0) - 1.0).abs());
    match h as u32 {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    }
}

/// Hue (0–360) of encoded RGB with the given max, min and their difference `d`.
fn hexagonal_hue([r, g, b]: [f64; 3], max: f64, d: f64) -> f64 {
    if d == 0.0 {
        return 0.0;
    }
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    if h < 0.0 { h + 360.0 } else { h }
}

fn hsb_to_srgb_encoded([h, s, v]: [f64; 3]) -> [f64; 3] {
    let (s, v) = (s / 100.0, v / 100.0);
    let c = v * s;
    let m = v - c;
    hue_sector(h, c).map(|x| x + m)
}

fn srgb_encoded_to_hsb(rgb: [f64; 3]) -> [f64; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let d = max - min;
    let s = if max == 0.0 { 0.0 } else { d / max * 100.0 };
    [hexagonal_hue(rgb, max, d), s, max * 100.0]
}

fn hsl_to_srgb_encoded([h, s, l]: [f64; 3]) -> [f64; 3] {
    let (s, l) = (s / 100.0, l / 100.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let m = l - c / 2.0;
    hue_sector(h, c).map(|x| x + m)
}

fn srgb_encoded_to_hsl(rgb: [f64; 3]) -> [f64; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let d = max - min;
    let l = (max + min) / 2.0;
    let denom = 1.0 - (2.0 * l - 1.0).abs();
    let s = if d == 0.0 || denom.abs() < 1e-12 { 0.0 } else { d / denom };
    [hexagonal_hue(rgb, max, d), s * 100.0, l * 100.0]
}

fn lms_to_opponent([l, m, s]: [f64; 3]) -> [f64; 3] {
    let half = (l + m) / 2.0;
    [(half - s) * 100.0, (l - m) * 100.0, half * 100.0]
}

fn opponent_to_lms([yb, rg, br]: [f64; 3]) -> [f64; 3] {
    let half = br / 100.0;
    let diff = rg / 100.0;
    [half + diff / 2.0, half - diff / 2.0, half - yb / 100.0]
}

// ---------------------------------------------------------------------------------------
// Color difference
// ---------------------------------------------------------------------------------------

/// The CIEDE2000 color difference between two CIE L*a*b* colors (kL = kC = kH = 1).
pub fn ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    const POW25_7: f64 = 6103515625.0;

    let c1ab = (a1 * a1 + b1 * b1).sqrt();
    let c2ab = (a2 * a2 + b2 * b2).sqrt();
    let cab_avg7 = ((c1ab + c2ab) / 2.0).powi(7);
    let g = 0.5 * (1.0 - (cab_avg7 / (cab_avg7 + POW25_7)).sqrt());
    let a1p = a1 * (1.0 + g);
    let a2p = a2 * (1.0 + g);
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();
    let hue = |b: f64, a: f64| {
        let h = b.atan2(a).to_degrees();
        if h < 0.0 { h + 360.0 } else { h }
    };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);

    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let d_hp = 2.0 * (c1p * c2p).sqrt() * (dhp / 2.0).to_radians().sin();

    let lp_avg = (l1 + l2) / 2.0;
    let cp_avg = (c1p + c2p) / 2.0;
    let hp_avg = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };

    let t = 1.0 - 0.17 * (hp_avg - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_avg).to_radians().cos()
        + 0.32 * (3.0 * hp_avg + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_avg - 63.0).to_radians().cos();
    let l50 = (lp_avg - 50.0) * (lp_avg - 50.0);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cp_avg;
    let sh = 1.0 + 0.015 * cp_avg * t;
    let cp_avg7 = cp_avg.powi(7);
    let rc = 2.0 * (cp_avg7 / (cp_avg7 + POW25_7)).sqrt();
    let d_theta = 30.0 * (-((hp_avg - 275.0) / 25.0).powi(2)).exp();
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;

    let tl = dlp / sl;
    let tc = dcp / sc;
    let th = d_hp / sh;
    (tl * tl + tc * tc + th * th + rt * tc * th).sqrt()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    /// In-gamut linear sRGB samples, including the corners of the gamut and grays.
    fn samples() -> Vec<[f64; 3]> {
        let steps = [0.0, 0.003, 0.05, 0.2, 0.5, 0.8, 1.0];
        let mut v = Vec::new();
        for &r in &steps {
            for &g in &steps {
                for &b in &steps {
                    v.push([r, g, b]);
                }
            }
        }
        v
    }

    #[test]
    fn every_model_round_trips_in_gamut() {
        for model in ColorModel::ALL {
            for rgb in samples() {
                let back = model.to_linear_srgb(model.from_linear_srgb(rgb));
                assert!(close(back, rgb, 1e-9), "{model:?}: {rgb:?} came back as {back:?}");
            }
        }
    }

    #[test]
    fn rgb_like_models_round_trip_out_of_gamut() {
        // Hue-based models are excluded: hue and saturation lose meaning outside the gamut.
        let models = [
            ColorModel::Srgb,
            ColorModel::LinearSrgb,
            ColorModel::AdobeRgb,
            ColorModel::Xyz,
            ColorModel::Lab,
            ColorModel::Lch,
            ColorModel::Oklab,
            ColorModel::Oklch,
            ColorModel::Cmy,
            ColorModel::Lms,
            ColorModel::Opponent,
        ];
        let wide = [[-0.2, 0.5, 1.3], [1.5, -0.1, 0.0], [0.0, 1.2, -0.05]];
        for model in models {
            for rgb in wide {
                let back = model.to_linear_srgb(model.from_linear_srgb(rgb));
                assert!(close(back, rgb, 1e-9), "{model:?}: {rgb:?} came back as {back:?}");
            }
        }
    }

    #[test]
    fn white_has_the_expected_coordinates() {
        let white = [1.0, 1.0, 1.0];
        assert!(close(ColorModel::Srgb.from_linear_srgb(white), [255.0; 3], 1e-9));
        assert!(close(ColorModel::Xyz.from_linear_srgb(white), D65, 1e-6));
        assert!(close(ColorModel::Lab.from_linear_srgb(white), [100.0, 0.0, 0.0], 1e-3));
        assert!(close(ColorModel::Oklab.from_linear_srgb(white), [1.0, 0.0, 0.0], 1e-4));
        assert!(close(ColorModel::Hsb.from_linear_srgb(white), [0.0, 0.0, 100.0], 1e-9));
        assert!(close(ColorModel::Hsl.from_linear_srgb(white), [0.0, 0.0, 100.0], 1e-9));
        assert!(close(ColorModel::Cmy.from_linear_srgb(white), [0.0; 3], 1e-9));
    }

    #[test]
    fn oklab_of_srgb_red_matches_css_color_4() {
        // CSS Color 4 gives oklab(0.62796 0.22486 0.12585) for sRGB red.
        let lab = ColorModel::Oklab.from_linear_srgb([1.0, 0.0, 0.0]);
        assert!(close(lab, [0.62796, 0.22486, 0.12585], 1e-4), "{lab:?}");
    }

    #[test]
    fn hsb_and_hsl_of_primaries() {
        let red = [1.0, 0.0, 0.0];
        let green = [0.0, 1.0, 0.0];
        let blue = [0.0, 0.0, 1.0];
        assert!(close(ColorModel::Hsb.from_linear_srgb(red), [0.0, 100.0, 100.0], 1e-9));
        assert!(close(ColorModel::Hsb.from_linear_srgb(green), [120.0, 100.0, 100.0], 1e-9));
        assert!(close(ColorModel::Hsl.from_linear_srgb(blue), [240.0, 100.0, 50.0], 1e-9));
    }

    #[test]
    fn srgb_transfer_is_continuous_and_mirrored() {
        // The two pieces of the curve meet at the threshold.
        assert!((srgb_decode(0.04045) - 0.04045 / 12.92).abs() < 1e-12);
        assert!((srgb_encode(srgb_decode(0.04046)) - 0.04046).abs() < 1e-12);
        assert_eq!(srgb_decode(-0.5), -srgb_decode(0.5));
        assert_eq!(srgb_encode(-0.5), -srgb_encode(0.5));
    }

    #[test]
    fn normalize_maps_ranges_and_round_trips() {
        let lab = [50.0, -64.0, 127.0];
        let n = ColorModel::Lab.normalize(lab);
        assert!(close(n, [0.5, -0.5, 127.0 / 128.0], 1e-12));
        assert!(close(ColorModel::Lab.denormalize(n), lab, 1e-12));
        let hsl = ColorModel::Hsl.normalize([180.0, 50.0, 25.0]);
        assert!(close(hsl, [0.5, 0.5, 0.25], 1e-12));
    }

    #[test]
    fn ids_round_trip() {
        for model in ColorModel::ALL {
            assert_eq!(ColorModel::from_id(model.id()), Some(model));
        }
        assert_eq!(ColorModel::from_id("nonsense"), None);
    }

    #[test]
    fn ciede2000_matches_sharma_test_data() {
        // Sharma, Wu and Dalal (2005), pairs 1–3.
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
            ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        ];
        for (a, b, want) in cases {
            let got = ciede2000(a, b);
            assert!((got - want).abs() < 1e-4, "{a:?} vs {b:?}: {got}, want {want}");
        }
        assert_eq!(ciede2000([40.0, 10.0, -5.0], [40.0, 10.0, -5.0]), 0.0);
    }
}
