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

impl Object {
    /// Whether it is `other` as an edit can tell: the same bitmap, placed alike.
    pub fn same_as(&self, other: &Object) -> bool {
        let Object { id, kind } = self;
        *id == other.id
            && match (kind, &other.kind) {
                (ObjectKind::Pixels(a), ObjectKind::Pixels(b)) => {
                    let PixelObject { image, offset } = a;
                    Arc::ptr_eq(image, &b.image) && *offset == b.offset
                }
                (ObjectKind::Image(a), ObjectKind::Image(b)) => {
                    let ImageObject { image, frame, transform, sampling } = a;
                    Arc::ptr_eq(image, &b.image)
                        && *frame == b.frame
                        && *transform == b.transform
                        && *sampling == b.sampling
                }
                _ => false,
            }
    }
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

/// Coordinates beyond this (2^36 pixels, 68 billion, far past any canvas) are not used: the
/// tiles' places (an `i32` of tiles) and arithmetic on rectangles would overflow past them,
/// and a selection's mask reaches only so far (`mask::MASK_REACH`, twice as far).
pub const MAX_COORD: i64 = 1 << 36;

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
    /// How it was last placed by expressions, to be placed so again (see [`Placement`]).
    pub placed: Option<Placement>,
}

/// How a layer was last placed by a box whose numbers were given as expressions naming the
/// image (its canvas, its layers; see [`crate::expr`]): those numbers, to be worked out
/// again (Re-apply) once what they name has moved. Kept until a box places the layer again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// The layers the box placed together (this one among them).
    pub layers: Vec<LayerId>,
    /// What it was given, each number once, in the order given.
    pub numbers: Vec<Placed>,
}

/// One of a box's numbers, as it was given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub number: BoxNumber,
    /// The expression, its layers named by id ([`crate::expr::Expr::text`]).
    pub expr: String,
    /// The unit a bare number in it is in.
    pub unit: crate::units::Unit,
    /// The box's point its position gives, and the point that counts from, of the canvas
    /// or of layer `from_layer`: each one of the nine points, by its index row by row from
    /// the top-left (so less than 9).
    pub anchor: u8,
    pub from_layer: Option<LayerId>,
    pub from_anchor: u8,
    /// For a side: whether the other one was kept in proportion with it.
    pub keep: bool,
}

/// One of a box's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoxNumber {
    X,
    Y,
    Width,
    Height,
    Angle,
}

impl BoxNumber {
    /// Each, with its name (as scripts' parameters and the language have it).
    pub const ALL: [(BoxNumber, &'static str); 5] = [
        (BoxNumber::X, "x"),
        (BoxNumber::Y, "y"),
        (BoxNumber::Width, "width"),
        (BoxNumber::Height, "height"),
        (BoxNumber::Angle, "angle"),
    ];

    pub fn name(self) -> &'static str {
        BoxNumber::ALL.iter().find(|n| n.0 == self).expect("each is listed").1
    }
}

impl Placement {
    /// Why it cannot be kept, if it cannot: a point out of the nine, a number given twice,
    /// an expression that does not read, or one that names the box itself (worked out
    /// again, it would move it again: none is ever kept).
    pub fn check(&self) -> Result<(), String> {
        for (i, p) in self.numbers.iter().enumerate() {
            if p.anchor >= 9 || p.from_anchor >= 9 {
                return Err(format!(
                    "its {} counts by point {} from point {}, of nine",
                    p.number.name(),
                    p.anchor,
                    p.from_anchor
                ));
            }
            if self.numbers[..i].iter().any(|q| q.number == p.number) {
                return Err(format!("it gives its {} twice", p.number.name()));
            }
            let expr =
                crate::expr::Expr::parse(&p.expr).map_err(|e| format!("its {} does not read: {e}", p.number.name()))?;
            if expr.names_box() {
                return Err(format!("its {} names the box itself", p.number.name()));
            }
        }
        Ok(())
    }
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
            placed: None,
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

    /// Whether it is `other` as an edit can tell: every setting alike, and what it holds the
    /// very same (its bitmaps the same ones, not compared pixel by pixel), so that a layer
    /// copied, changed and changed back is still it.
    pub fn same_as(&self, other: &Layer) -> bool {
        // Each field named, so that one added is compared too.
        let Layer { id, name, visible, locked, opacity, content, placed } = self;
        *id == other.id
            && *name == other.name
            && *visible == other.visible
            && *locked == other.locked
            && opacity.to_bits() == other.opacity.to_bits()
            && *placed == other.placed
            && match (content, &other.content) {
                (LayerContent::Objects(a), LayerContent::Objects(b)) => {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same_as(y))
                }
                (LayerContent::Group(a), LayerContent::Group(b)) => {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y) || x.same_as(y))
                }
                _ => false,
            }
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
        self.all_objects().iter().filter_map(|o| o.bounds()).reduce(|a, b| a.union(&b))
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
    /// Pixels per inch: how large the pixels are when printed, which converts lengths in
    /// other units (see [`crate::units`]). Always [`valid_ppi`](crate::units::valid_ppi).
    pub resolution: f64,
    /// Top-level layers, bottom to top.
    pub layers: Vec<Arc<Layer>>,
    /// What is selected, if anything: operations limited to it act through its mask.
    pub selection: Option<Arc<crate::selection::Selection>>,
    /// What was selected when the selection was last taken away (Deselect), for Reselect to
    /// bring back. Not saved in the file.
    pub deselected: Option<KeptSelection>,
    /// Selections saved by name (Save Selection), to load back: saved in the file, in the
    /// order they were first saved.
    pub saved: Vec<SavedSelection>,
}

