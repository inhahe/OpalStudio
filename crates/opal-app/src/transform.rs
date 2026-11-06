//! The transform box: placing a paste, and transforming layers.
//!
//! A session keeps the document being transformed as the document's pending edit, so the
//! whole session becomes one history state ("Paste" or "Transform") when committed, and
//! cancelling leaves the document as it was. Any other change to the document commits the
//! session first (see [`DocState::commit_pending`]).
//!
//! The box is a rectangle, `frame`, in its own coordinates, carried into the document by
//! `current`. For one object, the frame is the object's own frame and `current` its
//! transform; for several, the frame is their bounding box and starts untransformed. Every
//! object moves by the box's change since the session began.

use crate::clipboard::Pasted;
use crate::doc::{DocState, Pending};
use crate::image_ops::Drawing;
use crate::memory::Allowance;
use opal_core::document::{
    Document, ImageObject, Layer, LayerContent, LayerId, Object, ObjectId, ObjectKind, PixelObject, Position,
};
use opal_core::geom::{Affine, FRect, IRect};
use opal_core::resample::{self, Sampling};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Paste,
    Transform,
}

/// An object the box moves.
#[derive(Clone, Debug)]
pub struct Target {
    pub layer: LayerId,
    pub object: ObjectId,
    /// Its transform when the session began.
    pub start: Affine,
    /// Whether it was a pixel object, turned into an image object for the session; it
    /// becomes pixels again if it ends where whole pixels can hold it exactly.
    pub was_pixels: bool,
    /// How it was drawn, for an image object, which keeps it unless the box's own drawing
    /// method is changed; `None` to draw it with the box's.
    pub own: Option<Sampling>,
}

/// Where a paste goes.
#[derive(Clone, Debug)]
pub struct PasteInto {
    /// The new layer the pasted image goes on, and where it sits.
    pub layer: LayerId,
    pub position: Position,
    pub name: String,
    /// The layer it can go into instead (baked into its pixels when committed), if there is
    /// a suitable one.
    pub active: Option<LayerId>,
    /// Whether it goes into `active` rather than onto the new layer.
    pub into_active: bool,
}

/// What a drag on the box does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    Rotate,
    /// A corner, clockwise from the frame's top-left (0) to its bottom-left (3).
    Corner(usize),
    /// An edge: top (0), right (1), bottom (2), left (3).
    Edge(usize),
}

/// A drag in progress: what is dragged, where it began (document pixels), and the box's
/// transform then.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub handle: Handle,
    pub from: [f64; 2],
    pub start: Affine,
}

/// The pointer and keys during a drag, and the settings that shape it.
#[derive(Clone, Copy, Debug)]
pub struct DragInput {
    pub pointer: [f64; 2],
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub aspect_locked: bool,
    pub snapping: bool,
    /// How close (document pixels) an edge must come to snap.
    pub snap_distance: f64,
    /// Shift rotates in steps of this many radians.
    pub rotate_step: f64,
}

/// Lines to snap to: x coordinates of vertical lines and y coordinates of horizontal ones.
#[derive(Clone, Debug, Default)]
pub struct SnapTargets {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
}

/// The snap lines a drag ended on, to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guides {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
}

/// The box as numbers: where the frame's top-left corner is (document pixels), its size in
/// the document, its angle in degrees (clockwise), and whether it is mirrored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub angle: f64,
    pub mirrored: bool,
}

pub struct Session {
    /// Tells sessions apart: no two sessions get the same one.
    pub serial: u64,
    pub kind: Kind,
    pub targets: Vec<Target>,
    pub frame: FRect,
    /// The box's transform when the session began, and now.
    pub start: Affine,
    pub current: Affine,
    pub sampling: Sampling,
    pub paste: Option<PasteInto>,
    /// The layers selected before, restored by cancelling (the active one last).
    pub was_selected: Vec<LayerId>,
    pub drag: Option<Drag>,
    pub guides: Guides,
    /// Whether the objects are drawn with draft sampling now (while a drag lasts).
    pub draft: bool,
    /// Whether the box's drawing method was changed (so the session changed something even
    /// if the box never moved).
    pub sampling_changed: bool,
}

/// The smallest scale a drag may leave along an axis, so the box never collapses.
const MIN_SCALE: f64 = 1e-4;

