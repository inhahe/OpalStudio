//! Moving the selected layers by whole pixels: dragging with the Move tool, and nudging
//! with the arrow keys.

use crate::doc::{DocState, Pending};
use crate::transform::{Guides, constrain, snap, snap_targets};
use opal_core::document::{Document, Layer, LayerContent, LayerId, ObjectKind};
use opal_core::geom::{Affine, FRect};
use std::sync::Arc;

/// The edit a Move tool drag makes, pending while it goes.
pub const MOVE: &str = "Move";

/// The Move tool's drag under way on `d`, if one is.
pub fn moving(d: &DocState) -> Option<&MoveDrag> {
    match &d.drag {
        Some(crate::doc::Drag::Move(m)) => Some(m),
        _ => None,
    }
}

/// A Move tool drag in progress.
#[derive(Clone, Debug)]
pub struct MoveDrag {
    /// Where it began, in document pixels.
    pub from: [f64; 2],
    /// The bounds of what moves, where it began (for snapping).
    pub bounds: Option<FRect>,
    /// The snap lines in use, to show.
    pub guides: Guides,
    /// The move shown so far, if any.
    pub shown: Option<[i64; 2]>,
}

/// Moves everything in `layer` (and the layers inside it) by `d` whole pixels.
fn shift(layer: &mut Layer, d: [i64; 2]) {
    match &mut layer.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                match &mut o.kind {
                    ObjectKind::Pixels(p) => {
                        p.offset = [p.offset[0] + d[0], p.offset[1] + d[1]];
                    }
                    ObjectKind::Image(i) => {
                        i.transform = i.transform.then(Affine::translate(d[0] as f64, d[1] as f64));
                    }
                }
            }
        }
        LayerContent::Group(children) => {
            for c in children {
                shift(Arc::make_mut(c), d);
            }
        }
    }
}

/// `doc` with layers `ids` moved by `d`.
fn moved(doc: &Document, ids: &[LayerId], d: [i64; 2]) -> Document {
    let mut doc = doc.clone();
    for &id in ids {
        if let Some(l) = doc.layer_mut(id) {
            shift(l, d);
        }
    }
    doc
}

/// The layer whose content shows at document point `p` at least `min_alpha` opaque, the
/// topmost such; with `group`, the group around it instead (if it is in one).
pub fn pick(d: &DocState, p: [f64; 2], min_alpha: f32, group: bool) -> Option<LayerId> {
    let doc = d.doc();
    let hit = opal_core::composite::layers_at(&doc.layers, p[0].floor() as i64, p[1].floor() as i64)
        .into_iter()
        .find(|&(_, a)| a >= min_alpha)?
        .0;
    if group { doc.position(hit).and_then(|pos| pos.parent).or(Some(hit)) } else { Some(hit) }
}

/// The name of a locked layer among the selected layers (or inside a selected group, or a
/// locked group one is in), if any.
pub fn locked_in_selected_layers(d: &DocState) -> Option<String> {
    let doc = d.doc();
    d.selected_layers().iter().find_map(|&id| crate::commands::locked_for(doc, doc.layer(id)?)).map(|l| l.name.clone())
}

