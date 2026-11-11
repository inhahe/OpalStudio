//! What the layer commands do: adding, removing, arranging, merging and rasterizing.
//!
//! Commands that act on several layers act on the selection ([`DocState::selection`]): the
//! selected layers in stacking order, each once (a layer inside a selected group counts as
//! part of the group).

use crate::doc::{DocId, DocState};
use crate::edits::{composited, go_on};
use crate::memory::Allowance;
use crate::state::{AppState, MessageKind};
use opal_core::document::{
    Document, IdGen, Layer, LayerContent, LayerId, ObjectKind, PixelObject, Position, TreeError,
};
use opal_core::geom::{Affine, IRect};
use opal_core::raster::RasterImage;
use opal_core::resample::{self, Sampling};
use opal_io::Progress;
use std::sync::Arc;

/// The most pixels merging or rasterizing may produce beyond what the canvas itself needs
/// (2 GiB at 8 bytes a pixel). An image object enlarged far past the canvas could
/// otherwise ask for more memory than the computer has.
pub(crate) const MAX_PIXELS: i64 = 1 << 28;

/// The area to draw `image` through `transform` into: all it reaches, or, if that is more
/// than `MAX_PIXELS` and four canvases, only the part within `keep` (with `true`, to say
/// so); `None` if it reaches nothing. What drawing that takes is set aside from `memory`
/// ([`resample::render_cost`]); an error says there is not enough free even for that.
pub(crate) fn drawable(
    image: &RasterImage,
    transform: &Affine,
    sampling: &Sampling,
    keep: &IRect,
    memory: &mut Allowance,
) -> Result<Option<(IRect, bool)>, String> {
    let Some(footprint) = resample::footprint(image, transform, sampling) else { return Ok(None) };
    let limit = MAX_PIXELS.max(4 * keep.width().saturating_mul(keep.height()));
    let (area, cut) = if footprint.width().saturating_mul(footprint.height()) <= limit {
        (footprint, false)
    } else {
        (footprint.intersect(keep), true)
    };
    memory.take_tiles(resample::render_cost(image, transform, sampling, Some(&area)))?;
    Ok(Some((area, cut)))
}

/// What is said when part of an image object reaching far past the canvas was left out.
pub(crate) const CLIPPED: &str = "Part of an image reached so far past the canvas that drawing all of it would have \
     needed more memory than this computer has, so only the part around the canvas was kept.";

/// Where a layer goes to be just above layer `id` of `doc`: in its group, or, if a group it
/// is in is locked (what a locked group holds is part of it), just above the outermost such.
pub(crate) fn above(doc: &opal_core::document::Document, id: LayerId) -> Option<Position> {
    let groups = crate::commands::groups_above(doc, id);
    let anchor = groups.iter().rposition(|g| g.locked).map_or(id, |i| groups[i].id);
    doc.position(anchor).map(|p| Position { parent: p.parent, index: p.index + 1 })
}

impl AppState {
    /// Where a new layer goes: just above the active layer (see [`above`]), or at the top.
    pub(crate) fn above_active(d: &DocState) -> Position {
        d.active_layer()
            .and_then(|a| above(d.doc(), a))
            .unwrap_or(Position { parent: None, index: d.doc().layers.len() })
    }

    pub(crate) fn new_layer(&mut self, group: bool) {
        self.new_layer_named(group, None);
    }

    /// Adds a layer (or, `group`, a group) above the active one, called `name` or a
    /// numbered name; its id, if it was added (otherwise the user is told why not).
    pub(crate) fn new_layer_named(&mut self, group: bool, name: Option<&str>) -> Option<LayerId> {
        let d = self.active_mut().expect("available");
        let pos = Self::above_active(d);
        let label = if group { "New group" } else { "New layer" };
        let mut made = None;
        let result = d.apply(label, |doc, ids| {
            let id = ids.layer();
            let n = count_layers(doc) + 1;
            let name = name.map_or_else(|| format!("{} {n}", if group { "Group" } else { "Layer" }), str::to_owned);
            let layer = if group { Layer::new_group(id, name) } else { Layer::new_empty(id, name) };
            doc.insert(pos, layer)?;
            made = Some(id);
            Ok::<(), TreeError>(())
        });
        self.report(result);
        made
    }

