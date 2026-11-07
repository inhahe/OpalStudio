//! The undo history tree.
//!
//! Every node holds a complete snapshot of the document. Snapshots share everything
//! unchanged through `Arc`, so a node costs only what its action changed, and moving to
//! any node, on any branch, is a matter of picking up its snapshot. Past a memory budget
//! (see [`History::set_budget`]), the oldest states are dropped.

use crate::document::{Document, Layer, LayerContent, ObjectKind};
use crate::raster::RasterImage;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

/// Identifies a node of one [`History`]. Identifiers are never reused, so one stays valid
/// (or names no state, once that state is dropped) for the history's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub usize);

#[derive(Clone, Debug)]
pub struct Node {
    /// `None` only for the root: the state the document was opened or created in, or the
    /// oldest state left once older ones were dropped.
    pub parent: Option<NodeId>,
    /// In the order they were made.
    pub children: Vec<NodeId>,
    /// The child most recently visited, which redo follows.
    pub last_child: Option<NodeId>,
    /// What the action that produced this state did, for the history panel.
    pub label: String,
    pub created: SystemTime,
    doc: Document,
}

impl Node {
    /// The document as it was in this state.
    pub fn doc(&self) -> &Document {
        &self.doc
    }
}

#[derive(Clone, Debug)]
pub struct History {
    /// By identifier; `None` once dropped.
    nodes: Vec<Option<Node>>,
    root: NodeId,
    current: NodeId,
    /// How many bytes of pixels the other states may hold that the current one does not.
    budget: u64,
    /// How many states were dropped to keep within it.
    dropped: usize,
    /// The tiles the states hold, kept up to date as states come, go and are visited.
    usage: Usage,
    /// States not to be dropped to keep within the budget, however old: the one before an
    /// action made of several commits, until they are joined (see [`hold`](Self::hold)).
    held: Vec<NodeId>,
}

impl History {
    /// A history whose only state is `doc`, labeled `label` ("Open", "New image"), with no
    /// memory budget.
    pub fn new(doc: Document, label: impl Into<String>) -> History {
        let root = Node {
            parent: None,
            children: Vec::new(),
            last_child: None,
            label: label.into(),
            created: SystemTime::now(),
            doc,
        };
        let mut usage = Usage::default();
        usage.add(&root.doc);
        usage.visit(&root.doc);
        History {
            nodes: vec![Some(root)],
            root: NodeId(0),
            current: NodeId(0),
            budget: u64::MAX,
            dropped: 0,
            usage,
            held: Vec::new(),
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn current(&self) -> NodeId {
        self.current
    }

    /// The document in the current state.
    pub fn doc(&self) -> &Document {
        &self.node(self.current).doc
    }

    /// State `id`, which must not have been dropped (see [`contains`](Self::contains)).
    pub fn node(&self, id: NodeId) -> &Node {
        self.nodes[id.0].as_ref().unwrap_or_else(|| panic!("history state {id:?} was dropped"))
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes[id.0].as_mut().unwrap_or_else(|| panic!("history state {id:?} was dropped"))
    }

    /// Whether state `id` is still kept.
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.get(id.0).is_some_and(Option::is_some)
    }

    /// Every state kept, in the order they were made (the root first: every other state
    /// descends from it, and so was made after it).
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        let first = self.root.0;
        self.nodes[first..].iter().enumerate().filter_map(move |(i, n)| n.as_ref().map(|n| (NodeId(first + i), n)))
    }

    /// Records `doc` as the result of an action labeled `label`, as a new child of the
    /// current state, and makes it current. Earlier redo states stay reachable as a branch.
    /// The oldest states are then dropped if the others hold more than the budget.
    pub fn commit(&mut self, doc: Document, label: impl Into<String>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.usage.add(&doc);
        self.nodes.push(Some(Node {
            parent: Some(self.current),
            children: Vec::new(),
            last_child: None,
            label: label.into(),
            created: SystemTime::now(),
            doc,
        }));
        let parent = self.node_mut(self.current);
        parent.children.push(id);
        parent.last_child = Some(id);
        self.moved_to(id);
        id
    }

    /// Makes `id` current. Moving can leave the other states holding more than the budget
    /// (the state left behind joins them), so it trims too.
    fn moved_to(&mut self, id: NodeId) {
        self.current = id;
        let doc = &self.nodes[id.0].as_ref().expect("a state kept").doc;
        self.usage.visit(doc);
        self.trim();
    }

    pub fn can_undo(&self) -> bool {
        self.node(self.current).parent.is_some()
    }

    pub fn can_redo(&self) -> bool {
        self.redo_target().is_some()
    }

    /// Moves to the parent state. Returns whether there was one.
    pub fn undo(&mut self) -> bool {
        match self.node(self.current).parent {
            Some(p) => {
                let from = self.current;
                self.node_mut(p).last_child = Some(from);
                self.moved_to(p);
                true
            }
            None => false,
        }
    }

    /// Moves to the child state last visited (or the newest, if none was). Returns whether
    /// there was one.
    pub fn redo(&mut self) -> bool {
        match self.redo_target() {
            Some(c) => {
                self.moved_to(c);
                true
            }
            None => false,
        }
    }

    /// The state [`redo`](Self::redo) would move to.
    pub fn redo_target(&self) -> Option<NodeId> {
        self.redo_target_of(self.current)
    }

