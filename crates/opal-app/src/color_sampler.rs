//! Color samplers: points pinned on an image whose colour the Info panel shows, each in a
//! colour model of its own, as the edit being shown would leave it beside how it is (a box
//! dragged, an adjustment previewed); placed, moved and removed with the Color Sampler tool
//! (I), kept in the image's file, read by scripts. Not part of the image's history: they go
//! with it turned, flipped or resized (see `measure_tool`, whose measure is kept so too).

use crate::doc::{DocState, Drag};
use eframe::egui::{self, Color32, Pos2, Rect, Response, Stroke};
use opal_color::ColorModel;
use opal_core::document::Layer;
use opal_core::geom::{Affine, IRect};
use std::collections::HashMap;
use std::sync::Arc;

/// The most samplers an image holds.
pub const MOST: usize = 10;

/// How near (in points) a press must be to a sampler to take it.
const GRAB: f32 = 9.0;

/// Why a sampler cannot be placed when the image has [`MOST`].
pub const FULL: &str = "An image holds ten color samplers at most: drag one, or Alt+click one to remove it.";

/// A point pinned on an image whose colour is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampler {
    /// The middle of the pixel sampled, in document pixels as the image was when its
    /// `Document::carried` was `carried`.
    pub at: [f64; 2],
    pub carried: Affine,
    /// How its colour is shown.
    pub shown: Shown,
}

impl Sampler {
    /// A sampler of pixel `p` on an image whose `Document::carried` is `carried`.
    pub fn of(p: [i64; 2], carried: Affine, shown: Shown) -> Sampler {
        Sampler { at: [p[0] as f64 + 0.5, p[1] as f64 + 0.5], carried, shown }
    }

    /// The pixel it samples on an image whose `Document::carried` is `now`: the one its
    /// middle is carried into by what has turned, flipped or resized the image since.
    pub fn pixel(&self, now: Affine) -> [i64; 2] {
        let at = match self.carried.inverse() {
            Some(back) if now != self.carried => back.then(now).apply(self.at),
            _ => self.at,
        };
        [at[0].floor() as i64, at[1].floor() as i64]
    }
}

/// How a colour is shown: in one of the colour models, or as sRGB's hex code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Model(ColorModel),
    Hex,
}

impl Shown {
    /// Every way, in the order menus list them.
    pub fn all() -> impl Iterator<Item = Shown> {
        ColorModel::ALL.into_iter().map(Shown::Model).chain(std::iter::once(Shown::Hex))
    }

    /// Stable identifier, as files and scripts give it (a model's own, or `hex`).
    pub fn id(self) -> &'static str {
        match self {
            Shown::Model(m) => m.id(),
            Shown::Hex => "hex",
        }
    }

    pub fn from_id(id: &str) -> Option<Shown> {
        if id == "hex" { Some(Shown::Hex) } else { ColorModel::from_id(id).map(Shown::Model) }
    }

    /// Its name, as menus show it.
    pub fn name(self) -> &'static str {
        match self {
            Shown::Model(m) => m.name(),
            Shown::Hex => "Hex (sRGB)",
        }
    }
}

impl Default for Shown {
    fn default() -> Shown {
        Shown::Model(ColorModel::Srgb)
    }
}

/// A colour read off an image: its straight (not premultiplied) linear sRGB, the image's
/// own, and its alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Read {
    pub rgb: [f64; 3],
    pub alpha: f64,
}

impl Read {
    /// `self` as `shown` shows it: each channel's name and value (a hex code alone).
    pub fn told(&self, shown: Shown) -> Vec<(&'static str, String)> {
        match shown {
            Shown::Hex => vec![("Hex", self.hex())],
            Shown::Model(m) => {
                let v = m.from_linear_srgb(self.rgb);
                m.channels()
                    .iter()
                    .zip(v)
                    .map(|(c, v)| {
                        // Places enough for the model's range: none for 0–255 or 0–100, more
                        // for 0–1.
                        let span = c.max - c.min;
                        let places = if span >= 50.0 {
                            0
                        } else if span >= 2.0 {
                            2
                        } else {
                            4
                        };
                        (c.name, format!("{v:.places$}"))
                    })
                    .collect()
            }
        }
    }