    /// Copies each selected layer, just above itself.
    pub(crate) fn duplicate_layers(&mut self) {
        let d = self.active_mut().expect("available");
        let sel = d.selected_layers();
        let label = if sel.len() > 1 { "Duplicate layers" } else { "Duplicate layer" };
        let result = d.apply(label, |doc, ids| {
            // Copies put above the same locked group go each above the one before, keeping
            // their order.
            let mut above_same: std::collections::HashMap<(Option<LayerId>, usize), usize> = Default::default();
            for src in sel {
                let layer = doc.layer(src).ok_or(TreeError::NoSuchLayer(src))?;
                let mut copy = copy_with_new_ids(layer, ids);
                copy.name = format!("{} copy", layer.name);
                let pos = above(doc, src).ok_or(TreeError::NoSuchLayer(src))?;
                let before = above_same.entry((pos.parent, pos.index)).or_default();
                doc.insert(Position { index: pos.index + *before, ..pos }, copy)?;
                *before += 1;
            }
            Ok::<(), TreeError>(())
        });
        self.report(result);
    }

    pub(crate) fn delete_layers(&mut self) {
        let d = self.active_mut().expect("available");
        let sel = d.selected_layers();
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
        let sel = d.selected_layers();
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
        // Out of a locked group, into one, or within one, it would change what the group holds.
        let doc = d.doc();
        let into = to
            .parent
            .and_then(|g| doc.layer(g))
            .into_iter()
            .chain(to.parent.map(|g| crate::commands::groups_above(doc, g)).unwrap_or_default());
        if let Some(g) = crate::commands::groups_above(doc, id).into_iter().chain(into).find(|g| g.locked) {
            let why = format!("\"{}\" is locked: what it holds stays as it is. Unlock it first.", g.name);
            return self.message(MessageKind::Info, "Move layer", why);
        }
        let result = d.apply("Move layer", |doc, _| doc.move_layer(id, to));
        self.report(result);
    }

    pub(crate) fn merge_down(&mut self) {
        let d = self.active().expect("available");
        let upper = d.active_layer().expect("available");
        let pos = d.doc().position(upper).expect("the active layer exists");
        let below = d.doc().siblings(pos.parent).expect("its parent exists")[pos.index - 1].id;
        let pair: Vec<Arc<Layer>> =
            d.doc().siblings(pos.parent).expect("its parent exists")[pos.index - 1..=pos.index].to_vec();
        let memory = match check_size(&pair, &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Merge Down", why),
        };
        let work = move |doc: &mut Document, ids: &mut IdGen, progress: &Progress| {
            let image = composited(&pair, None, progress)?;
            tree(|| {
                let lower = doc.layer(below).ok_or(TreeError::NoSuchLayer(below))?;
                let merged = Layer::new_raster(below, ids.object(), lower.name.clone(), image);
                doc.remove(upper)?;
                *doc.layer_mut(below).ok_or(TreeError::NoSuchLayer(below))? = merged;
                Ok(())
            })
        };
        self.edit_later(d.id, "Merge down", memory, work, move |st, doc, ()| {
            if let Some(d) = st.doc_mut(doc) {
                d.select_only(below);
            }
        });
    }

