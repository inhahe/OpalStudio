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

    /// Duplicates the selected layers of the active image, each copy just above its layer, as
    /// one history state (see [`duplicate_in`]).
    pub(crate) fn duplicate_layers(&mut self) {
        let d = self.active_mut().expect("available");
        let sel = d.selected_layers();
        let label = if sel.len() > 1 { "Duplicate layers" } else { "Duplicate layer" };
        let result = d.apply(label, |doc, ids| duplicate_in(doc, ids, &sel).map(|_| ()));
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

    /// Changes the order of the selected layers of the active image as `how` says, as one
    /// history state (refused as [`order_refused`] says).
    pub(crate) fn order_layers(&mut self, how: Order) {
        let d = self.active_mut().expect("available");
        let ids = d.selected_layers();
        let result = d.apply(how.label(), |doc, _| reordered(doc, &ids, how));
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

    /// Defringe, Remove Black Matte or Remove White Matte (`m`) on the selected layers, off
    /// the interface thread, as one history state (see design.md, *Matting*); none if it
    /// changes nothing.
    pub(crate) fn matte(&mut self, m: Matting) {
        let d = self.active().expect("available");
        let sel = d.selected_layers();
        // Each tile with content is a piece of the work.
        let total: u64 = sel
            .iter()
            .filter_map(|&l| d.doc().layer(l))
            .flat_map(|l| l.all_objects())
            .map(|o| o.image().tile_count() as u64)
            .sum();
        let measure = crate::memory::Measure::here();
        let work = move |doc: &mut Document, _: &mut IdGen, progress: &Progress| {
            progress.begin(total);
            // Each tile it changes is a new one (the image as it was is kept, to undo to).
            let mut memory = measure.now();
            for id in sel {
                let layer = doc.layer_mut(id).ok_or_else(|| TreeError::NoSuchLayer(id).to_string())?;
                matte_layer(layer, m, &mut memory, progress)?;
            }
            Ok(())
        };
        self.edit_later(d.id, m.label(), Allowance::unlimited(), work, |_, _, ()| {});
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

/// What a matting command does (see design.md, *Matting*): Defringe, so many pixels in; or
/// a matte taken back out (0, black; 1, white).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Matting {
    Defringe(u32),
    RemoveMatte(f64),
}

impl Matting {
    /// What the history state it makes is called.
    pub fn label(self) -> &'static str {
        match self {
            Matting::Defringe(_) => "Defringe",
            Matting::RemoveMatte(m) if m < 0.5 => "Remove black matte",
            Matting::RemoveMatte(_) => "Remove white matte",
        }
    }

    /// How far round a pixel what it becomes can come from: Defringe's colours come from at
    /// most twice its width in (and a step for each side's edge).
    fn margin(self) -> i64 {
        match self {
            Matting::Defringe(w) => 2 * i64::from(w) + 2,
            Matting::RemoveMatte(_) => 0,
        }
    }
}

/// How many tiles a matte works out at once before writing those it changed into the bitmap
/// (whose tiles may go to disk): what it holds of them beyond it.
const MATTE_BATCH: usize = 64;

/// `image` matted as `m` says: tile by tile (those with content), each read with the margin
/// round it that its result depends on, so that it is what the whole bitmap at once would
/// give, those it changes taken from `memory`, the rest kept as they are (shared where they
/// were); `None` if it changes nothing. An error if it was cancelled, or memory ran out.
pub fn matted(
    image: &RasterImage,
    m: Matting,
    memory: &mut crate::memory::Allowance,
    progress: &Progress,
) -> Result<Option<RasterImage>, String> {
    use rayon::prelude::*;
    let coords: Vec<opal_core::tile::TileCoord> = image.tiles().map(|(c, _)| c).collect();
    let margin = m.margin();
    let mut out: Option<RasterImage> = None;
    for batch in coords.chunks(MATTE_BATCH) {
        let done: Vec<Option<(IRect, Vec<opal_core::tile::Pixel>)>> = batch
            .par_iter()
            .map(|c| {
                if progress.is_cancelled() {
                    return Err(crate::edits::CANCELLED.to_owned());
                }
                let tile = c.rect(0);
                let around = IRect::new(tile.x0 - margin, tile.y0 - margin, tile.x1 + margin, tile.y1 + margin);
                let mut px = image.read_rect(&around);
                match m {
                    Matting::Defringe(w) => {
                        opal_core::matting::defringe(&mut px, around.width() as usize, around.height() as usize, w)
                    }
                    Matting::RemoveMatte(matte) => opal_core::matting::remove_matte(&mut px, matte),
                }
                // The tile's own pixels, out of the margin.
                let (w, m) = (around.width() as usize, margin as usize);
                let own: Vec<opal_core::tile::Pixel> = (0..tile.height() as usize)
                    .flat_map(|y| px[(y + m) * w + m..(y + m) * w + m + tile.width() as usize].to_vec())
                    .collect();
                progress.advance(1);
                Ok((own != image.read_rect(&tile)).then_some((tile, own)))
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (tile, px) in done.into_iter().flatten() {
            memory.take_tiles(opal_core::raster::TILE_BYTES)?;
            out.get_or_insert_with(|| image.clone()).write_rect(&tile, &px);
        }
    }
    Ok(out)
}

/// `layer` and the layers inside it matted as `m` says: their pixel objects, and the bitmaps
/// their placed images are drawn from (the colours were wrong in the file, not in the
/// placing), what it changes taken from `memory`, telling `progress` as each tile is done;
/// an error if it was cancelled, or memory ran out. What it does not change is left as it was.
fn matte_layer(
    layer: &mut Layer,
    m: Matting,
    memory: &mut crate::memory::Allowance,
    progress: &Progress,
) -> Result<(), String> {
    match &mut layer.content {
        LayerContent::Objects(objects) => {
            for o in objects {
                let image = match &mut o.kind {
                    ObjectKind::Pixels(p) => &mut p.image,
                    ObjectKind::Image(i) => &mut i.image,
                };
                if let Some(new) = matted(image, m, memory, progress)? {
                    *image = Arc::new(new);
                }
            }
        }
        LayerContent::Group(children) => {
            for c in children {
                matte_layer(Arc::make_mut(c), m, memory, progress)?;
            }
        }
    }
    Ok(())
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

/// Puts in `doc` a copy of each of `layers`, named "… copy", just above its layer (see
/// [`above`]), given identifiers by `ids`. The copies, in the order of `layers`.
pub(crate) fn duplicate_in(
    doc: &mut opal_core::document::Document,
    ids: &mut IdGen,
    layers: &[LayerId],
) -> Result<Vec<LayerId>, TreeError> {
    // Copies put above the same locked group go each above the one before, keeping their
    // order.
    let mut above_same: std::collections::HashMap<(Option<LayerId>, usize), usize> = Default::default();
    let mut made = Vec::new();
    for &src in layers {
        let layer = doc.layer(src).ok_or(TreeError::NoSuchLayer(src))?;
        let mut copy = copy_with_new_ids(layer, ids);
        copy.name = format!("{} copy", layer.name);
        made.push(copy.id);
        let pos = above(doc, src).ok_or(TreeError::NoSuchLayer(src))?;
        let before = above_same.entry((pos.parent, pos.index)).or_default();
        doc.insert(Position { index: pos.index + *before, ..pos }, copy)?;
        *before += 1;
    }
    Ok(made)
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

/// How the selected layers' order changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Each to the top of its group, in the order they had.
    Front,
    /// Each to the bottom of its group, in the order they had.
    Back,
    /// Turned round, in the places they hold (in one group).
    Reverse,
}

impl Order {
    /// The history state it makes.
    fn label(self) -> &'static str {
        match self {
            Order::Front => "Bring to front",
            Order::Back => "Send to back",
            Order::Reverse => "Reverse layers",
        }
    }
}

/// `doc` with the layers `ids` (in document order) put in order as `how` says.
fn reordered(doc: &mut Document, ids: &[LayerId], how: Order) -> Result<(), TreeError> {
    if how == Order::Reverse {
        return doc.reverse(ids);
    }
    // Bottom to top, each to the top of its group, keeps their order; top to bottom, each to
    // the bottom.
    let ids: Vec<LayerId> = if how == Order::Front { ids.to_vec() } else { ids.iter().rev().copied().collect() };
    for id in ids {
        let parent = doc.position(id).ok_or(TreeError::NoSuchLayer(id))?.parent;
        let n = doc.siblings(parent).map_or(0, <[_]>::len);
        doc.move_layer(id, Position { parent, index: if how == Order::Front { n - 1 } else { 0 } })?;
    }
    Ok(())
}

/// Why Bring to Front, Send to Back or Reverse (`cmd`) cannot act on the active image's
/// selected layers now, if it cannot: none selected; a group they would move in locked; for
/// Reverse, fewer than two, or not all in one group; or they are where it would put them
/// already.
pub(crate) fn order_refused(st: &AppState, cmd: crate::commands::Cmd) -> Result<(), String> {
    use crate::commands::Cmd;
    let d = st.active().ok_or("Open or create an image first.")?;
    let ids = d.selected_layers();
    if ids.is_empty() {
        return Err("Select the layers to arrange in the Layers panel first.".into());
    }
    let doc = d.doc();
    for &id in &ids {
        if let Some(g) = crate::commands::groups_above(doc, id).into_iter().find(|g| g.locked) {
            return Err(format!("\"{}\" is locked: what it holds stays as it is. Unlock it first.", g.name));
        }
    }
    let how = match cmd {
        Cmd::BringToFront => Order::Front,
        Cmd::SendToBack => Order::Back,
        _ => {
            if ids.len() < 2 {
                return Err("Select two layers or more to reverse their order.".into());
            }
            let parent = |id| doc.position(id).map(|p| p.parent);
            if ids.iter().any(|&id| parent(id) != parent(ids[0])) {
                return Err(
                    "The selected layers must all be in one group (or all outside groups) to reverse them.".into()
                );
            }
            Order::Reverse
        }
    };
    // Already there: in each group, those of them in it are its topmost (or bottommost), one
    // after another. (Two or more reversed always change.)
    if how != Order::Reverse {
        let mut groups: Vec<(Option<LayerId>, Vec<usize>)> = Vec::new();
        for &id in &ids {
            let pos = doc.position(id).ok_or("A selected layer has gone.")?;
            match groups.iter_mut().find(|(p, _)| *p == pos.parent) {
                Some((_, at)) => at.push(pos.index),
                None => groups.push((pos.parent, vec![pos.index])),
            }
        }
        let there = groups.iter().all(|(parent, at)| {
            let n = doc.siblings(*parent).map_or(0, <[_]>::len);
            let want = if how == Order::Front { n - at.len()..n } else { 0..at.len() };
            at.iter().copied().eq(want)
        });
        if there {
            return Err(match how {
                Order::Front => "They are at the top of their groups already.".into(),
                _ => "They are at the bottom of their groups already.".into(),
            });
        }
    }
    Ok(())
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
    fn layers_go_to_the_front_or_back_of_their_groups_or_turn_round_and_say_why_not() {
        use crate::commands::Cmd;
        let mut st = app(10, 10);
        // Above the background: a, b, c, a group g holding x and y, and d.
        let [a, b, c] = [(); 3].map(|()| add_layer(&mut st, None, true));
        let g = {
            let d = st.active_mut().unwrap();
            let g = d.ids.layer();
            d.apply::<TreeError>("Group", |doc, _| {
                let n = doc.layers.len();
                doc.insert(Position { parent: None, index: n }, Layer::new_group(g, "g"))
            })
            .unwrap();
            g
        };
        let x = add_layer(&mut st, Some(g), true);
        let y = add_layer(&mut st, Some(g), true);
        let top = add_layer(&mut st, None, true);
        let order = |st: &AppState, parent: Option<LayerId>| -> Vec<LayerId> {
            st.active().unwrap().doc().siblings(parent).unwrap().iter().map(|l| l.id).collect()
        };
        let background = order(&st, None)[0];
        // a and b (in that order) and x: a, b to the top level's top in their order, x to g's.
        let d = st.active_mut().unwrap();
        d.select_only(a);
        d.toggle_selected(b);
        d.toggle_selected(x);
        st.order_layers(Order::Front);
        assert_eq!(order(&st, None), vec![background, c, g, top, a, b]);
        assert_eq!(order(&st, Some(g)), vec![y, x]);
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Bring to front"));
        // There already: refused, saying so.
        let why = order_refused(&st, Cmd::BringToFront).unwrap_err();
        assert!(why.contains("at the top of their groups already"), "{why}");
        // To the back: a, b under the background, x under y.
        st.order_layers(Order::Back);
        assert_eq!(order(&st, None), vec![a, b, background, c, g, top]);
        assert_eq!(order(&st, Some(g)), vec![x, y]);
        // Reversed: not with x in another group; a, b and c, a and c swapping.
        assert!(order_refused(&st, Cmd::ReverseLayers).unwrap_err().contains("must all be in one group"));
        let d = st.active_mut().unwrap();
        d.select_only(a);
        d.toggle_selected(c);
        assert_eq!(order_refused(&st, Cmd::ReverseLayers), Ok(()));
        st.order_layers(Order::Reverse);
        assert_eq!(order(&st, None), vec![c, b, background, a, g, top]);
        // One alone is not reversed; in a locked group, nothing moves.
        st.active_mut().unwrap().select_only(c);
        assert!(order_refused(&st, Cmd::ReverseLayers).unwrap_err().contains("two layers or more"));
        st.active_mut()
            .unwrap()
            .apply::<()>("Lock", |doc, _| {
                doc.layer_mut(g).unwrap().locked = true;
                Ok(())
            })
            .unwrap();
        st.active_mut().unwrap().select_only(y);
        assert!(order_refused(&st, Cmd::SendToBack).unwrap_err().contains("\"g\" is locked"));
        // All the layers at the top level, the topmost active; then none.
        let d = st.active_mut().unwrap();
        d.select_all_layers();
        assert_eq!((d.selected_layers(), d.active_layer()), (vec![c, b, background, a, g, top], Some(top)));
        d.deselect_layers();
        assert_eq!((d.selected_layers(), d.active_layer()), (vec![], None));
        // An edit adding no layer leaves none selected; one adding a layer makes it active.
        st.select_all();
        let d = st.active().unwrap();
        assert_eq!((d.selected_layers(), d.active_layer()), (vec![], None), "a layer selected again");
        let added = add_layer(&mut st, None, true);
        assert_eq!(st.active().unwrap().active_layer(), Some(added));
        // A layer only in the edit under way (a paste's, its box open) is among all.
        let d = st.active_mut().unwrap();
        let mut doc = d.doc().clone();
        let pasted = d.ids.layer();
        let n = doc.layers.len();
        doc.insert(Position { parent: None, index: n }, Layer::new_empty(pasted, "p")).unwrap();
        d.pending = Some(crate::doc::Pending { doc, label: "Paste".into() });
        d.select_all_layers();
        assert_eq!(d.active_layer(), Some(pasted));
    }

    #[test]
    fn transform_again_does_the_last_transform_to_the_layers_selected_now_or_to_copies() {
        use crate::commands::{Cmd, availability};
        use opal_core::geom::{Affine, FRect, IRect};
        let mut st = app(400, 300);
        let rect = |st: &mut AppState, r: IRect| {
            let d = st.active_mut().unwrap();
            let mut id = None;
            d.apply::<TreeError>("Add", |doc, ids| {
                let l = ids.layer();
                let red = pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
                let n = doc.layers.len();
                doc.insert(
                    Position { parent: None, index: n },
                    Layer::new_raster(l, ids.object(), "r", RasterImage::filled(&r, red)),
                )?;
                id = Some(l);
                Ok(())
            })
            .unwrap();
            id.unwrap()
        };
        let (a, b) = (rect(&mut st, IRect::new(10, 10, 30, 20)), rect(&mut st, IRect::new(100, 100, 120, 110)));
        let bounds = |st: &AppState, id| st.active().unwrap().doc().layer(id).unwrap().bounds().unwrap();
        // Nothing transformed yet: refused, saying so.
        let why = availability(&st, Cmd::TransformAgain).unwrap_err();
        assert!(why.contains("Transform layers first"), "{why}");
        // a moved by (50, 20) with the box: b moved the same by Transform Again.
        let sampling = opal_core::resample::Sampling::default();
        let d = st.active_mut().unwrap();
        d.select_only(a);
        assert!(d.begin_transform(sampling));
        let m = d.transform.as_ref().unwrap().start.then(Affine::translate(50.0, 20.0));
        d.set_box(m, None);
        d.commit_pending();
        assert_eq!(bounds(&st, a), FRect::new(60.0, 30.0, 80.0, 40.0));
        st.active_mut().unwrap().select_only(b);
        st.transform_again(false);
        assert_eq!(bounds(&st, b), FRect::new(150.0, 120.0, 170.0, 130.0));
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Transform again"));
        // As a copy: b stays, its copy moved on and selected, one history state; again, the
        // next copy from that one.
        let count = |st: &AppState| st.active().unwrap().doc().layers.len();
        let n = count(&st);
        st.transform_again(true);
        let first = st.active().unwrap().active_layer().unwrap();
        assert_ne!(first, b);
        assert_eq!(
            (bounds(&st, b), bounds(&st, first)),
            (FRect::new(150.0, 120.0, 170.0, 130.0), FRect::new(200.0, 140.0, 220.0, 150.0))
        );
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Transform again as a copy"));
        st.transform_again(true);
        let second = st.active().unwrap().active_layer().unwrap();
        assert_eq!((count(&st), bounds(&st, second)), (n + 2, FRect::new(250.0, 160.0, 270.0, 170.0)));
        assert!(st.active_mut().unwrap().undo());
        assert_eq!(count(&st), n + 1, "undone in more than one step");
        // Two at once: both copied, both copies moved on and selected.
        let d = st.active_mut().unwrap();
        d.select_only(a);
        d.toggle_selected(b);
        st.transform_again(true);
        let copies = st.active().unwrap().selected_layers();
        assert!(copies.len() == 2 && !copies.contains(&a) && !copies.contains(&b), "{copies:?}");
        let moved: Vec<FRect> = copies.iter().map(|&l| bounds(&st, l)).collect();
        assert_eq!(moved, vec![FRect::new(110.0, 50.0, 130.0, 60.0), FRect::new(200.0, 140.0, 220.0, 150.0)]);
        // A turn about a point: done again, as far again about the same point.
        let c = rect(&mut st, IRect::new(190, 100, 210, 110));
        let d = st.active_mut().unwrap();
        d.select_only(c);
        assert!(d.begin_transform(sampling));
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2).about(200.0, 150.0);
        let m = d.transform.as_ref().unwrap().start.then(quarter);
        d.set_box(m, None);
        d.commit_pending();
        st.transform_again(false);
        let r = bounds(&st, c);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(near(r.x0, 190.0) && near(r.y0, 190.0) && near(r.x1, 210.0) && near(r.y1, 200.0), "{r:?}");
    }

    #[test]
    fn a_whole_image_turn_that_leaves_every_layer_alike_still_carries_its_coordinates() {
        use crate::image_ops::Turn;
        use opal_core::geom::Affine;
        // Nothing but an empty layer: flipped, its layers are as they were.
        let mut st = AppState::new();
        st.create_image(&NewImage {
            width: 40,
            height: 20,
            background: Background::Transparent,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        let d = st.active_mut().unwrap();
        // A layer holding no object at all: nothing a flip changes.
        d.apply::<TreeError>("Empty", |doc, ids| {
            doc.layers.clear();
            doc.insert(Position { parent: None, index: 0 }, Layer::new_empty(ids.layer(), "e"))
        })
        .unwrap();
        let carried = d.doc().carried;
        d.last_transform = Some(crate::doc::LastTransform { delta: Affine::translate(5.0, 0.0), carried });
        st.turn(Turn::FlipHorizontal);
        let d = st.active().unwrap();
        assert_ne!(d.doc().carried, carried, "the flip was dropped as making no change");
        let m = d.transform_again_map().unwrap();
        assert_eq!(m.apply([0.0, 0.0]), [-5.0, 0.0], "a move right made before the flip, not mirrored");
    }

    #[test]
    fn transform_again_goes_with_the_image_turned_or_resized_and_leaves_nothing_if_refused() {
        use crate::commands::{Cmd, availability};
        use crate::image_ops::{ResizeImage, Turn};
        use opal_core::geom::{Affine, IRect};
        let mut st = app(400, 300);
        let d = st.active_mut().unwrap();
        let mut a = None;
        d.apply::<TreeError>("Add", |doc, ids| {
            let l = ids.layer();
            let red = pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
            let layer = Layer::new_raster(l, ids.object(), "a", RasterImage::filled(&IRect::new(10, 10, 30, 20), red));
            a = Some(l);
            doc.insert(Position { parent: None, index: 1 }, layer)
        })
        .unwrap();
        let a = a.unwrap();
        let sampling = opal_core::resample::Sampling::default();
        let d = st.active_mut().unwrap();
        d.select_only(a);
        assert!(d.begin_transform(sampling));
        // A box open: which transform is meant is the user's to say.
        let why = availability(&st, Cmd::TransformAgain).unwrap_err();
        assert!(why.contains("Apply or cancel the box"), "{why}");
        let d = st.active_mut().unwrap();
        let m = d.transform.as_ref().unwrap().start.then(Affine::translate(50.0, 20.0));
        d.set_box(m, None);
        d.commit_pending();
        // How far Transform Again would move a point.
        let moves = |st: &AppState| {
            let m = st.active().unwrap().transform_again_map().unwrap();
            let [x, y] = m.apply([7.0, 3.0]);
            [x - 7.0, y - 3.0].map(|v| (v * 1e6).round() / 1e6)
        };
        assert_eq!(moves(&st), [50.0, 20.0]);
        // Halved, half as far, the layer moving with the image; undone, as far as it was.
        let half = ResizeImage { width: 200, height: 150, keep_aspect: true, resolution: 72.0, keep_print_size: false };
        st.resize_image(&half, false);
        assert_eq!(moves(&st), [25.0, 10.0]);
        let before = st.active().unwrap().doc().layer(a).unwrap().bounds().unwrap();
        st.transform_again(false);
        let after = st.active().unwrap().doc().layer(a).unwrap().bounds().unwrap();
        assert_eq!([after.x0 - before.x0, after.y0 - before.y0], [25.0, 10.0]);
        let d = st.active_mut().unwrap();
        assert!(d.undo() && d.undo());
        assert_eq!(moves(&st), [50.0, 20.0]);
        // Turned right, a move right is one down.
        st.turn(Turn::Clockwise);
        assert_eq!(moves(&st), [-20.0, 50.0]);
        // A transform refused (past what a file can hold): as a copy, no copies left behind.
        let d = st.active_mut().unwrap();
        let carried = d.doc().carried;
        d.last_transform = Some(crate::doc::LastTransform { delta: Affine::scale(1e12, 1e12), carried });
        let (layers, state) = (d.doc().layers.len(), d.history.current());
        st.transform_again(true);
        let d = st.active().unwrap();
        assert_eq!((d.doc().layers.len(), d.history.current(), d.transform.is_none()), (layers, state, true));
        assert_eq!(d.selected_layers(), vec![a], "the selection not put back");
        assert!(st.messages.back().is_some_and(|m| m.title == "Transform again as a copy"));
        // A layer whose pixels hold nothing (all cleared, say): said so.
        let d = st.active_mut().unwrap();
        d.last_transform = Some(crate::doc::LastTransform { delta: Affine::translate(1.0, 0.0), carried });
        let mut empty = None;
        d.apply::<TreeError>("Add", |doc, ids| {
            let l = ids.layer();
            empty = Some(l);
            doc.insert(Position { parent: None, index: 1 }, Layer::new_raster(l, ids.object(), "e", RasterImage::new()))
        })
        .unwrap();
        st.active_mut().unwrap().select_only(empty.unwrap());
        st.transform_again(false);
        let said = st.messages.back().map(|m| m.body.as_str()).unwrap_or_default();
        assert_eq!(said, crate::transform::NOTHING_TO_TRANSFORM);
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

    /// A bitmap spanning several tiles: a 300 × 300 block from (100, 100), its outer two
    /// pixels red, within them blue going green across (so that where a colour comes from
    /// shows), its edge half transparent.
    fn fringed() -> RasterImage {
        let mut img = RasterImage::new();
        for y in 100..400 {
            for x in 100..400 {
                let from_edge = (x - 100).min(y - 100).min(399 - x).min(399 - y);
                let a = if from_edge == 0 { 0.5 } else { 1.0 };
                let g = (x - 100) as f32 / 300.0;
                let c = if from_edge < 2 { [a, 0.0, 0.0, a] } else { [0.0, g * a, a, a] };
                img.set_pixel(x, y, pixel_from_f32(c));
            }
        }
        img
    }

    /// `img` matted as `m` says, with memory enough (as it was, if that changes nothing).
    fn matted_freely(img: &RasterImage, m: Matting) -> RasterImage {
        matted(img, m, &mut Allowance::unlimited(), &Progress::default()).unwrap().unwrap_or_else(|| img.clone())
    }

    /// `img` within `bounds` matted as `m` says, all of it at once.
    fn matted_whole(img: &RasterImage, bounds: &IRect, m: Matting) -> Vec<opal_core::tile::Pixel> {
        let mut whole = img.read_rect(bounds);
        match m {
            Matting::Defringe(w) => {
                opal_core::matting::defringe(&mut whole, bounds.width() as usize, bounds.height() as usize, w)
            }
            Matting::RemoveMatte(matte) => opal_core::matting::remove_matte(&mut whole, matte),
        }
        whole
    }

    #[test]
    fn matting_tile_by_tile_gives_what_the_whole_bitmap_at_once_would() {
        let img = fringed();
        let bounds = IRect::new(98, 98, 402, 402);
        for m in [Matting::Defringe(2), Matting::Defringe(9), Matting::RemoveMatte(0.0)] {
            let tiled = matted_freely(&img, m);
            assert!(tiled.read_rect(&bounds) == matted_whole(&img, &bounds, m), "{m:?}: tiles disagree with the whole");
        }
        // Defringed by two, the red is gone.
        let p = matted_freely(&img, Matting::Defringe(2)).pixel(150, 101);
        assert!(p[0].to_f32() < 1e-3 && p[2].to_f32() > 0.99, "{p:?}");
        // A thin red strand from a blue block, across tiles: coloured only near the block, the
        // same tile by tile.
        let mut strand = RasterImage::new();
        for y in 100..200 {
            for x in 100..200 {
                strand.set_pixel(x, y, pixel_from_f32([0.0, 0.0, 1.0, 1.0]));
            }
        }
        for x in 200..600 {
            strand.set_pixel(x, 150, pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        }
        let bounds = IRect::new(90, 90, 610, 210);
        for w in [1, 3] {
            let tiled = matted_freely(&strand, Matting::Defringe(w));
            assert!(
                tiled.read_rect(&bounds) == matted_whole(&strand, &bounds, Matting::Defringe(w)),
                "{w}: tiles disagree"
            );
            assert_eq!(tiled.pixel(400, 150), strand.pixel(400, 150), "{w}: the strand recoloured far from the block");
        }
    }

    #[test]
    fn a_matte_that_changes_nothing_keeps_the_bitmap_and_its_shared_tiles() {
        // Opaque white, one tile shared by every place (as a new image's background is).
        let white = RasterImage::filled(&IRect::new(0, 0, 1000, 600), pixel_from_f32([1.0; 4]));
        for m in [Matting::RemoveMatte(0.0), Matting::RemoveMatte(1.0), Matting::Defringe(3)] {
            let same = matted(&white, m, &mut Allowance::memory(Some(0)), &Progress::default());
            assert!(same.is_ok_and(|s| s.is_none()), "{m:?} changed it, or asked for memory");
        }
        // Memory short for the tiles it changes: refused.
        let why = matted(&fringed(), Matting::Defringe(2), &mut Allowance::memory(Some(1000)), &Progress::default())
            .unwrap_err();
        assert!(why.contains("memory"), "{why}");
    }

    #[test]
    fn matting_works_on_the_selected_layers_pixels_and_placed_images_one_state() {
        let mut st = app(500, 500);
        let layer = add_layer(&mut st, None, true);
        // The layer's bitmap a fringed block, beside a placed image of one.
        st.active_mut()
            .unwrap()
            .apply::<TreeError>("Fringed", |doc, ids| {
                let l = doc.layer_mut(layer).ok_or(TreeError::NoSuchLayer(layer))?;
                let placed = opal_core::document::ImageObject {
                    image: Arc::new(fringed()),
                    frame: IRect::new(100, 100, 400, 400),
                    transform: Affine::translate(10.0, 0.0),
                    sampling: Sampling::default(),
                };
                if let LayerContent::Objects(objects) = &mut l.content {
                    objects[0].kind = ObjectKind::Pixels(PixelObject { image: Arc::new(fringed()), offset: [0, 0] });
                    objects.push(opal_core::document::Object { id: ids.object(), kind: ObjectKind::Image(placed) });
                }
                Ok(())
            })
            .unwrap();
        st.active_mut().unwrap().select_only(layer);
        let states = st.active().unwrap().history.nodes().count();
        st.run(crate::commands::Cmd::Defringe, &eframe::egui::Context::default());
        assert_eq!(st.defringe, Some(1.0), "no dialog");
        st.defringe = None;
        st.matte(Matting::Defringe(2));
        st.finish_edits();
        let d = st.active().unwrap();
        assert_eq!((d.history.nodes().count(), d.history.undo_label()), (states + 1, Some("Defringe")));
        let objects = d.doc().layer(layer).unwrap().all_objects();
        for o in objects {
            let image = match &o.kind {
                ObjectKind::Pixels(p) => &p.image,
                ObjectKind::Image(i) => &i.image,
            };
            assert!(image.pixel(150, 101)[0].to_f32() < 1e-3, "an object not defringed");
        }
        // Locked, it is refused.
        st.set_layer_flag(layer, false);
        let why = crate::commands::availability(&st, crate::commands::Cmd::RemoveWhiteMatte).unwrap_err();
        assert!(why.contains("locked"), "{why}");
    }
}
