//! The document: a canvas and its tree of layers.
//!
//! Layers have no boundary; the canvas only sets what is exported and how big the image
//! is. Layers are held in `Arc`s so a [`Document`] clone (a history snapshot) copies a list
//! of pointers; changing a layer copies that layer's small header and the path above it.

use crate::geom::{Affine, FRect, IRect};
use crate::raster::RasterImage;
use crate::resample::{self, Sampling};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Identifies a layer for the life of the document, across every history branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u64);

/// Identifies an object for the life of the document, across every history branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectId(pub u64);

/// Hands out identifiers. It lives beside the history, not inside a snapshot: two branches
/// that each add a layer must not give both layers the same identifier.
#[derive(Clone, Debug)]
pub struct IdGen {
    next: u64,
}

impl Default for IdGen {
    fn default() -> IdGen {
        IdGen { next: 1 }
    }
}

impl IdGen {
    pub fn layer(&mut self) -> LayerId {
        LayerId(self.take())
    }

    pub fn object(&mut self) -> ObjectId {
        ObjectId(self.take())
    }

    fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Makes sure future identifiers are above every identifier `doc` uses (after loading).
    pub fn reserve_above(&mut self, doc: &Document) {
        let mut max = 0;
        doc.for_each_layer(&mut |layer, _| {
            max = max.max(layer.id.0);
            if let LayerContent::Objects(objects) = &layer.content {
                for o in objects {
                    max = max.max(o.id.0);
                }
            }
        });
        self.next = self.next.max(max.saturating_add(1));
    }

    /// The next identifier that will be handed out (for saving).
    pub fn peek(&self) -> u64 {
        self.next
    }

    /// An `IdGen` whose next identifier is `next` (for loading).
    pub fn starting_at(next: u64) -> IdGen {
        IdGen { next: next.max(1) }
    }
}

/// The canvas: what gets exported, and the size of the image. It is a rectangle in the
/// document's coordinates, with its top-left corner at `(x, y)`; cropping moves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canvas {
    #[serde(default)]
    pub x: i64,
    #[serde(default)]
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl Canvas {
    /// A canvas of `width × height` with its corner at the origin.
    pub fn sized(width: u32, height: u32) -> Canvas {
        Canvas { x: 0, y: 0, width, height }
    }

    /// The canvas's pixels in document coordinates.
    pub fn rect(&self) -> IRect {
        IRect::new(self.x, self.y, self.x + i64::from(self.width), self.y + i64::from(self.height))
    }
}

/// One drawable thing inside a layer.
#[derive(Clone, Debug)]
pub struct Object {
    pub id: ObjectId,
    pub kind: ObjectKind,
}

#[derive(Clone, Debug)]
pub enum ObjectKind {
    Pixels(PixelObject),
    Image(ImageObject),
}

/// A bitmap on the document's pixel grid. It moves only by whole pixels, so moving it never
/// resamples it.
#[derive(Clone, Debug)]
pub struct PixelObject {
    pub image: Arc<RasterImage>,
    /// Where the bitmap's pixel `(0, 0)` sits in the document.
    pub offset: [i64; 2],
}

/// A bitmap with pixels of its own, drawn through a transform: a pasted image, or pixels
/// transformed freely. Transforming it again redraws from its own pixels, so nothing is
/// lost however often it is transformed.
#[derive(Clone, Debug)]
pub struct ImageObject {
    pub image: Arc<RasterImage>,
    /// The image's extent in its own pixels: the rectangle the transform box frames.
    pub frame: IRect,
    /// From the image's pixels to the document's.
    pub transform: Affine,
    pub sampling: Sampling,
}

/// Coordinates beyond this (2^40 pixels, a thousand times past any canvas) are not used:
/// they would overflow arithmetic on rectangles.
pub const MAX_COORD: i64 = 1 << 40;

/// The largest stretch or squash an image object's transform may apply, either way.
pub const MAX_SCALE: f64 = 1e6;

