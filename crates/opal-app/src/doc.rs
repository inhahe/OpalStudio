//! One open document: its history, its view, and what is selected in it.

use eframe::egui::Vec2;
use opal_core::composite::Projection;
use opal_core::document::{Document, IdGen, LayerId};
use opal_core::history::{History, NodeId};
use std::collections::HashSet;
use std::path::PathBuf;

/// Identifies an open document for the life of the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

/// How the canvas is shown: `zoom` device pixels per canvas pixel, with canvas pixel (0, 0)
/// at `offset` device pixels from the viewport's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub offset: Vec2,
    /// The viewport's size in device pixels when last shown.
    pub viewport: Vec2,
    /// Where the viewport was on screen when last shown, in points.
    pub screen_rect: eframe::egui::Rect,
    /// Whether the view has been fitted to a viewport yet.
    pub placed: bool,
}

impl Default for View {
    fn default() -> View {
        View {
            zoom: 1.0,
            offset: Vec2::ZERO,
            viewport: Vec2::ZERO,
            screen_rect: eframe::egui::Rect::NOTHING,
            placed: false,
        }
    }
}

pub const MIN_ZOOM: f32 = 1.0 / 128.0;
pub const MAX_ZOOM: f32 = 128.0;

impl View {
    /// Canvas coordinates of a point given in device pixels from the viewport's corner.
    pub fn to_canvas(self, px: Vec2) -> Vec2 {
        (px - self.offset) / self.zoom
    }

    /// Device pixels from the viewport's corner of a canvas point.
    pub fn to_screen(self, canvas: Vec2) -> Vec2 {
        canvas * self.zoom + self.offset
    }

    /// Zooms to `zoom`, keeping the canvas point under device pixel `anchor` where it is.
    pub fn zoom_about(&mut self, zoom: f32, anchor: Vec2) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let c = self.to_canvas(anchor);
        self.zoom = zoom;
        self.offset = anchor - c * zoom;
        self.snap();
    }

    /// Fits a canvas of `size` into a viewport of `viewport` device pixels, centered, with a
    /// margin, never zooming past `max`.
    pub fn fit(&mut self, size: Vec2, viewport: Vec2, max: f32) {
        let margin = 0.92;
        let z = (viewport.x / size.x).min(viewport.y / size.y) * margin;
        self.zoom = z.clamp(MIN_ZOOM, max.max(MIN_ZOOM));
        self.offset = (viewport - size * self.zoom) * 0.5;
        self.placed = true;
        self.snap();
    }

    /// Shows the canvas at 100%, centered on the viewport's center.
    pub fn actual_pixels(&mut self, viewport: Vec2) {
        self.zoom_about(1.0, viewport * 0.5);
    }

    /// At whole-number zoom levels, aligns canvas pixels with device pixels so they stay
    /// crisp.
    pub fn snap(&mut self) {
        if self.zoom >= 1.0 && (self.zoom - self.zoom.round()).abs() < 1e-4 {
            self.zoom = self.zoom.round();
            self.offset = Vec2::new(self.offset.x.round(), self.offset.y.round());
        }
    }
}

/// An edit in progress that has not been committed to history yet (a slider being dragged):
/// the document as it would be, and the label it will be committed under.
#[derive(Clone, Debug)]
pub struct Pending {
    pub doc: Document,
    pub label: String,
}

pub struct DocState {
    pub id: DocId,
    /// Shown in the document's tab: the file name, or "Untitled 1".
    pub title: String,
    /// The `.opal` file the document was last saved to or opened from.
    pub path: Option<PathBuf>,
    pub history: History,
    pub ids: IdGen,
    /// The history state with nothing to save: the one last saved to `path`, or, for an
    /// image just created or imported, the state it started in. Closing the document while
    /// at this state asks nothing.
    pub saved_at: Option<NodeId>,
    pub pending: Option<Pending>,
    pub active_layer: Option<LayerId>,
    /// Groups shown expanded in the layers panel.
    pub expanded: HashSet<LayerId>,
    pub view: View,
    pub projection: Projection,
}

impl DocState {
    pub fn new(id: DocId, title: String, doc: Document, ids: IdGen, label: &str) -> DocState {
        let active_layer = doc.layers.last().map(|l| l.id);
        let history = History::new(doc, label);
        DocState {
            id,
            title,
            path: None,
            saved_at: Some(history.current()),
            history,
            ids,
            pending: None,
            active_layer,
            expanded: HashSet::new(),
            view: View::default(),
            projection: Projection::new(),
        }
    }

    /// The document as shown: the pending edit if there is one, else the current state.
    pub fn doc(&self) -> &Document {
        match &self.pending {
            Some(p) => &p.doc,
            None => self.history.doc(),
        }
    }

    /// Whether there are changes that would be lost on closing.
    pub fn is_dirty(&self) -> bool {
        self.pending.is_some() || self.saved_at != Some(self.history.current())
    }

