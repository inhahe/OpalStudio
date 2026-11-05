//! The undo history tree.
//!
//! Every node holds a complete snapshot of the document. Snapshots share everything
//! unchanged through `Arc`, so a node costs only what its action changed, and moving to
//! any node, on any branch, is a matter of picking up its snapshot. Past a memory budget
//! (see [`History::set_budget`]), the oldest states are dropped.

use crate::document::{Document, Layer, LayerContent, ObjectKind};
use crate::raster::{RasterImage, TILE_BYTES};
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
        History { nodes: vec![Some(root)], root: NodeId(0), current: NodeId(0), budget: u64::MAX, dropped: 0, usage }
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
    fn redo_target_of(&self, id: NodeId) -> Option<NodeId> {
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
        let mut keep: HashSet<NodeId> =
            [Some(self.current), self.node(self.current).parent].into_iter().flatten().collect();
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
/// images keep. Layers, tiles and images are known by address: what a state holds never
/// changes, and an address counted here belongs to something a kept state holds, so it
/// cannot be reused while it is counted.
#[derive(Clone, Debug, Default)]
struct Usage {
    /// Each distinct layer the states hold: in how many states, its tiles and its images.
    layers: HashMap<usize, Held>,
    /// Each distinct tile and image: in how many of those layers.
    tiles: Counted<()>,
    images: Counted<Arc<RasterImage>>,
    /// The current state's layers, and their tiles and images, each with how many of
    /// those layers hold it.
    current_layers: HashSet<usize>,
    current: Counted<()>,
    current_images: Counted<()>,
}

#[derive(Clone, Debug)]
struct Held {
    states: usize,
    tiles: Vec<usize>,
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
    /// One more holder of `key` (with `value`, if it is new).
    fn add(&mut self, key: usize, value: impl FnOnce() -> V) {
        self.0.entry(key).or_insert_with(|| (0, value())).0 += 1;
    }

    /// One holder fewer of `key`, which goes once none is left.
    fn remove(&mut self, key: usize) {
        let e = self.0.get_mut(&key).expect("counted");
        e.0 -= 1;
        if e.0 == 0 {
            self.0.remove(&key);
        }
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn contains(&self, key: usize) -> bool {
        self.0.contains_key(&key)
    }
}

fn address<T>(a: &Arc<T>) -> usize {
    Arc::as_ptr(a) as usize
}

impl Usage {
    /// Counts a new state's document (only its layers not counted already are walked).
    fn add(&mut self, doc: &Document) {
        for l in layers_of(doc) {
            let (tiles, images) = (&mut self.tiles, &mut self.images);
            let e = self.layers.entry(address(l)).or_insert_with(|| {
                let (t, im) = held_by(l);
                t.iter().for_each(|&k| tiles.add(k, || ()));
                im.iter().for_each(|i| images.add(address(i), || i.clone()));
                Held { states: 0, tiles: t, images: im }
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
            held.tiles.into_iter().for_each(|k| self.tiles.remove(k));
            held.images.iter().for_each(|i| self.images.remove(address(i)));
        }
    }

    /// Makes `doc` (a counted state's) the current one. Only the layers it does not share
    /// with the state current before are walked.
    fn visit(&mut self, doc: &Document) {
        let now: HashSet<usize> = layers_of(doc).into_iter().map(address).collect();
        for k in self.current_layers.difference(&now) {
            let h = &self.layers[k];
            h.tiles.iter().for_each(|&t| self.current.remove(t));
            h.images.iter().for_each(|i| self.current_images.remove(address(i)));
        }
        for k in now.difference(&self.current_layers) {
            let h = &self.layers[k];
            h.tiles.iter().for_each(|&t| self.current.add(t, || ()));
            h.images.iter().for_each(|i| self.current_images.add(address(i), || ()));
        }
        self.current_layers = now;
    }

    /// The bytes the current state does not hold: tiles, and the reduced copies of images
    /// (built when shown, so counted as they are now).
    fn extra(&self) -> u64 {
        let tiles = (self.tiles.len() - self.current.len()) as u64 * TILE_BYTES;
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

/// The tiles (by address) and the images layer `l`'s own objects hold, each once.
fn held_by(l: &Layer) -> (Vec<usize>, Vec<Arc<RasterImage>>) {
    let LayerContent::Objects(objects) = &l.content else { return (Vec::new(), Vec::new()) };
    let mut images: Vec<Arc<RasterImage>> = objects
        .iter()
        .map(|o| match &o.kind {
            ObjectKind::Pixels(p) => p.image.clone(),
            ObjectKind::Image(i) => i.image.clone(),
        })
        .collect();
    images.sort_unstable_by_key(address);
    images.dedup_by_key(|i| address(i));
    let mut tiles: Vec<usize> = images.iter().flat_map(|i| i.tiles().map(|(_, t)| address(t))).collect();
    tiles.sort_unstable();
    tiles.dedup();
    (tiles, images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{IdGen, Layer, Position};

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
        }
        assert!(h.dropped() > 0);
    }
}