    /// sRGB's hex code, each channel clamped to its range.
    pub fn hex(&self) -> String {
        let [r, g, b] = self.srgb8();
        format!("#{r:02X}{g:02X}{b:02X}")
    }

    /// sRGB, 8 bits a channel, each clamped to its range.
    pub fn srgb8(&self) -> [u8; 3] {
        self.rgb.map(|v| (opal_color::srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8)
    }

    /// The colour as egui shows it (over nothing: its alpha kept).
    pub fn swatch(&self) -> Color32 {
        let [r, g, b] = self.srgb8();
        Color32::from_rgba_unmultiplied(r, g, b, (self.alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

/// How long what was read is kept while the image changes from frame to frame (a box
/// dragged), where it may lag (the Info panel's): composited at most this often then, not
/// every frame.
pub const FRESH: std::time::Duration = std::time::Duration::from_millis(100);

/// The most composited tiles kept (the pointer moving about takes more).
const MOST_TILES: usize = 64;

/// What has been read off one state of an image (its layers), and the tiles of it composited
/// for that, as the canvas composites them: kept until its layers change, so that a tile is
/// composited once, not each frame or each pixel. The layers are held weakly: none (a preview
/// given up, say) is kept from going by what was read of it, nor its place taken by another.
#[derive(Default)]
pub struct Readings {
    layers: Vec<std::sync::Weak<Layer>>,
    /// When they were taken.
    taken: Option<std::time::Instant>,
    /// Each tile composited (none where nothing shows).
    tiles: HashMap<opal_core::tile::TileCoord, Option<Arc<opal_core::tile::Tile>>>,
    read: HashMap<([i64; 2], i64), Read>,
}

impl Readings {
    /// What `layers` show averaged over the `size × size` pixels about pixel `p`: their
    /// premultiplied colours averaged, then made straight; and whether that is what they
    /// showed a moment before instead, which, with `lagging`, is kept for [`FRESH`] while they
    /// change.
    pub fn get(&mut self, layers: &[Arc<Layer>], p: [i64; 2], size: i64, lagging: bool) -> (Read, bool) {
        let same = self.layers.len() == layers.len()
            && self.layers.iter().zip(layers).all(|(was, l)| std::ptr::eq(was.as_ptr(), Arc::as_ptr(l)));
        if !same {
            let recent = self.taken.is_some_and(|t| t.elapsed() < FRESH);
            if lagging
                && recent
                && let Some(r) = self.read.get(&(p, size))
            {
                return (*r, true);
            }
            self.layers = layers.iter().map(Arc::downgrade).collect();
            self.taken = Some(std::time::Instant::now());
            self.tiles.clear();
            self.read.clear();
        }
        // (Many read, the pointer moving about: begun again, so as not to grow unbounded.)
        if self.read.len() > 256 {
            self.read.clear();
        }
        if self.tiles.len() > MOST_TILES {
            self.tiles.clear();
        }
        if let Some(r) = self.read.get(&(p, size)) {
            return (*r, false);
        }
        let h = size / 2;
        let rect = IRect::new(p[0] - h, p[1] - h, p[0] + h + 1, p[1] + h + 1);
        let mut sum = [0.0f64; 4];
        for (x, y) in (rect.y0..rect.y1).flat_map(|y| (rect.x0..rect.x1).map(move |x| (x, y))) {
            let c = opal_core::tile::TileCoord::containing(x, y);
            let tile =
                self.tiles.entry(c).or_insert_with(|| opal_core::composite::composite_tile(layers, c).map(Arc::new));
            if let Some(t) = tile {
                let at = c.rect(0);
                let px = opal_core::tile::pixel_to_f32(t.get((x - at.x0) as u32, (y - at.y0) as u32));
                for (s, v) in sum.iter_mut().zip(px) {
                    *s += f64::from(v);
                }
            }
        }
        let n = (size * size) as f64;
        let [r, g, b, a] = sum.map(|s| s / n);
        let rgb = if a > 0.0 { [r / a, g / a, b / a] } else { [0.0; 3] };
        let read = Read { rgb, alpha: a };
        self.read.insert((p, size), read);
        (read, false)
    }
}

/// The sizes the samplers may average over, as the tool's setting offers them.
pub const SIZES: &[(&str, &str)] = &[
    ("1", "Point"),
    ("3", "3 × 3 average"),
    ("5", "5 × 5 average"),
    ("11", "11 × 11 average"),
    ("31", "31 × 31 average"),
    ("51", "51 × 51 average"),
    ("101", "101 × 101 average"),
];

/// The ids of [`SIZES`], as scripts give them.
pub const SIZE_IDS: &[&str] = &["1", "3", "5", "11", "31", "51", "101"];

/// The ways a colour may be shown, as the tool's settings offer them: each [`Shown`]'s id and
/// name, in its order.
pub const SHOWN: &[(&str, &str)] = &[
    ("srgb", "sRGB"),
    ("linear-srgb", "Linear sRGB"),
    ("adobergb", "Adobe RGB (1998)"),
    ("hsb", "HSB / HSV"),
    ("hsl", "HSL"),
    ("cmy", "CMY"),
    ("lab", "CIE L*a*b*"),
    ("lch", "CIE LCh"),
    ("oklab", "OKLab"),
    ("oklch", "OKLCh"),
    ("xyz", "CIE XYZ"),
    ("lms", "LMS (cone response)"),
    ("opponent", "Color opponent"),
    ("hex", "Hex (sRGB)"),
];

/// The ids of [`SHOWN`], as scripts give them.
pub const SHOWN_IDS: &[&str] = &[
    "srgb",
    "linear-srgb",
    "adobergb",
    "hsb",
    "hsl",
    "cmy",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "xyz",
    "lms",
    "opponent",
    "hex",
];

/// `read` as scripts are told it (see the `color_at` query): its linear sRGB and alpha, sRGB
/// at 8 bits, its hex code, and its values as `shown` shows them (a hex code's, the code).
pub fn color_json(read: &Read, shown: Shown) -> serde_json::Value {
    let values = match shown {
        Shown::Hex => serde_json::json!(read.hex()),
        Shown::Model(m) => serde_json::json!(m.from_linear_srgb(read.rgb)),
    };
    serde_json::json!({
        "linear": read.rgb,
        "alpha": read.alpha,
        "srgb": read.srgb8(),
        "hex": read.hex(),
        "shown_as": shown.id(),
        "values": values,
    })
}

/// How large a square the samplers average over, as the tool's settings say (one pixel, or
/// an odd number a side).
pub fn sample_size(s: &crate::settings::Settings) -> i64 {
    s.choice("sampler.size").parse().unwrap_or(1)
}

/// How the tool's setting `key` says a colour is shown.
pub fn shown_by(s: &crate::settings::Settings, key: &str) -> Shown {
    Shown::from_id(&s.choice(key)).unwrap_or_default()
}

/// A menu of the ways a colour may be shown, `id` telling it apart; whether it changed.
fn shown_menu(ui: &mut egui::Ui, shown: &mut Shown, id: egui::IdSalt) -> bool {
    let before = *shown;
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown.name())
        .show_ui(ui, |ui| {
            for s in Shown::all() {
                ui.selectable_value(shown, s, s.name());
            }
        })
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Colour shown as"));
    *shown != before
}

/// `read` as `shown` shows it, in one line: each channel's name and value; with `before`
/// (an edit in progress changing it), each value as it was, then as it is.
fn line(read: &Read, before: Option<&Read>, shown: Shown) -> String {
    let now = read.told(shown);
    let was = before.map(|b| b.told(shown));
    let mut parts: Vec<String> = now
        .iter()
        .enumerate()
        .map(|(i, (name, v))| match &was {
            Some(w) if w[i].1 != *v => format!("{name} {} → {v}", w[i].1),
            _ => format!("{name} {v}"),
        })
        .collect();
    // Opacity, where there is some transparency.
    let alpha = |r: &Read| format!("{:.0}%", r.alpha * 100.0);
    if read.alpha < 0.9995 || before.is_some_and(|b| b.alpha < 0.9995) {
        parts.push(match before {
            Some(b) if alpha(b) != alpha(read) => format!("Opacity {} → {}", alpha(b), alpha(read)),
            _ => format!("Opacity {}", alpha(read)),
        });
    }
    parts.join("   ")
}

/// The Info panel: the colour under the pointer, and each of the active image's samplers (its
/// number and the pixel it samples, from the canvas's corner; a swatch; its colour in a way
/// chosen for it; a button removing it), as the image shows, and while an edit in progress
/// changes it, as it was before that (before → now); averaged as the Color Sampler tool's
/// Sample size says.
pub fn info_panel(ui: &mut egui::Ui, st: &mut crate::state::AppState) {
    use crate::tools::ToolId;
    let weak = st.palette.weak_text;
    let settings = st.tool_settings(ToolId::ColorSampler);
    let size = sample_size(settings);
    let mut pointer_shown = shown_by(settings, "sampler.pointer");
    if st.active().is_none() {
        ui.label(egui::RichText::new("Open or create an image to see its colours here.").color(weak));
        return;
    }
    // Under the pointer, in whichever image it is over (last frame's: the panel may be laid
    // out before the canvases), named if not the active one.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Pointer").strong());
        shown_menu(ui, &mut pointer_shown, egui::IdSalt::new("info-pointer"));
    });
    let active = st.active_doc;
    match st.cursor_shown.filter(|&(id, ..)| st.doc(id).is_some()) {
        Some((id, x, y)) => {
            let d = st.doc_mut(id).expect("filtered above");
            let c = d.doc().canvas;
            let layers = d.doc().layers.clone();
            let (read, late) = d.readings[0].get(&layers, [x, y], size, true);
            if late {
                ui.ctx().request_repaint_after(FRESH);
            }
            let whose = if Some(id) == active { String::new() } else { format!(" in {}", d.title) };
            ui.horizontal(|ui| {
                swatch(ui, &read);
                ui.label(format!("{}, {}{whose}", x - c.x, y - c.y));
            });
            ui.label(line(&read, None, pointer_shown));
        }
        // (As tall as a colour's two lines, so that nothing below moves as the pointer leaves
        // the image: for the panel's own buttons, say.)
        None => {
            ui.label(egui::RichText::new("Not over an image").color(weak));
            ui.label(" ");
        }
    }
    ui.separator();
    let d = st.active_mut().expect("checked above");
    let c = d.doc().canvas;
    // The samplers.
    let (readings, late) = d.sampler_readings(size, true);
    if late {
        ui.ctx().request_repaint_after(FRESH);
    }
    if readings.is_empty() {
        let how = "Click on the image with the Color Sampler tool (I) to pin a point whose colour shows here, \
                   as edits change it.";
        ui.add(egui::Label::new(egui::RichText::new(how).color(weak)).wrap());
    }
    let pixels = d.sampler_pixels();
    let mut remove = None;
    let mut changed = false;
    for (i, ((read, before), [x, y])) in readings.iter().zip(&pixels).enumerate() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("#{}", i + 1)).strong());
            swatch(ui, read);
            let at = if c.rect().contains(*x, *y) {
                format!("{}, {}", x - c.x, y - c.y)
            } else {
                format!("{}, {} (off the canvas)", x - c.x, y - c.y)
            };
            ui.label(at);
            changed |= shown_menu(ui, &mut d.samplers[i].shown, egui::IdSalt::new(("info-sampler", i)));
            if ui.small_button(crate::icons::CLOSE.to_string()).on_hover_text("Remove this sampler").clicked() {
                remove = Some(i);
            }
        });
        ui.label(line(read, before.as_ref(), d.samplers[i].shown));
    }
    if let Some(i) = remove {
        d.samplers.remove(i);
        changed = true;
    }
    if changed {
        d.samplers_changed();
    }
    if pointer_shown != shown_by(st.tool_settings(ToolId::ColorSampler), "sampler.pointer") {
        let s = st.tool_settings.get_mut(&ToolId::ColorSampler).expect("every tool has settings");
        s.set("sampler.pointer", crate::settings::Value::Choice(pointer_shown.id().to_owned()));
        st.settings_touched();
    }
}

