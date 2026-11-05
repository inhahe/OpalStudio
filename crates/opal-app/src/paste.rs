//! Copy, Paste and Transform: what the commands do.

use crate::clipboard::{Copied, Pasted, Read, ReadResult};
use crate::doc::DocId;
use crate::prefs;
use crate::state::{AppState, MessageKind, Reading};
use eframe::egui;
use opal_core::composite::composite_within;
use opal_core::document::{Document, IdGen, Layer, Position};
use opal_core::geom::Affine;
use std::sync::Arc;

impl AppState {
    /// Copies the selected layers (or, `merged`, everything visible) within the canvas,
    /// trimmed to their content. It is put on the clipboard off the interface thread (see
    /// [`AppState::poll_copies`]).
    pub(crate) fn copy(&mut self, merged: bool, ctx: &egui::Context) {
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
        let flat = composite_within(&layers, &canvas);
        let Some(bounds) = flat.content_bounds().map(|b| b.intersect(&canvas)).filter(|b| !b.is_empty()) else {
            let what = if merged { "Nothing is visible" } else { "The selected layers show nothing" };
            return self.message(
                MessageKind::Info,
                "Copy",
                format!("{what} within the canvas, so there is nothing to copy."),
            );
        };
        let pixels = flat.read_rect(&bounds);
        let from = bounds.translate(-canvas.x0, -canvas.y0);
        self.clipboard.write_image(Copied { from, pixels }, ctx);
    }

    /// Says how the copies put on the clipboard since the last frame went.
    pub(crate) fn poll_copies(&mut self) {
        for result in self.clipboard.written() {
            match result {
                Ok(from) => self.notify(format!("Copied {} × {} pixels.", from.width(), from.height())),
                Err(e) => self.message(MessageKind::Error, "Could not copy", e),
            }
        }
    }

    /// Pastes the clipboard's image: into the transform box on the active document, or as a
    /// new image when none is open. `in_place` puts it where it was copied from. Reading
    /// the system clipboard runs on a worker thread; the paste lands when it is done (see
    /// [`AppState::poll_paste`]), on the document active when it was asked for.
    pub(crate) fn paste(&mut self, in_place: bool, ctx: &egui::Context) {
        let target = self.active_doc;
        // This paste replaces one still being read, and what that one said.
        if let Some(r) = self.reading.take() {
            self.take_down(&r.notice);
        }
        match self.clipboard.read_image() {
            Read::Ready(result) => self.paste_read(*result, in_place, target),
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
                let notice = "Reading the clipboard…".to_string();
                self.reading = Some(Reading {
                    rx,
                    result: None,
                    in_place,
                    target,
                    box_then: d.and_then(|d| d.transform.as_ref()).map(|s| s.serial),
                    crop_then: d.and_then(|d| d.crop),
                    notice: notice.clone(),
                });
                self.notify(notice);
            }
        }
    }

    /// Places a paste whose clipboard reading has come back, unless the user is in the
    /// middle of something it would end: a dialog, a drag (`pointer_held`: it is placed
    /// when the button is let go), or a box opened on its document after it was asked for.
    pub(crate) fn poll_paste(&mut self, pointer_held: bool) {
        let Some(r) = &mut self.reading else { return };
        if r.result.is_none() {
            r.result = Some(match r.rx.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("reading the clipboard failed".into()),
            });
        }
        if pointer_held {
            return;
        }
        if let Some(until) = self.paste_waits() {
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
        self.paste_read(r.result.expect("received above"), r.in_place, r.target);
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
    /// was none).
    fn paste_read(&mut self, result: ReadResult, in_place: bool, target: Option<DocId>) {
        // Nothing about the clipboard matters to a paste whose image is gone.
        if target.is_some_and(|id| self.doc(id).is_none()) {
            return self.notify("The image the paste was for has been closed, so nothing was pasted.");
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
                return self.message(MessageKind::Info, "Paste", why);
            }
            Err(e) => return self.message(MessageKind::Error, "Could not paste", e),
        };
        for w in &pasted.warnings {
            self.message(MessageKind::Warning, "Pasted", w.clone());
        }
        let Some(target) = target else {
            return self.new_image_from(&pasted);
        };
        self.show_doc(target);
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
        d.begin_paste(&pasted, place, sampling, position, into);
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