/// A serial for a new session.
fn next_serial() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Session {
    pub fn label(&self) -> &'static str {
        match self.kind {
            Kind::Paste => "Paste",
            Kind::Transform => "Transform",
        }
    }

    /// The box's corners in the document, clockwise from the frame's top-left.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        self.frame.corners().map(|p| self.current.apply(p))
    }

    pub fn params(&self) -> Params {
        let m = &self.current;
        let sx = m.a.hypot(m.b);
        let sy = m.determinant() / sx;
        let [x, y] = m.apply([self.frame.x0, self.frame.y0]);
        Params {
            x,
            y,
            width: sx * self.frame.width(),
            height: sy.abs() * self.frame.height(),
            angle: m.b.atan2(m.a).to_degrees(),
            mirrored: sy < 0.0,
        }
    }

    /// The box with the numbers changed from `before` (what [`params`](Self::params) said)
    /// to `after`, as changes to its transform: the size by scaling along the frame's own
    /// sides, the angle by turning about its top-left corner, then the corner moved to the
    /// position. So whatever the transform holds besides (a shear, after a resize that
    /// stretched a turned image) stays.
    pub fn edited(&self, before: &Params, after: &Params) -> Affine {
        let corner = [self.frame.x0, self.frame.y0];
        let ratio = |a: f64, b: f64| if b > 0.0 { (a / b).max(MIN_SCALE) } else { 1.0 };
        let (kx, ky) = (ratio(after.width, before.width), ratio(after.height, before.height));
        let mut m = Affine::scale(kx, ky).about(corner[0], corner[1]).then(self.current);
        if after.angle != before.angle {
            let [x, y] = m.apply(corner);
            m = m.then(Affine::rotate((after.angle - before.angle).to_radians()).about(x, y));
        }
        let [x, y] = m.apply(corner);
        m.then(Affine::translate(after.x - x, after.y - y))
    }

    /// Where the box would be with the drag `drag` at `input`, and the snap lines used.
    pub fn dragged(&self, drag: &Drag, input: &DragInput, targets: &SnapTargets) -> (Affine, Guides) {
        let start = drag.start;
        let Some(inv) = start.inverse() else { return (start, Guides::default()) };
        let snapping = input.snapping != input.alt;
        let tol = if snapping { input.snap_distance } else { -1.0 };
        let frame = self.frame;
        let corners = frame.corners();
        let center = frame.center();
        match drag.handle {
            Handle::Move => {
                let mut d = [input.pointer[0] - drag.from[0], input.pointer[1] - drag.from[1]];
                if input.shift {
                    d = constrain(d);
                }
                let moved = start.then(Affine::translate(d[0], d[1]));
                let b = moved.bounds(&frame);
                let (sx, gx) = snap(&[b.x0, (b.x0 + b.x1) * 0.5, b.x1], &targets.xs, tol);
                let (sy, gy) = snap(&[b.y0, (b.y0 + b.y1) * 0.5, b.y1], &targets.ys, tol);
                let mut m = moved.then(Affine::translate(sx, sy));
                // A box on the pixel grid stays on it, so a paste at its own size stays crisp.
                if start.grid_map().is_some() {
                    m.e = m.e.round();
                    m.f = m.f.round();
                }
                (m, Guides { xs: gx.into_iter().collect(), ys: gy.into_iter().collect() })
            }
            Handle::Rotate => {
                let c = start.apply(center);
                let a0 = (drag.from[1] - c[1]).atan2(drag.from[0] - c[0]);
                let a1 = (input.pointer[1] - c[1]).atan2(input.pointer[0] - c[0]);
                let mut turn = a1 - a0;
                if input.shift && input.rotate_step > 0.0 {
                    let now = start.b.atan2(start.a);
                    let target = ((now + turn) / input.rotate_step).round() * input.rotate_step;
                    turn = target - now;
                }
                (start.then(Affine::rotate(turn).about(c[0], c[1])), Guides::default())
            }
            Handle::Corner(k) | Handle::Edge(k) => {
                let corner = matches!(drag.handle, Handle::Corner(_));
                // The point dragged, in the frame's coordinates, and the one held still.
                let grabbed = if corner { corners[k] } else { midpoint(corners[k], corners[(k + 1) % 4]) };
                let held = if input.ctrl {
                    center
                } else if corner {
                    corners[(k + 2) % 4]
                } else {
                    midpoint(corners[(k + 2) % 4], corners[(k + 3) % 4])
                };
                // The dragged point moves as far as the pointer has (it was grabbed near, not
                // necessarily on, the pointer), and it is what snaps.
                let at = start.apply(grabbed);
                let to = [at[0] + input.pointer[0] - drag.from[0], at[1] + input.pointer[1] - drag.from[1]];
                // Not onto a line through the held point: that would squash the box flat.
                let h = start.apply(held);
                let off = |lines: &[f64], v: f64| lines.iter().copied().filter(|t| (t - v).abs() >= 0.5).collect();
                let lines = SnapTargets { xs: off(&targets.xs, h[0]), ys: off(&targets.ys, h[1]) };
                let targets = &lines;
                let d0 = [grabbed[0] - held[0], grabbed[1] - held[1]];
                let ratio = |num: f64, den: f64| if den.abs() < 1e-12 { 1.0 } else { num / den };
                let locked = input.aspect_locked != input.shift;
                let (kx, ky) = if corner && !locked {
                    // The dragged point goes wherever the pointer does: each coordinate snaps.
                    let (sx, _) = snap(&[to[0]], &targets.xs, tol);
                    let (sy, _) = snap(&[to[1]], &targets.ys, tol);
                    let p = inv.apply([to[0] + sx, to[1] + sy]);
                    (ratio(p[0] - held[0], d0[0]), ratio(p[1] - held[1], d0[1]))
                } else {
                    // One scale moves the dragged point, along the line from the held point
                    // through it (an edge's midpoint and the one opposite share the frame's
                    // middle line, so an edge moves along it however the lock is set).
                    let p = inv.apply(to);
                    let d = [p[0] - held[0], p[1] - held[1]];
                    let s = if corner {
                        // The pointer projected onto the diagonal through the held point.
                        (d[0] * d0[0] + d[1] * d0[1]) / (d0[0] * d0[0] + d0[1] * d0[1]).max(1e-12)
                    } else if k % 2 == 0 {
                        ratio(d[1], d0[1])
                    } else {
                        ratio(d[0], d0[0])
                    };
                    let s = snap_along(h, [at[0] - h[0], at[1] - h[1]], s, targets, tol);
                    match (corner || locked, k % 2) {
                        (true, _) => (s, s),
                        (false, 0) => (1.0, s),
                        (false, _) => (s, 1.0),
                    }
                };
                let keep = |k: f64| if k.abs() < MIN_SCALE { MIN_SCALE.copysign(k) } else { k };
                let m = Affine::scale(keep(kx), keep(ky)).about(held[0], held[1]).then(start);
                // The lines the dragged point ended on (not those it would have, when the
                // smallest scale stopped it short of them).
                let c = m.apply(grabbed);
                let on = |v: f64, lines: &[f64]| -> Vec<f64> {
                    let hit = |t: f64| tol >= 0.0 && (t - v).abs() <= 1e-9 * t.abs().max(1.0);
                    lines.iter().copied().find(|&t| hit(t)).into_iter().collect()
                };
                (m, Guides { xs: on(c[0], &targets.xs), ys: on(c[1], &targets.ys) })
            }
        }
    }

    /// The transform that fits the box inside `canvas` (or, with `fill`, makes it cover
    /// the canvas), centered, scaled evenly from how it is now (so its angle, proportions
    /// and any shear stay).
    pub fn fitted(&self, canvas: &IRect, fill: bool) -> Affine {
        let m = self.current;
        let Some(inv) = m.inverse() else { return m };
        let c = canvas.to_f().center();
        let mid = m.apply(self.frame.center());
        let half = [canvas.width() as f64 * 0.5, canvas.height() as f64 * 0.5];
        let k = if fill {
            // Each canvas corner, as an offset in the frame from its center, must lie within
            // half the frame.
            let hf = [self.frame.width() * 0.5, self.frame.height() * 0.5];
            let k = canvas.to_f().corners().iter().fold(0.0f64, |k, &q| {
                let a = inv.apply([mid[0] + q[0] - c[0], mid[1] + q[1] - c[1]]);
                let o = inv.apply(mid);
                k.max(((a[0] - o[0]) / hf[0]).abs()).max(((a[1] - o[1]) / hf[1]).abs())
            });
            k.max(MIN_SCALE)
        } else {
            // Each corner of the box, as an offset from its center, must lie within half the
            // canvas.
            self.corners().iter().fold(f64::INFINITY, |k, p| {
                let d = [(p[0] - mid[0]).abs(), (p[1] - mid[1]).abs()];
                let kx = if d[0] > 0.0 { half[0] / d[0] } else { f64::INFINITY };
                let ky = if d[1] > 0.0 { half[1] / d[1] } else { f64::INFINITY };
                k.min(kx).min(ky)
            })
        };
        if !k.is_finite() {
            return m;
        }
        m.then(Affine::translate(c[0] - mid[0], c[1] - mid[1])).then(Affine::scale(k, k).about(c[0], c[1]))
    }
}

