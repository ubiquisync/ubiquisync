mod iter;
mod locate;
mod ops;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<Id, T> {
    Insert {
        id: ElementId<Id>,
        insert: Insert<Id, T>,
    },
    Delete {
        id: ElementId<Id>,
        count: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insert<Id, T> {
    pub parent_id: Option<ElementId<Id>>,
    pub side: Side,
    pub right_origin: Option<ElementId<Id>>,
    pub content: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementId<Id> {
    pub op_id: Id,
    pub index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

pub struct List<Id, T> {
    root: Node<Id, T>,
    nodes: Vec<Node<Id, T>>,
    nodes_by_id: HashMap<ElementId<Id>, NodeIdx>,
    // nodes who are missing their parent, keyed by the parent ID
    // and point to the list of unparented nodes
    pending_parent: HashMap<ElementId<Id>, Vec<NodeRef<Id>>>,
}

#[derive(Debug, Error)]
pub enum EffectError {
    #[error("node not found")]
    NodeNotFound,
}

#[derive(Debug, Error)]
pub enum PrepareError {
    #[error("node not found")]
    NodeNotFound,
    #[error("invalid state")]
    InvalidState,
}

#[derive(Debug, Clone, Copy)]
struct NodeIdx(usize);

#[derive(Debug, Clone)]
struct NodeRef<Id> {
    // the actual consensus element ID
    id: ElementId<Id>,
    // the local index in the nodes Vec
    index: NodeIdx,
}

struct Node<Id, T> {
    parent_id: Option<ElementId<Id>>,
    parent_index: Option<NodeIdx>,
    side: Side,
    left_children: Vec<NodeRef<Id>>,
    right_children: Vec<NodeRef<Id>>,
    // content is None only if this is the root pseudo-node
    content: Option<T>,

    effect_deleted: bool,
    effect_size: usize,

    prepare_state: PrepareState,
    prepare_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrepareState {
    UnInserted,
    Inserted,
    Deleted(u32),
}

struct InsertPosition<Id> {
    parent: Option<NodeRef<Id>>,
    side: Side,
    right_origin: Option<NodeRef<Id>>,
}

#[derive(Debug, Clone, Copy)]
pub enum View {
    Effect,
    Prepare,
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Default for List<Id, T> {
    fn default() -> Self {
        Self {
            root: Node {
                parent_id: None,
                parent_index: None,
                left_children: vec![],
                right_children: vec![],
                content: None,
                side: Side::Right,
                effect_deleted: false,
                effect_size: 0,
                prepare_state: PrepareState::Inserted,
                prepare_size: 0,
            },
            nodes: vec![],
            nodes_by_id: HashMap::new(),
            pending_parent: HashMap::new(),
        }
    }
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
    pub fn size(&self, view: View) -> usize {
        match view {
            View::Effect => self.root.effect_size,
            View::Prepare => self.root.prepare_size,
        }
    }
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Node<Id, T> {
    fn size(&self, view: View) -> usize {
        match view {
            View::Effect => self.effect_size,
            View::Prepare => self.prepare_size,
        }
    }

    fn inserted(&self, view: View) -> bool {
        match view {
            View::Effect => !self.effect_deleted,
            View::Prepare => self.prepare_state == PrepareState::Inserted,
        }
    }

    fn not_uninserted(&self, view: View) -> bool {
        match view {
            View::Effect => true,
            View::Prepare => self.prepare_state != PrepareState::UnInserted,
        }
    }
}