    /// The state redo would move to from `id`.
    pub fn redo_target_of(&self, id: NodeId) -> Option<NodeId> {
        let n = self.node(id);
        n.last_child.or_else(|| n.children.last().copied())
    }

    /// The label of the action [`undo`](Self::undo) would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo().then(|| self.node(self.current).label.as_str())
    }

    /// The label of the action [`redo`](Self::redo) would repeat.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo_target().map(|c| self.node(c).label.as_str())
    }

    /// Moves to any state kept. Every state on the way from the root to it records the
    /// branch taken, so undo and redo afterwards follow this branch.
    pub fn goto(&mut self, target: NodeId) {
        assert!(self.contains(target), "no history state {target:?}");
        let mut child = target;
        while let Some(parent) = self.node(child).parent {
            self.node_mut(parent).last_child = Some(child);
            child = parent;
        }
        self.moved_to(target);
    }

    /// The states from the root to `id`, root first.
    pub fn path_from_root(&self, id: NodeId) -> Vec<NodeId> {
        let mut path = vec![id];
        let mut n = id;
        while let Some(p) = self.node(n).parent {
            path.push(p);
            n = p;
        }
        path.reverse();
        path
    }

    /// Limits the pixels the states other than the current one hold, beyond what they share
    /// with it, to `bytes` (`u64::MAX` for no limit), dropping the oldest states now and
    /// after each new one as needed.
    pub fn set_budget(&mut self, bytes: u64) {
        if bytes != self.budget {
            self.budget = bytes;
            self.trim();
        }
    }

    /// The budget set: what the states other than the current one may hold beyond it.
    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// Keeps state `id` from being dropped to keep within the budget, until
    /// [`release`](Self::release)d: the state before an action that commits several states
    /// to be joined into one, so that undoing it still gets back to where it began.
    pub fn hold(&mut self, id: NodeId) {
        self.held.push(id);
    }

    /// Lets go of state `id` held by [`hold`](Self::hold) (once for each hold), and drops
    /// what is past the budget now.
    pub fn release(&mut self, id: NodeId) {
        if let Some(i) = self.held.iter().position(|&h| h == id) {
            self.held.remove(i);
        }
        self.trim();
    }

    /// How many states have been dropped to keep within the budget.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// The bytes of pixels the states other than the current one hold that it does not:
    /// what the history costs on top of the document itself.
    pub fn memory(&self) -> u64 {
        self.usage.extra()
    }

    /// Drops the oldest states while the others hold more than the budget. A state can be
    /// dropped when it has no children, or is the root with one child (which becomes the
    /// root), so the history stays one tree. Never dropped: the current state, the one
    /// before it (so the last action can be undone, however large), and every state redo
    /// goes through from here, the newest work, which undoing to look back must not lose;
    /// past the budget, the history waits for the next action to trim it.
    fn trim(&mut self) {
        // Within the budget (as almost always), nothing to look for: the states to keep are
        // as many as redo goes through, which a script's undoing can make many.
        if self.usage.extra() <= self.budget {
            return;
        }
        let mut keep: HashSet<NodeId> =
            [Some(self.current), self.node(self.current).parent].into_iter().flatten().collect();
        keep.extend(self.held.iter().copied());
        let mut at = self.current;
        while let Some(next) = self.redo_target_of(at) {
            keep.insert(next);
            at = next;
        }
        while self.usage.extra() > self.budget {
            let droppable = |(id, n): &(NodeId, &Node)| {
                !keep.contains(id) && (n.children.is_empty() || (*id == self.root && n.children.len() == 1))
            };
            let Some(oldest) = self.nodes().find(droppable).map(|(id, _)| id) else { break };
            self.remove(oldest);
            self.dropped += 1;
        }
    }

    /// Joins the states one script (say) made one after another into one, as if they were
    /// one action. What it undid goes first: a branch off the way from the root to the
    /// current state that is all in `made`, wherever it hangs. Then along the way, each line
    /// of states in `made` becomes its last, put in its first's place and labeled `label`,
    /// and the others go. A state with something else made from it (by someone else) stays,
    /// and ends its line. A line's last state keeps its identifier (it is the same document,
    /// given an earlier parent), so whatever refers to it still does; a line of one state is
    /// labeled too. A state of `made` that is also `apart` (a step a script named, inside the
    /// script's run) is no part of a line, and ends one. Returns how many states went.
    pub fn join(&mut self, made: &HashSet<NodeId>, label: &str, apart: &HashSet<NodeId>) -> usize {
        let path = self.path_from_root(self.current);
        let on_path: HashSet<NodeId> = path.iter().copied().collect();
        let mut gone = 0;
        for &id in &path {
            let undone: Vec<NodeId> = (self.node(id).children.iter().copied())
                .filter(|c| !on_path.contains(c) && self.all_made(*c, made))
                .collect();
            for branch in undone {
                gone += self.drop_branch(branch);
            }
        }
        let joinable = |n: &NodeId| made.contains(n) && !apart.contains(n);
        let mut first = None;
        for (i, &id) in path.iter().enumerate() {
            if !joinable(&id) {
                continue;
            }
            let start = *first.get_or_insert(id);
            let next = path.get(i + 1).filter(|n| joinable(n));
            // What else is made from it is someone else's now (the script's undone went).
            let joins = next.is_some_and(|&next| self.node(id).children.iter().all(|&c| c == next));
            if !joins {
                if start != id {
                    gone += self.join_line(start, id, label);
                } else {
                    self.node_mut(id).label = label.to_owned();
                }
                first = None;
            }
        }
        gone
    }

    /// Whether state `id` and everything descending from it are in `made`.
    fn all_made(&self, id: NodeId, made: &HashSet<NodeId>) -> bool {
        // Walked with a stack of its own: a script's undone states can hang many deep.
        let mut to_see = vec![id];
        while let Some(at) = to_see.pop() {
            if !made.contains(&at) {
                return false;
            }
            to_see.extend(&self.node(at).children);
        }
        true
    }

    /// Drops state `branch` and all that descends from it (none of it current, nor on the
    /// way to it). Returns how many states went.
    fn drop_branch(&mut self, branch: NodeId) -> usize {
        if let Some(p) = self.node(branch).parent {
            let p = self.node_mut(p);
            p.children.retain(|&c| c != branch);
            if p.last_child == Some(branch) {
                p.last_child = None;
            }
        }
        let mut going = vec![branch];
        let mut i = 0;
        while i < going.len() {
            let below = self.node(going[i]).children.clone();
            going.extend(below);
            i += 1;
        }
        for id in &going {
            let node = self.nodes[id.0].take().expect("a state kept");
            self.usage.forget(&node.doc);
        }
        going.len()
    }

    /// Puts `end` in the place of `start`, an earlier state on its way from the root,
    /// labeled `label`; the states between (`start` included) and what else descends from
    /// them go. Returns how many.
    fn join_line(&mut self, start: NodeId, end: NodeId, label: &str) -> usize {
        let mut line = Vec::new();
        let mut at = end;
        while at != start {
            at = self.node(at).parent.expect("start is on the way from the root to end");
            line.push(at);
        }
        let parent = self.node(start).parent;
        match parent {
            Some(p) => {
                let p = self.node_mut(p);
                for c in &mut p.children {
                    if *c == start {
                        *c = end;
                    }
                }
                if p.last_child == Some(start) {
                    p.last_child = Some(end);
                }
            }
            None => self.root = end,
        }
        let e = self.node_mut(end);
        e.parent = parent;
        e.label = label.to_owned();
        let on_line: HashSet<NodeId> = line.iter().copied().collect();
        let mut going = line;
        let mut i = 0;
        while i < going.len() {
            let side = self.node(going[i]).children.iter().filter(|&&c| c != end && !on_line.contains(&c));
            going.extend(side.copied().collect::<Vec<_>>());
            i += 1;
        }
        for id in &going {
            let node = self.nodes[id.0].take().expect("a state kept");
            self.usage.forget(&node.doc);
        }
        going.len()
    }

    fn remove(&mut self, id: NodeId) {
        let node = self.nodes[id.0].take().expect("a state kept");
        self.usage.forget(&node.doc);
        match node.parent {
            Some(p) => {
                let parent = self.node_mut(p);
                parent.children.retain(|&c| c != id);
                if parent.last_child == Some(id) {
                    parent.last_child = None;
                }
            }
            None => {
                let child = node.children[0];
                self.node_mut(child).parent = None;
                self.root = child;
            }
        }
    }
}

