//! What is selected (design.md, *Selections*): Select All, Deselect and Invert, and the
//! commands that act on the selection (Clear, Cut, Crop to Selection). Copy copies within it
//! (`paste.rs`); the canvas shows its outline (`canvas.rs`).

use crate::state::{AppState, MessageKind};
use opal_core::document::{Canvas, Document, Layer, LayerContent, LayerId, ObjectKind};
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
        let layers: Vec<LayerId> = cleared(d).iter().filter(|l| !l.is_group()).map(|l| l.id).collect();
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

/// What Clear and Cut act on: the selected layers, and inside the selected groups the layers
/// and groups shown (a hidden one is left as it is: nothing of it is seen), each after the
/// group it is in.
fn cleared(d: &crate::doc::DocState) -> Vec<&Layer> {
    fn walk<'a>(l: &'a Layer, out: &mut Vec<&'a Layer>) {
        out.push(l);
        for c in l.children().unwrap_or(&[]) {
            if c.visible {
                walk(c, out);
            }
        }
    }
    let doc = d.doc();
    let mut out = Vec::new();
    for id in d.selected_layers() {
        if let Some(l) = doc.layer(id) {
            walk(l, &mut out);
        }
    }
    out
}

/// Why the selected layers cannot be cleared within the selection, if they cannot: none is
/// selected, or one (or a group) is hidden, or one it would clear (or a group it is in) is
/// locked, or holds a placed image (whose pixels are its own, drawn through a transform:
/// rasterize it first).
pub(crate) fn clear_refused(d: &crate::doc::DocState) -> Result<(), String> {
    if d.pixel_selection().is_none() {
        return Err("Select an area first (the Select tool, or Select All).".into());
    }
    let doc = d.doc();
    let selected: Vec<&Layer> = d.selected_layers().into_iter().filter_map(|l| doc.layer(l)).collect();
    if selected.is_empty() {
        return Err("Select a layer in the Layers panel first.".into());
    }
    if let Some(l) = selected.iter().find(|l| !l.visible) {
        return Err(format!("\"{}\" is hidden: clearing it would change nothing you can see. Show it first.", l.name));
    }
    let layers = cleared(d);
    if let Some(l) = layers.iter().find(|l| l.locked) {
        return Err(format!("\"{}\" is locked. Unlock it first.", l.name));
    }
    if let Some(l) = layers.iter().find(|l| !l.is_group() && crate::layer_ops::has_image_object(l)) {
        return Err(format!(
            "\"{}\" holds a placed image, whose pixels are its own, drawn through its transform. Rasterize it first.",
            l.name
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::state::{AppState, Background, NewImage};
    use opal_core::document::{Document, IdGen, Layer, LayerContent, LayerId, ObjectKind, Position};
    use opal_core::geom::IRect;
    use opal_core::raster::RasterImage;

    #[test]
    fn a_selected_group_is_cleared_where_it_shows_and_refused_as_its_own_flags_say() {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 300, height: 200, background: Background::White, resolution: 72.0 });
        // A group holding A, shown, and B, hidden, each red all over.
        let red = opal_core::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let mut ids: [Option<LayerId>; 3] = [None; 3];
        let d = st.active_mut().unwrap();
        d.apply("Group", |doc: &mut Document, made: &mut IdGen| {
            let g = made.layer();
            doc.insert(Position { parent: None, index: 1 }, Layer::new_group(g, "G"))?;
            for (i, name) in ["A", "B"].into_iter().enumerate() {
                let id = made.layer();
                let mut l = Layer::new_raster(id, made.object(), name, RasterImage::filled(&IRect::new(0, 0, 300, 200), red));
                l.visible = name == "A";
                doc.insert(Position { parent: Some(g), index: i }, l)?;
                ids[i + 1] = Some(id);
            }
            ids[0] = Some(g);
            Ok::<(), opal_core::document::TreeError>(())
        })
        .unwrap();
        let [g, a, b] = ids.map(Option::unwrap);
        d.select_only(g);
        st.select_all();
        let flag = |st: &mut AppState, id: LayerId, f: fn(&mut Layer)| {
            st.active_mut()
                .unwrap()
                .apply("Flag", |doc, _| {
                    f(doc.layer_mut(id).unwrap());
                    Ok::<(), ()>(())
                })
                .unwrap();
        };
        let refused = |st: &AppState| super::clear_refused(st.active().unwrap()).err().unwrap_or_default();
        // The group locked: refused, though nothing inside it is.
        flag(&mut st, g, |l| l.locked = true);
        assert!(refused(&st).contains("\"G\" is locked"), "{}", refused(&st));
        // Hidden instead: refused too.
        flag(&mut st, g, |l| (l.locked, l.visible) = (false, false));
        assert!(refused(&st).contains("\"G\" is hidden"), "{}", refused(&st));
        // Shown, with B (hidden inside it) locked: B is no part of it, so cleared, A alone.
        flag(&mut st, g, |l| l.visible = true);
        flag(&mut st, b, |l| l.locked = true);
        assert_eq!(refused(&st), "");
        st.clear_selected();
        let alpha = |st: &AppState, id: LayerId| {
            let LayerContent::Objects(o) = &st.active().unwrap().doc().layer(id).unwrap().content else { panic!() };
            match o.first().map(|o| &o.kind) {
                Some(ObjectKind::Pixels(p)) => opal_core::tile::pixel_to_f32(p.image.pixel(150, 100))[3],
                _ => 0.0,
            }
        };
        assert_eq!([alpha(&st, a), alpha(&st, b)], [0.0, 1.0], "A cleared, B, hidden, left as it was");
        // A locked: refused.
        flag(&mut st, a, |l| l.locked = true);
        assert!(refused(&st).contains("\"A\" is locked"), "{}", refused(&st));
    }
}
