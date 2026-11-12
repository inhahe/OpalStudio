//! Copy, Paste and Transform: what the commands do.

use crate::clipboard::{Copied, Pasted, Read, ReadResult};
use crate::doc::DocId;
use crate::prefs;
use crate::state::{AppState, MessageKind, PasteAnswer, Reading};
use crate::transform::{Kind, PasteAs, PrintSize};
use eframe::egui;
use opal_core::composite::composite_within;
use opal_core::document::{Document, IdGen, Layer, Position};
use opal_core::geom::Affine;
use serde_json::json;
use std::sync::Arc;

/// A copy on its way to the clipboard.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Copying {
    /// The API call it answers, if any.
    pub answer: Option<u64>,
    /// The cut it is part of, if it is: the document by its number, the history state of
    /// the cut, and the one it was made from. Taken back should the copy not reach the
    /// clipboard.
    pub cut: Option<(DocId, opal_core::history::NodeId, opal_core::history::NodeId)>,
}

impl AppState {
    /// Copies the selected layers (or, `merged`, everything visible) within the canvas,
    /// trimmed to their content. It is put on the clipboard off the interface thread (see
    /// [`AppState::poll_copies`]).
    pub(crate) fn copy(&mut self, merged: bool, ctx: &egui::Context) {
        self.copy_within(merged, crate::memory::Allowance::now(), None, ctx);
    }

    /// [`copy`](Self::copy), or why not if it would need more than `memory` (or more than
    /// the clipboard holds), answering API call `answer`, if given, once the copy is on the
    /// clipboard. Whether a copy was started (if not, it was refused, and said so).
    pub(crate) fn copy_within(
        &mut self,
        merged: bool,
        mut memory: crate::memory::Allowance,
        answer: Option<u64>,
        ctx: &egui::Context,
    ) -> bool {
        let d = self.active().expect("available");
        let doc = d.doc();
        let canvas = doc.canvas.rect();
        let layers: Vec<Arc<Layer>> = if merged { doc.layers.clone() } else { d.selected_as_shown() };
        if let Err(why) = memory.take_tiles(opal_core::composite::composite_cost(&layers, Some(&canvas))) {
            self.message(MessageKind::Info, "Copy", why);
            return false;
        }
        let flat = composite_within(&layers, &canvas);
        // Within the selection, as much of each pixel as it is selected.
        let flat = match &doc.selection {
            Some(s) => opal_core::mask::keep_selected(&flat, [0, 0], s.mask()),
            None => flat,
        };
        let Some(bounds) = flat.content_bounds().map(|b| b.intersect(&canvas)).filter(|b| !b.is_empty()) else {
            let what = match (merged, doc.selection.is_some()) {
                (true, false) => "Nothing is visible",
                (true, true) => "Nothing visible is selected",
                (false, false) => "The selected layers show nothing",
                (false, true) => "The selected layers show nothing selected",
            };
            let faint = if merged { String::new() } else { crate::select::faint_note(d) };
            let why = format!("{what} within the canvas, so there is nothing to copy.{faint}");
            self.message(MessageKind::Info, "Copy", why);
            return false;
        };
        let (w, h) = (bounds.width() as u64, bounds.height() as u64);
        let copies = crate::memory::Need::at_once(w * h * crate::clipboard::COPY_BYTES);
        if let Err(why) = crate::clipboard::check_copy(w, h).and_then(|()| memory.take(copies)) {
            self.message(MessageKind::Info, "Copy", why);
            return false;
        }
        let pixels = flat.read_rect(&bounds);
        let from = bounds.translate(-canvas.x0, -canvas.y0);
        let resolution = doc.resolution;
        // Each copy's result comes back in the order they were started.
        self.copying.push_back(Copying { answer, cut: None });
        // The cap on tiles is kept until the copy is on the clipboard.
        self.clipboard.write_image(Copied { from, pixels, resolution, _hold: memory.into_hold() }, ctx);
        true
    }