fn midpoint(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

/// A movement kept horizontal, vertical or diagonal, whichever is nearest.
pub(crate) fn constrain([x, y]: [f64; 2]) -> [f64; 2] {
    let (ax, ay) = (x.abs(), y.abs());
    if ax > 2.0 * ay {
        [x, 0.0]
    } else if ay > 2.0 * ax {
        [0.0, y]
    } else {
        let m = (ax + ay) * 0.5;
        [m.copysign(x), m.copysign(y)]
    }
}

/// The smallest shift that brings one of `values` onto one of `targets`, if one is within
/// `tol`, with the target it lands on.
pub(crate) fn snap(values: &[f64], targets: &[f64], tol: f64) -> (f64, Option<f64>) {
    let mut best: Option<(f64, f64)> = None;
    for &v in values {
        for &t in targets {
            let d = t - v;
            if d.abs() <= tol && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                best = Some((d, t));
            }
        }
    }
    best.map_or((0.0, None), |(d, t)| (d, Some(t)))
}

/// For a point at `from + s · along` that only moves along that line, as `s` changes: the
/// `s` that puts it on the nearest of `targets`' lines within `tol` of it, measured by how
/// far the point moves, or `s` itself when none is.
fn snap_along(from: [f64; 2], along: [f64; 2], s: f64, targets: &SnapTargets, tol: f64) -> f64 {
    let length = along[0].hypot(along[1]);
    let mut best: Option<(f64, f64)> = None;
    for (axis, lines) in [(0, &targets.xs), (1, &targets.ys)] {
        // A line parallel to the motion is never reached (and when it is the point's line,
        // the point is on it wherever it moves).
        if along[axis] == 0.0 {
            continue;
        }
        for &t in lines {
            let at = (t - from[axis]) / along[axis];
            let moved = (at - s).abs() * length;
            if moved <= tol && best.is_none_or(|(b, _)| moved < b) {
                best = Some((moved, at));
            }
        }
    }
    best.map_or(s, |(_, at)| at)
}

/// The lines things snap to: the canvas's edges and center, and the edges and centers of
/// the visible layers other than `except`.
pub fn snap_targets(doc: &Document, except: &[LayerId]) -> SnapTargets {
    // Layers that show: hidden groups hide everything inside them.
    fn walk(layers: &[Arc<Layer>], except: &[LayerId], t: &mut SnapTargets) {
        for l in layers {
            if !l.visible {
                continue;
            }
            match l.children() {
                Some(c) => walk(c, except, t),
                None => {
                    if !except.contains(&l.id)
                        && let Some(b) = l.bounds()
                    {
                        t.xs.extend([b.x0, b.center()[0], b.x1]);
                        t.ys.extend([b.y0, b.center()[1], b.y1]);
                    }
                }
            }
        }
    }
    let c = doc.canvas.rect().to_f();
    let mut t = SnapTargets { xs: vec![c.x0, c.center()[0], c.x1], ys: vec![c.y0, c.center()[1], c.y1] };
    walk(&doc.layers, except, &mut t);
    t
}

/// The object `id` in layer `layer` of `doc`, for changing.
fn object_mut(doc: &mut Document, layer: LayerId, id: ObjectId) -> Option<&mut Object> {
    match &mut doc.layer_mut(layer)?.content {
        LayerContent::Objects(objects) => objects.iter_mut().find(|o| o.id == id),
        LayerContent::Group(_) => None,
    }
}

/// Turns the pixel objects of `layer` (and of the layers inside it) into image objects,
/// recording every object as a target.
fn take_targets(layer: &mut Layer, sampling: Sampling, out: &mut Vec<(Target, FRect)>) {
    let id = layer.id;
    match &mut layer.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                let was_pixels = matches!(o.kind, ObjectKind::Pixels(_));
                let own = match &o.kind {
                    ObjectKind::Image(i) => Some(i.sampling),
                    ObjectKind::Pixels(_) => None,
                };
                if let ObjectKind::Pixels(p) = &o.kind {
                    let Some(frame) = p.image.content_bounds() else { continue };
                    o.kind = ObjectKind::Image(ImageObject {
                        image: p.image.clone(),
                        frame,
                        transform: Affine::translate(p.offset[0] as f64, p.offset[1] as f64),
                        sampling,
                    });
                }
                if let ObjectKind::Image(i) = &o.kind {
                    let target = Target { layer: id, object: o.id, start: i.transform, was_pixels, own };
                    out.push((target, i.frame.to_f()));
                }
            }
        }
        LayerContent::Group(children) => {
            for c in children {
                take_targets(Arc::make_mut(c), sampling, out);
            }
        }
    }
}