/// The pixels the states hold, counted once per distinct tile, and the reduced copies their
/// images keep. Layers, chunks of tiles, tiles and images are known by address: what a state
/// holds never changes, and an address counted here belongs to something a kept state holds,
/// so it cannot be reused while it is counted.
///
/// Tiles are counted by the chunks images keep them in (see `RasterImage::chunks`), which
/// an image shares with its edited copy but for the chunks the edit changed: so counting a
/// new state walks the tiles of the chunks new to the account alone, not every tile of each
/// layer it changed.
#[derive(Clone, Debug, Default)]
struct Usage {
    /// Each distinct layer the states hold: in how many states, its chunks and its images.
    layers: HashMap<usize, Held>,
    /// Each distinct chunk: in how many of those layers, with its tiles (each once) and the
    /// memory each takes.
    chunks: Counted<Vec<(usize, u64)>>,
    /// Each distinct tile (with the memory it takes): in how many of those chunks.
    tiles: Counted<u64>,
    /// Each distinct image: in how many of those layers.
    images: Counted<Arc<RasterImage>>,
    /// The memory those tiles take.
    tile_bytes: u64,
    /// The current state's layers, and their chunks, tiles and images, each with how many
    /// of its holders hold it, and the memory those tiles take.
    current_layers: HashSet<usize>,
    current_chunks: Counted<()>,
    current: Counted<u64>,
    current_images: Counted<()>,
    current_bytes: u64,
}

#[derive(Clone, Debug)]
struct Held {
    states: usize,
    /// Its images' chunks, by address (one two images share, twice).
    chunks: Vec<usize>,
    images: Vec<Arc<RasterImage>>,
}

/// Things counted by address, each with how many holders it has, and a value kept with it.
#[derive(Clone, Debug)]
struct Counted<V>(HashMap<usize, (usize, V)>);

impl<V> Default for Counted<V> {
    fn default() -> Counted<V> {
        Counted(HashMap::new())
    }
}

impl<V> Counted<V> {
    /// One more holder of `key` (with `value`, if it is new); whether it is.
    fn add(&mut self, key: usize, value: impl FnOnce() -> V) -> bool {
        let e = self.0.entry(key).or_insert_with(|| (0, value()));
        e.0 += 1;
        e.0 == 1
    }