impl ImageObject {
    /// Whether `m` can place an image object: finite, invertible, moving it no further than
    /// `MAX_COORD`, and stretching or squashing it no more than `MAX_SCALE` in any direction.
    /// The editor keeps every transform it makes within this, and files must too.
    pub fn transform_ok(m: &Affine) -> bool {
        let Some(inv) = m.inverse() else { return false };
        let [su, sv] = m.row_norms();
        let [qu, qv] = inv.row_norms();
        m.is_finite()
            && [m.e, m.f].iter().all(|v| v.abs() <= MAX_COORD as f64)
            && su.max(sv) <= MAX_SCALE
            && qu.max(qv) <= MAX_SCALE
    }

    /// The corners of the frame in the document, clockwise from the image's top-left.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        self.frame.to_f().corners().map(|p| self.transform.apply(p))
    }
}

impl Object {
    /// The object's own pixels, whatever its kind.
    pub fn image(&self) -> &Arc<RasterImage> {
        match &self.kind {
            ObjectKind::Pixels(p) => &p.image,
            ObjectKind::Image(i) => &i.image,
        }
    }

    /// A pixel object holding `image` at the document's origin.
    pub fn pixels(id: ObjectId, image: RasterImage) -> Object {
        Object { id, kind: ObjectKind::Pixels(PixelObject { image: Arc::new(image), offset: [0, 0] }) }
    }

    /// The document pixels the object can draw into: a bound for compositing (an image
    /// object's reaches past its frame by its filter's reach).
    pub fn extent(&self) -> Option<IRect> {
        match &self.kind {
            ObjectKind::Pixels(p) => Some(p.image.content_bounds()?.translate(p.offset[0], p.offset[1])),
            ObjectKind::Image(o) => resample::footprint(&o.image, &o.transform, &o.sampling),
        }
    }

    /// The object's outline in the document: its content's bounds for pixels, the
    /// transformed frame for an image. Clockwise from the top-left.
    pub fn outline(&self) -> Option<[[f64; 2]; 4]> {
        match &self.kind {
            ObjectKind::Pixels(p) => {
                let r = p.image.content_bounds()?.translate(p.offset[0], p.offset[1]);
                Some(r.to_f().corners())
            }
            ObjectKind::Image(o) => Some(o.corners()),
        }
    }

    /// The bounding box of [`outline`](Self::outline).
    pub fn bounds(&self) -> Option<FRect> {
        self.outline().map(|c| FRect::bounding(&c))
    }
}

/// What a layer holds.
#[derive(Clone, Debug)]
pub enum LayerContent {
    /// Drawable objects, bottom to top.
    Objects(Vec<Object>),
    /// Child layers, bottom to top, composited together and then onto what is beneath.
    Group(Vec<Arc<Layer>>),
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    /// A locked layer cannot be changed, merged or deleted (it can still be reordered).
    pub locked: bool,
    /// 0–1.
    pub opacity: f32,
    pub content: LayerContent,
}

impl Layer {
    /// An empty layer for drawing on.
    pub fn new_empty(id: LayerId, name: impl Into<String>) -> Layer {
        Layer {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            content: LayerContent::Objects(Vec::new()),
        }
    }

    /// A layer holding one pixel object.
    pub fn new_raster(id: LayerId, object: ObjectId, name: impl Into<String>, image: RasterImage) -> Layer {
        Layer::with_objects(id, name, vec![Object::pixels(object, image)])
    }

    /// A layer holding `objects`, bottom to top.
    pub fn with_objects(id: LayerId, name: impl Into<String>, objects: Vec<Object>) -> Layer {
        let mut l = Layer::new_empty(id, name);
        l.content = LayerContent::Objects(objects);
        l
    }

    /// An empty group.
    pub fn new_group(id: LayerId, name: impl Into<String>) -> Layer {
        let mut l = Layer::new_empty(id, name);
        l.content = LayerContent::Group(Vec::new());
        l
    }

