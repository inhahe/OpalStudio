//! The undo history tree.
//!
//! Every node holds a complete snapshot of the document. Snapshots share everything
//! unchanged through `Arc`, so a node costs only what its action changed, and moving to
//! any node, on any branch, is a matter of picking up its snapshot.

use crate::document::Document;
use std::time::SystemTime;

/// Identifies a node of one [`History`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub usize);

#[derive(Clone, Debug)]
pub struct Node {
    /// `None` only for the root, the state the document was opened or created in.
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
    nodes: Vec<Node>,
    current: NodeId,
}

impl History {
    /// A history whose only state is `doc`, labeled `label` ("Open", "New image").
    pub fn new(doc: Document, label: impl Into<String>) -> History {
        let root = Node {
            parent: None,
            children: Vec::new(),
            last_child: None,
            label: label.into(),
            created: SystemTime::now(),
            doc,
        };
        History { nodes: vec![root], current: NodeId(0) }
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn current(&self) -> NodeId {
        self.current
    }

    /// The document in the current state.
    pub fn doc(&self) -> &Document {
        &self.nodes[self.current.0].doc
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    /// Every node, in the order they were made (the root first).
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().map(|(i, n)| (NodeId(i), n))
    }

    /// Records `doc` as the result of an action labeled `label`, as a new child of the
    /// current state, and makes it current. Earlier redo states stay reachable as a branch.
    pub fn commit(&mut self, doc: Document, label: impl Into<String>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: Some(self.current),
            children: Vec::new(),
            last_child: None,
            label: label.into(),
            created: SystemTime::now(),
            doc,
        });
        let parent = &mut self.nodes[self.current.0];
        parent.children.push(id);
        parent.last_child = Some(id);
        self.current = id;
        id
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
                self.nodes[p.0].last_child = Some(self.current);
                self.current = p;
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
                self.current = c;
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

    /// Moves to any state. Every state on the way from the root to it records the branch
    /// taken, so undo and redo afterwards follow this branch.
    pub fn goto(&mut self, target: NodeId) {
        assert!(target.0 < self.nodes.len(), "no history node {target:?}");
        let mut child = target;
        while let Some(parent) = self.nodes[child.0].parent {
            self.nodes[parent.0].last_child = Some(child);
            child = parent;
        }
        self.current = target;
    }

    /// The states from the root to `id`, root first.
    pub fn path_from_root(&self, id: NodeId) -> Vec<NodeId> {
        let mut path = vec![id];
        let mut n = id;
        while let Some(p) = self.nodes[n.0].parent {
            path.push(p);
            n = p;
        }
        path.reverse();
        path
    }
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
}