    /// Merges the visible selected layers into one, where the topmost of them was; hidden
    /// ones are left as they are.
    pub(crate) fn merge_selected(&mut self) {
        let d = self.active().expect("available");
        let doc = d.doc();
        let sel: Vec<LayerId> = d.selected_layers().into_iter().filter(|&l| shows_in(doc, l)).collect();
        let Some(&top) = sel.last() else { return };
        let layers: Vec<Arc<Layer>> = sel.iter().map(|&l| Arc::new(doc.layer(l).expect("selected").clone())).collect();
        let memory = match check_size(&layers, &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Merge Selected", why),
        };
        let adjacent = adjacent_siblings(doc, &sel);
        // The layers as the edit finds them (a rename left open is finished as it begins).
        let work = move |doc: &mut Document, ids: &mut IdGen, progress: &Progress| {
            let layers: Vec<Arc<Layer>> =
                sel.iter().filter_map(|&l| doc.layer(l)).map(|l| Arc::new(l.clone())).collect();
            // What compositing the selected groups leaves out stays, just below the result.
            let kept: Vec<Arc<Layer>> = layers.iter().flat_map(|l| hidden_inside(l)).collect();
            let image = composited(&layers, None, progress)?;
            tree(|| {
                let name = doc.layer(top).ok_or(TreeError::NoSuchLayer(top))?.name.clone();
                for &l in &sel[..sel.len() - 1] {
                    doc.remove(l)?;
                }
                *doc.layer_mut(top).ok_or(TreeError::NoSuchLayer(top))? =
                    Layer::new_raster(top, ids.object(), name, image);
                let pos = doc.position(top).ok_or(TreeError::NoSuchLayer(top))?;
                for (i, h) in kept.into_iter().enumerate() {
                    doc.insert(Position { parent: pos.parent, index: pos.index + i }, (*h).clone())?;
                }
                Ok(())
            })
        };
        self.edit_later(d.id, "Merge selected", memory, work, move |st, doc, ()| {
            if let Some(d) = st.doc_mut(doc) {
                d.select_only(top);
            }
            if !adjacent {
                st.notify(
                    "The merged layers were not next to each other: the layers that lay between them are now \
                     below the merge, which can change how the image looks where they overlap.",
                );
            }
        });
    }

    /// Merges every visible layer into one, where the topmost of them was; hidden layers
    /// stay, in their order, below or above it as they were.
    pub(crate) fn merge_visible(&mut self) {
        let d = self.active().expect("available");
        let memory = match check_size(&d.doc().layers, &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Merge Visible", why),
        };
        let work = |doc: &mut Document, ids: &mut IdGen, progress: &Progress| {
            let image = composited(&doc.layers, None, progress)?;
            let last_visible =
                doc.layers.iter().rposition(|l| shows(l)).ok_or_else(|| TreeError::IndexOutOfRange.to_string())?;
            let keep = doc.layers[last_visible].clone();
            let mut below = Vec::new();
            let mut above = Vec::new();
            for (i, l) in doc.layers.iter().enumerate() {
                let side = if i < last_visible { &mut below } else { &mut above };
                if shows(l) {
                    // Hidden layers inside a visible group stay too.
                    side.extend(hidden_inside(l));
                } else {
                    side.push(l.clone());
                }
            }
            let merged = Layer::new_raster(keep.id, ids.object(), keep.name.clone(), image);
            doc.layers = below.into_iter().chain([Arc::new(merged)]).chain(above).collect();
            Ok(())
        };
        self.edit_later(d.id, "Merge visible", memory, work, |_, _, ()| {});
    }

