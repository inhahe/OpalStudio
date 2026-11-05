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
    /// trimmed to their content.
    pub(crate) fn copy(&mut self, merged: bool) {
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
        match self.clipboard.write_image(Copied { from, pixels }) {
            Ok(()) => self.notify(format!("Copied {} × {} pixels.", bounds.width(), bounds.height())),
            Err(e) => self.message(MessageKind::Error, "Could not copy", e),
        }
    }

    /// Pastes the clipboard's image: into the transform box on the active document, or as a
    /// new image when none is open. `in_place` puts it where it was copied from. Reading
    /// the system clipboard runs on a worker thread; the paste lands when it is done (see
    /// [`AppState::poll_paste`]), on the document active when it was asked for.
    pub(crate) fn paste(&mut self, in_place: bool, ctx: &egui::Context) {
        let target = self.active_doc;
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
                self.reading = Some(Reading { rx, in_place, target });
                self.notify("Reading the clipboard…");
            }
        }
    }

    /// Finishes a paste whose clipboard reading has come back.
    pub(crate) fn poll_paste(&mut self) {
        let Some(r) = &self.reading else { return };
        let result = match r.rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("reading the clipboard failed".into()),
        };
        let r = self.reading.take().expect("checked above");
        self.notice = None;
        self.paste_read(result, r.in_place, r.target);
    }

    /// Pastes what reading the clipboard gave, into document `target` (or a new image if it
    /// is gone, or was none).
    fn paste_read(&mut self, result: ReadResult, in_place: bool, target: Option<DocId>) {
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
        let Some(target) = target.filter(|&id| self.doc(id).is_some()) else {
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