/// A small square of `read`'s colour, over a checkerboard where it is not opaque.
fn swatch(ui: &mut egui::Ui, read: &Read) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    if read.alpha < 1.0 {
        let half = r.size() * 0.5;
        for (corner, grey) in [
            (r.min, 200),
            (r.min + half, 200),
            (r.min + egui::vec2(half.x, 0.0), 140),
            (r.min + egui::vec2(0.0, half.y), 140),
        ] {
            painter.rect_filled(Rect::from_min_size(corner, half), 0.0, Color32::from_gray(grey));
        }
    }
    painter.rect_filled(r, 0.0, read.swatch());
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);
}

impl DocState {
    /// Where its samplers are, each the pixel it samples now.
    pub fn sampler_pixels(&self) -> Vec<[i64; 2]> {
        let now = self.doc().carried;
        self.samplers.iter().map(|s| s.pixel(now)).collect()
    }

    /// What its samplers read, each `size` pixels a side: as the image shows now (an edit in
    /// progress, if one is), and as its history has it if that differs.
    ///
    /// With `lagging` (for the Info panel; not for scripts, which want them as they are), what
    /// was read a moment before is kept while the image changes from frame to frame, said by
    /// the second value (see [`Readings::get`]).
    pub fn sampler_readings(&mut self, size: i64, lagging: bool) -> (Vec<(Read, Option<Read>)>, bool) {
        let pixels = self.sampler_pixels();
        let shown = self.doc().layers.clone();
        let kept = self.history.doc().layers.clone();
        let differ = !opal_core::composite::same_layers(&shown, &kept);
        let mut lagged = false;
        let read = pixels
            .iter()
            .map(|&p| {
                let (now, late) = self.readings[0].get(&shown, p, size, lagging);
                lagged |= late;
                let before = differ.then(|| self.readings[1].get(&kept, p, size, false).0);
                (now, before.filter(|b| *b != now))
            })
            .collect();
        (read, lagged)
    }

