//! What the layer commands do: adding, removing, arranging, merging and rasterizing.
//!
//! Commands that act on several layers act on the selection ([`DocState::selection`]): the
//! selected layers in stacking order, each once (a layer inside a selected group counts as
//! part of the group).

use crate::doc::{DocId, DocState};
use crate::state::{AppState, MessageKind};
use opal_core::composite::composite_all;
use opal_core::document::{
    Document, IdGen, Layer, LayerContent, LayerId, Object, ObjectKind, PixelObject, Position, TreeError,
};
use opal_core::geom::IRect;
use opal_core::resample;
use std::sync::Arc;

/// The most pixels merging or rasterizing may produce (2 GiB at 8 bytes a pixel). An
/// image object enlarged far past the canvas could otherwise ask for more memory than
/// the computer has.
const MAX_PIXELS: i64 = 1 << 28;

impl AppState {
    /// Where a new layer goes: just above the active layer, in its group.
    fn above_active(d: &DocState) -> Position {
        d.active_layer()
            .and_then(|a| d.doc().position(a))
            .map(|p| Position { parent: p.parent, index: p.index + 1 })
            .unwrap_or(Position { parent: None, index: d.doc().layers.len() })
    }

    pub(crate) fn new_layer(&mut self, group: bool) {
        let d = self.active_mut().expect("available");
        let pos = Self::above_active(d);
        let label = if group { "New group" } else { "New layer" };
        let result = d.apply(label, |doc, ids| {
            let id = ids.layer();
            let n = count_layers(doc) + 1;
            let layer = if group {
                Layer::new_group(id, format!("Group {n}"))
            } else {
                Layer::new_empty(id, format!("Layer {n}"))
            };
            doc.insert(pos, layer)
        });
        self.report(result);
    }