    /// One holder fewer of `key`, which goes once none is left; its value then.
    fn remove(&mut self, key: usize) -> Option<V> {
        let e = self.0.get_mut(&key).expect("counted");
        e.0 -= 1;
        if e.0 == 0 { self.0.remove(&key).map(|(_, v)| v) } else { None }
    }

    fn get(&self, key: usize) -> &V {
        &self.0.get(&key).expect("counted").1
    }

    fn contains(&self, key: usize) -> bool {
        self.0.contains_key(&key)
    }
}

fn address<T>(a: &Arc<T>) -> usize {
    Arc::as_ptr(a) as usize
}

impl Usage {
    /// Counts a new state's document (only its layers not counted already are walked, and
    /// of their chunks, only the tiles of those not counted already).
    fn add(&mut self, doc: &Document) {
        for l in layers_of(doc) {
            let Usage { layers, chunks, tiles, images, tile_bytes, .. } = self;
            let e = layers.entry(address(l)).or_insert_with(|| {
                let held = images_of(l);
                let mut ids = Vec::new();
                for image in &held {
                    images.add(address(image), || image.clone());
                    for (id, chunk) in image.chunks() {
                        ids.push(id);
                        if chunks.add(id, || distinct(chunk)) {
                            for &(t, b) in chunks.get(id) {
                                if tiles.add(t, || b) {
                                    *tile_bytes += b;
                                }
                            }
                        }
                    }
                }
                Held { states: 0, chunks: ids, images: held }
            });
            e.states += 1;
        }
    }

    /// Takes away what a dropped state's document alone held.
    fn forget(&mut self, doc: &Document) {
        for l in layers_of(doc) {
            let key = address(l);
            let e = self.layers.get_mut(&key).expect("counted");
            e.states -= 1;
            if e.states > 0 {
                continue;
            }
            let held = self.layers.remove(&key).expect("counted");
            for id in held.chunks {
                for (t, _) in self.chunks.remove(id).into_iter().flatten() {
                    if let Some(b) = self.tiles.remove(t) {
                        self.tile_bytes -= b;
                    }
                }
            }
            held.images.iter().for_each(|i| {
                self.images.remove(address(i));
            });
        }
    }

    /// Makes `doc` (a counted state's) the current one. Only the layers it does not share
    /// with the state current before are walked, and of their chunks, only the tiles of
    /// those the current state did not hold.
    fn visit(&mut self, doc: &Document) {
        let now: HashSet<usize> = layers_of(doc).into_iter().map(address).collect();
        let Usage { layers, chunks, current_layers, current_chunks, current, current_images, current_bytes, .. } = self;
        for k in current_layers.difference(&now) {
            let h = &layers[k];
            for &id in &h.chunks {
                if current_chunks.remove(id).is_some() {
                    for &(t, _) in chunks.get(id) {
                        if let Some(b) = current.remove(t) {
                            *current_bytes -= b;
                        }
                    }
                }
            }
            h.images.iter().for_each(|i| {
                current_images.remove(address(i));
            });
        }
        for k in now.difference(current_layers) {
            let h = &layers[k];
            for &id in &h.chunks {
                if current_chunks.add(id, || ()) {
                    for &(t, b) in chunks.get(id) {
                        if current.add(t, || b) {
                            *current_bytes += b;
                        }
                    }
                }
            }
            h.images.iter().for_each(|i| {
                current_images.add(address(i), || ());
            });
        }
        *current_layers = now;
    }

    /// The bytes the current state does not hold: tiles, and the reduced copies of images
    /// (built when shown, so counted as they are now).
    fn extra(&self) -> u64 {
        let tiles = self.tile_bytes - self.current_bytes;
        let reduced: u64 = self
            .images
            .0
            .iter()
            .filter(|(k, _)| !self.current_images.contains(**k))
            .map(|(_, (_, i))| i.reduced_bytes())
            .sum();
        tiles + reduced
    }
}