    /// Samplers changed: told to whatever shows them (and [`is_dirty`](DocState::is_dirty)
    /// compares them with the file's).
    pub fn samplers_changed(&mut self) {
        self.revision += 1;
    }

    /// Its samplers as its file keeps them.
    pub fn pinned(&self) -> Vec<opal_io::native::Pinned> {
        let now = self.doc().carried;
        self.samplers
            .iter()
            .map(|s| opal_io::native::Pinned { at: s.pixel(now), shown: s.shown.id().to_owned() })
            .collect()
    }

    /// Its samplers as a file kept them (the first [`MOST`]; one shown a way not known, as
    /// sRGB).
    pub fn set_pinned(&mut self, pinned: &[opal_io::native::Pinned]) {
        let carried = self.doc().carried;
        self.samplers = pinned
            .iter()
            .take(MOST)
            .map(|p| Sampler::of(p.at, carried, Shown::from_id(&p.shown).unwrap_or_default()))
            .collect();
        self.samplers_changed();
    }
}

/// A drag of a sampler: which one, where it was before it (put back if it is given up), and
/// where in it the pointer took it (document pixels from its pixel's middle), so that it does
/// not jump to the pointer.
#[derive(Clone, Copy, Debug)]
pub struct SamplerDrag {
    index: usize,
    was: Sampler,
    grip: [f64; 2],
}