    /// Copies each selected layer, just above itself.
    pub(crate) fn duplicate_layers(&mut self) {
        let d = self.active_mut().expect("available");
        let sel = d.selection();
        let label = if sel.len() > 1 { "Duplicate layers" } else { "Duplicate layer" };
        let result = d.apply(label, |doc, ids| {
            for src in sel {
                let layer = doc.layer(src).ok_or(TreeError::NoSuchLayer(src))?;
                let mut copy = copy_with_new_ids(layer, ids);
                copy.name = format!("{} copy", layer.name);
                let pos = doc.position(src).ok_or(TreeError::NoSuchLayer(src))?;
                doc.insert(Position { parent: pos.parent, index: pos.index + 1 }, copy)?;
            }
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    pub(crate) fn delete_layers(&mut self) {
        let d = self.active_mut().expect("available");
        let sel = d.selection();
        let label = if sel.len() > 1 { "Delete layers" } else { "Delete layer" };
        let result = d.apply(label, |doc, _| {
            for id in sel {
                doc.remove(id)?;
            }
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    /// Puts the selected layers into a new group, where the topmost of them was.
    pub(crate) fn group_layers(&mut self) {
        let d = self.active_mut().expect("available");
        let sel = d.selection();
        let Some(&top) = sel.last() else { return };
        let mut group_id = None;
        let result = d.apply("Group layers", |doc, ids| {
            let pos = doc.position(top).ok_or(TreeError::NoSuchLayer(top))?;
            let id = ids.layer();
            let n = count_layers(doc) + 1;
            doc.insert(
                Position { parent: pos.parent, index: pos.index + 1 },
                Layer::new_group(id, format!("Group {n}")),
            )?;
            for (i, l) in sel.iter().enumerate() {
                doc.move_layer(*l, Position { parent: Some(id), index: i })?;
            }
            group_id = Some(id);
            Ok::<(), TreeError>(())
        });
        self.report(result);
        if let (Some(g), Some(d)) = (group_id, self.active_mut()) {
            d.expanded.insert(g);
        }
    }

    pub(crate) fn move_layer(&mut self, up: bool) {
        let d = self.active_mut().expect("available");
        let id = d.active_layer().expect("available");
        let pos = d.doc().position(id).expect("the active layer exists");
        let to = Position { parent: pos.parent, index: if up { pos.index + 1 } else { pos.index - 1 } };
        let result = d.apply(if up { "Move layer up" } else { "Move layer down" }, |doc, _| doc.move_layer(id, to));
        self.report(result);
    }

    /// Moves a layer to `to` (from dragging in the Layers panel).
    pub fn move_layer_to(&mut self, id: LayerId, to: Position) {
        let Some(d) = self.active_mut() else { return };
        if d.doc().position(id) == Some(to) {
            return;
        }
        let result = d.apply("Move layer", |doc, _| doc.move_layer(id, to));
        self.report(result);
    }

    pub(crate) fn merge_down(&mut self) {
        let d = self.active_mut().expect("available");
        let upper = d.active_layer().expect("available");
        let pos = d.doc().position(upper).expect("the active layer exists");
        let below = d.doc().siblings(pos.parent).expect("its parent exists")[pos.index - 1].id;
        let pair: Vec<Arc<Layer>> =
            d.doc().siblings(pos.parent).expect("its parent exists")[pos.index - 1..=pos.index].to_vec();
        if let Err(why) = check_size(&pair) {
            return self.message(MessageKind::Info, "Merge Down", why);
        }
        let d = self.active_mut().expect("available");
        let result = d.apply("Merge down", |doc, ids| {
            let image = composite_all(&pair);
            let lower = doc.layer(below).ok_or(TreeError::NoSuchLayer(below))?;
            let merged = Layer::new_raster(below, ids.object(), lower.name.clone(), image);
            doc.remove(upper)?;
            *doc.layer_mut(below).ok_or(TreeError::NoSuchLayer(below))? = merged;
            Ok::<(), TreeError>(())
        });
        self.report(result);
        if let Some(d) = self.active_mut() {
            d.select_only(below);
        }
    }

    /// Merges the visible selected layers into one, where the topmost of them was; hidden
    /// ones are left as they are.
    pub(crate) fn merge_selected(&mut self) {
        let d = self.active().expect("available");
        let doc = d.doc();
        let sel: Vec<LayerId> =
            d.selection().into_iter().filter(|&l| doc.layer(l).is_some_and(|l| l.visible)).collect();
        let Some(&top) = sel.last() else { return };
        let layers: Vec<Arc<Layer>> = sel.iter().map(|&l| Arc::new(doc.layer(l).expect("selected").clone())).collect();
        if let Err(why) = check_size(&layers) {
            return self.message(MessageKind::Info, "Merge Selected", why);
        }
        let adjacent = adjacent_siblings(doc, &sel);
        let d = self.active_mut().expect("available");
        let result = d.apply("Merge selected", |doc, ids| {
            let image = composite_all(&layers);
            let name = doc.layer(top).ok_or(TreeError::NoSuchLayer(top))?.name.clone();
            for &l in &sel[..sel.len() - 1] {
                doc.remove(l)?;
            }
            *doc.layer_mut(top).ok_or(TreeError::NoSuchLayer(top))? = Layer::new_raster(top, ids.object(), name, image);
            Ok::<(), TreeError>(())
        });
        let ok = result.is_ok();
        self.report(result);
        if ok {
            if let Some(d) = self.active_mut() {
                d.select_only(top);
            }
            if !adjacent {
                self.notify(
                    "The merged layers were not next to each other: the layers that lay between them are now \
                     below the merge, which can change how the image looks where they overlap.",
                );
            }
        }
    }

    /// Merges every visible layer into one, where the topmost of them was; hidden layers
    /// stay, in their order, below or above it as they were.
    pub(crate) fn merge_visible(&mut self) {
        let d = self.active().expect("available");
        let top = d.doc().layers.clone();
        if let Err(why) = check_size(&top) {
            return self.message(MessageKind::Info, "Merge Visible", why);
        }
        let d = self.active_mut().expect("available");
        let result = d.apply("Merge visible", |doc, ids| {
            let image = composite_all(&doc.layers);
            let last_visible = doc.layers.iter().rposition(|l| l.visible).ok_or(TreeError::IndexOutOfRange)?;
            let keep = doc.layers[last_visible].clone();
            let mut below = Vec::new();
            let mut above = Vec::new();
            for (i, l) in doc.layers.iter().enumerate() {
                let side = if i < last_visible { &mut below } else { &mut above };
                if l.visible {
                    // Hidden layers inside a visible group stay too.
                    side.extend(hidden_inside(l));
                } else {
                    side.push(l.clone());
                }
            }
            let merged = Layer::new_raster(keep.id, ids.object(), keep.name.clone(), image);
            doc.layers = below.into_iter().chain([Arc::new(merged)]).chain(above).collect();
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    /// Turns the active group into one pixel layer with the group's name, opacity and
    /// visibility; hidden layers inside it stay, below it.
    pub(crate) fn merge_group(&mut self) {
        let d = self.active().expect("available");
        let id = d.active_layer().expect("available");
        let group = d.doc().layer(id).expect("exists").clone();
        let children = group.children().expect("a group").to_vec();
        if let Err(why) = check_size(&children) {
            return self.message(MessageKind::Info, "Merge Group", why);
        }
        let d = self.active_mut().expect("available");
        let result = d.apply("Merge group", |doc, ids| {
            let image = composite_all(&children);
            let pos = doc.position(id).ok_or(TreeError::NoSuchLayer(id))?;
            let hidden: Vec<Arc<Layer>> =
                children.iter().flat_map(|c| if c.visible { hidden_inside(c) } else { vec![c.clone()] }).collect();
            let mut merged = Layer::new_raster(id, ids.object(), group.name.clone(), image);
            merged.opacity = group.opacity;
            merged.visible = group.visible;
            *doc.layer_mut(id).ok_or(TreeError::NoSuchLayer(id))? = merged;
            for (i, h) in hidden.into_iter().enumerate() {
                doc.insert(Position { parent: pos.parent, index: pos.index + i }, (*h).clone())?;
            }
            Ok::<(), TreeError>(())
        });
        self.report(result);
        if let Some(d) = self.active_mut() {
            d.select_only(id);
        }
    }

    /// Adds a layer holding everything visible merged, above the active layer (outside any
    /// group), keeping the originals.
    pub(crate) fn stamp_visible(&mut self) {
        let d = self.active().expect("available");
        let layers = d.doc().layers.clone();
        if let Err(why) = check_size(&layers) {
            return self.message(MessageKind::Info, "Stamp Visible", why);
        }
        let index = d.active_layer().and_then(|a| d.doc().path(a)).map_or(layers.len(), |p| p[0] + 1);
        let d = self.active_mut().expect("available");
        let result = d.apply("Stamp visible", |doc, ids| {
            let image = composite_all(&doc.layers);
            doc.insert(Position { parent: None, index }, Layer::new_raster(ids.layer(), ids.object(), "Stamp", image))
        });
        self.report(result);
    }

    pub(crate) fn flatten(&mut self) {
        let d = self.active().expect("available");
        if let Err(why) = check_size(&d.doc().layers.clone()) {
            return self.message(MessageKind::Info, "Flatten Image", why);
        }
        let d = self.active_mut().expect("available");
        let result = d.apply("Flatten image", |doc, ids| {
            let image = composite_all(&doc.layers);
            doc.layers = vec![Arc::new(Layer::new_raster(ids.layer(), ids.object(), "Background", image))];
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    /// Turns the image objects of the selected layers (and of the layers inside selected
    /// groups) into pixel objects, drawn as the canvas shows them.
    pub(crate) fn rasterize(&mut self) {
        let d = self.active().expect("available");
        let sel = d.selection();
        let doc = d.doc();
        let targets: Vec<Arc<Layer>> = sel.iter().filter_map(|&l| doc.layer(l)).map(|l| Arc::new(l.clone())).collect();
        if let Err(why) = check_size(&targets) {
            return self.message(MessageKind::Info, "Rasterize", why);
        }
        let d = self.active_mut().expect("available");
        let result = d.apply("Rasterize", |doc, _| {
            for id in sel {
                let layer = doc.layer_mut(id).ok_or(TreeError::NoSuchLayer(id))?;
                rasterize_layer(layer);
            }
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    pub(crate) fn toggle_layer(&mut self, visibility: bool) {
        let d = self.active_mut().expect("available");
        let id = d.active_layer().expect("available");
        self.set_layer_flag(id, visibility);
    }

    /// Flips a layer's visibility (`visibility`) or lock.
    pub fn set_layer_flag(&mut self, id: LayerId, visibility: bool) {
        let Some(d) = self.active_mut() else { return };
        let Some(l) = d.doc().layer(id) else { return };
        let label = match (visibility, l.visible, l.locked) {
            (true, true, _) => "Hide layer",
            (true, false, _) => "Show layer",
            (false, _, true) => "Unlock layer",
            (false, _, false) => "Lock layer",
        };
        let result = d.apply(label, |doc, _| {
            let l = doc.layer_mut(id).ok_or(TreeError::NoSuchLayer(id))?;
            if visibility {
                l.visible = !l.visible;
            } else {
                l.locked = !l.locked;
            }
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    /// Ends a rename in progress: with `true`, gives the layer the text typed (in the
    /// document it was started in, whichever is active now); with `false`, drops it.
    pub fn finish_rename(&mut self, keep: bool) {
        if let Some((doc, id, name)) = self.renaming.take()
            && keep
        {
            self.rename_layer(doc, id, name);
        }
    }

    pub fn rename_layer(&mut self, doc: DocId, id: LayerId, name: String) {
        let Some(d) = self.doc_mut(doc) else { return };
        if d.doc().layer(id).is_none_or(|l| l.name == name) || name.trim().is_empty() {
            return;
        }
        let result = d.apply("Rename layer", |doc, _| {
            doc.layer_mut(id).ok_or(TreeError::NoSuchLayer(id))?.name = name;
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }
}

/// Turns every image object in `layer`, and in the layers inside it, into a pixel object.
fn rasterize_layer(layer: &mut Layer) {
    match &mut layer.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                if let ObjectKind::Image(i) = &o.kind {
                    let image = resample::render_image(&i.image, &i.transform, &i.sampling, None);
                    o.kind = ObjectKind::Pixels(PixelObject { image: Arc::new(image), offset: [0, 0] });
                }
            }
        }
        LayerContent::Group(children) => {
            for c in children {
                rasterize_layer(Arc::make_mut(c));
            }
        }
    }
}

/// Whether `layer`, or a layer inside it, holds an image object.
pub(crate) fn has_image_object(layer: &Layer) -> bool {
    layer.all_objects().iter().any(|o| matches!(o.kind, ObjectKind::Image(_)))
}

/// The hidden layers inside `layer` whose groups are all visible, at any depth: what
/// compositing `layer` leaves out, and merging it would lose.
fn hidden_inside(layer: &Layer) -> Vec<Arc<Layer>> {
    let mut out = Vec::new();
    for c in layer.children().unwrap_or(&[]) {
        if c.visible {
            out.extend(hidden_inside(c));
        } else {
            out.push(c.clone());
        }
    }
    out
}

/// Whether `layers` (in stacking order) are siblings with nothing between them.
fn adjacent_siblings(doc: &Document, layers: &[LayerId]) -> bool {
    let positions: Vec<Position> = layers.iter().filter_map(|&l| doc.position(l)).collect();
    positions.windows(2).all(|w| w[0].parent == w[1].parent && w[0].index + 1 == w[1].index)
}

/// Refuses work whose result would exceed `MAX_PIXELS`.
fn check_size(layers: &[Arc<Layer>]) -> Result<(), String> {
    let bounds = layers
        .iter()
        .flat_map(|l| l.all_objects())
        .filter_map(Object::extent)
        .fold(IRect::new(0, 0, 0, 0), |a, b| a.union(&b));
    let pixels = bounds.width().saturating_mul(bounds.height());
    if pixels > MAX_PIXELS {
        let gb = pixels as f64 * 8.0 / 1e9;
        Err(format!(
            "The result would be {} × {} pixels ({gb:.0} GB), far past the canvas. Shrink or crop what \
             reaches that far first.",
            bounds.width(),
            bounds.height()
        ))
    } else {
        Ok(())
    }
}

/// How many layers the document has, groups included (for naming new ones).
pub(crate) fn count_layers(doc: &Document) -> usize {
    let mut n = 0;
    doc.for_each_layer(&mut |_, _| n += 1);
    n
}

/// A copy of `layer` (and, for a group, everything in it) with fresh identifiers. Pixels are
/// shared until changed.
pub(crate) fn copy_with_new_ids(layer: &Layer, ids: &mut IdGen) -> Layer {
    let mut c = layer.clone();
    c.id = ids.layer();
    match &mut c.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                o.id = ids.object();
            }
        }
        LayerContent::Group(children) => {
            *children = children.iter().map(|ch| Arc::new(copy_with_new_ids(ch, ids))).collect();
        }
    }
    c
}