impl DocState {
    /// Starts placing `pasted` on a new layer at `position`, carried into the document by
    /// `place`. `into` is the layer it may go into instead.
    pub fn begin_paste(
        &mut self,
        pasted: &Pasted,
        place: Affine,
        sampling: Sampling,
        position: Position,
        into: Option<LayerId>,
    ) {
        self.commit_pending();
        let was_selected = self.selection_with_active_last();
        let (layer_id, object) = (self.ids.layer(), self.ids.object());
        let frame = IRect::from_size(i64::from(pasted.width), i64::from(pasted.height));
        let image = ImageObject { image: Arc::new(pasted.image.clone()), frame, transform: place, sampling };
        let name = "Pasted image".to_owned();
        let layer =
            Layer::with_objects(layer_id, name.clone(), vec![Object { id: object, kind: ObjectKind::Image(image) }]);
        let mut doc = self.history.doc().clone();
        if doc.insert(position, layer).is_err() {
            return;
        }
        self.pending = Some(Pending { doc, label: "Paste".into() });
        self.transform = Some(Session {
            serial: next_serial(),
            kind: Kind::Paste,
            targets: vec![Target { layer: layer_id, object, start: place, was_pixels: false, own: None }],
            frame: frame.to_f(),
            start: place,
            current: place,
            sampling,
            paste: Some(PasteInto { layer: layer_id, position, name, active: into, into_active: false }),
            was_selected,
            drag: None,
            guides: Guides::default(),
            draft: false,
            sampling_changed: false,
        });
        self.select_only(layer_id);
        self.revision += 1;
    }