    /// Records an action: `f` changes a copy of the current document, which becomes a new
    /// history state labeled `label`. Nothing is recorded if `f` fails. A layer the action
    /// adds becomes the active layer.
    pub fn apply<E>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Document, &mut IdGen) -> Result<(), E>,
    ) -> Result<(), E> {
        self.commit_pending();
        let before = self.history.doc().clone();
        let mut d = before.clone();
        f(&mut d, &mut self.ids)?;
        self.history.commit(d, label);
        self.fix_active_layer(Some(&before));
        Ok(())
    }

    /// Changes the pending edit (starting one labeled `label` if none is in progress), for
    /// changes made continuously, like dragging a slider, that become one history state.
    pub fn edit_pending(&mut self, label: &str, f: impl FnOnce(&mut Document)) {
        let p =
            self.pending.get_or_insert_with(|| Pending { doc: self.history.doc().clone(), label: label.to_owned() });
        f(&mut p.doc);
    }

    /// Commits the pending edit, if any, as one history state.
    pub fn commit_pending(&mut self) {
        if let Some(p) = self.pending.take() {
            self.history.commit(p.doc, p.label);
        }
    }

    pub fn undo(&mut self) -> bool {
        self.commit_pending();
        let before = self.history.doc().clone();
        let moved = self.history.undo();
        if moved {
            self.fix_active_layer(Some(&before));
        }
        moved
    }

    pub fn redo(&mut self) -> bool {
        self.commit_pending();
        let before = self.history.doc().clone();
        let moved = self.history.redo();
        if moved {
            self.fix_active_layer(Some(&before));
        }
        moved
    }

    pub fn goto(&mut self, node: NodeId) {
        self.commit_pending();
        let before = self.history.doc().clone();
        self.history.goto(node);
        self.fix_active_layer(Some(&before));
    }

    /// After the layers changed from `before`: if layers appeared (an action added one, or
    /// moving through history restored one), the topmost of them becomes active; otherwise,
    /// if the active layer no longer exists, the layer now in its place does (or the one
    /// below it, or the top layer).
    fn fix_active_layer(&mut self, before: Option<&Document>) {
        let doc = self.history.doc();
        let now: Vec<LayerId> = doc.layer_ids();
        if let Some(before) = before {
            let old: HashSet<LayerId> = before.layer_ids().into_iter().collect();
            if let Some(restored) = now.iter().rev().find(|id| !old.contains(id)) {
                self.active_layer = Some(*restored);
                return;
            }
        }
        if self.active_layer.is_some_and(|a| now.contains(&a)) {
            return;
        }
        let place = before.zip(self.active_layer).and_then(|(b, a)| b.position(a));
        self.active_layer = place
            .and_then(|pos| {
                let siblings = doc.siblings(pos.parent)?;
                let i = pos.index.min(siblings.len().checked_sub(1)?);
                Some(siblings[i].id)
            })
            .or_else(|| now.last().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::document::{Layer, Position};

    fn state() -> DocState {
        let mut ids = IdGen::default();
        let mut d = Document::new(10, 10);
        d.insert(Position { parent: None, index: 0 }, Layer::new_empty(ids.layer(), "Background")).unwrap();
        DocState::new(DocId(1), "t".into(), d, ids, "New image")
    }

    fn add_layer(s: &mut DocState, name: &str) -> LayerId {
        let mut new = None;
        s.apply::<()>("Add", |d, ids| {
            let id = ids.layer();
            let index = d.layers.len();
            d.insert(Position { parent: None, index }, Layer::new_empty(id, name)).unwrap();
            new = Some(id);
            Ok(())
        })
        .unwrap();
        let id = new.unwrap();
        s.active_layer = Some(id);
        id
    }

    #[test]
    fn a_pending_edit_becomes_one_history_state() {
        let mut s = state();
        let bg = s.active_layer.unwrap();
        for op in [0.9, 0.7, 0.5] {
            s.edit_pending("Opacity", |d| d.layer_mut(bg).unwrap().opacity = op);
        }
        assert!(s.is_dirty());
        assert_eq!(s.doc().layer(bg).unwrap().opacity, 0.5);
        assert_eq!(s.history.doc().layer(bg).unwrap().opacity, 1.0, "nothing committed yet");
        s.commit_pending();
        assert_eq!(s.history.undo_label(), Some("Opacity"));
        assert!(s.undo());
        assert_eq!(s.doc().layer(bg).unwrap().opacity, 1.0);
    }

    #[test]
    fn undoing_a_deletion_selects_the_restored_layer() {
        let mut s = state();
        let top = add_layer(&mut s, "Top");
        s.apply::<()>("Delete", |d, _| {
            d.remove(top).unwrap();
            Ok(())
        })
        .unwrap();
        assert_ne!(s.active_layer, Some(top), "the deleted layer is no longer active");
        s.undo();
        assert_eq!(s.active_layer, Some(top));
    }

    #[test]
    fn undoing_an_addition_selects_a_remaining_layer() {
        let mut s = state();
        let bg = s.active_layer.unwrap();
        add_layer(&mut s, "Top");
        s.undo();
        assert_eq!(s.active_layer, Some(bg));
    }

    #[test]
    fn saving_marks_the_state_clean() {
        let mut s = state();
        assert!(!s.is_dirty(), "an untouched new image has nothing to save");
        add_layer(&mut s, "first");
        assert!(s.is_dirty());
        s.saved_at = Some(s.history.current());
        assert!(!s.is_dirty());
        add_layer(&mut s, "x");
        assert!(s.is_dirty());
        s.undo();
        assert!(!s.is_dirty(), "back at the saved state");
    }

    #[test]
    fn zooming_keeps_the_anchor_point_still() {
        let mut v = View { offset: Vec2::new(10.0, 20.0), placed: true, ..View::default() };
        let anchor = Vec2::new(300.0, 200.0);
        let before = v.to_canvas(anchor);
        v.zoom_about(2.5, anchor);
        assert!((v.to_canvas(anchor) - before).length() < 1e-3);
        v.fit(Vec2::new(1000.0, 500.0), Vec2::new(500.0, 500.0), 1.0);
        assert!(v.zoom < 0.5 && v.zoom > 0.4);
    }
}