    /// Says how the copies put on the clipboard since the last frame went: to the API call
    /// a copy answers, if it does (see [`AppState::answers`]). A cut whose copy did not reach
    /// the clipboard is taken back, if nothing has been done since.
    pub(crate) fn poll_copies(&mut self) {
        for result in self.clipboard.written() {
            let Copying { answer, cut } = self.copying.pop_front().unwrap_or_default();
            match result {
                Ok(from) => {
                    self.notify(format!("Copied {} × {} pixels.", from.width(), from.height()));
                    if let Some(n) = answer {
                        self.answered
                            .push((n, Ok(serde_json::json!({ "width": from.width(), "height": from.height() }))));
                    }
                }
                Err(e) => {
                    let e = e.trim_end_matches('.');
                    let said = match cut.map(|(doc, state, from)| self.doc_mut(doc).map(|d| d.take_back(state, from))) {
                        // Not a cut, or one on an image closed since.
                        None | Some(None) => format!("{e}."),
                        // Taken back: undone already, or now.
                        Some(Some(true)) => format!("{e}, so nothing was cut."),
                        Some(Some(false)) => format!("{e}. What was cut stays erased: undo it to get it back."),
                    };
                    match answer {
                        Some(n) => self.answered.push((n, Err(format!("Could not copy: {said}")))),
                        None => self.message(MessageKind::Error, "Could not copy", said),
                    }
                }
            }
        }
    }

    /// Pastes the clipboard's image: into the transform box on the active document, or as a
    /// new image when none is open. `in_place` puts it where it was copied from. Reading
    /// the system clipboard runs on a worker thread; the paste lands when it is done (see
    /// [`AppState::poll_paste`]), on the document active when it was asked for.
    pub(crate) fn paste(&mut self, in_place: bool, ctx: &egui::Context) {
        self.paste_answering(in_place, self.active_doc, None, ctx);
    }