    /// Starts transforming the selected layers. Pixel objects become image objects for the
    /// session, drawn with `sampling` (image objects keep their own). Returns whether there
    /// was anything to transform.
    pub fn begin_transform(&mut self, sampling: Sampling) -> bool {
        self.commit_pending();
        let was_selected = self.selection_with_active_last();
        let mut doc = self.history.doc().clone();
        let mut found = Vec::new();
        for id in self.selection() {
            if let Some(l) = doc.layer_mut(id) {
                take_targets(l, sampling, &mut found);
            }
        }
        if found.is_empty() {
            return false;
        }
        let (frame, start) = match &found[..] {
            [(t, frame)] => (*frame, t.start),
            _ => {
                let b = found
                    .iter()
                    .map(|(t, f)| t.start.bounds(f))
                    .reduce(|a, b| FRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
                    .expect("not empty");
                (b, Affine::IDENTITY)
            }
        };
        // The box shows the drawing method of its one image object; with several objects,
        // the default for pixels, which each image object keeps its own method over.
        let sampling = match &found[..] {
            [(t, _)] => t.own.unwrap_or(sampling),
            _ => sampling,
        };
        self.pending = Some(Pending { doc, label: "Transform".into() });
        self.transform = Some(Session {
            serial: next_serial(),
            kind: Kind::Transform,
            targets: found.into_iter().map(|(t, _)| t).collect(),
            frame,
            start,
            current: start,
            sampling,
            paste: None,
            was_selected,
            drag: None,
            guides: Guides::default(),
            draft: false,
            sampling_changed: false,
        });
        self.revision += 1;
        true
    }

    /// Puts the box at `m`, drawing with `draft` sampling (while dragging) or the session's.
    /// Nothing happens if that is how it already is.
    pub fn set_box(&mut self, m: Affine, draft: Option<Sampling>) {
        let Some(s) = self.transform.as_mut() else { return };
        let Some(inv) = s.start.inverse() else { return };
        if m.inverse().is_none() || (m == s.current && draft.is_some() == s.draft) {
            return;
        }
        let delta = inv.then(m);
        // Every object must stay where a file can hold it (see ImageObject::transform_ok);
        // a drag or a typed number past that leaves the box where it was.
        if !s.targets.iter().all(|t| ImageObject::transform_ok(&t.start.then(delta))) {
            return;
        }
        s.current = m;
        s.draft = draft.is_some();
        let boxed = s.sampling;
        let targets = s.targets.clone();
        let Some(p) = self.pending.as_mut() else { return };
        for t in &targets {
            if let Some(Object { kind: ObjectKind::Image(i), .. }) = object_mut(&mut p.doc, t.layer, t.object) {
                i.transform = t.start.then(delta);
                i.sampling = draft.unwrap_or(t.own.unwrap_or(boxed));
            }
        }
        self.revision += 1;
    }

    /// Changes how the session's objects are drawn: all of them, their own methods included.
    pub fn set_box_sampling(&mut self, sampling: Sampling) {
        let Some(s) = self.transform.as_mut() else { return };
        s.sampling = sampling;
        s.sampling_changed = true;
        for t in &mut s.targets {
            t.own = None;
        }
        // Redrawn even though the box stays where it is.
        s.draft = true;
        let m = s.current;
        self.set_box(m, None);
    }

    /// For a paste, switches between placing it on its new layer and putting it into the
    /// active layer (where it is baked into the pixels when committed).
    pub fn toggle_paste_into_active(&mut self) {
        let Some(s) = self.transform.as_mut() else { return };
        let Some(paste) = s.paste.as_mut() else { return };
        let Some(dest) = paste.active else { return };
        let Some(p) = self.pending.as_mut() else { return };
        let t = &mut s.targets[0];
        let Some(object) = object_mut(&mut p.doc, t.layer, t.object).cloned() else { return };
        if paste.into_active {
            // Back onto its own layer.
            if let Some(Layer { content: LayerContent::Objects(objects), .. }) = p.doc.layer_mut(dest) {
                objects.retain(|o| o.id != object.id);
            }
            let layer = Layer::with_objects(paste.layer, paste.name.clone(), vec![object]);
            if p.doc.insert(paste.position, layer).is_err() {
                return;
            }
            t.layer = paste.layer;
        } else {
            let Some(Layer { content: LayerContent::Objects(objects), .. }) = p.doc.layer_mut(dest) else { return };
            objects.push(object);
            let _ = p.doc.remove(paste.layer);
            t.layer = dest;
        }
        paste.into_active = !paste.into_active;
        let active = t.layer;
        self.select_only(active);
        self.revision += 1;
    }

    /// Ends the session, leaving the document as it was.
    pub fn cancel_transform(&mut self) {
        let Some(s) = self.transform.take() else { return };
        self.pending = None;
        self.revision += 1;
        let doc = self.history.doc();
        let kept: Vec<LayerId> = s.was_selected.into_iter().filter(|&l| doc.layer(l).is_some()).collect();
        if let Some(&active) = kept.last() {
            self.select_only(active);
            for &l in &kept {
                if !self.is_selected(l) {
                    self.toggle_selected(l);
                }
            }
            self.make_active(active);
        }
    }

    /// Makes the session's result final in the pending document (called just before it is
    /// committed): the chosen sampling everywhere, pixel objects that end on the pixel grid
    /// back as pixels, and a paste into the active layer baked into it. A transform that
    /// changed nothing is dropped instead, leaving no history state.
    pub(crate) fn finish_transform(&mut self) {
        let unchanged = self
            .transform
            .as_ref()
            .is_some_and(|s| s.kind == Kind::Transform && s.current == s.start && !s.sampling_changed);
        if unchanged {
            return self.cancel_transform();
        }
        let Some(s) = self.transform.take() else { return };
        let Some(p) = self.pending.as_mut() else { return };
        // Pixel objects ending exactly on the grid are pixels again (see `image_ops::on_grid`):
        // moved, the same pixels; turned or flipped, copied exactly if there is memory for
        // it, else left images, which are drawn as exactly.
        let mut to = Drawing::new(p.doc.canvas.rect(), Allowance::now());
        let (mut turned, mut refused) = (0, Vec::new());
        for t in &s.targets {
            let Some(o) = object_mut(&mut p.doc, t.layer, t.object) else { continue };
            let ObjectKind::Image(i) = &mut o.kind else { continue };
            i.sampling = t.own.unwrap_or(s.sampling);
            if t.was_pixels
                && let Some(g) = i.transform.grid_map()
            {
                turned += 1;
                match crate::image_ops::on_grid(&i.image, &i.transform, &g, &i.sampling, &mut to) {
                    Ok(pixels) => o.kind = pixels,
                    Err(why) => refused.push(why),
                }
            }
        }
        if let Some(why) = refused.first() {
            let which =
                if turned == 1 { "The layer".to_string() } else { format!("{} of the {turned} layers", refused.len()) };
            self.notice = Some(format!("{which} stay images, drawn as exactly, not as pixels: {why}"));
        } else if to.clipped {
            self.notice = Some(crate::layer_ops::CLIPPED.into());
        }
        if let Some(paste) = &s.paste
            && paste.into_active
            && let Some(dest) = paste.active
        {
            match bake_into(&mut p.doc, dest, s.targets[0].object, to.memory()) {
                Ok(true) => self.notice = Some(crate::layer_ops::CLIPPED.into()),
                Ok(false) => {}
                Err(why) => {
                    self.notice =
                        Some(format!("The paste stays an image in the layer, not drawn into its pixels: {why}"))
                }
            }
        }
    }

    /// The selected layers with the active one last (to restore them).
    fn selection_with_active_last(&self) -> Vec<LayerId> {
        let mut v: Vec<LayerId> = self.selection().into_iter().filter(|&l| Some(l) != self.active_layer()).collect();
        v.extend(self.active_layer());
        v
    }
}

/// Draws image object `object` of layer `dest` into the layer's pixels: into its topmost
/// pixel object if it has one there, else as a new pixel object.
/// Returns whether part of it, reaching too far past the canvas, was left out (see
/// `layer_ops::drawable`), or, leaving it as it is, why it cannot be drawn: more than
/// `memory` (what the rest of the work left) allows.
fn bake_into(doc: &mut Document, dest: LayerId, object: ObjectId, memory: &mut Allowance) -> Result<bool, String> {
    let canvas = doc.canvas.rect();
    let Some(Layer { content: LayerContent::Objects(objects), .. }) = doc.layer_mut(dest) else { return Ok(false) };
    let Some(at) = objects.iter().position(|o| o.id == object) else { return Ok(false) };
    let ObjectKind::Image(i) = &objects[at].kind else { return Ok(false) };
    let drawable = crate::layer_ops::drawable(&i.image, &i.transform, &i.sampling, &canvas, memory)?;
    let ObjectKind::Image(i) = objects.remove(at).kind else { unreachable!("an image object, just seen") };
    // An image that shows nothing goes, leaving nothing.
    let Some((area, clipped)) = drawable else { return Ok(false) };
    match objects.last_mut() {
        Some(Object { kind: ObjectKind::Pixels(p), .. }) => {
            let local = i.transform.then(Affine::translate(-p.offset[0] as f64, -p.offset[1] as f64));
            let area = area.translate(-p.offset[0], -p.offset[1]);
            let drawn = resample::render_image(&i.image, &local, &i.sampling, Some(&area));
            Arc::make_mut(&mut p.image).draw_over(&drawn);
        }
        _ => {
            let drawn = resample::render_image(&i.image, &i.transform, &i.sampling, Some(&area));
            objects.push(Object {
                id: object,
                kind: ObjectKind::Pixels(PixelObject { image: Arc::new(drawn), offset: [0, 0] }),
            });
        }
    }
    Ok(clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(frame: FRect, current: Affine) -> Session {
        Session {
            serial: next_serial(),
            kind: Kind::Transform,
            targets: Vec::new(),
            frame,
            start: current,
            current,
            sampling: Sampling::default(),
            paste: None,
            was_selected: Vec::new(),
            drag: None,
            guides: Guides::default(),
            draft: false,
            sampling_changed: false,
        }
    }

    fn input(pointer: [f64; 2]) -> DragInput {
        DragInput {
            pointer,
            shift: false,
            ctrl: false,
            alt: false,
            aspect_locked: true,
            snapping: false,
            snap_distance: 5.0,
            rotate_step: 15f64.to_radians(),
        }
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn a_layer_turned_back_on_the_grid_is_pixels_again_if_the_copy_fits() {
        // Flipped across the canvas: a copy, exactly.
        let flip = |s: &mut DocState| {
            assert!(s.begin_transform(Sampling::default()));
            let m = s.transform.as_ref().unwrap().current.then(Affine::scale(-1.0, 1.0).about(25.0, 25.0));
            s.set_box(m, None);
            s.commit_pending();
        };
        let only = |s: &DocState| s.doc().layers[0].objects()[0].kind.clone();
        let mut s = doc_state();
        flip(&mut s);
        let ObjectKind::Pixels(p) = only(&s) else { panic!("not pixels again") };
        assert_eq!(p.image.pixel(46, 3), opal_core::tile::pixel_from_f32([1.0; 4]), "not flipped across x = 25");
        assert!(s.notice.is_none());
        // With no memory free, it stays an image, drawn as exactly, and says so.
        let mut s = doc_state();
        let _none = crate::memory::testing::pretend_free(Some(0));
        flip(&mut s);
        assert!(matches!(only(&s), ObjectKind::Image(_)), "drawn into pixels all the same");
        let notice = s.notice.clone().unwrap_or_default();
        assert!(notice.starts_with("The layer stay") && notice.contains("memory"), "{notice}");
    }

    #[test]
    fn a_paste_showing_nothing_goes_when_drawn_into_the_layer() {
        use opal_core::document::IdGen;
        use opal_core::raster::RasterImage;
        let mut ids = IdGen::default();
        let mut doc = Document::new(100, 100);
        let image = ImageObject {
            image: Arc::new(RasterImage::new()),
            frame: IRect::from_size(10, 10),
            transform: Affine::scale(2.0, 2.0),
            sampling: Sampling::default(),
        };
        let (layer, object) = (ids.layer(), ids.object());
        let pasted = Object { id: object, kind: ObjectKind::Image(image) };
        doc.insert(Position { parent: None, index: 0 }, Layer::with_objects(layer, "l", vec![pasted])).unwrap();
        assert_eq!(bake_into(&mut doc, layer, object, &mut Allowance::memory(None)), Ok(false));
        assert!(doc.layer(layer).unwrap().objects().is_empty(), "an empty image object stayed");
    }

    #[test]
    fn a_paste_too_large_to_draw_into_the_layer_stays_an_image() {
        use opal_core::document::IdGen;
        use opal_core::raster::RasterImage;
        let mut ids = IdGen::default();
        let mut doc = Document::new(300_000, 300_000);
        let mut img = RasterImage::new();
        img.set_pixel(0, 0, opal_core::tile::pixel_from_f32([1.0; 4]));
        img.set_pixel(255, 255, opal_core::tile::pixel_from_f32([1.0; 4]));
        // Enlarged to cover most of the canvas: some 650 GB drawn.
        let image = ImageObject {
            image: Arc::new(img),
            frame: IRect::from_size(256, 256),
            transform: Affine::scale(1100.0, 1100.0),
            sampling: Sampling::default(),
        };
        let (layer, object) = (ids.layer(), ids.object());
        let pasted = Object { id: object, kind: ObjectKind::Image(image) };
        doc.insert(Position { parent: None, index: 0 }, Layer::with_objects(layer, "l", vec![pasted])).unwrap();
        let why = bake_into(&mut doc, layer, object, &mut Allowance::memory(Some(16_000_000_000))).unwrap_err();
        assert!(why.contains("memory"), "{why}");
        let objects = doc.layer(layer).unwrap().objects();
        assert!(matches!(objects, [Object { kind: ObjectKind::Image(_), .. }]), "the paste was not kept as it was");
    }

    #[test]
    fn transforming_several_objects_keeps_each_image_s_own_drawing_method() {
        use crate::doc::DocId;
        use opal_core::document::IdGen;
        use opal_core::raster::RasterImage;
        use opal_core::resample::Method;
        use opal_core::tile::pixel_from_f32;
        let mut ids = IdGen::default();
        let mut img = RasterImage::new();
        img.set_pixel(1, 1, pixel_from_f32([1.0; 4]));
        let nearest = Sampling { method: Method::Nearest, anti_ringing: false };
        let art = ImageObject {
            image: Arc::new(img.clone()),
            frame: IRect::from_size(4, 4),
            transform: Affine::IDENTITY,
            sampling: nearest,
        };
        let mut doc = Document::new(50, 50);
        let (a, b) = (ids.layer(), ids.layer());
        doc.insert(Position { parent: None, index: 0 }, Layer::new_raster(a, ids.object(), "photo", img)).unwrap();
        let art_layer = Layer::with_objects(b, "art", vec![Object { id: ids.object(), kind: ObjectKind::Image(art) }]);
        doc.insert(Position { parent: None, index: 1 }, art_layer).unwrap();
        let mut s = DocState::new(DocId(1), "t".into(), doc, ids, "New");
        s.select_only(a);
        s.toggle_selected(b);
        assert!(s.begin_transform(Sampling::default()));
        let m = s.transform.as_ref().unwrap().current.then(Affine::translate(3.0, 0.0));
        s.set_box(m, None);
        s.commit_pending();
        let ObjectKind::Image(i) = &s.doc().layer(b).unwrap().objects()[0].kind else { panic!() };
        assert_eq!(i.sampling, nearest);
    }

    fn doc_state() -> DocState {
        use opal_core::document::IdGen;
        use opal_core::raster::RasterImage;
        use opal_core::tile::pixel_from_f32;
        let mut ids = IdGen::default();
        let mut img = RasterImage::new();
        img.set_pixel(3, 3, pixel_from_f32([1.0; 4]));
        let mut doc = Document::new(50, 50);
        doc.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "l", img))
            .unwrap();
        DocState::new(crate::doc::DocId(1), "t".into(), doc, ids, "New")
    }

    #[test]
    fn a_transform_that_changed_nothing_leaves_no_history_state() {
        let mut s = doc_state();
        let states = s.history.nodes().count();
        assert!(s.begin_transform(Sampling::default()));
        s.commit_pending();
        assert_eq!(s.history.nodes().count(), states);
        assert!(!s.is_dirty());
        assert!(s.transform.is_none());
    }

    #[test]
    fn the_box_stays_within_what_a_file_can_hold() {
        let mut s = doc_state();
        assert!(s.begin_transform(Sampling::default()));
        let before = s.transform.as_ref().unwrap().current;
        // Moved two trillion pixels away: past what a saved file may hold.
        s.set_box(before.then(Affine::translate(2e12, 0.0)), None);
        assert_eq!(s.transform.as_ref().unwrap().current, before);
    }

    #[test]
    fn layers_in_hidden_groups_are_not_snapped_to() {
        let mut doc = Document::new(100, 100);
        let mut ids = opal_core::document::IdGen::default();
        let g = ids.layer();
        let mut group = Layer::new_group(g, "g");
        group.visible = false;
        doc.insert(Position { parent: None, index: 0 }, group).unwrap();
        let mut img = opal_core::raster::RasterImage::new();
        img.set_pixel(37, 37, opal_core::tile::pixel_from_f32([1.0; 4]));
        doc.insert(Position { parent: Some(g), index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "x", img))
            .unwrap();
        let t = snap_targets(&doc, &[]);
        assert!(!t.xs.contains(&37.0), "{:?}", t.xs);
    }

    #[test]
    fn editing_a_number_keeps_a_shear() {
        // A turned image stretched one way afterwards: its frame is sheared in the document.
        let m = Affine::rotate(0.4).then(Affine::scale(0.5, 1.0));
        let s = session(FRect::new(0.0, 0.0, 100.0, 50.0), m);
        let p = s.params();
        let wider = s.edited(&p, &Params { width: p.width * 2.0, ..p });
        // Only the frame's top edge doubles: the left edge stays as it was.
        let left = |m: Affine| {
            [m.apply([0.0, 50.0])[0] - m.apply([0.0, 0.0])[0], m.apply([0.0, 50.0])[1] - m.apply([0.0, 0.0])[1]]
        };
        assert!(close(left(wider), left(m)), "the shear was lost");
        let top = |m: Affine| m.apply([100.0, 0.0])[0] - m.apply([0.0, 0.0])[0];
        assert!((top(wider) - 2.0 * top(m)).abs() < 1e-9);
    }

    #[test]
    fn a_corner_grabbed_off_center_moves_with_the_pointer_without_jumping() {
        let s = session(FRect::new(0.0, 0.0, 100.0, 100.0), Affine::IDENTITY);
        // Grabbed 5 pixels inside the corner, then not moved.
        let drag = Drag { handle: Handle::Corner(2), from: [95.0, 95.0], start: s.current };
        let (m, _) = s.dragged(&drag, &input([95.0, 95.0]), &SnapTargets::default());
        assert!(close(m.apply([100.0, 100.0]), [100.0, 100.0]), "the corner jumped to the pointer");
    }

    #[test]
    fn a_corner_drag_scales_about_the_opposite_corner_keeping_the_aspect() {
        let s = session(FRect::new(0.0, 0.0, 100.0, 50.0), Affine::translate(10.0, 10.0));
        let drag = Drag { handle: Handle::Corner(2), from: [110.0, 60.0], start: s.current };
        let (m, _) = s.dragged(&drag, &input([210.0, 70.0]), &SnapTargets::default());
        let mut s2 = session(s.frame, m);
        s2.current = m;
        let c = s2.corners();
        assert!(close(c[0], [10.0, 10.0]), "the opposite corner stays: {:?}", c[0]);
        let p = s2.params();
        assert!((p.width / p.height - 2.0).abs() < 1e-9, "aspect kept: {p:?}");
        // Shift frees it: the corner follows the pointer.
        let (m, _) = s.dragged(&drag, &DragInput { shift: true, ..input([210.0, 70.0]) }, &SnapTargets::default());
        assert!(close(m.apply([100.0, 50.0]), [210.0, 70.0]));
    }

    #[test]
    fn an_edge_drag_with_the_lock_open_scales_one_side() {
        let s = session(FRect::new(0.0, 0.0, 100.0, 50.0), Affine::IDENTITY);
        let drag = Drag { handle: Handle::Edge(1), from: [100.0, 25.0], start: s.current };
        let free = DragInput { aspect_locked: false, ..input([150.0, 40.0]) };
        let (m, _) = s.dragged(&drag, &free, &SnapTargets::default());
        assert!(close(m.apply([100.0, 50.0]), [150.0, 50.0]), "only the width changes");
        assert!(close(m.apply([0.0, 0.0]), [0.0, 0.0]), "the left edge stays");
    }

    #[test]
    fn ctrl_scales_about_the_center() {
        let s = session(FRect::new(0.0, 0.0, 100.0, 100.0), Affine::IDENTITY);
        let drag = Drag { handle: Handle::Corner(2), from: [100.0, 100.0], start: s.current };
        let (m, _) = s.dragged(&drag, &DragInput { ctrl: true, ..input([150.0, 150.0]) }, &SnapTargets::default());
        assert!(close(m.apply([50.0, 50.0]), [50.0, 50.0]));
        assert!(close(m.apply([100.0, 100.0]), [150.0, 150.0]));
    }

    #[test]
    fn moves_snap_and_shift_rotation_steps() {
        let s = session(FRect::new(0.0, 0.0, 100.0, 100.0), Affine::IDENTITY);
        let targets = SnapTargets { xs: vec![500.0], ys: vec![] };
        let drag = Drag { handle: Handle::Move, from: [50.0, 50.0], start: s.current };
        let snapping = DragInput { snapping: true, ..input([447.0, 50.0]) };
        let (m, g) = s.dragged(&drag, &snapping, &targets);
        assert_eq!((m.e, m.f), (400.0, 0.0), "the right edge snaps to 500");
        assert_eq!(g.xs, vec![500.0]);
        let (m, _) = s.dragged(&drag, &DragInput { alt: true, ..snapping }, &targets);
        assert_eq!(m.e, 397.0, "Alt turns snapping off for the drag");

        let drag = Drag { handle: Handle::Rotate, from: [150.0, 50.0], start: s.current };
        let (m, _) = s.dragged(&drag, &DragInput { shift: true, ..input([140.0, 90.0]) }, &targets);
        let angle = m.b.atan2(m.a).to_degrees();
        assert!((angle - 30.0).abs() < 1e-9, "steps of 15°: {angle}");
    }

    #[test]
    fn a_scaled_corner_or_edge_lands_on_the_line_it_snaps_to() {
        let s = session(FRect::new(0.0, 0.0, 100.0, 50.0), Affine::IDENTITY);
        let targets = SnapTargets { xs: vec![500.0], ys: vec![27.0, 242.0] };
        let on = |p: [f64; 2]| DragInput { snapping: true, ..input(p) };
        // The aspect kept: the corner moves along the diagonal, and stops on whichever line
        // it is nearer along it.
        let corner = Drag { handle: Handle::Corner(2), from: [100.0, 50.0], start: s.current };
        let (m, g) = s.dragged(&corner, &on([497.0, 248.5]), &targets);
        assert!(close(m.apply([100.0, 50.0]), [500.0, 250.0]), "{:?}", m.apply([100.0, 50.0]));
        assert_eq!((g.xs, g.ys), (vec![500.0], vec![]));
        let (m, g) = s.dragged(&corner, &on([480.0, 243.0]), &targets);
        assert!(close(m.apply([100.0, 50.0]), [484.0, 242.0]), "{:?}", m.apply([100.0, 50.0]));
        assert_eq!((g.xs, g.ys), (vec![], vec![242.0]));
        assert!(close(m.apply([0.0, 0.0]), [0.0, 0.0]), "the opposite corner moved");
        // A wide box's corner 3 pixels from a line across it: reaching that would take the
        // corner 300 pixels along the diagonal, so it does not snap.
        let wide = session(FRect::new(0.0, 0.0, 1000.0, 10.0), Affine::IDENTITY);
        let far = Drag { handle: Handle::Corner(2), from: [1000.0, 10.0], start: wide.current };
        let (m, g) = wide.dragged(&far, &on([2000.0, 20.0]), &SnapTargets { xs: vec![], ys: vec![23.0] });
        assert!(close(m.apply([1000.0, 10.0]), [2000.0, 20.0]), "{:?}", m.apply([1000.0, 10.0]));
        assert!(g.ys.is_empty());

        // Shrunk toward the held corner, which lies on lines of its own: not flattened onto them.
        let at_origin = SnapTargets { xs: vec![0.0, 500.0], ys: vec![0.0, 500.0] };
        let near = Drag { handle: Handle::Corner(2), from: [100.0, 50.0], start: s.current };
        for aspect_locked in [true, false] {
            let (m, g) = s.dragged(&near, &DragInput { aspect_locked, ..on([4.0, 2.0]) }, &at_origin);
            assert!(close(m.apply([100.0, 50.0]), [4.0, 2.0]), "locked {aspect_locked}: {:?}", m.apply([100.0, 50.0]));
            assert_eq!((g.xs, g.ys), (vec![], vec![]));
        }

        // An edge moves one way only: it snaps that way, and shows no line it does not touch.
        for aspect_locked in [false, true] {
            let edge = Drag { handle: Handle::Edge(1), from: [100.0, 25.0], start: s.current };
            let (m, g) = s.dragged(&edge, &DragInput { aspect_locked, ..on([497.0, 26.0]) }, &targets);
            assert!((m.apply([100.0, 25.0])[0] - 500.0).abs() < 1e-6, "locked {aspect_locked}");
            assert_eq!((g.xs, g.ys), (vec![500.0], vec![]), "locked {aspect_locked}");
        }

        // Turned, the edge moves at a slant, and still stops on the line.
        let turned = session(s.frame, Affine::rotate(30f64.to_radians()));
        let start = turned.current;
        let edge = Drag { handle: Handle::Edge(1), from: start.apply([100.0, 25.0]), start };
        let free = DragInput { aspect_locked: false, ..on(start.apply([590.0, 25.0])) };
        let (m, g) = turned.dragged(&edge, &free, &targets);
        assert!((m.apply([100.0, 25.0])[0] - 500.0).abs() < 1e-6, "{:?}", m.apply([100.0, 25.0]));
        assert_eq!(g.xs, vec![500.0]);
    }

    #[test]
    fn params_round_trip_and_fit_keeps_inside_the_canvas() {
        let s = session(FRect::new(0.0, 0.0, 300.0, 100.0), Affine::IDENTITY);
        let p = Params { x: 20.0, y: 30.0, width: 150.0, height: 75.0, angle: 30.0, mirrored: false };
        let mut s2 = session(s.frame, s.edited(&s.params(), &p));
        let q = s2.params();
        for (a, b) in [(p.x, q.x), (p.y, q.y), (p.width, q.width), (p.height, q.height), (p.angle, q.angle)] {
            assert!((a - b).abs() < 1e-9, "{p:?} vs {q:?}");
        }
        let canvas = IRect::from_size(400, 400);
        s2.current = s2.fitted(&canvas, false);
        let b = FRect::bounding(&s2.corners());
        assert!(b.x0 >= -1e-6 && b.y0 >= -1e-6 && b.x1 <= 400.0 + 1e-6 && b.y1 <= 400.0 + 1e-6, "{b:?}");
        assert!((b.x1 - b.x0 - 400.0).abs() < 1e-6 || (b.y1 - b.y0 - 400.0).abs() < 1e-6, "it touches two sides");
        s2.current = s2.fitted(&canvas, true);
        let inv = s2.current.inverse().unwrap();
        for p in canvas.to_f().corners() {
            let q = inv.apply(p);
            assert!(
                q[0] >= -1e-6 && q[0] <= 300.0 + 1e-6 && q[1] >= -1e-6 && q[1] <= 100.0 + 1e-6,
                "fill covers the canvas"
            );
        }
    }
}
