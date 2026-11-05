//! The undo history tree.
//!
//! Every node holds a complete snapshot of the document. Snapshots share everything
//! unchanged through `Arc`, so a node costs only what its action changed, and moving to
//! any node, on any branch, is a matter of picking up its snapshot. Past a memory budget
//! (see [`History::set_budget`]), the oldest states are dropped.

use crate::document::{Document, Layer, LayerContent, ObjectKind};
use crate::tile::{Pixel, TILE_PIXELS};
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

/// The bytes one tile's pixels take.
const TILE_BYTES: u64 = (TILE_PIXELS * std::mem::size_of::<Pixel>()) as u64;

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

    /// Every state kept, in the order they were made (the root first).
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().filter_map(|(i, n)| n.as_ref().map(|n| (NodeId(i), n)))
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
        self.trim();
        id
    }

    fn moved_to(&mut self, id: NodeId) {
        self.current = id;
        let doc = &self.nodes[id.0].as_ref().expect("a state kept").doc;
        self.usage.visit(doc);
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
        let n = self.node(self.current);
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
    /// dropped when it is not the current one and either has no children, or is the root
    /// with one child (which becomes the root), so the history stays one tree.
    fn trim(&mut self) {
        while self.usage.extra() > self.budget {
            let droppable = |(id, n): &(NodeId, &Node)| {
                *id != self.current && (n.children.is_empty() || (*id == self.root && n.children.len() == 1))
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

/// The pixels the states hold, counted once per distinct tile. Layers and tiles are known
/// by address: what a state holds never changes, and an address counted here belongs to
/// something a kept state holds, so it cannot be reused while it is counted.
#[derive(Clone, Debug, Default)]
struct Usage {
    /// Each distinct layer the states hold: in how many states, and its tiles.
    layers: HashMap<usize, (usize, Vec<usize>)>,
    /// Each distinct tile: in how many of those layers.
    tiles: HashMap<usize, usize>,
    /// The current state's tiles (all of them counted in `tiles`).
    current: HashSet<usize>,
}

impl Usage {
    /// Counts a new state's document (only its layers not counted already are walked).
    fn add(&mut self, doc: &Document) {
        for l in layers_of(doc) {
            let tiles = &mut self.tiles;
            let e = self.layers.entry(Arc::as_ptr(l) as usize).or_insert_with(|| {
                let t = tiles_of(l);
                t.iter().for_each(|&x| *tiles.entry(x).or_default() += 1);
                (0, t)
            });
            e.0 += 1;
        }
    }

    /// Takes away what a dropped state's document alone held.
    fn forget(&mut self, doc: &Document) {
        for l in layers_of(doc) {
            let key = Arc::as_ptr(l) as usize;
            let e = self.layers.get_mut(&key).expect("counted");
            e.0 -= 1;
            if e.0 > 0 {
                continue;
            }
            for t in self.layers.remove(&key).expect("counted").1 {
                let n = self.tiles.get_mut(&t).expect("counted");
                *n -= 1;
                if *n == 0 {
                    self.tiles.remove(&t);
                }
            }
        }
    }

    /// Makes `doc` (a counted state's) the current one.
    fn visit(&mut self, doc: &Document) {
        self.current.clear();
        for l in layers_of(doc) {
            self.current.extend(&self.layers[&(Arc::as_ptr(l) as usize)].1);
        }
    }

    /// The bytes of the tiles the current state does not hold.
    fn extra(&self) -> u64 {
        (self.tiles.len() - self.current.len()) as u64 * TILE_BYTES
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

/// The tiles (by address) layer `l`'s own objects hold, each once.
fn tiles_of(l: &Layer) -> Vec<usize> {
    let LayerContent::Objects(objects) = &l.content else { return Vec::new() };
    let mut out: Vec<usize> = objects
        .iter()
        .flat_map(|o| {
            let image = match &o.kind {
                ObjectKind::Pixels(p) => &p.image,
                ObjectKind::Image(i) => &i.image,
            };
            image.tiles().map(|(_, t)| Arc::as_ptr(t) as usize)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
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
        h.set_budget(0);
        assert_eq!(h.nodes().map(|(id, _)| id).collect::<Vec<_>>(), [other]);
        assert_eq!(h.root(), other);
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