impl DocState {
    /// The bounds of the selected layers' content.
    pub fn selected_layers_bounds(&self) -> Option<FRect> {
        let doc = self.doc();
        self.selected_layers()
            .iter()
            .filter_map(|&l| doc.layer(l)?.bounds())
            .reduce(|a, b| FRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
    }

    /// Shows the selected layers moved by `d` from where they are in history, as a pending
    /// "Move" (committed when the drag ends). Moving by nothing leaves no pending edit.
    pub fn move_selected_layers_to(&mut self, d: [i64; 2]) {
        if self.pending_label().is_some_and(|l| l != MOVE) || self.transform.is_some() {
            self.commit_pending();
        }
        self.pending = (d != [0, 0])
            .then(|| Pending { doc: moved(self.history.doc(), &self.selected_layers(), d), label: MOVE.into() });
        self.revision += 1;
    }

    /// Moves the selected layers by `d` more, as a pending "Nudge" that gathers consecutive
    /// nudges into one history state (committed when the keys are let go).
    pub fn nudge_selected_layers(&mut self, d: [i64; 2]) {
        let ids = self.selected_layers();
        self.edit_pending("Nudge", |doc| {
            for id in ids {
                if let Some(l) = doc.layer_mut(id) {
                    shift(l, d);
                }
            }
        });
    }

    /// Where a Move tool drag from `drag.from` to `pointer` puts the selection: whole
    /// pixels, kept horizontal, vertical or diagonal with `shift`, snapped within
    /// `snap_distance` (document pixels; negative for none).
    pub fn move_delta(
        &self,
        drag: &MoveDrag,
        pointer: [f64; 2],
        shift: bool,
        snap_distance: f64,
    ) -> ([i64; 2], Guides) {
        let mut d = [pointer[0] - drag.from[0], pointer[1] - drag.from[1]];
        if shift {
            d = constrain(d);
        }
        let mut guides = Guides::default();
        if let Some(b) = drag.bounds
            && snap_distance >= 0.0
        {
            // What moves never snaps to itself: the layers inside a moved group included.
            let targets = snap_targets(self.history.doc(), &self.selected_layers_and_inside());
            let (x0, x1, y0, y1) = (b.x0 + d[0], b.x1 + d[0], b.y0 + d[1], b.y1 + d[1]);
            let (gx, tx) = snap(&[x0, (x0 + x1) * 0.5, x1], &targets.xs, snap_distance);
            let (gy, ty) = snap(&[y0, (y0 + y1) * 0.5, y1], &targets.ys, snap_distance);
            d = [d[0] + gx, d[1] + gy];
            guides = Guides { xs: tx.into_iter().collect(), ys: ty.into_iter().collect() };
        }
        ([d[0].round() as i64, d[1].round() as i64], guides)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::DocId;
    use opal_core::document::{IdGen, Position};
    use opal_core::raster::RasterImage;
    use opal_core::tile::pixel_from_f32;

    fn doc_with_dot() -> (DocState, LayerId) {
        let mut ids = IdGen::default();
        let mut img = RasterImage::new();
        img.set_pixel(5, 5, pixel_from_f32([1.0; 4]));
        let mut d = Document::new(100, 100);
        let l = ids.layer();
        d.insert(Position { parent: None, index: 0 }, Layer::new_raster(l, ids.object(), "dot", img)).unwrap();
        (DocState::new(DocId(1), "t".into(), d, ids, "New"), l)
    }

    #[test]
    fn a_move_is_one_history_state_and_moves_by_whole_pixels() {
        let (mut s, l) = doc_with_dot();
        s.move_selected_layers_to([3, 0]);
        s.move_selected_layers_to([10, -2]);
        s.commit_pending();
        assert_eq!(s.history.undo_label(), Some("Move"));
        let ObjectKind::Pixels(p) = &s.doc().layer(l).unwrap().objects()[0].kind else { panic!() };
        assert_eq!(p.offset, [10, -2], "moved from where it was, not added up");
        assert!(s.undo());
        let ObjectKind::Pixels(p) = &s.doc().layer(l).unwrap().objects()[0].kind else { panic!() };
        assert_eq!(p.offset, [0, 0]);
    }

    #[test]
    fn a_move_by_nothing_records_nothing() {
        let (mut s, _) = doc_with_dot();
        let states = s.history.nodes().count();
        s.move_selected_layers_to([4, 0]);
        s.move_selected_layers_to([0, 0]);
        s.commit_pending();
        assert_eq!(s.history.nodes().count(), states);
        assert!(!s.is_dirty());
    }

    #[test]
    fn a_moved_group_does_not_snap_to_its_own_layers() {
        let (mut s, dot) = doc_with_dot();
        let g = s.ids.layer();
        s.apply::<opal_core::document::TreeError>("Group", |d, _| {
            d.insert(Position { parent: None, index: 1 }, Layer::new_group(g, "g"))?;
            d.move_layer(dot, Position { parent: Some(g), index: 0 })
        })
        .unwrap();
        s.select_only(g);
        let drag =
            MoveDrag { from: [5.0, 5.0], bounds: s.selected_layers_bounds(), guides: Guides::default(), shown: None };
        // Two pixels right: within snapping distance of the dot's own edges, which must not
        // pull it back.
        let (d, _) = s.move_delta(&drag, [7.0, 5.0], false, 3.0);
        assert_eq!(d, [2, 0]);
    }

    #[test]
    fn nudges_add_up_into_one_state() {
        let (mut s, l) = doc_with_dot();
        s.nudge_selected_layers([1, 0]);
        s.nudge_selected_layers([1, 0]);
        s.nudge_selected_layers([0, 10]);
        s.commit_pending();
        assert_eq!(s.history.undo_label(), Some("Nudge"));
        let ObjectKind::Pixels(p) = &s.doc().layer(l).unwrap().objects()[0].kind else { panic!() };
        assert_eq!(p.offset, [2, 10]);
    }

    #[test]
    fn moves_snap_to_the_canvas_edges() {
        let (s, _) = doc_with_dot();
        let drag =
            MoveDrag { from: [5.0, 5.0], bounds: s.selected_layers_bounds(), guides: Guides::default(), shown: None };
        // The dot (5..6) dragged to x = 96.6: its right edge, at 97.6, snaps to the canvas's at 100.
        let (d, g) = s.move_delta(&drag, [96.6, 5.0], false, 3.0);
        assert_eq!(d, [94, 0]);
        assert_eq!(g.xs, vec![100.0]);
        let (d, _) = s.move_delta(&drag, [96.6, 5.0], false, -1.0);
        assert_eq!(d, [92, 0], "no snapping: just rounded");
    }
}
