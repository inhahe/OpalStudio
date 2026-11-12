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
/// at `offset` device pixels from the viewport's top-left corner, were the view not turned;
/// then turned `angle` degrees clockwise about the viewport's middle (design.md, *Rotating
/// the view*).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub offset: Vec2,
    /// How far the view is turned, in degrees clockwise (-180 to 180).
    pub angle: f32,
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
            angle: 0.0,
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
    /// The cosine and sine of the angle: exactly 1 and 0 when it is 0, so that a view not
    /// turned maps points exactly as it did before views turned.
    pub fn turning(self) -> (f32, f32) {
        if self.angle == 0.0 {
            (1.0, 0.0)
        } else {
            let a = self.angle.to_radians();
            (a.cos(), a.sin())
        }
    }

    /// `v` (a direction, in device pixels) turned as the view is: clockwise on the screen,
    /// whose y goes down.
    pub fn turn(self, v: Vec2) -> Vec2 {
        let (c, s) = self.turning();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// `v` turned back (see [`turn`](Self::turn)).
    pub fn unturn(self, v: Vec2) -> Vec2 {
        let (c, s) = self.turning();
        Vec2::new(v.x * c + v.y * s, v.y * c - v.x * s)
    }

    /// Where a point on the screen (device pixels from the viewport's corner) would be were
    /// the view not turned.
    pub fn unturned(self, px: Vec2) -> Vec2 {
        let middle = self.viewport * 0.5;
        middle + self.unturn(px - middle)
    }

    /// Where a canvas point would be on the screen were the view not turned: what the GPU
    /// places tiles by, before it turns them.
    pub fn to_unturned(self, canvas: Vec2) -> Vec2 {
        canvas * self.zoom + self.offset
    }

    /// Canvas coordinates of a point given in device pixels from the viewport's corner.
    pub fn to_canvas(self, px: Vec2) -> Vec2 {
        (self.unturned(px) - self.offset) / self.zoom
    }

    /// Device pixels from the viewport's corner of a canvas point.
    pub fn to_screen(self, canvas: Vec2) -> Vec2 {
        let middle = self.viewport * 0.5;
        middle + self.turn(self.to_unturned(canvas) - middle)
    }

    /// The smallest canvas rectangle holding all the view shows.
    pub fn shown(self) -> Rect {
        let corners = [Vec2::ZERO, Vec2::new(self.viewport.x, 0.0), self.viewport, Vec2::new(0.0, self.viewport.y)];
        Rect::from_points(&corners.map(|c| self.to_canvas(c).to_pos2()))
    }

    /// Zooms to `zoom`, keeping the canvas point under device pixel `anchor` where it is.
    pub fn zoom_about(&mut self, zoom: f32, anchor: Vec2) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let c = self.to_canvas(anchor);
        self.zoom = zoom;
        self.offset = self.unturned(anchor) - c * zoom;
        self.snap();
    }

    /// Moves what the view shows `delta` device pixels across the screen.
    pub fn scroll(&mut self, delta: Vec2) {
        self.offset += self.unturn(delta);
    }

    /// Turns the view to `angle` degrees clockwise (taken to -180 to 180), about the
    /// viewport's middle.
    pub fn turn_to(&mut self, angle: f32) {
        let a = (angle + 180.0).rem_euclid(360.0) - 180.0;
        // Straight again exactly, not nearly, and crisp again at a whole zoom.
        self.angle = if a.abs() < 1e-3 { 0.0 } else { a };
        self.snap();
    }

    /// Fits `canvas` (in canvas pixels), as the view turns it, into a viewport of `viewport`
    /// device pixels, centered, with a margin, never zooming past `max`.
    pub fn fit(&mut self, canvas: Rect, viewport: Vec2, max: f32) {
        let margin = 0.92;
        let (c, s) = self.turning();
        let size = canvas.size();
        let turned = Vec2::new(size.x * c.abs() + size.y * s.abs(), size.x * s.abs() + size.y * c.abs());
        let z = (viewport.x / turned.x).min(viewport.y / turned.y) * margin;
        self.zoom = z.clamp(MIN_ZOOM, max.max(MIN_ZOOM));
        // Its middle in the viewport's, which the view turns about.
        self.viewport = viewport;
        self.offset = viewport * 0.5 - canvas.center().to_vec2() * self.zoom;
        self.placed = true;
        self.snap();
    }

    /// Shows the canvas at 100%, centered on the viewport's center.
    pub fn actual_pixels(&mut self, viewport: Vec2) {
        self.zoom_about(1.0, viewport * 0.5);
    }

    /// At whole-number zoom levels, aligns canvas pixels with device pixels so they stay
    /// crisp: where the view is straight, as only there they can be.
    pub fn snap(&mut self) {
        if self.zoom >= 1.0 && (self.zoom - self.zoom.round()).abs() < 1e-4 {
            self.zoom = self.zoom.round();
            if self.angle == 0.0 {
                self.offset = Vec2::new(self.offset.x.round(), self.offset.y.round());
            }
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

/// A drag under way on an image's canvas (see [`DocState::drag`]).
#[derive(Clone, Debug)]
pub enum Drag {
    /// The Move tool's, moving the selected layers (its edit, [`MOVE`](crate::move_tool::MOVE),
    /// pending).
    Move(crate::move_tool::MoveDrag),
    /// On the Crop tool's box.
    Crop(crate::edit_box::BoxDrag),
    /// The Select tool's, drawing a new shape.
    Select(crate::select_tool::SelectDrag),
    /// On the box round the selection's last shape (its edit pending).
    Shape(crate::select_tool::ShapeDrag),
    /// On the outline the outline tool edits (its edit pending).
    Outline(crate::outline_tool::EditDrag),
    /// Redrawing a stretch of that outline (boxed: it is by far the largest).
    Reshape(Box<crate::outline_tool::Reshaping>),
    /// Of the symmetry's center or axis (the outline edited's: its edit pending).
    Symmetry(crate::outline_tool::SymmetryDrag),
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
    /// Before and After, while on: the state of its history shown left of the line.
    pub compare: Option<Compare>,
    /// Counts every change to what the document shows (for repainting).
    pub revision: u64,
    /// The transform box, while one is open. Its document is the pending edit.
    pub transform: Option<crate::transform::Session>,
    /// What the last transform box applied to layers did (Transform, or a script's transform):
    /// what Transform Again does again (see [`transform_again_map`](Self::transform_again_map)).
    /// Not part of its history, as in Photoshop: an undo leaves it.
    pub last_transform: Option<LastTransform>,
    /// The Crop tool's box, while the tool is in use.
    pub crop: Option<crate::edit_box::EditBox>,
    /// The drag under way on its canvas, if one is (one at most), given up as one
    /// ([`give_up_drag`](Self::give_up_drag)).
    pub drag: Option<Drag>,
    /// An outline being drawn with the outline tool.
    pub outlining: Option<crate::outline_tool::Outlining>,
    /// The points picked on the outline the outline tool edits (the selection's last step,
    /// when it is one), with that outline: none once the last step is another.
    pub picked: Option<crate::outline_tool::Picked>,
    /// Where the symmetry the outline tool draws with is centered, once moved (the canvas's
    /// middle until then), and its axis's angle (degrees clockwise from up); not part of the
    /// image's history, nor kept in its file.
    pub symmetry_center: Option<[f64; 2]>,
    pub symmetry_angle: f64,
    /// The rectangle the transform box's fields fit it into (its top-left corner from the
    /// canvas's, and its size), once one has been typed; the canvas, whatever it is then,
    /// until then.
    pub fit_rect: Option<[f64; 4]>,
    /// What the boxes' positions count from (see `box_fields::Origin`).
    pub origin: crate::box_fields::Origin,
    /// Something an operation on the document wants said in the status bar.
    pub notice: Option<String>,
    /// The outline of the selection's mask (the marching ants' path), with the mask it is of
    /// and the level it runs at (see `DocState::selection_outline`): the mask weakly, so that
    /// it keeps alive no mask that history has let go (and no other mask takes that one's
    /// place while it is held so).
    pub outline: Option<(std::sync::Weak<opal_core::mask::Mask>, u16, crate::select::Ants)>,
    /// What the selection overlay keeps of the mask's tiles (see `mask_view`).
    pub overlay: crate::mask_view::Textures,
    /// The egui pass the image was last drawn in (see `AppState::leave_undrawn`).
    pub drawn: u64,
    /// Whether the user has been told that history states are being dropped.
    told_dropped: bool,
    /// The groups (by number) a call being carried out is in: the script run its
    /// connection is, and the step it is in, if they are.
    pub(crate) groups: Vec<u64>,
    /// The states each group going on has made, joined into one when it ends.
    made_by: HashMap<u64, HashSet<NodeId>>,
    /// The states steps made (each one step's), which the run they are in keeps apart.
    named: HashSet<NodeId>,
    /// An edit worked out off the interface thread, while one is (see `edits`): the image is
    /// not to be changed until it is applied or cancelled.
    pub busy: Option<crate::edits::Busy>,
}

/// Before and After (see design.md, *Comparing*): the state of an image's history shown
/// left of a line across its view, the image as it is right of it.
pub struct Compare {
    /// The state shown as it was.
    pub node: opal_core::history::NodeId,
    /// Where the line is, as a fraction of the view's width.
    pub split: f32,
    /// What draws the state, as the image's own projector draws it now.
    pub projector: crate::projector::Projector,
}

impl Compare {
    /// The image as it was at `node`, the line in the middle.
    pub fn new(node: opal_core::history::NodeId) -> Compare {
        Compare { node, split: 0.5, projector: crate::projector::Projector::new() }
    }
}

/// What a transform box applied to layers did, kept for Transform Again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LastTransform {
    /// Its map of the image (the box's [`Session::delta`](crate::transform::Session::delta)).
    pub delta: opal_core::geom::Affine,
    /// The image's [`carried`](opal_core::document::Document::carried) when it was applied:
    /// the coordinates `delta` is in.
    pub carried: opal_core::geom::Affine,
}

/// Why [`DocState::apply`] made no history state.
#[derive(Debug, PartialEq)]
pub enum NotMade<E> {
    /// An edit is under way on the image (see `edits`): nothing may change it, and the status
    /// bar says so.
    Busy,
    /// What the action does failed, saying why.
    Failed(E),
}

impl<E: std::fmt::Display> std::fmt::Display for NotMade<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotMade::Busy => write!(f, "An edit is under way on the image; this was not done."),
            NotMade::Failed(why) => why.fmt(f),
        }
    }
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
            compare: None,
            revision: 0,
            transform: None,
            drag: None,
            last_transform: None,
            crop: None,
            fit_rect: None,
            origin: crate::box_fields::Origin::default(),
            notice: None,
            outline: None,
            overlay: Default::default(),
            drawn: 0,
            outlining: None,
            picked: None,
            symmetry_center: None,
            symmetry_angle: 0.0,
            told_dropped: false,
            groups: Vec::new(),
            made_by: HashMap::new(),
            named: HashSet::new(),
            busy: None,
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

    /// Selects every layer at the top level (a group as a whole), the topmost made active.
    pub fn select_all_layers(&mut self) {
        let ids: Vec<LayerId> = self.doc().layers.iter().map(|l| l.id).collect();
        self.active = ids.last().copied();
        self.selected = ids.into_iter().collect();
    }

    /// Selects no layer.
    pub fn deselect_layers(&mut self) {
        self.selected.clear();
        self.active = None;
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

    /// Whether there are changes that would be lost on closing: what the image shows (a
    /// pending edit included) would be saved otherwise than the state with nothing to save
    /// was. What is selected is no such change: files do not keep it.
    pub fn is_dirty(&self) -> bool {
        match self.saved_at {
            Some(s) if self.history.contains(s) => !self.doc().saves_as(self.history.node(s).doc()),
            _ => true,
        }
    }

    /// Records an action: `f` changes a copy of the current document, which becomes a new
    /// history state labeled `label`. Nothing is recorded if `f` fails, nor (`f` not called) if
    /// an edit is under way on the image, which the status bar then says. A layer the action
    /// adds becomes the active layer.
    pub fn apply<E>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Document, &mut IdGen) -> Result<(), E>,
    ) -> Result<(), NotMade<E>> {
        if self.refused() {
            return Err(NotMade::Busy);
        }
        self.commit_pending();
        let mut d = self.history.doc().clone();
        let mut ids = self.ids.clone();
        f(&mut d, &mut ids).map_err(NotMade::Failed)?;
        let groups = self.groups.clone();
        self.commit_made(d, ids, label, &groups);
        Ok(())
    }

    /// Makes `made`, worked out from the current state with the identifiers `ids` (which it
    /// gave out from this image's), the next history state, labeled `label`, made by the
    /// script `groups` (see [`group_ended`](Self::group_ended)).
    pub(crate) fn commit_made(&mut self, made: Document, ids: IdGen, label: &str, groups: &[u64]) {
        debug_assert!(self.busy.is_none(), "the image changed while an edit was under way");
        let before = self.history.doc().clone();
        self.ids = ids;
        let id = self.history.commit(made, label);
        for &group in groups {
            self.made_by.entry(group).or_default().insert(id);
        }
        self.revision += 1;
        self.fix_active_layer(Some(&before));
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
        if self.refused() {
            return;
        }
        if self.pending.as_ref().is_some_and(|p| p.label != label) || self.transform.is_some() {
            self.commit_pending();
        }
        let p =
            self.pending.get_or_insert_with(|| Pending { doc: self.history.doc().clone(), label: label.to_owned() });
        f(&mut p.doc);
        self.revision += 1;
    }

    /// Gives up the pending edit, if one labeled `label` is in progress, leaving the document
    /// as it was.
    pub fn discard_pending(&mut self, label: &str) {
        if self.pending_label() == Some(label) {
            self.pending = None;
            self.revision += 1;
        }
    }

    /// Whether a drag on its canvas is under way: one of the tools', or on the transform box.
    pub fn dragging(&self) -> bool {
        self.drag.is_some() || self.transform.as_ref().is_some_and(|s| s.drag.is_some())
    }

    /// What Transform Again does to the image as it is now: the last transform, carried
    /// through what has turned or resized the whole image since it was applied, or back
    /// through what of that has been undone (conjugated by the change of coordinates), so that
    /// it does to the image what it did then. None if no box has been applied to layers.
    pub fn transform_again_map(&self) -> Option<opal_core::geom::Affine> {
        let last = self.last_transform?;
        // From the coordinates it was made in to those of now.
        let change = last.carried.inverse()?.then(self.doc().carried);
        Some(change.inverse()?.then(last.delta).then(change))
    }

    /// Gives up the drag under way, if one is, as Esc does: the edit it has pending
    /// discarded (and the image's symmetry, a drag of it, put back where it began).
    pub fn give_up_drag(&mut self) {
        let label = match self.drag.take() {
            None => return,
            Some(Drag::Move(_)) => Some(crate::move_tool::MOVE),
            Some(Drag::Shape(s)) => Some(crate::select_tool::ShapeDrag::label(s.drag.part)),
            Some(Drag::Outline(_)) => Some(crate::outline_tool::EDIT),
            Some(Drag::Symmetry(s)) => {
                (self.symmetry_center, self.symmetry_angle) = s.was;
                Some(crate::outline_tool::MOVE_SYMMETRY)
            }
            Some(Drag::Crop(_) | Drag::Select(_) | Drag::Reshape(_)) => None,
        };
        match label {
            Some(label) => self.discard_pending(label),
            None => self.revision += 1,
        }
    }

    /// The label of the pending edit, if one is in progress.
    pub fn pending_label(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.label.as_str())
    }

    /// Commits the pending edit, if any, as one history state, made in the groups the
    /// document is in now (as [`apply`](Self::apply) records it); a transform box in
    /// progress is applied first.
    pub fn commit_pending(&mut self) {
        // Neither begins while the image is busy (see `refused`), and the edit commits both first.
        debug_assert!(
            self.busy.is_none() || (self.pending.is_none() && self.transform.is_none()),
            "an edit in progress on an image busy with another"
        );
        self.finish_transform();
        if let Some(mut p) = self.pending.take() {
            // A selection edited to nothing (kept while the edit went on, so that its box
            // stayed to be put right) is no selection once done.
            if p.doc.selection.as_ref().is_some_and(|s| s.is_empty()) {
                p.doc.selection = None;
            }
            // An edit that came back to where it began is no state of its own.
            if p.doc.same_as(self.history.doc()) {
                self.revision += 1;
                return;
            }
            let id = self.history.commit(p.doc, p.label);
            for &group in &self.groups {
                self.made_by.entry(group).or_default().insert(id);
            }
            self.revision += 1;
        }
    }

    /// The selected layers as they look, hidden or not (they were chosen): what Copy and Layer
    /// via Copy take.
    pub(crate) fn selected_as_shown(&self) -> Vec<std::sync::Arc<opal_core::document::Layer>> {
        let doc = self.doc();
        self.selected_layers()
            .iter()
            .filter_map(|&l| doc.layer(l))
            .map(|l| std::sync::Arc::new(opal_core::document::Layer { visible: true, ..l.clone() }))
            .collect()
    }

    /// Why nothing may change the image now, if nothing may: an edit is under way on it (see
    /// `edits`). Told in the status bar, then: what was asked is not done.
    pub(crate) fn refused(&mut self) -> bool {
        let why = self.busy.as_ref().map(crate::edits::Busy::why);
        let refused = why.is_some();
        if why.is_some() {
            self.notice = why;
        }
        refused
    }

    /// Takes back history state `id`, made from `from` (see [`History::take_back`]), unless it
    /// is the current one with an edit in progress on it, or an edit is under way on the image;
    /// whether it did.
    pub fn take_back(&mut self, id: NodeId, from: NodeId) -> bool {
        if self.busy.is_some() || self.history.current() == id && (self.pending.is_some() || self.transform.is_some()) {
            return false;
        }
        let before = self.history.doc().clone();
        let done = self.history.take_back(id, from);
        if done {
            self.revision += 1;
            self.fix_active_layer(Some(&before));
        }
        done
    }

    pub fn undo(&mut self) -> bool {
        if self.refused() {
            return false;
        }
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
        if self.refused() {
            return false;
        }
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
        if self.refused() {
            return;
        }
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
        // None active (Deselect Layers): none still, an edit adding no layer.
        if self.active.is_none_or(|a| now.contains(&a)) {
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
        // Selecting changes nothing a file keeps.
        let all = opal_core::selection::Selection::new(opal_core::selection::Step::Invert);
        s.set_pixel_selection(Some(all), "Select all");
        assert!(!s.is_dirty(), "a selection alone asks to be saved");
        add_layer(&mut s, "y");
        assert!(s.is_dirty());
    }

    #[test]
    fn a_layer_edit_brought_back_where_it_began_makes_no_state() {
        let mut s = state();
        let id = s.active_layer().unwrap();
        let states = s.history.nodes().count();
        let opacity =
            |s: &mut DocState, v: f32| s.edit_pending("Change opacity", |doc| doc.layer_mut(id).unwrap().opacity = v);
        opacity(&mut s, 0.4);
        opacity(&mut s, 1.0);
        s.commit_pending();
        assert_eq!(s.history.nodes().count(), states, "a state that changed nothing");
        assert!(!s.is_dirty());
        opacity(&mut s, 0.4);
        s.commit_pending();
        assert_eq!(s.history.nodes().count(), states + 1);
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
    fn a_turned_view_turns_about_its_middle_and_zooms_scrolls_and_fits_as_it_shows() {
        let near = |a: Vec2, b: Vec2| (a - b).length() < 1e-2;
        let mut v = View {
            zoom: 2.0,
            offset: Vec2::new(30.0, -40.0),
            viewport: Vec2::new(800.0, 600.0),
            placed: true,
            ..View::default()
        };
        let middle = v.to_canvas(v.viewport * 0.5);
        v.turn_to(37.0);
        // The middle stays; every point comes back as it went; canvas x runs 37° below the
        // screen's x, clockwise.
        assert!(near(v.to_canvas(v.viewport * 0.5), middle));
        for p in [Vec2::ZERO, Vec2::new(123.0, 456.0), Vec2::new(-80.0, 900.0)] {
            assert!(near(v.to_screen(v.to_canvas(p)), p), "{p:?}");
        }
        let along = v.to_screen(middle + Vec2::X) - v.to_screen(middle);
        assert!(near(along, Vec2::angled(37f32.to_radians()) * 2.0), "{along:?}");
        // Zooming keeps the point under the pointer; scrolling moves the image as the pointer moves.
        let anchor = Vec2::new(100.0, 500.0);
        let under = v.to_canvas(anchor);
        v.zoom_about(3.0, anchor);
        assert!(near(v.to_canvas(anchor), under));
        let before = v.to_screen(under);
        v.scroll(Vec2::new(25.0, -10.0));
        assert!(near(v.to_screen(under), before + Vec2::new(25.0, -10.0)));
        // Fitting fits the canvas as the view turns it: a long strip turned upright fits by its
        // length down the viewport.
        let strip = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 100.0));
        v.turn_to(90.0);
        v.fit(strip, Vec2::new(800.0, 600.0), 10.0);
        let ends = [v.to_screen(Vec2::ZERO), v.to_screen(Vec2::new(1000.0, 100.0))];
        assert!((ends[0].y - ends[1].y).abs() <= 600.0 && (ends[0].y - ends[1].y).abs() > 500.0, "{ends:?}");
        assert!(near(v.to_screen(strip.center().to_vec2()), Vec2::new(400.0, 300.0)));
        // Angles are kept within a turn; nearly straight is straight.
        v.turn_to(200.0);
        assert_eq!(v.angle, -160.0);
        v.turn_to(360.0004);
        assert_eq!(v.angle, 0.0);
        // Straight at a whole zoom, its pixels are the screen's again.
        v.zoom_about(2.0, Vec2::new(0.3, 0.3));
        v.turn_to(10.0);
        v.scroll(Vec2::new(0.4, 0.3));
        v.turn_to(0.0);
        assert_eq!(v.offset, v.offset.round(), "not crisp");
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