    /// Turns the active group into one pixel layer with the group's name, opacity and
    /// visibility; hidden layers inside it stay, below it.
    pub(crate) fn merge_group(&mut self) {
        let d = self.active().expect("available");
        let id = d.active_layer().expect("available");
        let children = d.doc().layer(id).and_then(|g| g.children()).expect("a group").to_vec();
        let memory = match check_size(&children, &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Merge Group", why),
        };
        // The group as the edit finds it (a rename left open is finished as it begins).
        let work = move |doc: &mut Document, ids: &mut IdGen, progress: &Progress| {
            let group = doc.layer(id).ok_or_else(|| TreeError::NoSuchLayer(id).to_string())?.clone();
            let children = group.children().ok_or_else(|| TreeError::NotAGroup(id).to_string())?.to_vec();
            let image = composited(&children, None, progress)?;
            tree(|| {
                let pos = doc.position(id).ok_or(TreeError::NoSuchLayer(id))?;
                let hidden: Vec<Arc<Layer>> =
                    children.iter().flat_map(|c| if shows(c) { hidden_inside(c) } else { vec![c.clone()] }).collect();
                let mut merged = Layer::new_raster(id, ids.object(), group.name.clone(), image);
                merged.opacity = group.opacity;
                merged.visible = group.visible;
                *doc.layer_mut(id).ok_or(TreeError::NoSuchLayer(id))? = merged;
                for (i, h) in hidden.into_iter().enumerate() {
                    doc.insert(Position { parent: pos.parent, index: pos.index + i }, (*h).clone())?;
                }
                Ok(())
            })
        };
        self.edit_later(d.id, "Merge group", memory, work, move |st, doc, ()| {
            if let Some(d) = st.doc_mut(doc) {
                d.select_only(id);
            }
        });
    }

    /// Adds a layer holding everything visible merged, above the active layer (outside any
    /// group), keeping the originals.
    pub(crate) fn stamp_visible(&mut self) {
        let d = self.active().expect("available");
        let layers = &d.doc().layers;
        let memory = match check_size(layers, &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Stamp Visible", why),
        };
        let index = d.active_layer().and_then(|a| d.doc().path(a)).map_or(layers.len(), |p| p[0] + 1);
        let work = move |doc: &mut Document, ids: &mut IdGen, progress: &Progress| {
            let image = composited(&doc.layers, None, progress)?;
            let stamp = Layer::new_raster(ids.layer(), ids.object(), "Stamp", image);
            tree(|| doc.insert(Position { parent: None, index }, stamp))
        };
        self.edit_later(d.id, "Stamp visible", memory, work, |_, _, ()| {});
    }

    pub(crate) fn flatten(&mut self) {
        let d = self.active().expect("available");
        let memory = match check_size(&d.doc().layers, &d.doc().canvas.rect()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Flatten Image", why),
        };
        let work = |doc: &mut Document, ids: &mut IdGen, progress: &Progress| {
            let image = composited(&doc.layers, None, progress)?;
            doc.layers = vec![Arc::new(Layer::new_raster(ids.layer(), ids.object(), "Background", image))];
            Ok(())
        };
        self.edit_later(d.id, "Flatten image", memory, work, |_, _, ()| {});
    }

    /// Turns the image objects of the selected layers (and of the layers inside selected
    /// groups) into pixel objects, drawn as the canvas shows them.
    pub(crate) fn rasterize(&mut self) {
        let d = self.active().expect("available");
        let sel = d.selected_layers();
        let doc = d.doc();
        let targets: Vec<Arc<Layer>> = sel.iter().filter_map(|&l| doc.layer(l)).map(|l| Arc::new(l.clone())).collect();
        let memory = match rasterize_fits(&targets, &d.doc().canvas.rect(), Allowance::now()) {
            Ok(memory) => memory,
            Err(why) => return self.message(MessageKind::Info, "Rasterize", why),
        };
        // Each image object drawn is a piece of the work.
        let objects = targets.iter().flat_map(|l| l.all_objects()).filter(|o| matches!(o.kind, ObjectKind::Image(_)));
        let total = objects.count() as u64;
        let work = move |doc: &mut Document, _: &mut IdGen, progress: &Progress| {
            progress.begin(total);
            for id in sel {
                let layer = doc.layer_mut(id).ok_or_else(|| TreeError::NoSuchLayer(id).to_string())?;
                rasterize_layer(layer, progress)?;
            }
            Ok(())
        };
        self.edit_later(d.id, "Rasterize", memory, work, |_, _, ()| {});
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

/// Turns every image object in `layer`, and in the layers inside it, into a pixel object,
/// telling `progress` as each is done, and stopping if the edit was cancelled.
fn rasterize_layer(layer: &mut Layer, progress: &Progress) -> Result<(), String> {
    match &mut layer.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                if let ObjectKind::Image(i) = &o.kind {
                    go_on(progress)?;
                    let image = resample::render_image(&i.image, &i.transform, &i.sampling, None);
                    o.kind = ObjectKind::Pixels(PixelObject { image: Arc::new(image), offset: [0, 0] });
                    progress.advance(1);
                }
            }
        }
        LayerContent::Group(children) => {
            for c in children {
                rasterize_layer(Arc::make_mut(c), progress)?;
            }
        }
    }
    Ok(())
}