impl DocState {
    /// The sampler dragged by `g` put back where it was, if it is there still.
    pub(crate) fn sampler_back(&mut self, g: SamplerDrag) {
        if let Some(s) = self.samplers.get_mut(g.index) {
            *s = g.was;
        }
    }
}

/// Which sampler of `d`'s is under screen point `p` (by `to_screen`), the nearest, if any.
fn under(d: &DocState, p: Pos2, to_screen: &dyn Fn([f64; 2]) -> Pos2) -> Option<usize> {
    let now = d.doc().carried;
    d.samplers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let [x, y] = s.pixel(now);
            (i, to_screen([x as f64 + 0.5, y as f64 + 0.5]).distance(p))
        })
        .filter(|&(_, dist)| dist <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// What a frame's presses and drags with the Color Sampler tool do on `d`'s canvas (by
/// `to_doc` and `to_screen`): a click away from the samplers places one there (shown as
/// `shown`), on the canvas, unless the image has [`MOST`] (said, through `notice`); a drag
/// of one moves it (Esc gives the drag up), and Alt+click on one removes it.
pub fn canvas(
    ctx: &egui::Context,
    d: &mut DocState,
    response: &Response,
    to_doc: &dyn Fn(Pos2) -> [f64; 2],
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
    shown: Shown,
    notice: &mut Option<String>,
) {
    let (pressed, down, at, alt, ctrl) = ctx.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.latest_pos(),
            i.modifiers.alt,
            i.modifiers.command,
        )
    });
    // (Where it was pressed; or, pressed and let go within the frame, where it is. Not with
    // Ctrl: Ctrl+click picks a layer, whatever the tool.)
    if pressed
        && !ctrl
        && d.drag.is_none()
        && response.contains_pointer()
        && let Some(p) = ctx.input(|i| i.pointer.press_origin().or_else(|| i.pointer.interact_pos()))
    {
        match under(d, p, to_screen) {
            Some(i) if alt => {
                d.samplers.remove(i);
                d.samplers_changed();
            }
            Some(index) => {
                let [px, py] = d.samplers[index].pixel(d.doc().carried);
                let [x, y] = to_doc(p);
                let grip = [x - (px as f64 + 0.5), y - (py as f64 + 0.5)];
                d.drag = Some(Drag::Sampler(SamplerDrag { index, was: d.samplers[index], grip }));
            }
            None => {
                let [x, y] = to_doc(p);
                let pixel = [x.floor() as i64, y.floor() as i64];
                if !d.doc().canvas.rect().contains(pixel[0], pixel[1]) {
                    // (Off the canvas: nothing to sample.)
                } else if d.samplers.len() >= MOST {
                    *notice = Some(FULL.into());
                } else {
                    d.samplers.push(Sampler::of(pixel, d.doc().carried, shown));
                    d.samplers_changed();
                }
            }
        }
    }
    let Some(Drag::Sampler(g)) = d.drag else { return };
    let index = g.index;
    // (Nothing removes one while a button is held: scripts' calls wait, and the panel's
    // buttons want the pointer. Should one go all the same, the drag stops.)
    debug_assert!(index < d.samplers.len(), "a sampler removed while dragged");
    if index >= d.samplers.len() {
        d.drag = None;
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        d.give_up_drag();
        return;
    }
    if down && let Some(pos) = at {
        let [x, y] = to_doc(pos);
        let [x, y] = [x - g.grip[0], y - g.grip[1]];
        let c = d.doc().canvas.rect();
        // Kept on the canvas.
        let pixel = [(x.floor() as i64).clamp(c.x0, c.x1 - 1), (y.floor() as i64).clamp(c.y0, c.y1 - 1)];
        let moved = Sampler::of(pixel, d.doc().carried, d.samplers[index].shown);
        if moved.pixel(moved.carried) != d.samplers[index].pixel(d.doc().carried) {
            d.samplers[index] = moved;
            d.samplers_changed();
        }
    }
    if !down {
        d.drag = None;
    }
}