    pub fn is_group(&self) -> bool {
        matches!(self.content, LayerContent::Group(_))
    }

    /// The child layers, if this is a group.
    pub fn children(&self) -> Option<&[Arc<Layer>]> {
        match &self.content {
            LayerContent::Group(c) => Some(c),
            LayerContent::Objects(_) => None,
        }
    }

    /// The objects this layer holds, bottom to top (none for a group).
    pub fn objects(&self) -> &[Object] {
        match &self.content {
            LayerContent::Objects(o) => o,
            LayerContent::Group(_) => &[],
        }
    }

    /// The objects this layer and, for a group, every layer inside it hold.
    pub fn all_objects(&self) -> Vec<&Object> {
        fn walk<'a>(l: &'a Layer, out: &mut Vec<&'a Object>) {
            match &l.content {
                LayerContent::Objects(o) => out.extend(o),
                LayerContent::Group(c) => c.iter().for_each(|c| walk(c, out)),
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out
    }

    /// The bounding box of the outlines of everything in the layer, or `None` if it is
    /// empty.
    pub fn bounds(&self) -> Option<FRect> {
        self.all_objects()
            .iter()
            .filter_map(|o| o.bounds())
            .reduce(|a, b| FRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
    }
}

/// Where a layer sits: in the group `parent` (`None` for the top level), at `index` counted
/// from the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub parent: Option<LayerId>,
    pub index: usize,
}

/// Why a change to the layer tree could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeError {
    /// No layer has this identifier.
    NoSuchLayer(LayerId),
    /// The layer named as a parent is not a group.
    NotAGroup(LayerId),
    /// The index is past the end of the parent's children.
    IndexOutOfRange,
    /// A group cannot be moved into itself or one of its descendants.
    IntoItself,
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeError::NoSuchLayer(id) => write!(f, "no layer with id {}", id.0),
            TreeError::NotAGroup(id) => write!(f, "layer {} is not a group", id.0),
            TreeError::IndexOutOfRange => f.write_str("position out of range"),
            TreeError::IntoItself => f.write_str("a group cannot be moved into itself"),
        }
    }
}

impl std::error::Error for TreeError {}

#[derive(Clone, Debug)]
pub struct Document {
    pub canvas: Canvas,
    /// Top-level layers, bottom to top.
    pub layers: Vec<Arc<Layer>>,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Document {
        Document { canvas: Canvas::sized(width, height), layers: Vec::new() }
    }

    /// Visits every layer depth first, bottom to top, with its depth (0 for top level).
    pub fn for_each_layer(&self, f: &mut dyn FnMut(&Layer, usize)) {
        fn walk(layers: &[Arc<Layer>], depth: usize, f: &mut dyn FnMut(&Layer, usize)) {
            for l in layers {
                f(l, depth);
                if let Some(c) = l.children() {
                    walk(c, depth + 1, f);
                }
            }
        }
        walk(&self.layers, 0, f);
    }

    /// Every layer's identifier, depth first, bottom to top.
    pub fn layer_ids(&self) -> Vec<LayerId> {
        let mut ids = Vec::new();
        self.for_each_layer(&mut |l, _| ids.push(l.id));
        ids
    }