/// What `f`, a change to the layer tree, came to, any failure told as text.
fn tree(f: impl FnOnce() -> Result<(), TreeError>) -> Result<(), String> {
    f().map_err(|e| e.to_string())
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
        if shows(c) {
            out.extend(hidden_inside(c));
        } else {
            out.push(c.clone());
        }
    }
    out
}

/// Whether compositing draws `layer` at all: visible, and not at 0% opacity. A layer that
/// is not drawn would be lost by merging it, so merges leave it as it is.
fn shows(layer: &Layer) -> bool {
    layer.visible && layer.opacity > 0.0
}

/// Whether layer `id` of `doc` is drawn: it, and every group around it, [`shows`].
pub(crate) fn shows_in(doc: &Document, id: LayerId) -> bool {
    let Some(path) = doc.path(id) else { return false };
    let mut layers: &[Arc<Layer>] = &doc.layers;
    for &i in &path {
        let l = &layers[i];
        if !shows(l) {
            return false;
        }
        layers = l.children().unwrap_or(&[]);
    }
    true
}

/// Whether `layers` (in stacking order) are siblings with nothing between them.
fn adjacent_siblings(doc: &Document, layers: &[LayerId]) -> bool {
    let positions: Vec<Position> = layers.iter().filter_map(|&l| doc.position(l)).collect();
    positions.windows(2).all(|w| w[0].parent == w[1].parent && w[0].index + 1 == w[1].index)
}

/// Refuses merging `layers` if the result would need more memory than is free; else the
/// memory measured, to keep while merging.
pub(crate) fn check_size(layers: &[Arc<Layer>], canvas: &IRect) -> Result<Allowance, String> {
    fits(layers, canvas, Allowance::now())
}

/// Whether rasterizing `layers` fits in `memory`, and within `MAX_PIXELS` beyond four times
/// `canvas` (see [`fits`]): it draws each image object in them, hidden or not, whole
/// ([`resample::render_cost`]), and copies no pixel object.
fn rasterize_fits(layers: &[Arc<Layer>], canvas: &IRect, mut memory: Allowance) -> Result<Allowance, String> {
    let objects: Vec<&opal_core::document::Object> =
        layers.iter().flat_map(|l| l.all_objects()).filter(|o| matches!(o.kind, ObjectKind::Image(_))).collect();
    let reached = objects
        .iter()
        .map(|o| o.extent().map_or(0, |e| e.width().saturating_mul(e.height())))
        .fold(0, i64::saturating_add);
    within_reach(reached, canvas)?;
    for o in objects {
        if let ObjectKind::Image(i) = &o.kind {
            memory.take_tiles(resample::render_cost(&i.image, &i.transform, &i.sampling, None))?;
        }
    }
    Ok(memory)
}

/// Refuses a result of `pixels` pixels more than `MAX_PIXELS` beyond four times `canvas`:
/// something reaches far past the canvas.
fn within_reach(pixels: i64, canvas: &IRect) -> Result<(), String> {
    let limit = MAX_PIXELS.max(4 * canvas.width().saturating_mul(canvas.height()));
    if pixels > limit {
        return Err(format!(
            "The result would need about {} of memory, because something reaches far past the canvas. \
             Shrink or crop what reaches that far first.",
            opal_io::bytes((pixels as u64).saturating_mul(8))
        ));
    }
    Ok(())
}