    /// [`paste`](Self::paste) into document `target` (a new image if none), for the API call
    /// `answer` names, if any: once read, the paste is placed where the call says and
    /// applied, and the call answered (or told why not).
    pub(crate) fn paste_answering(
        &mut self,
        in_place: bool,
        target: Option<DocId>,
        answer: Option<PasteAnswer>,
        ctx: &egui::Context,
    ) {
        // This paste replaces one still being read, and what that one said.
        if let Some(r) = self.reading.take() {
            self.take_down(&r.notice);
            if let Some(a) = r.answer {
                self.answered.push((a.n, Err("Another paste was asked for before this one was placed.".into())));
            }
        }
        let (rx, result) = match self.clipboard.read_image() {
            // Read at once (a copy made here): placed now, unless it must wait as one read
            // later would (see `poll_paste`).
            Read::Ready(result) => (std::sync::mpsc::channel().1, Some(*result)),
            Read::Later(work) => {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    // The receiver may be gone (another paste replaced this one); then the
                    // result is simply dropped.
                    let _ = tx.send(work());
                    ctx.request_repaint();
                });
                (rx, None)
            }
        };
        let ready = result.is_some();
        let d = target.and_then(|t| self.doc(t));
        let notice = if ready { String::new() } else { self.reading_notice().to_string() };
        self.reading = Some(Reading {
            rx,
            result,
            in_place,
            target,
            box_then: d.and_then(|d| d.transform.as_ref()).map(|s| s.serial),
            crop_then: d.and_then(|d| d.crop),
            notice: notice.clone(),
            answer,
        });
        if ready {
            self.poll_paste(false);
        } else {
            self.notify(notice);
        }
    }

    /// Places a paste whose clipboard reading has come back, unless the user is in the
    /// middle of something it would end: a dialog, a drag (`pointer_held`: it is placed
    /// when the button is let go), or a box opened on its document after it was asked for.
    pub(crate) fn poll_paste(&mut self, pointer_held: bool) {
        let reading = self.reading_notice();
        let Some(r) = &mut self.reading else { return };
        if r.result.is_none() {
            match r.rx.try_recv() {
                Ok(result) => r.result = Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Kept true as boxes open and close.
                    if r.notice != reading {
                        r.notice = reading.to_string();
                        self.notify(reading);
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    r.result = Some(Err("reading the clipboard failed".into()))
                }
            }
        }
        if pointer_held {
            return;
        }
        if let Some(until) = self.paste_waits() {
            // What it waits for takes Esc; once placed, its own box does, and cancels it.
            let text = format!("The paste will be placed when {until}.");
            let r = self.reading.as_mut().expect("checked above");
            if r.notice != text {
                r.notice = text.clone();
                self.notify(text);
            }
            return;
        }
        let r = self.reading.take().expect("checked above");
        self.take_down(&r.notice);
        self.paste_read(r.result.expect("received above"), r.in_place, r.target, r.answer);
    }

    /// What the status bar says while the clipboard is read: that Esc cancels the paste,
    /// unless a box would take Esc first (see [`box_takes_escape`](Self::box_takes_escape)).
    fn reading_notice(&self) -> &'static str {
        if self.box_takes_escape() {
            "Reading the clipboard…"
        } else {
            "Reading the clipboard… (Esc cancels the paste)"
        }
    }

    /// Whether Esc goes to a box on the image shown (a transform box, or the crop box while
    /// cropping) rather than to a paste.
    pub(crate) fn box_takes_escape(&self) -> bool {
        let crop_tool = self.tool() == crate::tools::ToolId::Crop;
        self.active().is_some_and(|d| d.transform.is_some() || (crop_tool && d.crop.is_some()) || d.outlining.is_some())
    }

    /// Gives up the paste being read or waiting to be placed, if there is one.
    pub(crate) fn cancel_paste(&mut self) {
        if let Some(r) = self.reading.take() {
            self.take_down(&r.notice);
            self.notify("Paste cancelled.");
            if let Some(a) = r.answer {
                self.answered.push((a.n, Err("The paste was cancelled.".into())));
            }
        }
    }

    /// Clears the status bar if it still shows `notice`.
    fn take_down(&mut self, notice: &str) {
        if self.notice.as_ref().is_some_and(|(text, _)| text == notice) {
            self.notice = None;
        }
    }

    /// What a paste that has been read waits for, if it must wait.
    fn paste_waits(&self) -> Option<&'static str> {
        let r = self.reading.as_ref()?;
        if self.dialog_open() {
            return Some("the dialog closes");
        }
        let d = r.target.and_then(|t| self.doc(t))?;
        if d.busy.is_some() {
            Some("the edit under way on the image finishes")
        } else if d.transform.as_ref().is_some_and(|s| Some(s.serial) != r.box_then) {
            Some("the transform box closes")
        } else if d.crop.is_some() && d.crop != r.crop_then {
            Some("you crop or leave the Crop tool")
        } else {
            None
        }
    }

    /// Pastes what reading the clipboard gave, into document `target` (a new image if it
    /// was none); for the API call `answer` names, placed where it says, applied, and
    /// answered. What a paste would tell in a dialog, the call is told instead.
    fn paste_read(&mut self, result: ReadResult, in_place: bool, target: Option<DocId>, answer: Option<PasteAnswer>) {
        let n = answer.as_ref().map(|a| a.n);
        let refuse = |st: &mut AppState, kind: MessageKind, title: &str, body: String| match n {
            Some(n) => st.answered.push((n, Err(format!("{title}: {body}")))),
            None => st.message(kind, title, body),
        };
        // Nothing about the clipboard matters to a paste whose image is gone.
        if target.is_some_and(|id| self.doc(id).is_none()) {
            let why = "The image the paste was for has been closed, so nothing was pasted.";
            return match n {
                Some(n) => self.answered.push((n, Err(why.into()))),
                None => self.notify(why),
            };
        }
        let pasted = match result {
            Ok(Some(p)) => p,
            Ok(None) => {
                let why = if self.clipboard.has_text() {
                    "The clipboard holds text, not an image. (Pasting text as a text layer comes with \
                     text layers.)"
                } else {
                    "The clipboard holds no image. Copy one first: here, in another program, or an image \
                     file in Explorer."
                };
                return refuse(self, MessageKind::Info, "Paste", why.into());
            }
            Err(e) => return refuse(self, MessageKind::Error, "Could not paste", e),
        };
        if n.is_none() {
            for w in &pasted.warnings {
                self.message(MessageKind::Warning, "Pasted", w.clone());
            }
        }
        let notes: Vec<String> = pasted.warnings.iter().map(|w| format!("Pasted: {w}")).collect();
        let Some(target) = target else {
            self.new_image_from(&pasted);
            if let Some(n) = n {
                let id = self.active_doc.expect("just made");
                self.answered.push((n, Ok(crate::api::with_notes(json!({ "document": id.0 }), notes))));
            }
            return;
        };
        self.show_doc(target);
        self.begin_pasted(&pasted, in_place, PasteAs { kind: Kind::Paste, name: "Pasted image".into(), print: None });
        if let Some(a) = answer {
            let d = self.doc_mut(target).expect("shown above");
            let before = std::mem::replace(&mut d.groups, a.groups);
            let placed = self.apply_box(&a.place, a.into_active);
            if let Some(d) = self.doc_mut(target) {
                d.groups = before;
            }
            self.answered.push((a.n, placed.map(|v| crate::api::with_notes(v, notes))));
        }
    }

    /// Opens the transform box on the active document with `pasted` on a new layer above
    /// the active layer, as `how` says: where it was copied from (`in_place`, which a file
    /// placed at a print size cannot be), or else centered on the part of the canvas in
    /// view, at the size `how.print` says or else its own unless that is larger than the
    /// canvas (and the preference fits it).
    pub(crate) fn begin_pasted(&mut self, pasted: &Pasted, in_place: bool, how: PasteAs) {
        assert!(!in_place || how.print.is_none(), "a paste in place is put where it was, unscaled");
        let sampling = prefs::sampling(&self.prefs, "transform");
        let fit_large = self.prefs.bool("transform.fit_large");
        let d = self.active_mut().expect("checked above");
        let canvas = d.doc().canvas.rect();
        let (w, h) = (f64::from(pasted.width), f64::from(pasted.height));
        let place = match (in_place, pasted.from) {
            (true, Some(r)) => Affine::translate((canvas.x0 + r.x0) as f64, (canvas.y0 + r.y0) as f64),
            (true, None) => Affine::translate(canvas.x0 as f64, canvas.y0 as f64),
            (false, _) => {
                // Centered on the part of the canvas in view.
                let v = d.view;
                let a = v.to_canvas(eframe::egui::Vec2::ZERO);
                let b = v.to_canvas(v.viewport);
                let seen = opal_core::geom::IRect::new(a.x as i64, a.y as i64, b.x.ceil() as i64, b.y.ceil() as i64);
                let seen = seen.intersect(&canvas);
                let [cx, cy] = if seen.is_empty() { canvas.to_f().center() } else { seen.to_f().center() };
                let [kx, ky] = match how.print {
                    Some(p) if p.on => p.print,
                    Some(p) => p.pixels,
                    None => [fit_scale(w, h, &canvas, fit_large); 2],
                };
                let (pw, ph) = (w * kx, h * ky);
                if [kx, ky] == [1.0, 1.0] {
                    // Whole pixels, so it stays crisp.
                    Affine::translate((cx - pw * 0.5).round(), (cy - ph * 0.5).round())
                } else {
                    Affine::scale(kx, ky).then(Affine::translate(cx - pw * 0.5, cy - ph * 0.5))
                }
            }
        };
        let active = d.active_layer().and_then(|a| d.doc().layer(a));
        let into = active.filter(|l| !l.is_group() && crate::commands::locked_for(d.doc(), l).is_none()).map(|l| l.id);
        let position = AppState::above_active(d);
        d.begin_paste(pasted, place, sampling, position, into, how);
    }

    /// Opens the transform box on the active document with the image file at `path` on a
    /// new layer called `name` (or after the file), placed as a paste is: what Place does
    /// once a file is chosen. At its print size if `print_size` (scaled by its resolution
    /// against the image's, each way), else pixel for pixel; the box's button switches
    /// between the two. A file that does not say its resolution, or says one the image's
    /// is too far from to scale by, is placed pixel for pixel. What the file's reading said
    /// comes back as warnings.
    pub(crate) fn place_file(
        &mut self,
        path: &std::path::Path,
        name: Option<&str>,
        print_size: bool,
    ) -> Result<Vec<String>, String> {
        let imported = opal_io::image_files::import(path, crate::memory::free()).map_err(|e| e.to_string())?;
        let pasted = Pasted {
            width: imported.width,
            height: imported.height,
            image: imported.image,
            from: None,
            resolution: imported.resolution,
            warnings: imported.warnings,
        };
        let mut warnings = pasted.warnings.clone();
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        let name = name.map(str::to_owned).or(stem).unwrap_or_else(|| "Placed image".to_owned());
        let fit_large = self.prefs.bool("transform.fit_large");
        let d = self.active().expect("Place needs an image");
        let (ppi, canvas) = (d.doc().resolution, d.doc().canvas.rect());
        let (w, h) = (f64::from(pasted.width), f64::from(pasted.height));
        let factor =
            pasted.ppi().map(|[x, y]| [ppi / x, ppi / y]).filter(|f| f.iter().any(|&k| (k - 1.0).abs() > 1e-9));
        let print = factor.and_then(|[fx, fy]| {
            let s = fit_scale(w * fx, h * fy, &canvas, fit_large);
            let print = [fx * s, fy * s];
            let pixels = [fit_scale(w, h, &canvas, fit_large); 2];
            // Within what an image's placing may stretch or squash it by, both ways.
            let max = opal_core::document::MAX_SCALE;
            let usable = |k: [f64; 2]| k.iter().all(|&v| (1.0 / max..=max).contains(&v));
            if usable(print) && usable(pixels) {
                Some(PrintSize { print, pixels, on: print_size })
            } else {
                warnings.push(
                    "Its resolution is too far from the image's to scale it by, so it is placed pixel for pixel."
                        .to_owned(),
                );
                None
            }
        });
        self.begin_pasted(&pasted, false, PasteAs { kind: Kind::Place, name, print });
        Ok(warnings)
    }

    /// Asks for an image file and places it (see [`place_file`](Self::place_file)).
    pub(crate) fn place_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Place")
            .add_filter("PNG and JPEG images", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return;
        };
        match self.place_file(&path, None, self.prefs.bool("transform.place_print_size")) {
            Ok(warnings) => {
                for w in warnings {
                    self.message(MessageKind::Warning, "Placed", w);
                }
            }
            Err(e) => self.message(MessageKind::Error, "Could not place the image", e),
        }
    }

    /// Opens the clipboard's image as a new image of its own size.
    fn new_image_from(&mut self, pasted: &Pasted) {
        let mut ids = IdGen::default();
        let mut doc = Document::new(pasted.width, pasted.height);
        doc.resolution = pasted.ppi().map_or(self.default_resolution(), |[across, _]| across);
        let layer = Layer::new_raster(ids.layer(), ids.object(), "Pasted image", pasted.image.clone());
        doc.insert(Position { parent: None, index: 0 }, layer).expect("an empty document takes a layer");
        self.new_untitled(doc, ids, "Paste");
    }

    /// Opens the transform box on the selected layers.
    /// Transform Again: what the last transform box applied to layers did (`last_transform`),
    /// done to the selected layers now, as one history state; with `copy`, to copies of them
    /// made first, each just above its layer, which become the ones selected, so that pressed
    /// again, the next copies go on from them: a row, or a ring.
    pub(crate) fn transform_again(&mut self, copy: bool) {
        let sampling = prefs::sampling(&self.prefs, "transform");
        let label = if copy { "Transform again as a copy" } else { "Transform again" };
        let mut made = std::collections::HashSet::new();
        if copy {
            let copies = self.duplicate_layers();
            let Some(&first) = copies.first() else { return };
            let d = self.active_mut().expect("available");
            made.insert(d.history.current());
            d.select_only(first);
            for &c in &copies[1..] {
                d.toggle_selected(c);
            }
        }
        let d = self.active_mut().expect("available");
        let delta = d.last_transform.expect("available");
        if !d.begin_transform(sampling) {
            return;
        }
        let m = d.transform.as_ref().expect("begun").start.then(delta);
        if let Some(why) = d.box_refused(&m) {
            d.cancel_transform();
            return self.message(MessageKind::Info, label, why);
        }
        d.set_box(m, None);
        if let Some(p) = d.pending.as_mut() {
            p.label = label.to_owned();
        }
        d.commit_pending();
        made.insert(d.history.current());
        if made.len() > 1 {
            d.join_states(&made, label);
        }
    }

    pub(crate) fn transform(&mut self) {
        let sampling = prefs::sampling(&self.prefs, "transform");
        let d = self.active_mut().expect("available");
        // (Refused while an edit is under way, the status bar says why.)
        if !d.begin_transform(sampling) && d.busy.is_none() {
            self.message(MessageKind::Info, "Transform", "The selected layers hold nothing to transform.");
        }
    }
}

