//! What is selected (design.md, *Selections*): Select All, Deselect and Invert, and the
//! commands that act on the selection (Clear, Cut, Crop to Selection). Copy copies within it
//! (`paste.rs`); the canvas shows its outline (`canvas.rs`).

use crate::state::{AppState, MessageKind};
use opal_core::document::{Canvas, Document, LayerContent, LayerId, ObjectKind};
use opal_core::geom::IRect;
use opal_core::mask::{Combine, Extent, Mask, Shape, ShapeKind};
use opal_core::selection::{Selection, Step};
use std::sync::Arc;

/// A selection's outline: closed lines, in document coordinates (see `Mask::outline`).
pub type Outline = Arc<Vec<Vec<[f64; 2]>>>;

impl crate::doc::DocState {
    /// What is selected now, if anything.
    pub fn pixel_selection(&self) -> Option<&Arc<Selection>> {
        self.doc().selection.as_ref()
    }

    /// Makes `selection` what is selected (nothing, if it is `None` or selects nothing), as a
    /// history state labeled `label`.
    pub fn set_pixel_selection(&mut self, selection: Option<Selection>, label: &str) {
        let selection = selection.filter(|s| !s.is_empty()).map(Arc::new);
        let _ = self.apply(label, |doc, _| {
            doc.selection = selection;
            Ok::<(), ()>(())
        });
    }

    /// The outline of what is selected (the marching ants' path, in document coordinates),
    /// worked out once for each selection's mask and kept with it.
    pub fn selection_outline(&mut self) -> Option<Outline> {
        let mask = self.pixel_selection()?.mask().clone();
        if let Some((of, lines)) = &self.outline
            && Arc::ptr_eq(of, &mask)
        {
            return Some(lines.clone());
        }
        let lines = Arc::new(mask.outline(opal_core::mask::HALF));
        self.outline = Some((mask, lines.clone()));
        Some(lines)
    }
}

impl AppState {
    /// Selects the canvas, as Photoshop's does: what lies past it (layers have no edge) is not
    /// selected.
    pub(crate) fn select_all(&mut self) {
        let d = self.active_mut().expect("available");
        let r = d.doc().canvas.rect().to_f();
        let shape = Shape {
            kind: ShapeKind::Rectangle,
            center: r.center(),
            size: [r.width(), r.height()],
            angle: 0.0,
            smooth: false,
        };
        let s = Selection::new(Step::Shape { shape, combine: Combine::Replace });
        d.set_pixel_selection(Some(s), "Select all");
    }

    pub(crate) fn deselect(&mut self) {
        let d = self.active_mut().expect("available");
        d.set_pixel_selection(None, "Deselect");
    }

    /// Selects what is not selected, and nothing of what is (with everything selected,
    /// nothing is left selected).
    pub(crate) fn invert_selection(&mut self) {
        let d = self.active_mut().expect("available");
        let s = d.pixel_selection().expect("available").then(Step::Invert);
        d.set_pixel_selection(Some(s), "Invert selection");
    }

    /// Erases what is selected on the selected layers (as much of each pixel as it is
    /// selected), past the canvas too.
    pub(crate) fn clear_selected(&mut self) {
        self.clear_within("Clear", None);
    }

    /// Erases what is selected on the selected layers, as a history state labeled `label`
    /// (only within `area`, if given); whether anything was erased (if not, the user is told
    /// why).
    fn clear_within(&mut self, label: &str, area: Option<&IRect>) -> bool {
        let d = self.active_mut().expect("available");
        let mut mask = d.pixel_selection().expect("available").mask().clone();
        if let Some(r) = area {
            let r = r.to_f();
            let shape =
                Shape { kind: ShapeKind::Rectangle, center: r.center(), size: [r.width(), r.height()], angle: 0.0, smooth: false };
            mask = Arc::new(mask.combined(&Mask::of(&shape), Combine::Intersect));
        }
        let layers = d.selected_layers_and_inside();
        let mut changed = false;
        let result = d.apply(label, |doc, _| {
            changed = clear(doc, &layers, &mask);
            if changed { Ok(()) } else { Err(()) }
        });
        if result.is_err() {
            self.notify("Nothing selected on the selected layers had pixels to clear.");
        }
        changed
    }