/// Whether the result of merging `layers` fits in `memory`, and within `MAX_PIXELS` beyond
/// four times `canvas` (more means something reaches far past it). The second is judged
/// first, from the tiles the pixel objects hold (each once, however many share it) and the
/// area each image object reaches, which bounds the work of finding exactly what merging
/// would make ([`composite_cost`](opal_core::composite::composite_cost)), set aside next.
fn fits(layers: &[Arc<Layer>], canvas: &IRect, mut memory: Allowance) -> Result<Allowance, String> {
    let tile = i64::from(opal_core::tile::TILE_SIZE).pow(2);
    let mut stored = std::collections::HashSet::new();
    let mut reached = 0i64;
    for o in layers.iter().flat_map(|l| l.all_objects()) {
        match &o.kind {
            ObjectKind::Pixels(p) => stored.extend(p.image.tiles().map(|(_, t)| Arc::as_ptr(t))),
            ObjectKind::Image(_) => {
                reached = reached.saturating_add(o.extent().map_or(0, |e| e.width().saturating_mul(e.height())))
            }
        }
    }
    within_reach((stored.len() as i64).saturating_mul(tile).saturating_add(reached), canvas)?;
    memory.take_tiles(opal_core::composite::composite_cost(layers, None))?;
    Ok(memory)
}

/// How many layers the document has, groups included (for naming new ones).
pub(crate) fn count_layers(doc: &Document) -> usize {
    let mut n = 0;
    doc.for_each_layer(&mut |_, _| n += 1);
    n
}