    /// The indices leading from the top level to the layer: `[i]` for a top-level layer,
    /// `[i, j]` for child `j` of group `i`, and so on.
    pub fn path(&self, id: LayerId) -> Option<Vec<usize>> {
        fn find(layers: &[Arc<Layer>], id: LayerId, path: &mut Vec<usize>) -> bool {
            for (i, l) in layers.iter().enumerate() {
                path.push(i);
                if l.id == id {
                    return true;
                }
                if let Some(c) = l.children()
                    && find(c, id, path)
                {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        find(&self.layers, id, &mut path).then_some(path)
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.path(id).map(|p| self.layer_at(&p))
    }

    /// The layer for changing; copies it (and the groups above it) if a snapshot shares it.
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        let path = self.path(id)?;
        let (last, groups) = path.split_last()?;
        let mut layers = &mut self.layers;
        for &i in groups {
            match &mut Arc::make_mut(&mut layers[i]).content {
                LayerContent::Group(c) => layers = c,
                LayerContent::Objects(_) => unreachable!("path runs through a non-group"),
            }
        }
        Some(Arc::make_mut(&mut layers[*last]))
    }

    /// Where the layer sits.
    pub fn position(&self, id: LayerId) -> Option<Position> {
        let path = self.path(id)?;
        let (index, groups) = path.split_last()?;
        let parent = if groups.is_empty() { None } else { Some(self.layer_at(groups).id) };
        Some(Position { parent, index: *index })
    }

    fn layer_at(&self, path: &[usize]) -> &Layer {
        let mut layers: &[Arc<Layer>] = &self.layers;
        let mut layer: &Layer = &layers[path[0]];
        for &i in &path[1..] {
            layers = layer.children().expect("path runs through a non-group");
            layer = &layers[i];
        }
        layer
    }

    /// The children of `parent` (or the top level), for changing.
    fn siblings_mut(&mut self, parent: Option<LayerId>) -> Result<&mut Vec<Arc<Layer>>, TreeError> {
        match parent {
            None => Ok(&mut self.layers),
            Some(p) => {
                let layer = self.layer_mut(p).ok_or(TreeError::NoSuchLayer(p))?;
                match &mut layer.content {
                    LayerContent::Group(c) => Ok(c),
                    LayerContent::Objects(_) => Err(TreeError::NotAGroup(p)),
                }
            }
        }
    }

    /// The children of `parent` (or the top level).
    pub fn siblings(&self, parent: Option<LayerId>) -> Option<&[Arc<Layer>]> {
        match parent {
            None => Some(&self.layers),
            Some(p) => self.layer(p)?.children(),
        }
    }

    /// Inserts `layer` at `pos`.
    pub fn insert(&mut self, pos: Position, layer: Layer) -> Result<(), TreeError> {
        let siblings = self.siblings_mut(pos.parent)?;
        if pos.index > siblings.len() {
            return Err(TreeError::IndexOutOfRange);
        }
        siblings.insert(pos.index, Arc::new(layer));
        Ok(())
    }

    /// Removes the layer and returns it with where it was.
    pub fn remove(&mut self, id: LayerId) -> Result<(Position, Arc<Layer>), TreeError> {
        let pos = self.position(id).ok_or(TreeError::NoSuchLayer(id))?;
        let siblings = self.siblings_mut(pos.parent)?;
        Ok((pos, siblings.remove(pos.index)))
    }

    /// Moves the layer to `to`, whose index counts positions after the layer has been taken
    /// out.
    pub fn move_layer(&mut self, id: LayerId, to: Position) -> Result<(), TreeError> {
        if let Some(p) = to.parent {
            let into_itself = p == id || self.is_descendant(p, id);
            if into_itself {
                return Err(TreeError::IntoItself);
            }
            if !self.layer(p).ok_or(TreeError::NoSuchLayer(p))?.is_group() {
                return Err(TreeError::NotAGroup(p));
            }
        }
        let (from, layer) = self.remove(id)?;
        let siblings = self.siblings_mut(to.parent)?;
        if to.index > siblings.len() {
            // Put it back where it was so a failed move changes nothing.
            let back = self.siblings_mut(from.parent).expect("its old parent still exists");
            back.insert(from.index, layer);
            return Err(TreeError::IndexOutOfRange);
        }
        siblings.insert(to.index, layer);
        Ok(())
    }

    /// Whether `layer` is inside the group `ancestor`, at any depth.
    pub fn is_descendant(&self, layer: LayerId, ancestor: LayerId) -> bool {
        match (self.path(layer), self.path(ancestor)) {
            (Some(l), Some(a)) => l.len() > a.len() && l.starts_with(&a),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(ids: &mut IdGen, names: &[&str]) -> (Document, Vec<LayerId>) {
        let mut d = Document::new(100, 100);
        let mut out = Vec::new();
        for (i, n) in names.iter().enumerate() {
            let id = ids.layer();
            d.insert(Position { parent: None, index: i }, Layer::new_empty(id, *n)).unwrap();
            out.push(id);
        }
        (d, out)
    }

    #[test]
    fn insert_find_and_remove() {
        let mut ids = IdGen::default();
        let (mut d, l) = doc_with(&mut ids, &["a", "b", "c"]);
        assert_eq!(d.layer(l[1]).unwrap().name, "b");
        assert_eq!(d.position(l[2]), Some(Position { parent: None, index: 2 }));
        let (pos, removed) = d.remove(l[1]).unwrap();
        assert_eq!(pos, Position { parent: None, index: 1 });
        assert_eq!(removed.name, "b");
        assert_eq!(d.layer_ids(), vec![l[0], l[2]]);
    }

    #[test]
    fn groups_nest_and_paths_follow() {
        let mut ids = IdGen::default();
        let (mut d, l) = doc_with(&mut ids, &["a"]);
        let g = ids.layer();
        d.insert(Position { parent: None, index: 1 }, Layer::new_group(g, "group")).unwrap();
        let child = ids.layer();
        d.insert(Position { parent: Some(g), index: 0 }, Layer::new_empty(child, "child")).unwrap();
        assert_eq!(d.path(child), Some(vec![1, 0]));
        assert_eq!(d.position(child), Some(Position { parent: Some(g), index: 0 }));
        assert!(d.is_descendant(child, g));
        assert!(!d.is_descendant(l[0], g));
        assert_eq!(
            d.insert(Position { parent: Some(l[0]), index: 0 }, Layer::new_empty(ids.layer(), "x")),
            Err(TreeError::NotAGroup(l[0]))
        );
    }

    #[test]
    fn moving_a_group_into_itself_is_refused_and_changes_nothing() {
        let mut ids = IdGen::default();
        let mut d = Document::new(10, 10);
        let g = ids.layer();
        d.insert(Position { parent: None, index: 0 }, Layer::new_group(g, "g")).unwrap();
        let inner = ids.layer();
        d.insert(Position { parent: Some(g), index: 0 }, Layer::new_group(inner, "inner")).unwrap();
        assert_eq!(d.move_layer(g, Position { parent: Some(inner), index: 0 }), Err(TreeError::IntoItself));
        assert_eq!(d.move_layer(g, Position { parent: Some(g), index: 0 }), Err(TreeError::IntoItself));
        assert_eq!(d.path(inner), Some(vec![0, 0]));
        assert_eq!(d.move_layer(inner, Position { parent: None, index: 5 }), Err(TreeError::IndexOutOfRange));
        assert_eq!(d.path(inner), Some(vec![0, 0]), "a failed move puts the layer back");
    }

    #[test]
    fn changing_a_layer_leaves_snapshots_alone() {
        let mut ids = IdGen::default();
        let (mut d, l) = doc_with(&mut ids, &["a", "b"]);
        let snapshot = d.clone();
        d.layer_mut(l[0]).unwrap().name = "renamed".into();
        assert_eq!(snapshot.layer(l[0]).unwrap().name, "a");
        assert_eq!(d.layer(l[0]).unwrap().name, "renamed");
        assert!(Arc::ptr_eq(&snapshot.layers[1], &d.layers[1]), "the untouched layer is still shared");
    }

    #[test]
    fn ids_are_reserved_above_a_loaded_document() {
        let mut ids = IdGen::default();
        let (d, _) = doc_with(&mut ids, &["a", "b"]);
        let mut fresh = IdGen::default();
        fresh.reserve_above(&d);
        assert!(fresh.layer().0 > d.layer_ids().iter().map(|l| l.0).max().unwrap());
    }
}