    /// Copies what the selected layers show within the selection and the canvas, then erases
    /// that from them (what lies past the canvas, which Copy leaves out, stays). The copy
    /// reaches the clipboard off the interface thread; should it not, the erasing is taken
    /// back (see [`AppState::poll_copies`]). `answer`: the API call to answer once the copy is
    /// on the clipboard. Whether the copy was started (if not, it was refused, and said why).
    pub(crate) fn cut(&mut self, answer: Option<u64>, ctx: &eframe::egui::Context) -> bool {
        let before = self.copying.len();
        self.copy_within(false, crate::memory::Allowance::now(), answer, ctx);
        if self.copying.len() == before {
            return false;
        }
        let canvas = self.active().expect("available").doc().canvas.rect();
        if self.clear_within("Cut", Some(&canvas)) {
            let d = self.active().expect("available");
            let made = (d.id, d.history.current());
            self.copying.back_mut().expect("the copy just started").cut = Some(made);
        }
        true
    }

    /// Makes the canvas the rectangle round what is selected.
    pub(crate) fn crop_to_selection(&mut self) {
        let d = self.active_mut().expect("available");
        let Extent::Within(b) = d.pixel_selection().expect("available").mask().extent() else {
            return self.message(
                MessageKind::Info,
                "Crop to Selection",
                "The selection reaches past every edge: there is no rectangle round it.",
            );
        };
        let max = i64::from(crate::dialogs::MAX_SIDE);
        let canvas = Canvas { x: b.x0, y: b.y0, width: b.width().min(max) as u32, height: b.height().min(max) as u32 };
        if canvas == d.doc().canvas {
            return self.notify("The canvas is already the selection's rectangle.");
        }
        let result = d.apply("Crop to selection", |doc, _| {
            doc.canvas = canvas;
            Ok::<(), String>(())
        });
        self.report(result);
    }
}

/// Erases what `mask` selects from the pixel objects of `layers` in `doc`; whether anything
/// changed.
fn clear(doc: &mut Document, layers: &[LayerId], mask: &Mask) -> bool {
    let mut changed = false;
    for &id in layers {
        let Some(layer) = doc.layer_mut(id) else { continue };
        let LayerContent::Objects(objects) = &mut layer.content else { continue };
        for o in objects.iter_mut() {
            let ObjectKind::Pixels(p) = &mut o.kind else { continue };
            let cleared = opal_core::mask::clear_selected(&p.image, p.offset, mask);
            if !same_tiles(&cleared, &p.image) {
                p.image = Arc::new(cleared);
                changed = true;
            }
        }
    }
    changed
}

/// Whether two bitmaps hold the very same tiles.
fn same_tiles(a: &opal_core::raster::RasterImage, b: &opal_core::raster::RasterImage) -> bool {
    a.tile_count() == b.tile_count() && a.tiles().all(|(c, t)| b.tile(c).is_some_and(|u| Arc::ptr_eq(t, u)))
}

/// Why the selected layers cannot be cleared within the selection, if they cannot: none
/// holds pixels to clear, or one is locked, hidden, or holds a placed image (whose pixels
/// are its own, drawn through a transform: rasterize it first).
pub(crate) fn clear_refused(d: &crate::doc::DocState) -> Result<(), String> {
    if d.pixel_selection().is_none() {
        return Err("Select an area first (the Select tool, or Select All).".into());
    }
    let doc = d.doc();
    let layers: Vec<&opal_core::document::Layer> =
        d.selected_layers_and_inside().into_iter().filter_map(|l| doc.layer(l)).filter(|l| !l.is_group()).collect();
    if layers.is_empty() {
        return Err("Select a layer in the Layers panel first.".into());
    }
    if let Some(l) = layers.iter().find(|l| crate::commands::first_locked(l).is_some()) {
        return Err(format!("\"{}\" is locked. Unlock it first.", l.name));
    }
    if let Some(l) = layers.iter().find(|l| !l.visible) {
        return Err(format!("\"{}\" is hidden: clearing it would change nothing you can see. Show it first.", l.name));
    }
    if let Some(l) = layers.iter().find(|l| crate::layer_ops::has_image_object(l)) {
        return Err(format!(
            "\"{}\" holds a placed image, whose pixels are its own, drawn through its transform. Rasterize it first.",
            l.name
        ));
    }
    Ok(())
}
