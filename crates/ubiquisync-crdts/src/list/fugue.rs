mod iter;
mod locate;
mod ops;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    id::ElementId,
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
    parent_id: Option<ElementId<Id>>,
    side: Side,
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

pub struct List<Id, T> {
    root: Node<Id, T>,
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
    Deleted(usize),
}

struct InsertPosition<Id> {
    parent: Option<NodeRef<Id>>,
    side: Side,
    right_origin: Option<NodeRef<Id>>,
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

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Walkable<Id>
    for List<Id, T>
{
    type LogicalOp = LogicalOp<T>;

    type PhysicalOp = Op<Id, T>;

    type PrepareOp = PrepareOp<Id>;

    type LogicalOpErr = ();

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
                Op::Insert(self.create_insert(view, offset as usize, content))
            }
            LogicalOp::Delete { offset, count } => {
                // TODO safely convert size, could come from wire
                Op::Delete(self.create_delete(view, offset as usize, count as usize))
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