/// A copy of `layer` (and, for a group, everything in it) with fresh identifiers. Pixels are
/// shared until changed. A copy has not been placed: it has no placement to re-apply.
pub(crate) fn copy_with_new_ids(layer: &Layer, ids: &mut IdGen) -> Layer {
    let mut c = layer.clone();
    c.id = ids.layer();
    c.placed = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, Background, NewImage};
    use opal_core::tile::pixel_from_f32;

    /// An app with a `w × h` white image open.
    fn app(w: u32, h: u32) -> AppState {
        let mut st = AppState::new();
        st.create_image(&NewImage {
            width: w,
            height: h,
            background: Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        st
    }

    fn add_layer(st: &mut AppState, parent: Option<LayerId>, visible: bool) -> LayerId {
        let d = st.active_mut().unwrap();
        let mut id = None;
        d.apply::<TreeError>("Add", |doc, ids| {
            let l = ids.layer();
            let mut img = RasterImage::new();
            img.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
            let mut layer = Layer::new_raster(l, ids.object(), "x", img);
            layer.visible = visible;
            let index = doc.siblings(parent).map_or(0, <[_]>::len);
            doc.insert(Position { parent, index }, layer)?;
            id = Some(l);
            Ok(())
        })
        .unwrap();
        id.unwrap()
    }

    #[test]
    fn merging_selected_keeps_hidden_layers_inside_a_selected_group() {
        let mut st = app(10, 10);
        let g = {
            let d = st.active_mut().unwrap();
            let g = d.ids.layer();
            d.apply::<TreeError>("Group", |doc, _| {
                doc.insert(Position { parent: None, index: 1 }, Layer::new_group(g, "g"))
            })
            .unwrap();
            g
        };
        let hidden = add_layer(&mut st, Some(g), false);
        let _shown = add_layer(&mut st, Some(g), true);
        let x = add_layer(&mut st, None, true);
        let d = st.active_mut().unwrap();
        d.select_only(g);
        d.toggle_selected(x);
        st.merge_selected();
        let doc = st.active().unwrap().doc();
        assert!(doc.layer(hidden).is_some(), "the hidden layer inside the group was lost");
        assert!(!doc.layer(hidden).unwrap().visible);
    }

    #[test]
    fn layers_inside_hidden_groups_are_not_merged_in() {
        let mut st = app(10, 10);
        let g = {
            let d = st.active_mut().unwrap();
            let g = d.ids.layer();
            d.apply::<TreeError>("Group", |doc, _| {
                let mut group = Layer::new_group(g, "g");
                group.visible = false;
                doc.insert(Position { parent: None, index: 1 }, group)
            })
            .unwrap();
            g
        };
        let inside = add_layer(&mut st, Some(g), true);
        let a = add_layer(&mut st, None, true);
        let b = add_layer(&mut st, None, true);
        let d = st.active_mut().unwrap();
        d.select_only(inside);
        d.toggle_selected(a);
        d.toggle_selected(b);
        st.merge_selected();
        let doc = st.active().unwrap().doc();
        assert!(doc.layer(inside).is_some(), "a layer that did not show was merged away");
        assert!(doc.layer(a).is_none(), "the shown layers were merged");
    }

    #[test]
    fn size_checks_allow_large_canvases_and_count_tiles_not_bounding_boxes() {
        // A 20000 × 15000 canvas filled (tiles shared, so this is cheap) flattens.
        let st = app(20_000, 15_000);
        let d = st.active().unwrap();
        assert!(check_size(&d.doc().layers, &d.doc().canvas.rect()).is_ok());
        // Two dots far apart are two tiles, whatever their bounding box.
        let mut img = RasterImage::new();
        img.set_pixel(0, 0, pixel_from_f32([1.0; 4]));
        img.set_pixel(1_000_000, 1_000_000, pixel_from_f32([1.0; 4]));
        let mut ids = IdGen::default();
        let layer = Arc::new(Layer::new_raster(ids.layer(), ids.object(), "dots", img));
        assert!(check_size(&[layer], &IRect::from_size(10, 10)).is_ok());
    }

    #[test]
    fn drawing_far_past_the_canvas_keeps_the_part_around_it() {
        let canvas = IRect::from_size(100, 100);
        let mut memory = Allowance::of(None);
        let s = Sampling::default();
        let img = RasterImage::filled(&IRect::from_size(10, 10), pixel_from_f32([1.0; 4]));
        let near = Affine::scale(20.0, 20.0).then(Affine::translate(-50.0, -50.0));
        let reach = resample::footprint(&img, &near, &s);
        assert_eq!(drawable(&img, &near, &s, &canvas, &mut memory), Ok(reach.map(|r| (r, false))));
        let huge = Affine::scale(200_000.0, 100.0).then(Affine::translate(-1_000_000.0, 0.0));
        assert_eq!(drawable(&img, &huge, &s, &canvas, &mut memory), Ok(Some((IRect::new(0, 0, 100, 100), true))));
        assert_eq!(drawable(&RasterImage::new(), &near, &s, &canvas, &mut memory), Ok(None));
    }

    #[test]
    fn drawing_more_than_is_free_is_refused() {
        let canvas = IRect::from_size(100, 100);
        let s = Sampling::default();
        // Nine tiles each unlike the others (an area across tile edges), and room for ten.
        let img = RasterImage::filled(&IRect::new(-50, -50, 300, 300), pixel_from_f32([1.0; 4]));
        let same = Affine::translate(0.0, 0.0);
        let mut memory = Allowance::memory(Some(10 * opal_core::raster::TILE_BYTES));
        assert!(drawable(&img, &same, &s, &canvas, &mut memory).is_ok());
        let why = drawable(&img, &same, &s, &canvas, &mut memory).unwrap_err();
        assert!(why.contains("memory"), "{why}");
    }

    #[test]
    fn rasterizing_counts_the_image_objects_it_draws_hidden_or_not_and_nothing_else() {
        use opal_core::document::{ImageObject, Object};
        let mut ids = IdGen::default();
        let canvas = IRect::from_size(2000, 2000);
        let photo = RasterImage::filled(&IRect::from_size(1024, 1024), pixel_from_f32([0.3, 0.5, 0.7, 1.0]));
        let image = |img: &RasterImage| ImageObject {
            image: Arc::new(img.clone()),
            frame: IRect::from_size(1024, 1024),
            transform: Affine::rotate(0.2),
            sampling: Sampling::default(),
        };
        // A hidden layer's image object is drawn all the same.
        let mut hidden = Layer::with_objects(
            ids.layer(),
            "hidden",
            vec![Object { id: ids.object(), kind: ObjectKind::Image(image(&photo)) }],
        );
        hidden.visible = false;
        let hidden = [Arc::new(hidden)];
        let need = resample::render_cost(&photo, &Affine::rotate(0.2), &Sampling::default(), None);
        assert!(need > 0);
        assert!(rasterize_fits(&hidden, &canvas, Allowance::memory(Some(need))).is_ok());
        let why = rasterize_fits(&hidden, &canvas, Allowance::memory(Some(need - 1))).unwrap_err();
        assert!(why.contains("memory"), "{why}");
        // A large pixel object beside it is not copied, so it is not counted.
        let big = RasterImage::filled(&IRect::from_size(20_000, 20_000), pixel_from_f32([1.0; 4]));
        let pixels = ObjectKind::Pixels(opal_core::document::PixelObject { image: Arc::new(big), offset: [0, 0] });
        let both = Layer::with_objects(
            ids.layer(),
            "both",
            vec![
                Object { id: ids.object(), kind: pixels },
                Object { id: ids.object(), kind: ObjectKind::Image(image(&photo)) },
            ],
        );
        assert!(rasterize_fits(&[Arc::new(both)], &canvas, Allowance::memory(Some(need))).is_ok());
    }

    #[test]
    fn merging_something_enlarged_as_far_as_it_goes_says_how_much_it_would_need() {
        use opal_core::document::{ImageObject, Object};
        let mut ids = IdGen::default();
        let img = RasterImage::filled(&IRect::from_size(1600, 1600), pixel_from_f32([1.0; 4]));
        let image = ImageObject {
            image: Arc::new(img),
            frame: IRect::from_size(1600, 1600),
            transform: opal_core::geom::Affine::scale(1e6, 1e6),
            sampling: Default::default(),
        };
        let object = Object { id: ids.object(), kind: ObjectKind::Image(image) };
        let layer = Arc::new(Layer::with_objects(ids.layer(), "far", vec![object]));
        let why = fits(&[layer], &IRect::from_size(100, 100), Allowance::of(None)).unwrap_err();
        assert!(why.contains("reaches far past the canvas"), "{why}");
        // 2.6e18 pixels: more bytes than a u64 counts, said as the most it can.
        assert!(why.contains(&opal_io::bytes(u64::MAX)), "{why}");
    }

    #[test]
    fn merging_more_than_is_free_is_refused_counting_only_the_tiles_made() {
        // A large canvas filled with one tile: merging its layer alone makes nothing.
        let st = app(20_000, 15_000);
        let layers = &st.active().unwrap().doc().layers;
        let canvas = IRect::from_size(20_000, 15_000);
        assert!(fits(layers, &canvas, Allowance::memory(Some(0))).is_ok());
        // A layer of its own over part of it: the tiles it overlaps are made.
        let mut img = RasterImage::new();
        for x in 0..300 {
            img.set_pixel(x * 3, 0, pixel_from_f32([0.0, 0.0, 1.0, 1.0]));
        }
        let mut ids = IdGen::default();
        let top = Arc::new(Layer::new_raster(ids.layer(), ids.object(), "dots", img));
        let both = [layers[0].clone(), top];
        let four = 4 * opal_core::raster::TILE_BYTES;
        assert!(fits(&both, &canvas, Allowance::memory(Some(four))).is_ok());
        let why = fits(&both, &canvas, Allowance::memory(Some(four - 1))).unwrap_err();
        assert!(why.contains("memory"), "{why}");
    }
}
