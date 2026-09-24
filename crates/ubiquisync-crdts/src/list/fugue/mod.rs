mod iter;
mod locate;
mod ops;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    id::ElementId,
    list::fugue::locate::LogicalOpError,
    walker::{CountableOp, View, Walkable},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalOp<T> {
    Insert { offset: u64, content: Vec<T> },
    Delete { offset: u64, count: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<Id, T> {
    Insert(Insert<Id, T>),
    Delete(Vec<Delete<Id>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insert<Id, T> {
    parent: Parent<Id>,
    right_origin: Option<ElementId<Id>>,
    content: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delete<Id> {
    id: ElementId<Id>,
    count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareOp<Id> {
    Insert { id: ElementId<Id>, count: u64 },
    Delete(Vec<Delete<Id>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parent<Id> {
    Root,
    Node { id: ElementId<Id>, side: Side },
}

pub struct List<Id, T> {
    root: NodeBase,
    nodes: Vec<Node<Id, T>>,
    nodes_by_id: HashMap<ElementId<Id>, NodeIdx>,
    // nodes who are missing their parent, keyed by the parent ID
    // and point to the list of unparented nodes
    pending_parent: HashMap<ElementId<Id>, Vec<NodeRef<Id>>>,
    // nodes that have been deleted before we've even seen them
    pending_delete: HashSet<ElementId<Id>>,
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
pub(crate) struct NodeIdx(usize);

#[derive(Debug, Clone)]
struct NodeRef<Id> {
    // the actual consensus element ID
    id: ElementId<Id>,
    // the local index in the nodes Vec
    index: NodeIdx,
}

pub(crate) struct Node<Id, T> {
    node_ref: NodeRef<Id>,
    parent: Parent<Id>,
    parent_index: Option<NodeIdx>,
    next_sibling: Option<NodeIdx>,
    content: T,
    base: NodeBase,
    effect_deleted: bool,
    prepare_state: PrepareState,
    first_left_child: Option<NodeIdx>,
}

struct NodeBase {
    first_right_child: Option<NodeIdx>,

    effect_size: usize,
    prepare_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrepareState {
    UnInserted,
    Inserted,
    Deleted(usize),
}

struct InsertPosition {
    parent: Option<NodeIdx>,
    side: Side,
    right_origin: Option<NodeIdx>,
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Default for List<Id, T> {
    fn default() -> Self {
        Self {
            root: NodeBase {
                first_right_child: None,
                effect_size: 0,
                prepare_size: 0,
            },
            nodes: vec![],
            nodes_by_id: HashMap::new(),
            pending_parent: HashMap::new(),
            pending_delete: HashSet::new(),
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

impl NodeBase {
    pub(crate) fn size(&self, view: View) -> usize {
        match view {
            View::Effect => self.effect_size,
            View::Prepare => self.prepare_size,
        }
    }
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Node<Id, T> {
    pub(crate) fn size(&self, view: View) -> usize {
        self.base.size(view)
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

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Walkable<Id>
    for List<Id, T>
{
    type LogicalOp = LogicalOp<T>;

    type PhysicalOp = Op<Id, T>;

    type PrepareOp = PrepareOp<Id>;

    type LogicalOpErr = LogicalOpError;

    type EffectErr = EffectError;

    type PrepareErr = PrepareError;

    fn logical_to_physical(
        &self,
        view: View,
        op: Self::LogicalOp,
    ) -> Result<Self::PhysicalOp, Self::LogicalOpErr> {
        Ok(match op {
            LogicalOp::Insert { offset, content } => {
                // TODO safely convert size, could come from wire
                Op::Insert(self.create_insert(view, offset as usize, content)?)
            }
            LogicalOp::Delete { offset, count } => {
                // TODO safely convert size, could come from wire
                Op::Delete(self.create_delete(view, offset as usize, count as usize)?)
            }
        })
    }

    fn apply(
        &mut self,
        id: ElementId<Id>,
        op: Self::PhysicalOp,
    ) -> Result<Self::PrepareOp, Self::EffectErr> {
        self.apply_op(id, op)
    }

    fn prepare_advance(&mut self, op: &Self::PrepareOp) -> Result<(), Self::PrepareErr> {
        self.prepare_advance(op)
    }

    fn prepare_retract(&mut self, op: &Self::PrepareOp) -> Result<(), Self::PrepareErr> {
        self.prepare_retract(op)
    }
}

impl<T> CountableOp for LogicalOp<T> {
    fn num_elements(&self) -> u64 {
        match self {
            LogicalOp::Insert { content, .. } => content.len() as u64,
            LogicalOp::Delete { .. } => 0,
        }
    }
}

impl<Id, T> CountableOp for Op<Id, T> {
    fn num_elements(&self) -> u64 {
        match self {
            Op::Insert(Insert { content, .. }) => content.len() as u64,
            Op::Delete(_) => 0,
        }
    }
}
