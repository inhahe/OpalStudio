//! Copy, Paste and Transform: what the commands do.

use crate::clipboard::{Copied, Pasted, Read, ReadResult};
use crate::doc::DocId;
use crate::prefs;
use crate::state::{AppState, MessageKind, PasteAnswer, Reading};
use crate::transform::Kind;
use eframe::egui;
use opal_core::composite::composite_within;
use opal_core::document::{Document, IdGen, Layer, Position};
use opal_core::geom::Affine;
use serde_json::json;
use std::sync::Arc;

impl AppState {
    /// Copies the selected layers (or, `merged`, everything visible) within the canvas,
    /// trimmed to their content. It is put on the clipboard off the interface thread (see
    /// [`AppState::poll_copies`]).
    pub(crate) fn copy(&mut self, merged: bool, ctx: &egui::Context) {
        self.copy_within(merged, crate::memory::Allowance::now(), None, ctx);
    }

    /// [`copy`](Self::copy), answering API call `n` once the copy is on the clipboard;
    /// whether a copy was started (if not, it was refused, and said so).
    pub(crate) fn copy_answering(&mut self, merged: bool, n: u64, ctx: &egui::Context) -> bool {
        let before = self.copies_answered.len();
        self.copy_within(merged, crate::memory::Allowance::now(), Some(n), ctx);
        self.copies_answered.len() > before
    }

    /// [`copy`](Self::copy), or why not if it would need more than `memory` (or more than
    /// the clipboard holds).
    pub(crate) fn copy_within(
        &mut self,
        merged: bool,
        mut memory: crate::memory::Allowance,
        answer: Option<u64>,
        ctx: &egui::Context,
    ) {
        let d = self.active().expect("available");
        let doc = d.doc();
        let canvas = doc.canvas.rect();
        let layers: Vec<Arc<Layer>> = if merged {
            doc.layers.clone()
        } else {
            // What the selected layers look like, hidden or not: they were chosen to copy.
            d.selection()
                .iter()
                .filter_map(|&l| doc.layer(l))
                .map(|l| Arc::new(Layer { visible: true, ..l.clone() }))
                .collect()
        };
        if let Err(why) = memory.take_tiles(opal_core::composite::composite_cost(&layers, Some(&canvas))) {
            return self.message(MessageKind::Info, "Copy", why);
        }
        let flat = composite_within(&layers, &canvas);
        let Some(bounds) = flat.content_bounds().map(|b| b.intersect(&canvas)).filter(|b| !b.is_empty()) else {
            let what = if merged { "Nothing is visible" } else { "The selected layers show nothing" };
            return self.message(
                MessageKind::Info,
                "Copy",
                format!("{what} within the canvas, so there is nothing to copy."),
            );
        };
        let (w, h) = (bounds.width() as u64, bounds.height() as u64);
        let copies = crate::memory::Need::at_once(w * h * crate::clipboard::COPY_BYTES);
        if let Err(why) = crate::clipboard::check_copy(w, h).and_then(|()| memory.take(copies)) {
            return self.message(MessageKind::Info, "Copy", why);
        }
        let pixels = flat.read_rect(&bounds);
        let from = bounds.translate(-canvas.x0, -canvas.y0);
        // Each copy's result comes back in the order they were started.
        self.copies_answered.push_back(answer);
        // The cap on tiles is kept until the copy is on the clipboard.
        self.clipboard.write_image(Copied { from, pixels, _hold: memory.into_hold() }, ctx);
    }

    /// Says how the copies put on the clipboard since the last frame went: to the API call
    /// a copy answers, if it does (see [`AppState::answers`]).
    pub(crate) fn poll_copies(&mut self) {
        for result in self.clipboard.written() {
            let answer = self.copies_answered.pop_front().flatten();
            match (result, answer) {
                (Ok(from), answer) => {
                    self.notify(format!("Copied {} × {} pixels.", from.width(), from.height()));
                    if let Some(n) = answer {
                        self.answered
                            .push((n, Ok(serde_json::json!({ "width": from.width(), "height": from.height() }))));
                    }
                }
                (Err(e), Some(n)) => self.answered.push((n, Err(format!("Could not copy: {e}")))),
                (Err(e), None) => self.message(MessageKind::Error, "Could not copy", e),
            }
        }
    }

    /// Pastes the clipboard's image: into the transform box on the active document, or as a
    /// new image when none is open. `in_place` puts it where it was copied from. Reading
    /// the system clipboard runs on a worker thread; the paste lands when it is done (see
    /// [`AppState::poll_paste`]), on the document active when it was asked for.
    pub(crate) fn paste(&mut self, in_place: bool, ctx: &egui::Context) {
        self.paste_answering(in_place, None, ctx);
    }