/// Every layer of `doc`, groups and what they hold included.
fn layers_of(doc: &Document) -> Vec<&Arc<Layer>> {
    fn walk<'a>(layers: &'a [Arc<Layer>], out: &mut Vec<&'a Arc<Layer>>) {
        for l in layers {
            out.push(l);
            if let LayerContent::Group(children) = &l.content {
                walk(children, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&doc.layers, &mut out);
    out
}

/// The images layer `l`'s own objects hold, each once.
fn images_of(l: &Layer) -> Vec<Arc<RasterImage>> {
    let LayerContent::Objects(objects) = &l.content else { return Vec::new() };
    let mut images: Vec<Arc<RasterImage>> = objects
        .iter()
        .map(|o| match &o.kind {
            ObjectKind::Pixels(p) => p.image.clone(),
            ObjectKind::Image(i) => i.image.clone(),
        })
        .collect();
    images.sort_unstable_by_key(address);
    images.dedup_by_key(|i| address(i));
    images
}

/// The tiles of a chunk (by address) with the memory each takes, each once.
fn distinct<'a>(tiles: impl Iterator<Item = &'a Arc<crate::tile::Tile>>) -> Vec<(usize, u64)> {
    let mut out: Vec<(usize, u64)> = tiles.map(|t| (address(t), t.bytes())).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{IdGen, Layer, Position};
    use crate::raster::TILE_BYTES;

    fn with_layer(doc: &Document, ids: &mut IdGen, name: &str) -> Document {
        let mut d = doc.clone();
        let index = d.layers.len();
        d.insert(Position { parent: None, index }, Layer::new_empty(ids.layer(), name)).unwrap();
        d
    }

    fn names(h: &History) -> Vec<String> {
        h.doc().layers.iter().map(|l| l.name.clone()).collect()
    }

    /// `doc` with pixel `(x, 0)` of its first layer changed: a tile of its own.
    fn painted(doc: &Document, x: i64) -> Document {
        let mut d = doc.clone();
        let id = d.layers[0].id;
        let LayerContent::Objects(objects) = &mut d.layer_mut(id).unwrap().content else { panic!() };
        let ObjectKind::Pixels(p) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut p.image).set_pixel(x, 0, crate::tile::pixel_from_f32([1.0, 0.0, x as f32 / 100.0, 1.0]));
        d
    }

    /// A history of one raster layer, painted `n` times.
    fn painted_history(n: usize) -> History {
        let mut ids = IdGen::default();
        let mut doc = Document::new(10, 10);
        let layer = Layer::new_raster(ids.layer(), ids.object(), "paint", crate::raster::RasterImage::new());
        doc.insert(Position { parent: None, index: 0 }, layer).unwrap();
        let mut h = History::new(painted(&doc, 0), "New");
        for i in 1..=n {
            let d = painted(h.doc(), i as i64);
            h.commit(d, format!("Paint {i}"));
        }
        h
    }

    #[test]
    fn undo_and_redo_walk_a_line() {
        let mut ids = IdGen::default();
        let mut h = History::new(Document::new(10, 10), "New");
        let d1 = with_layer(h.doc(), &mut ids, "a");
        h.commit(d1, "Add a");
        let d2 = with_layer(h.doc(), &mut ids, "b");
        h.commit(d2, "Add b");
        assert_eq!(names(&h), ["a", "b"]);
        assert_eq!(h.undo_label(), Some("Add b"));
        assert!(h.undo());
        assert_eq!(names(&h), ["a"]);
        assert_eq!(h.redo_label(), Some("Add b"));
        assert!(h.undo());
        assert!(!h.undo());
        assert!(h.redo());
        assert!(h.redo());
        assert!(!h.redo());
        assert_eq!(names(&h), ["a", "b"]);
    }

    #[test]
    fn a_new_action_after_undo_branches_instead_of_discarding() {
        let mut ids = IdGen::default();
        let mut h = History::new(Document::new(10, 10), "New");
        let d = with_layer(h.doc(), &mut ids, "a");
        let a = h.commit(d, "Add a");
        h.undo();
        let d = with_layer(h.doc(), &mut ids, "b");
        let b = h.commit(d, "Add b");
        assert_eq!(h.node(h.root()).children, vec![a, b]);
        // Redo from the root follows the branch visited last.
        h.undo();
        assert_eq!(h.redo_target(), Some(b));
        // Jumping to the other branch makes it the one redo follows.
        h.goto(a);
        assert_eq!(names(&h), ["a"]);
        h.undo();
        assert_eq!(h.redo_target(), Some(a));
    }

    #[test]
    fn goto_records_the_whole_path() {
        let mut ids = IdGen::default();
        let mut h = History::new(Document::new(10, 10), "New");
        let d = with_layer(h.doc(), &mut ids, "a");
        let a = h.commit(d, "a");
        let d = with_layer(h.doc(), &mut ids, "a2");
        let a2 = h.commit(d, "a2");
        h.goto(h.root());
        let d = with_layer(h.doc(), &mut ids, "b");
        h.commit(d, "b");
        h.goto(a2);
        h.goto(h.root());
        assert!(h.redo());
        assert_eq!(h.current(), a);
        assert!(h.redo());
        assert_eq!(h.current(), a2);
        assert_eq!(h.path_from_root(a2), vec![h.root(), a, a2]);
    }

    #[test]
    fn past_the_budget_the_oldest_states_are_dropped() {
        let mut h = painted_history(5);
        // Six states, each with its own copy of the one tile.
        assert_eq!(h.memory(), 5 * TILE_BYTES);
        h.set_budget(2 * TILE_BYTES);
        assert_eq!(h.dropped(), 3);
        assert_eq!(h.memory(), 2 * TILE_BYTES);
        let labels: Vec<&str> = h.nodes().map(|(_, n)| n.label.as_str()).collect();
        assert_eq!(labels, ["Paint 3", "Paint 4", "Paint 5"]);
        assert_eq!(h.node(h.root()).parent, None);
        assert!(!h.contains(NodeId(0)));
        // Undo stops at the oldest state left; new states keep the oldest going.
        assert!(h.undo() && h.undo() && !h.undo());
        h.goto(NodeId(5));
        let d = painted(h.doc(), 50);
        h.commit(d, "Paint 6");
        assert_eq!(h.nodes().map(|(_, n)| n.label.as_str()).collect::<Vec<_>>(), ["Paint 4", "Paint 5", "Paint 6"]);
    }

    #[test]
    fn a_state_held_is_kept_past_the_budget_until_let_go() {
        // The state before an action made of several commits, as Re-apply All holds it.
        let mut h = painted_history(1);
        h.set_budget(TILE_BYTES);
        let before = h.current();
        h.hold(before);
        for i in 0..4 {
            let d = painted(h.doc(), 60 + i);
            h.commit(d, "Part");
        }
        assert!(h.contains(before), "the state held was dropped");
        h.release(before);
        assert!(!h.contains(before), "let go, it was not dropped as the budget says");
    }

    #[test]
    fn a_state_holding_an_eight_bit_tile_counts_its_own_size() {
        let mut ids = IdGen::default();
        let mut doc = Document::new(10, 10);
        let eight = crate::raster::RasterImage::from_srgb8(1, 1, &[[200, 100, 50, 255]]);
        doc.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "8", eight))
            .unwrap();
        let mut h = History::new(doc, "Open");
        // Painting makes the tile working pixels; the state before holds the 8-bit one.
        let d = painted(h.doc(), 5);
        h.commit(d, "Paint");
        assert_eq!(h.memory(), TILE_BYTES / 2);
        assert!(h.undo());
        assert_eq!(h.memory(), TILE_BYTES, "the state after holds a working tile");
    }

    #[test]
    fn what_the_current_state_shares_costs_the_history_nothing() {
        let mut ids = IdGen::default();
        let mut h = painted_history(0);
        // States that only add empty layers share the painted tile with the current one.
        for name in ["a", "b", "c"] {
            let d = with_layer(h.doc(), &mut ids, name);
            h.commit(d, name);
        }
        assert_eq!(h.memory(), 0);
        h.set_budget(0);
        assert_eq!(h.dropped(), 0, "nothing was worth dropping");
    }

    #[test]
    fn a_branch_point_at_the_root_drops_the_oldest_leaf_first() {
        let mut h = painted_history(2);
        h.goto(h.root());
        let d = painted(h.doc(), 70);
        let other = h.commit(d, "Other");
        // The root has two branches, so the oldest state that can go is the end of the
        // first: "Paint 2", then "Paint 1", and then the root itself.
        h.set_budget(2 * TILE_BYTES);
        assert_eq!(h.nodes().map(|(_, n)| n.label.as_str()).collect::<Vec<_>>(), ["New", "Paint 1", "Other"]);
        // Then all it may: the state before the current one stays.
        h.set_budget(0);
        assert_eq!(h.nodes().map(|(id, _)| id).collect::<Vec<_>>(), [h.root(), other]);
        assert_eq!(h.node(other).parent, Some(h.root()));
    }

    #[test]
    fn the_last_action_can_be_undone_however_large() {
        let mut h = painted_history(1);
        h.set_budget(0);
        // The painting replaced the only tile, so the state before holds a tile alone.
        assert_eq!(h.memory(), TILE_BYTES);
        assert!(h.undo(), "the edit cannot be undone");
        // Moving back makes the other state the one before (the parent stays as it was).
        assert!(h.redo());
        let d = painted(h.doc(), 9);
        h.commit(d, "Paint 2");
        assert_eq!(h.nodes().count(), 2, "only the state before is kept past the budget");
        assert!(h.undo());
    }

    #[test]
    fn moving_through_history_keeps_to_the_budget_one_step_either_way() {
        // Three states of one tile each, then one sharing that tile and adding another.
        let mut h = painted_history(2);
        let d = painted(h.doc(), 300);
        let last = h.commit(d, "Paint far");
        h.set_budget(2 * TILE_BYTES);
        assert_eq!(h.dropped(), 0);
        // Back one step: the state left behind holds a tile of its own, which puts the others
        // over the budget.
        let root = h.root();
        assert!(h.undo());
        assert!(!h.contains(root), "moving back did not trim");
        assert!(h.memory() <= 2 * TILE_BYTES);
        // Tighter still: what is left is one step either way, which stays past the budget.
        h.set_budget(TILE_BYTES);
        assert!(h.memory() > TILE_BYTES);
        assert_eq!(h.redo_target(), Some(last), "the state redo goes to was dropped");
        assert!(h.undo(), "the state before was dropped");
    }

    #[test]
    fn undoing_to_look_back_never_loses_the_newest_work() {
        // Each state adds a tile of its own to the one before's.
        let mut h = painted_history(0);
        let d = painted(h.doc(), 300);
        h.commit(d, "Paint 1");
        let d = painted(h.doc(), 600);
        let newest = h.commit(d, "Paint 2");
        h.set_budget(TILE_BYTES);
        // Back to the start: the newest states are over the budget, and stay.
        assert!(h.undo() && h.undo());
        assert!(h.redo() && h.redo(), "the newest work was dropped");
        assert_eq!(h.current(), newest);
    }

    #[test]
    fn the_reduced_copies_of_images_only_old_states_hold_are_counted() {
        let mut h = painted_history(0);
        let before = h.memory();
        // Built while shown, then left behind by a new state with another image.
        let LayerContent::Objects(objects) = &h.doc().layers[0].content else { panic!() };
        let ObjectKind::Pixels(p) = &objects[0].kind else { panic!() };
        p.image.reduced(1);
        let built = p.image.reduced_bytes();
        assert!(built > 0);
        let d = painted(h.doc(), 3);
        h.commit(d, "Paint");
        assert_eq!(h.memory(), before + TILE_BYTES + built);
    }

    /// What the states other than the current one hold that it does not, counted tile by
    /// tile, apart from the account: the distinct tiles of their images, and the reduced
    /// copies of their images.
    fn by_tile(h: &History) -> u64 {
        let images =
            |d: &Document| -> Vec<Arc<RasterImage>> { layers_of(d).into_iter().flat_map(|l| images_of(l)).collect() };
        let tiles = |d: &Document| -> HashMap<usize, u64> {
            images(d).iter().flat_map(|i| i.tiles().map(|(_, t)| (address(t), t.bytes())).collect::<Vec<_>>()).collect()
        };
        let current = tiles(h.doc());
        let current_images: HashSet<usize> = images(h.doc()).iter().map(address).collect();
        let mut others: HashMap<usize, u64> = HashMap::new();
        let mut other_images: HashMap<usize, Arc<RasterImage>> = HashMap::new();
        for (_, n) in h.nodes() {
            others.extend(tiles(&n.doc).into_iter().filter(|(k, _)| !current.contains_key(k)));
            other_images.extend(
                images(&n.doc).into_iter().map(|i| (address(&i), i)).filter(|(k, _)| !current_images.contains(k)),
            );
        }
        others.values().sum::<u64>() + other_images.values().map(|i| i.reduced_bytes()).sum::<u64>()
    }

    #[test]
    fn one_tile_repeated_through_a_large_layer_is_counted_once_and_an_edit_one_tile() {
        // 64 × 64 tiles, one tile everywhere: 16 chunks, each holding it.
        let mut ids = IdGen::default();
        let mut doc = Document::new(256 * 64, 256 * 64);
        let white = RasterImage::filled(
            &crate::geom::IRect::from_size(256 * 64, 256 * 64),
            crate::tile::pixel_from_f32([1.0; 4]),
        );
        doc.insert(Position { parent: None, index: 0 }, Layer::new_raster(ids.layer(), ids.object(), "canvas", white))
            .unwrap();
        let mut h = History::new(doc, "new");
        let mut edited = h.doc().clone();
        let canvas = edited.layers[0].id;
        let crate::document::LayerContent::Objects(objects) = &mut edited.layer_mut(canvas).unwrap().content else {
            panic!()
        };
        let ObjectKind::Pixels(p) = &mut objects[0].kind else { panic!() };
        Arc::make_mut(&mut p.image).set_pixel(5000, 5000, crate::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
        h.commit(edited, "paint");
        // The state before holds only the white tile, which this one holds too.
        assert_eq!((h.memory(), by_tile(&h)), (0, 0));
        h.undo();
        // This one holds the painted tile alone.
        assert_eq!((h.memory(), by_tile(&h)), (TILE_BYTES, TILE_BYTES));
    }

    /// The memory `h` counts is what counting its states afresh, and tile by tile, gives.
    fn counted_right(h: &History) -> bool {
        let mut u = Usage::default();
        h.nodes().for_each(|(_, n)| u.add(&n.doc));
        u.visit(h.doc());
        h.memory() == u.extra() && h.memory() == by_tile(h)
    }

    /// Paints `h` once more (pixel `x`), as a script would: the state, added to `made`.
    fn scripted(h: &mut History, made: &mut HashSet<NodeId>, x: i64) -> NodeId {
        let d = painted(h.doc(), x);
        let id = h.commit(d, format!("Paint {x}"));
        made.insert(id);
        id
    }

    #[test]
    fn a_script_s_states_join_into_its_last_in_place_of_its_first() {
        let mut h = painted_history(1);
        let before = h.current();
        let mut made = HashSet::new();
        // Each in the first tile, so each state holds a copy of its own.
        scripted(&mut h, &mut made, 1);
        let last = [2, 3].map(|x| scripted(&mut h, &mut made, x))[1];
        let memory = h.memory();
        assert_eq!(h.join(&made, "Script: go.py", &HashSet::new()), 2);
        assert_eq!(h.current(), last, "the last state is the same one");
        assert_eq!(h.node(last).parent, Some(before));
        assert_eq!(h.node(before).children, vec![last]);
        assert_eq!(h.undo_label(), Some("Script: go.py"));
        assert_eq!(h.nodes().count(), 3);
        assert!(h.memory() < memory && counted_right(&h), "the states gone are no longer counted");
        h.undo();
        assert_eq!(h.current(), before);
        assert_eq!(h.redo_target(), Some(last));
    }

    #[test]
    fn what_a_script_undid_goes_with_the_states_joined() {
        let mut h = painted_history(1);
        let mut made = HashSet::new();
        let first = scripted(&mut h, &mut made, 300);
        let undone = scripted(&mut h, &mut made, 600);
        h.undo();
        let last = scripted(&mut h, &mut made, 900);
        assert_eq!(h.join(&made, "Script", &HashSet::new()), 2);
        assert!(!h.contains(first) && !h.contains(undone));
        assert_eq!(h.path_from_root(last).len(), 3);
        assert!(counted_right(&h));
    }

    #[test]
    fn a_state_someone_else_made_between_or_from_ends_a_line() {
        // The user's change between two of the script's: nothing to join, each of the
        // script's a line of its own (named as its run).
        let mut h = painted_history(1);
        let mut made = HashSet::new();
        let a = scripted(&mut h, &mut made, 300);
        let d = painted(h.doc(), 400);
        let theirs = h.commit(d, "Paint by hand");
        let b = scripted(&mut h, &mut made, 600);
        assert_eq!(h.join(&made, "Script", &HashSet::new()), 0);
        let labels = [a, theirs, b].map(|n| h.node(n).label.clone());
        assert_eq!(labels, ["Script", "Paint by hand", "Script"]);
        // The user went back to one of the script's states and made something from it: it
        // stays (what was made from it depends on it), and the script's line starts after it.
        let mut h = painted_history(1);
        let mut made = HashSet::new();
        let kept = scripted(&mut h, &mut made, 300);
        let gone = scripted(&mut h, &mut made, 600);
        let last = scripted(&mut h, &mut made, 900);
        h.goto(kept);
        let d = painted(h.doc(), 400);
        let theirs = h.commit(d, "Paint by hand");
        h.goto(last);
        assert_eq!(h.join(&made, "Script", &HashSet::new()), 1);
        assert!(!h.contains(gone));
        assert_eq!(h.node(kept).children, vec![last, theirs]);
        assert_eq!(h.node(theirs).parent, Some(kept));
        assert!(counted_right(&h));
    }

    #[test]
    fn a_script_s_line_from_the_root_makes_its_last_the_root() {
        let mut h = painted_history(0);
        let mut made = HashSet::new();
        let first = scripted(&mut h, &mut made, 1);
        let last = scripted(&mut h, &mut made, 2);
        // The state the image was opened in is dropped (to keep the budget), so the
        // script's first state is the root.
        h.set_budget(0);
        assert_eq!(h.root(), first);
        assert_eq!(h.join(&made, "Script", &HashSet::new()), 1);
        assert_eq!(h.root(), last);
        assert!(!h.can_undo());
        assert_eq!(h.nodes().map(|(id, _)| id).collect::<Vec<_>>(), [last]);
        assert!(counted_right(&h));
    }

    #[test]
    fn a_state_kept_apart_ends_a_line_and_a_lone_one_is_named_too() {
        let mut h = painted_history(1);
        let mut made = HashSet::new();
        let before = scripted(&mut h, &mut made, 300);
        let step = scripted(&mut h, &mut made, 600);
        h.node_mut(step).label = "A step".to_owned();
        let after = scripted(&mut h, &mut made, 900);
        assert_eq!(h.join(&made, "Script", &HashSet::from([step])), 0);
        assert_eq!(
            [&h.node(before).label, &h.node(step).label, &h.node(after).label].map(|l| l.as_str()),
            ["Script", "A step", "Script"],
            "the step is kept as it is, the run on either side of it each a line"
        );
    }

    #[test]
    fn what_a_script_undid_in_a_step_goes_with_its_run() {
        let mut h = painted_history(1);
        let before = h.current();
        let mut made = HashSet::new();
        let first = scripted(&mut h, &mut made, 1);
        let stepped = scripted(&mut h, &mut made, 2);
        h.undo();
        let last = scripted(&mut h, &mut made, 3);
        assert_eq!(h.join(&made, "Script", &HashSet::from([stepped])), 2);
        assert!(!h.contains(first) && !h.contains(stepped));
        assert_eq!(h.undo_label(), Some("Script"));
        assert_eq!(h.node(last).parent, Some(before));
    }

    #[test]
    fn what_a_script_undid_goes_though_a_step_ends_the_line_it_hangs_from() {
        let mut h = painted_history(1);
        let mut made = HashSet::new();
        let a = scripted(&mut h, &mut made, 1);
        let undone = scripted(&mut h, &mut made, 2);
        h.undo();
        let step = scripted(&mut h, &mut made, 3);
        // And after the step, another undone.
        let after = scripted(&mut h, &mut made, 4);
        h.undo();
        let last = scripted(&mut h, &mut made, 5);
        h.join(&made, "Script", &HashSet::from([step]));
        assert!(!h.contains(undone) && !h.contains(after), "what the script undid stayed beside its step");
        assert_eq!(h.node(a).children, [step]);
        assert_eq!(h.node(step).children, [last]);
        assert!(counted_right(&h));
    }

    #[test]
    fn a_script_s_long_undone_branch_goes_without_overflowing_the_stack() {
        let mut h = History::new(Document::new(10, 10), "New");
        let mut made = HashSet::new();
        let commit = |h: &mut History, made: &mut HashSet<NodeId>| {
            let d = h.doc().clone();
            made.insert(h.commit(d, "Rename"));
        };
        commit(&mut h, &mut made);
        // Many states, all undone: a branch that deep hangs off the first.
        for _ in 0..50_000 {
            commit(&mut h, &mut made);
        }
        for _ in 0..50_000 {
            h.undo();
        }
        commit(&mut h, &mut made);
        assert_eq!(h.join(&made, "Script", &HashSet::new()), 50_001);
        assert_eq!(h.nodes().count(), 2);
    }

    #[test]
    fn the_running_count_matches_a_fresh_one() {
        let fresh = |h: &History| {
            let mut u = Usage::default();
            h.nodes().for_each(|(_, n)| u.add(&n.doc));
            u.visit(h.doc());
            u.extra()
        };
        let mut h = painted_history(3);
        let mut ids = IdGen::default();
        for step in 0..40 {
            match step % 5 {
                // A pixel in the first tile (a copy of it), or in a tile of its own (shared
                // with the states after it), so states hold different numbers of tiles.
                0 | 1 => {
                    let d = painted(h.doc(), if step % 5 == 0 { step } else { step * 300 });
                    h.commit(d, "paint");
                }
                2 => {
                    h.undo();
                }
                3 => {
                    let d = with_layer(h.doc(), &mut ids, "empty");
                    h.commit(d, "add");
                }
                _ => h.set_budget(if step % 10 == 4 { 3 * TILE_BYTES } else { u64::MAX }),
            }
            assert_eq!(h.memory(), fresh(&h), "after step {step}");
            assert_eq!(h.memory(), by_tile(&h), "after step {step}, counted tile by tile");
        }
        assert!(h.dropped() > 0);
    }
}