/// How much a paste `w × h` pixels is scaled by to fit `canvas` when it is larger and
/// `fit_large` says so (else 1): evenly, as large as fits.
fn fit_scale(w: f64, h: f64, canvas: &opal_core::geom::IRect, fit_large: bool) -> f64 {
    let (cw, ch) = (canvas.width() as f64, canvas.height() as f64);
    if fit_large && (w > cw || h > ch) { (cw / w).min(ch / h) } else { 1.0 }
}

/// Image files for tests.
#[cfg(test)]
pub(crate) mod testing {
    use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};

    /// Writes a white JPEG `w × h` pixels to `path`, recording `ppi` pixels per inch across
    /// and down if given, none if not.
    pub fn white_jpeg(path: &std::path::Path, w: u32, h: u32, ppi: Option<[u16; 2]>) {
        let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut jpeg = JpegEncoder::new_with_quality(file, 100);
        jpeg.set_pixel_density(match ppi {
            Some([x, y]) => PixelDensity { density: (x, y), unit: PixelDensityUnit::Inches },
            None => PixelDensity { density: (1, 1), unit: PixelDensityUnit::PixelAspectRatio },
        });
        let white = image::RgbImage::from_pixel(w, h, image::Rgb([255; 3]));
        jpeg.encode_image(&white).unwrap();
    }
}
