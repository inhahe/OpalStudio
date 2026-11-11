//! One open document: its history, its view, and what is selected in it.

use eframe::egui::{Pos2, Rect, Vec2};
use opal_core::document::{Canvas, Document, IdGen, LayerId};
use opal_core::history::{History, NodeId};
use std::collections::{HashMap, HashSet};
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

/// What the status bar says the first time a document's history states are dropped.
pub const DROP_NOTE: &str =
    "The oldest history states are being dropped, to keep history within its memory limit (Preferences › History).";

/// Far enough out that the largest canvas (`dialogs::MAX_SIDE`) fits a window.
pub const MIN_ZOOM: f32 = 1.0 / 1024.0;
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

    /// Fits `canvas` (in canvas pixels) into a viewport of `viewport` device pixels,
    /// centered, with a margin, never zooming past `max`.
    pub fn fit(&mut self, canvas: Rect, viewport: Vec2, max: f32) {
        let margin = 0.92;
        let size = canvas.size();
        let z = (viewport.x / size.x).min(viewport.y / size.y) * margin;
        self.zoom = z.clamp(MIN_ZOOM, max.max(MIN_ZOOM));
        self.offset = (viewport - size * self.zoom) * 0.5 - canvas.min.to_vec2() * self.zoom;
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

/// The canvas's rectangle in canvas pixels, for the view.
pub fn canvas_rect(c: &Canvas) -> Rect {
    Rect::from_min_size(Pos2::new(c.x as f32, c.y as f32), Vec2::new(c.width as f32, c.height as f32))
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
    /// The layer the panels and single-layer commands act on. Always one of `selected`.
    active: Option<LayerId>,
    /// The layers selected in the Layers panel, which commands that act on several layers
    /// act on. Not part of history, like the active layer.
    selected: HashSet<LayerId>,
    /// Groups shown expanded in the layers panel.
    pub expanded: HashSet<LayerId>,
    pub view: View,
    pub projector: crate::projector::Projector,
    /// Counts every change to what the document shows (for repainting).
    pub revision: u64,
    /// The transform box, while one is open. Its document is the pending edit.
    pub transform: Option<crate::transform::Session>,
    /// A Move tool drag in progress.
    pub moving: Option<crate::move_tool::MoveDrag>,
    /// The Crop tool's box, while the tool is in use, and a drag on it.
    pub crop: Option<crate::crop::CropBox>,
    pub crop_drag: Option<crate::crop::CropDrag>,
    /// The rectangle the transform box's fields fit it into (its top-left corner from the
    /// canvas's, and its size), once one has been typed; the canvas, whatever it is then,
    /// until then.
    pub fit_rect: Option<[f64; 4]>,
    /// What the boxes' positions count from (see `box_fields::Origin`).
    pub origin: crate::box_fields::Origin,
    /// Something an operation on the document wants said in the status bar.
    pub notice: Option<String>,
    /// Whether the user has been told that history states are being dropped.
    told_dropped: bool,
    /// The groups (by number) a call being carried out is in: the script run its
    /// connection is, and the step it is in, if they are.
    pub(crate) groups: Vec<u64>,
    /// The states each group going on has made, joined into one when it ends.
    made_by: HashMap<u64, HashSet<NodeId>>,
    /// The states steps made (each one step's), which the run they are in keeps apart.
    named: HashSet<NodeId>,
}

impl DocState {
    pub fn new(id: DocId, title: String, doc: Document, ids: IdGen, label: &str) -> DocState {
        let active = doc.layers.last().map(|l| l.id);
        let history = History::new(doc, label);
        DocState {
            id,
            title,
            path: None,
            saved_at: Some(history.current()),
            history,
            ids,
            pending: None,
            active,
            selected: active.into_iter().collect(),
            expanded: HashSet::new(),
            view: View::default(),
            projector: crate::projector::Projector::new(),
            revision: 0,
            transform: None,
            moving: None,
            crop: None,
            fit_rect: None,
            origin: crate::box_fields::Origin::default(),
            crop_drag: None,
            notice: None,
            told_dropped: false,
            groups: Vec::new(),
            made_by: HashMap::new(),
            named: HashSet::new(),
        }
    }

    /// Limits the memory the history may use beyond the document itself (see
    /// [`History::set_budget`]).
    pub fn set_history_budget(&mut self, bytes: u64) {
        self.history.set_budget(bytes);
    }

    /// Whether the user is yet to be told that history states are being dropped (the app
    /// says it once the status bar is free; see [`DROP_NOTE`]).
    pub fn drop_note_due(&self) -> bool {
        self.history.dropped() > 0 && !self.told_dropped
    }

    /// Records that the user was told that history states are being dropped.
    pub fn drop_note_told(&mut self) {
        self.told_dropped = true;
    }

    /// The active layer: the one the panels and single-layer commands act on.
    pub fn active_layer(&self) -> Option<LayerId> {
        self.active
    }

    /// The active layer and the selected layers, to be put back ([`set_layer_selection_state`]).
    ///
    /// [`set_layer_selection_state`]: Self::set_layer_selection_state
    pub(crate) fn layer_selection_state(&self) -> (Option<LayerId>, HashSet<LayerId>) {
        (self.active, self.selected.clone())
    }

    pub(crate) fn set_layer_selection_state(&mut self, (active, selected): (Option<LayerId>, HashSet<LayerId>)) {
        self.active = active;
        self.selected = selected;
    }

    /// Makes `id` the active layer and the only one selected.
    pub fn select_only(&mut self, id: LayerId) {
        self.active = Some(id);
        self.selected = HashSet::from([id]);
    }

    /// Adds `id` to the selection, making it active, or takes it out (unless it is the only
    /// one selected); the active layer then moves to another selected layer.
    pub fn toggle_selected(&mut self, id: LayerId) {
        if !self.selected.contains(&id) {
            self.selected.insert(id);
            self.active = Some(id);
        } else if self.selected.len() > 1 {
            self.selected.remove(&id);
            if self.active == Some(id) {
                self.active = self.selected_layers().last().copied();
            }
        }
    }

    /// Selects the layers from the active one to `id` in `order` (the order the Layers
    /// panel lists them in), making `id` active.
    pub fn select_range(&mut self, id: LayerId, order: &[LayerId]) {
        let from = self.active.and_then(|a| order.iter().position(|&l| l == a));
        let to = order.iter().position(|&l| l == id);
        match (from, to) {
            (Some(a), Some(b)) => {
                let (lo, hi) = (a.min(b), a.max(b));
                self.selected = order[lo..=hi].iter().copied().collect();
                self.active = Some(id);
            }
            _ => self.select_only(id),
        }
    }

    /// Makes a selected layer the active one, keeping the selection.
    pub fn make_active(&mut self, id: LayerId) {
        if self.selected.contains(&id) {
            self.active = Some(id);
        } else {
            self.select_only(id);
        }
    }

    pub fn is_selected(&self, id: LayerId) -> bool {
        self.selected.contains(&id)
    }

    /// The selected layers that exist, in document order (depth first, bottom to top),
    /// leaving out those inside a selected group: what commands acting on several layers
    /// act on, each layer once.
    pub fn selected_layers(&self) -> Vec<LayerId> {
        // One walk, which does not look inside a selected group.
        fn walk(
            layers: &[std::sync::Arc<opal_core::document::Layer>],
            selected: &HashSet<LayerId>,
            out: &mut Vec<LayerId>,
        ) {
            for l in layers {
                if selected.contains(&l.id) {
                    out.push(l.id);
                } else if let Some(c) = l.children() {
                    walk(c, selected, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.doc().layers, &self.selected, &mut out);
        out
    }

    /// The selected layers and every layer inside the selected groups.
    pub fn selected_layers_and_inside(&self) -> Vec<LayerId> {
        let doc = self.doc();
        let mut out = Vec::new();
        for id in self.selected_layers() {
            if let Some(l) = doc.layer(id) {
                let mut add = |l: &opal_core::document::Layer| out.push(l.id);
                fn walk(l: &opal_core::document::Layer, f: &mut dyn FnMut(&opal_core::document::Layer)) {
                    f(l);
                    for c in l.children().unwrap_or(&[]) {
                        walk(c, f);
                    }
                }
                walk(l, &mut add);
            }
        }
        out
    }

    /// The name to suggest for files made from this document: the title without any
    /// extension ("photo" for an imported photo.jpg).
    pub fn stem(&self) -> String {
        std::path::Path::new(&self.title)
            .file_stem()
            .map_or_else(|| self.title.clone(), |s| s.to_string_lossy().into_owned())
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
        let id = self.history.commit(d, label);
        for &group in &self.groups {
            self.made_by.entry(group).or_default().insert(id);
        }
        self.revision += 1;
        self.fix_active_layer(Some(&before));
        Ok(())
    }

    /// Joins the states in `made`, made one after another, into one labeled `label` (see
    /// [`History::join`]).
    pub fn join_states(&mut self, made: &HashSet<NodeId>, label: &str) {
        self.history.join(made, label, &self.named);
        self.revision += 1;
    }

    /// Joins the states group `group` made one after another into one, labeled `label` (see
    /// [`History::join`]), now that it has ended. What a `step` made is kept apart from the
    /// run it is in, when that ends.
    pub fn group_ended(&mut self, group: u64, label: &str, step: bool) {
        let Some(made) = self.made_by.remove(&group) else { return };
        self.history.join(&made, label, &self.named);
        // The history panel and the undo label show it changed.
        self.revision += 1;
        if step {
            self.named.extend(made);
        }
        let history = &self.history;
        self.named.retain(|&id| history.contains(id));
    }

    /// Changes the pending edit (starting one labeled `label` if none is in progress), for
    /// changes made continuously, like dragging a slider, that become one history state. A
    /// pending edit of another kind (a transform box, say) is committed first: two edits
    /// never merge into one state.
    pub fn edit_pending(&mut self, label: &str, f: impl FnOnce(&mut Document)) {
        if self.pending.as_ref().is_some_and(|p| p.label != label) || self.transform.is_some() {
            self.commit_pending();
        }
        let p =
            self.pending.get_or_insert_with(|| Pending { doc: self.history.doc().clone(), label: label.to_owned() });
        f(&mut p.doc);
        self.revision += 1;
    }

    /// The label of the pending edit, if one is in progress.
    pub fn pending_label(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.label.as_str())
    }

    /// Commits the pending edit, if any, as one history state, made in the groups the
    /// document is in now (as [`apply`](Self::apply) records it); a transform box in
    /// progress is applied first.
    pub fn commit_pending(&mut self) {
        self.finish_transform();
        if let Some(p) = self.pending.take() {
            let id = self.history.commit(p.doc, p.label);
            for &group in &self.groups {
                self.made_by.entry(group).or_default().insert(id);
            }
            self.revision += 1;
        }
    }

    pub fn undo(&mut self) -> bool {
        self.commit_pending();
        let before = self.history.doc().clone();
        let moved = self.history.undo();
        if moved {
            self.revision += 1;
            self.fix_active_layer(Some(&before));
        }
        moved
    }

    pub fn redo(&mut self) -> bool {
        self.commit_pending();
        let before = self.history.doc().clone();
        let moved = self.history.redo();
        if moved {
            self.revision += 1;
            self.fix_active_layer(Some(&before));
        }
        moved
    }

    pub fn goto(&mut self, node: NodeId) {
        self.commit_pending();
        // Committing can drop old states to keep history within its budget.
        if !self.history.contains(node) {
            self.drop_note_told();
            self.notice =
                Some("That state was dropped to keep history within its memory limit (Preferences › History).".into());
            return;
        }
        let before = self.history.doc().clone();
        self.history.goto(node);
        self.revision += 1;
        self.fix_active_layer(Some(&before));
    }

    /// After the layers changed from `before`, picks the active layer (see `fix_active`).
    /// The selection loses layers that no longer exist; if it loses the active one, or
    /// the active layer changed, the active layer alone is selected.
    fn fix_active_layer(&mut self, before: Option<&Document>) {
        let was = self.active;
        self.fix_active(before);
        let now: HashSet<LayerId> = self.history.doc().layer_ids().into_iter().collect();
        self.selected.retain(|l| now.contains(l));
        match self.active {
            Some(a) if a != was.unwrap_or(a) || !self.selected.contains(&a) => self.select_only(a),
            Some(_) => {}
            None => self.selected.clear(),
        }
    }

    /// If layers appeared (an action added one, or moving through history restored one),
    /// the topmost of them becomes active; otherwise, if the active layer no longer exists,
    /// the layer now in its place does (or the one below it, or the top layer).
    fn fix_active(&mut self, before: Option<&Document>) {
        let doc = self.history.doc();
        let now: Vec<LayerId> = doc.layer_ids();
        if let Some(before) = before {
            let old: HashSet<LayerId> = before.layer_ids().into_iter().collect();
            if let Some(restored) = now.iter().rev().find(|id| !old.contains(id)) {
                self.active = Some(*restored);
                return;
            }
        }
        if self.active.is_some_and(|a| now.contains(&a)) {
            return;
        }
        let place = before.zip(self.active).and_then(|(b, a)| b.position(a));
        self.active = place
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
        s.select_only(id);
        id
    }

    #[test]
    fn a_pending_edit_becomes_one_history_state() {
        let mut s = state();
        let bg = s.active_layer().unwrap();
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
        assert_ne!(s.active_layer(), Some(top), "the deleted layer is no longer active");
        s.undo();
        assert_eq!(s.active_layer(), Some(top));
    }

    #[test]
    fn undoing_an_addition_selects_a_remaining_layer() {
        let mut s = state();
        let bg = s.active_layer().unwrap();
        add_layer(&mut s, "Top");
        s.undo();
        assert_eq!(s.active_layer(), Some(bg));
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
        let canvas = Rect::from_min_size(Pos2::new(-200.0, 50.0), Vec2::new(1000.0, 500.0));
        v.fit(canvas, Vec2::new(500.0, 500.0), 1.0);
        assert!(v.zoom < 0.5 && v.zoom > 0.4);
        let center = v.to_screen(canvas.center().to_vec2());
        assert!(
            (center - Vec2::new(250.0, 250.0)).length() < 1.0,
            "the canvas is centered wherever it lies: {center:?}"
        );
    }

    #[test]
    fn going_to_a_state_that_committing_drops_does_nothing() {
        let mut s = state();
        let layer = add_layer(&mut s, "paint");
        let root = s.history.root();
        let paint = |s: &mut DocState, x: i64| {
            s.apply::<()>("Paint", |d, ids| {
                let mut img = opal_core::raster::RasterImage::new();
                img.set_pixel(x, 0, opal_core::tile::pixel_from_f32([1.0; 4]));
                d.remove(layer).unwrap();
                d.insert(Position { parent: None, index: 1 }, Layer::new_raster(layer, ids.object(), "paint", img))
                    .unwrap();
                Ok(())
            })
            .unwrap();
        };
        paint(&mut s, 0);
        paint(&mut s, 1);
        s.history.set_budget(0);
        // Only the current state and the one before are left.
        let oldest = s.history.root();
        assert_ne!(oldest, root, "states were dropped");
        assert_eq!(s.history.node(s.history.current()).parent, Some(oldest));
        // With an edit pending, going to the oldest commits the edit first, which makes the
        // oldest droppable, and drops it.
        let bg = s.doc().layers[0].id;
        s.edit_pending("Opacity", |d| d.layer_mut(bg).unwrap().opacity = 0.5);
        s.goto(oldest);
        assert!(!s.history.contains(oldest));
        assert_eq!(s.history.undo_label(), Some("Opacity"), "the edit was committed and stays current");
        assert!(s.notice.as_ref().is_some_and(|n| n.contains("Preferences")), "nothing says why the click did nothing");
    }

    #[test]
    fn dropping_history_states_is_said_once() {
        let mut s = state();
        let layer = add_layer(&mut s, "paint");
        let paint = |s: &mut DocState, x: i64| {
            s.apply::<()>("Paint", |d, ids| {
                let mut img = opal_core::raster::RasterImage::new();
                img.set_pixel(x, 0, opal_core::tile::pixel_from_f32([1.0; 4]));
                let l = Layer::new_raster(layer, ids.object(), "paint", img);
                d.remove(layer).unwrap();
                d.insert(Position { parent: None, index: 1 }, l).unwrap();
                Ok(())
            })
            .unwrap();
        };
        for x in 0..4 {
            paint(&mut s, x);
        }
        assert!(!s.drop_note_due());
        s.set_history_budget(0);
        assert!(s.history.dropped() > 0);
        assert!(s.drop_note_due(), "the first drop is to be said");
        s.drop_note_told();
        paint(&mut s, 9);
        assert!(!s.drop_note_due(), "and only the first");
    }
}