/// A selection kept aside (what Deselect took away; one saved by name), with how the image
/// has been turned, flipped, resized or cropped since (`since`): it is carried so once, when
/// it is brought back ([`Selection::transformed`](crate::selection::Selection::transformed)),
/// rather than each of those carrying a selection nobody may ask for again.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptSelection {
    pub selection: Arc<crate::selection::Selection>,
    pub since: Affine,
}

impl KeptSelection {
    /// `selection`, kept as it is now.
    pub fn new(selection: Arc<crate::selection::Selection>) -> KeptSelection {
        KeptSelection { selection, since: Affine::IDENTITY }
    }
}

/// A selection saved by name in the image (Save Selection).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedSelection {
    pub name: String,
    pub kept: KeptSelection,
}

impl SavedSelection {
    /// The name `name` comes to as a saved selection's: without the spaces round it, `None` if
    /// nothing is left.
    pub fn name_of(name: &str) -> Option<&str> {
        Some(name.trim()).filter(|n| !n.is_empty())
    }

    /// Whether it is the one `name` names: names are told apart without the spaces round them
    /// and without regard to case.
    pub fn named(&self, name: &str) -> bool {
        self.name.trim().to_lowercase() == name.trim().to_lowercase()
    }
}

impl Document {
    pub fn new(width: u32, height: u32) -> Document {
        Document {
            canvas: Canvas::sized(width, height),
            resolution: crate::units::DEFAULT_PPI,
            layers: Vec::new(),
            selection: None,
            deselected: None,
            saved: Vec::new(),
        }
    }

    /// Whether saving it would write what saving `other` would: all it keeps in a file the same
    /// (its layers alike: see [`Layer::same_as`]; its saved selections the same ones, each made
    /// anew when saved), whatever either has selected, which files do not keep.
    pub fn saves_as(&self, other: &Document) -> bool {
        self.canvas == other.canvas
            && self.resolution == other.resolution
            && self.layers.len() == other.layers.len()
            && self.layers.iter().zip(&other.layers).all(|(a, b)| Arc::ptr_eq(a, b) || a.same_as(b))
            && self.saved.len() == other.saved.len()
            && self.saved.iter().zip(&other.saved).all(|(a, b)| {
                a.name == b.name && Arc::ptr_eq(&a.kept.selection, &b.kept.selection) && a.kept.since == b.kept.since
            })
    }

    /// The saved selection named `name` (see [`SavedSelection::named`]).
    pub fn saved_selection(&self, name: &str) -> Option<&SavedSelection> {
        self.saved.iter().find(|s| s.named(name))
    }

    /// The selections it holds: what is selected, what Reselect would bring back, and those
    /// saved.
    pub fn selections(&self) -> impl Iterator<Item = &Arc<crate::selection::Selection>> {
        let kept = self.deselected.iter().chain(self.saved.iter().map(|s| &s.kept));
        self.selection.iter().chain(kept.map(|k| &k.selection))
    }

    /// Whether it is `other` as far as an edit can tell: what saving it keeps, and what is
    /// selected.
    pub fn same_as(&self, other: &Document) -> bool {
        self.saves_as(other) && self.selection == other.selection && self.deselected == other.deselected
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

    #[test]
    fn a_placement_read_from_a_file_must_be_one_that_could_be_made() {
        let placed = |number, expr: &str, anchor| Placed {
            number,
            expr: expr.to_owned(),
            unit: crate::units::Unit::Px,
            anchor,
            from_layer: None,
            from_anchor: 0,
            keep: false,
        };
        let of = |numbers| Placement { layers: vec![LayerId(1)], numbers };
        assert_eq!(of(vec![placed(BoxNumber::X, "layer(3).right + 5", 4)]).check(), Ok(()));
        assert!(of(vec![placed(BoxNumber::X, "1", 9)]).check().unwrap_err().contains("of nine"));
        let twice = of(vec![placed(BoxNumber::Y, "1", 0), placed(BoxNumber::Y, "2", 0)]);
        assert!(twice.check().unwrap_err().contains("gives its y twice"));
        assert!(of(vec![placed(BoxNumber::Angle, "1 +", 0)]).check().unwrap_err().contains("does not read"));
        assert!(of(vec![placed(BoxNumber::X, "x + 1", 0)]).check().unwrap_err().contains("names the box itself"));
    }
}