    /// [`paste`](Self::paste), for the API call `answer` names, if any: once read, the paste
    /// is placed where the call says and applied, and the call answered (or told why not).
    pub(crate) fn paste_answering(&mut self, in_place: bool, answer: Option<PasteAnswer>, ctx: &egui::Context) {
        let target = self.active_doc;
        // This paste replaces one still being read, and what that one said.
        if let Some(r) = self.reading.take() {
            self.take_down(&r.notice);
            if let Some(a) = r.answer {
                self.answered.push((a.n, Err("Another paste was asked for before this one was placed.".into())));
            }
        }
        match self.clipboard.read_image() {
            Read::Ready(result) => self.paste_read(*result, in_place, target, answer),
            Read::Later(work) => {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    // The receiver may be gone (another paste replaced this one); then the
                    // result is simply dropped.
                    let _ = tx.send(work());
                    ctx.request_repaint();
                });
                let d = target.and_then(|t| self.doc(t));
                let notice = self.reading_notice().to_string();
                self.reading = Some(Reading {
                    rx,
                    result: None,
                    in_place,
                    target,
                    box_then: d.and_then(|d| d.transform.as_ref()).map(|s| s.serial),
                    crop_then: d.and_then(|d| d.crop),
                    notice: notice.clone(),
                    answer,
                });
                self.notify(notice);
            }
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
        let crop_tool = self.tool == crate::tools::ToolId::Crop;
        self.active().is_some_and(|d| d.transform.is_some() || (crop_tool && d.crop.is_some()))
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
        if d.transform.as_ref().is_some_and(|s| Some(s.serial) != r.box_then) {
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
        self.begin_pasted(&pasted, in_place, Kind::Paste, "Pasted image");
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

    /// Opens the transform box on the active document with `pasted` on a new layer called
    /// `name`, above the active layer, as a session of `kind`: where it was copied from
    /// (`in_place`), or else centered on the part of the canvas in view, at its own size
    /// unless that is larger than the canvas (and the preference fits it).
    pub(crate) fn begin_pasted(&mut self, pasted: &Pasted, in_place: bool, kind: Kind, name: &str) {
        let sampling = prefs::sampling(&self.prefs, "transform");
        let fit_large = self.prefs.bool("transform.fit_large");
        let d = self.active_mut().expect("checked above");
        let canvas = d.doc().canvas.rect();
        let (w, h) = (f64::from(pasted.width), f64::from(pasted.height));
        let place = match (in_place, pasted.from) {
            (true, Some(r)) => Affine::translate((canvas.x0 + r.x0) as f64, (canvas.y0 + r.y0) as f64),
            (true, None) => Affine::translate(canvas.x0 as f64, canvas.y0 as f64),
            (false, _) => {
                // Centered on the part of the canvas in view, at its own size unless that is
                // larger than the canvas.
                let v = d.view;
                let a = v.to_canvas(eframe::egui::Vec2::ZERO);
                let b = v.to_canvas(v.viewport);
                let seen = opal_core::geom::IRect::new(a.x as i64, a.y as i64, b.x.ceil() as i64, b.y.ceil() as i64);
                let seen = seen.intersect(&canvas);
                let [cx, cy] = if seen.is_empty() { canvas.to_f().center() } else { seen.to_f().center() };
                let (cw, ch) = (canvas.width() as f64, canvas.height() as f64);
                let s = if fit_large && (w > cw || h > ch) { (cw / w).min(ch / h) } else { 1.0 };
                if s == 1.0 {
                    // Whole pixels, so it stays crisp.
                    Affine::translate((cx - w * 0.5).round(), (cy - h * 0.5).round())
                } else {
                    Affine::scale(s, s).then(Affine::translate(cx - w * s * 0.5, cy - h * s * 0.5))
                }
            }
        };
        let active = d.active_layer().and_then(|a| d.doc().layer(a));
        let into = active.filter(|l| !l.is_group() && !l.locked).map(|l| l.id);
        let position = d
            .active_layer()
            .and_then(|a| d.doc().position(a))
            .map(|p| Position { parent: p.parent, index: p.index + 1 })
            .unwrap_or(Position { parent: None, index: d.doc().layers.len() });
        d.begin_paste(pasted, place, sampling, position, into, kind, name);
    }

    /// Opens the transform box on the active document with the image file at `path` on a
    /// new layer called `name` (or after the file), placed as a paste is: what Place does
    /// once a file is chosen. What the file's reading said comes back as warnings.
    pub(crate) fn place_file(&mut self, path: &std::path::Path, name: Option<&str>) -> Result<Vec<String>, String> {
        let imported = opal_io::image_files::import(path, crate::memory::free()).map_err(|e| e.to_string())?;
        let pasted = Pasted {
            width: imported.width,
            height: imported.height,
            image: imported.image,
            from: None,
            warnings: imported.warnings,
        };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        let name = name.map(str::to_owned).or(stem).unwrap_or_else(|| "Placed image".to_owned());
        self.begin_pasted(&pasted, false, Kind::Place, &name);
        Ok(pasted.warnings)
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
        match self.place_file(&path, None) {
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
        let layer = Layer::new_raster(ids.layer(), ids.object(), "Pasted image", pasted.image.clone());
        doc.insert(Position { parent: None, index: 0 }, layer).expect("an empty document takes a layer");
        self.new_untitled(doc, ids, "Paste");
    }

    /// Opens the transform box on the selected layers.
    pub(crate) fn transform(&mut self) {
        let sampling = prefs::sampling(&self.prefs, "transform");
        let d = self.active_mut().expect("available");
        if !d.begin_transform(sampling) {
            self.message(MessageKind::Info, "Transform", "The selected layers hold nothing to transform.");
        }
    }
}