/// Draws `d`'s samplers on `rect` (by `to_screen`): a ring about each pixel sampled, its
/// number beside it, light over dark to show on any image.
pub fn draw(ui: &egui::Ui, d: &DocState, accent: Color32, rect: Rect, to_screen: &dyn Fn([f64; 2]) -> Pos2) {
    let painter = ui.painter().with_clip_rect(rect);
    for (i, [x, y]) in d.sampler_pixels().into_iter().enumerate() {
        let c = to_screen([x as f64 + 0.5, y as f64 + 0.5]);
        painter.circle_stroke(c, 6.0, Stroke::new(3.0, Color32::from_black_alpha(160)));
        painter.circle_stroke(c, 6.0, Stroke::new(1.5, Color32::WHITE));
        for (a, b) in [(egui::vec2(-9.0, 0.0), egui::vec2(-3.0, 0.0)), (egui::vec2(3.0, 0.0), egui::vec2(9.0, 0.0))]
            .into_iter()
            .chain([(egui::vec2(0.0, -9.0), egui::vec2(0.0, -3.0)), (egui::vec2(0.0, 3.0), egui::vec2(0.0, 9.0))])
        {
            painter.line_segment([c + a, c + b], Stroke::new(1.5, accent));
        }
        let galley = painter.layout_no_wrap(format!("{}", i + 1), egui::FontId::proportional(11.0), Color32::WHITE);
        let r = Rect::from_min_size(c + egui::vec2(8.0, 4.0), galley.size());
        painter.rect_filled(r.expand2(egui::vec2(3.0, 1.0)), 2.0, Color32::from_black_alpha(190));
        painter.galley(r.min, galley, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_told_in_its_model_s_units_or_as_a_hex_code() {
        // Linear 0.21586 is sRGB's 128.
        let grey = Read { rgb: [0.21586; 3], alpha: 1.0 };
        assert_eq!(grey.hex(), "#808080");
        let told = grey.told(Shown::Model(ColorModel::Srgb));
        assert_eq!(told, vec![("Red", "128".into()), ("Green", "128".into()), ("Blue", "128".into())]);
        let lin = grey.told(Shown::Model(ColorModel::LinearSrgb));
        assert_eq!(lin[0], ("Red", "0.2159".into()));
        let hsb = Read { rgb: [1.0, 0.0, 0.0], alpha: 1.0 }.told(Shown::Model(ColorModel::Hsb));
        assert_eq!(hsb.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>(), ["0", "100", "100"]);
        assert_eq!(Shown::from_id("hex"), Some(Shown::Hex));
        assert_eq!(Shown::from_id("lab"), Some(Shown::Model(ColorModel::Lab)));
        assert_eq!(Shown::all().count(), ColorModel::ALL.len() + 1);
    }

    #[test]
    fn what_is_read_keeps_no_layers_alive() {
        use opal_core::document::{IdGen, Layer};
        let mut ids = IdGen::default();
        let mut img = opal_core::raster::RasterImage::new();
        img.set_pixel(0, 0, opal_core::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        let layers = vec![Arc::new(Layer::new_raster(ids.layer(), ids.object(), "red", img))];
        let mut readings = Readings::default();
        assert_eq!(readings.get(&layers, [0, 0], 1, false).0.srgb8(), [255, 0, 0]);
        let gone = Arc::downgrade(&layers[0]);
        drop(layers);
        assert!(gone.upgrade().is_none(), "kept alive by what was read");
    }

    #[test]
    fn a_sampler_goes_with_the_image_turned_and_back() {
        let s = Sampler::of([10, 20], Affine::IDENTITY, Shown::default());
        assert_eq!(s.pixel(Affine::IDENTITY), [10, 20]);
        // A quarter turn clockwise about the origin: (10.5, 20.5) to (-20.5, 10.5).
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2);
        assert_eq!(s.pixel(quarter), [-21, 10]);
        let turned = Sampler::of([-21, 10], quarter, Shown::default());
        assert_eq!(turned.pixel(Affine::IDENTITY), [10, 20]);
    }

    #[test]
    fn samplers_read_the_image_and_as_it_was_while_an_edit_in_progress_changes_it() {
        use crate::state::{AppState, Background, NewImage};
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 40, height: 30, background: Background::White, resolution: 72.0 });
        let d = st.active_mut().unwrap();
        let carried = d.doc().carried;
        d.samplers.push(Sampler::of([5, 6], carried, Shown::default()));
        let white = Read { rgb: [1.0; 3], alpha: 1.0 };
        let close =
            |a: &Read, b: &Read| (0..3).all(|k| (a.rgb[k] - b.rgb[k]).abs() < 1e-3) && (a.alpha - b.alpha).abs() < 1e-3;
        let (r, _) = d.sampler_readings(1, false);
        assert!(r.len() == 1 && close(&r[0].0, &white) && r[0].1.is_none(), "{r:?}");
        // The background hidden by an edit in progress: nothing there now, white before. Where
        // it may lag (the panel's), what was read a moment before is kept a moment, and said.
        let bg = d.doc().layers[0].id;
        d.edit_pending("Hide", |doc| doc.layer_mut(bg).unwrap().visible = false);
        let (r, late) = d.sampler_readings(1, true);
        assert!(late && close(&r[0].0, &white), "{r:?}");
        // (Read as before, a pixel: not what was read of the image as it was.)
        let (r, late) = d.sampler_readings(1, false);
        assert!(!late);
        assert_eq!(r[0].0.alpha, 0.0);
        assert!(r[0].1.is_some_and(|b| close(&b, &white)), "{r:?}");
        // A moment later, as it is, where it may lag too.
        d.readings[0].taken = d.readings[0].taken.map(|t| t - FRESH);
        let bg2 = d.doc().layers[0].id;
        d.edit_pending("Hide", |doc| doc.layer_mut(bg2).unwrap().opacity = 0.5);
        let (r, late) = d.sampler_readings(1, true);
        assert!(!late && (r[0].0.alpha - 0.0).abs() < 1e-6, "{r:?}");
        // Off the canvas, half a square of it read: half as opaque.
        d.discard_pending("Hide");
        let edge = Sampler::of([0, 10], carried, Shown::default());
        d.samplers = vec![edge];
        let (r, _) = d.sampler_readings(3, false);
        assert!((r[0].0.alpha - 2.0 / 3.0).abs() < 1e-3 && close(&Read { alpha: r[0].0.alpha, ..white }, &r[0].0));
        // Kept in the file: changed since, the image has changes to save.
        assert!(d.samplers_saved.is_empty());
        d.samplers_saved = d.pinned();
        assert_eq!(d.samplers_saved, vec![opal_io::native::Pinned { at: [0, 10], shown: "srgb".into() }]);
        d.samplers[0].shown = Shown::Hex;
        assert!(d.is_dirty());
        d.set_pinned(&[opal_io::native::Pinned { at: [1, 2], shown: "no such".into() }]);
        assert_eq!((d.sampler_pixels(), d.samplers[0].shown), (vec![[1, 2]], Shown::default()));
    }

    #[test]
    fn the_ways_a_colour_is_shown_are_listed_alike_wherever_they_are_offered() {
        let ids: Vec<&str> = Shown::all().map(Shown::id).collect();
        assert_eq!(ids, SHOWN_IDS);
        assert_eq!(SHOWN.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
        assert_eq!(
            SHOWN.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
            Shown::all().map(Shown::name).collect::<Vec<_>>()
        );
        assert!(SIZES.iter().all(|(id, _)| id.parse::<i64>().is_ok_and(|n| n % 2 == 1)));
        assert_eq!(SIZES.iter().map(|(id, _)| *id).collect::<Vec<_>>(), SIZE_IDS);
    }
}
