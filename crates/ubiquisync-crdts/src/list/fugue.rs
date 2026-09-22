use std::collections::HashMap;

use smallvec::{SmallVec, smallvec};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<Id, T> {
    Insert {
        parent_id: Option<ElementId<Id>>,
        side: Side,
        right_origin: Option<ElementId<Id>>,
        content: Vec<T>,
    },
    Delete {
        id: ElementId<Id>,
        count: u32,
    },
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
    fn insert(
        &mut self,
        id: ElementId<Id>,
        parent_id: Option<ElementId<Id>>,
        side: Side,
        content: T,
    ) {
        let mut node = Node {
            parent_id: parent_id.clone(),
            parent_index: None,
            side,
            left_children: vec![],
            right_children: vec![],
            content: Some(content),
            effect_deleted: false,
            effect_size: 0,
            prepare_state: PrepareState::Inserted,
            prepare_size: 0,
        };

        let index = NodeIdx(self.nodes.len());
        let node_ref = NodeRef {
            id: id.clone(),
            index,
        };

        // first see if any pending entries were waiting for this one
        if let Some(pending) = self.pending_parent.remove(&id) {
            for pending_id in pending {
                let pending_node = &mut self.nodes[pending_id.index.0];
                pending_node.parent_index = Some(index);
                match pending_node.side {
                    Side::Left => insert_child(&mut node.left_children, pending_id),
                    Side::Right => insert_child(&mut node.right_children, pending_id),
                }
                node.effect_size +=
                    pending_node.effect_size + usize::from(!pending_node.effect_deleted);
                node.prepare_size += pending_node.prepare_size
                    + usize::from(pending_node.prepare_state == PrepareState::Inserted);
            }
        }
        let effect_delta = node.effect_size + 1;
        let prepare_delta = node.prepare_size + 1;

        // add node now that we're done mutating it
        // we can't do it with push_mut earlier, because we need a mutable ref self.node below for parent
        self.nodes.push(node);
        self.nodes_by_id.insert(id, index);

        let parent_index = {
            let (parent, parent_index) = match parent_id {
                Some(ref parent_id) => {
                    // check if we can resolve this entry's parent
                    if let Some(parent_index) = self.nodes_by_id.get(parent_id) {
                        (&mut self.nodes[parent_index.0], Some(*parent_index))
                    } else {
                        // add to pending id list
                        self.pending_parent
                            .entry(parent_id.clone())
                            .or_default()
                            .push(node_ref);
                        return;
                    }
                }
                None => (&mut self.root, None),
            };
            match side {
                Side::Left => insert_child(&mut parent.left_children, node_ref),
                Side::Right => insert_child(&mut parent.right_children, node_ref),
            }
            self.visit_ancestors(parent_id, parent_index, |parent| {
                parent.effect_size += effect_delta;
                parent.prepare_size += prepare_delta;
            });

            parent_index
        };
        self.nodes[index.0].parent_index = parent_index;
    }

    fn delete(&mut self, id: ElementId<Id>) -> Result<(), EffectError> {
        self.visit_node_and_ancestors(
            id,
            |node| {
                if node.effect_deleted {
                    // already deleted!
                    return Ok(false);
                }
                node.effect_deleted = true;
                Ok(true)
            },
            |parent| parent.effect_size = parent.effect_size.checked_sub(1).expect("non-zero size"),
            EffectError::NodeNotFound,
        )
    }

    fn prepare_insert(&mut self, id: ElementId<Id>) -> Result<(), PrepareError> {
        self.visit_node_and_ancestors(
            id,
            |node| match node.prepare_state {
                PrepareState::UnInserted => {
                    node.prepare_state = PrepareState::Inserted;
                    Ok(true)
                }
                _ => Err(PrepareError::InvalidState),
            },
            |parent| parent.prepare_size += 1,
            PrepareError::NodeNotFound,
        )
    }

    fn prepare_uninsert(&mut self, id: ElementId<Id>) -> Result<(), PrepareError> {
        self.visit_node_and_ancestors(
            id,
            |node| match node.prepare_state {
                PrepareState::Inserted => {
                    node.prepare_state = PrepareState::UnInserted;
                    Ok(true)
                }
                _ => Err(PrepareError::InvalidState),
            },
            |parent| parent.prepare_size -= 1,
            PrepareError::NodeNotFound,
        )
    }

    fn prepare_delete(&mut self, id: ElementId<Id>) -> Result<(), PrepareError> {
        self.visit_node_and_ancestors(
            id,
            |node| {
                let delete_count = match node.prepare_state {
                    PrepareState::Deleted(n) => n,
                    PrepareState::Inserted => 0,
                    PrepareState::UnInserted => return Err(PrepareError::InvalidState),
                };
                node.prepare_state = PrepareState::Deleted(delete_count + 1);
                Ok(delete_count == 0) // only update parents when this is the first tracked delete
            },
            |parent| {
                parent.prepare_size = parent.prepare_size.checked_sub(1).expect("non-zero size")
            },
            PrepareError::NodeNotFound,
        )
    }

    fn prepare_undelete(&mut self, id: ElementId<Id>) -> Result<(), PrepareError> {
        self.visit_node_and_ancestors(
            id,
            |node| {
                let delete_count = match node.prepare_state {
                    PrepareState::Deleted(n) => n,
                    _ => return Err(PrepareError::InvalidState),
                };
                if delete_count == 1 {
                    node.prepare_state = PrepareState::Inserted;
                    Ok(true)
                } else {
                    node.prepare_state = PrepareState::Deleted(delete_count - 1);
                    Ok(false)
                }
            },
            |parent| parent.prepare_size += 1,
            PrepareError::NodeNotFound,
        )
    }

    fn visit_node_and_ancestors<F, G, Err>(
        &mut self,
        id: ElementId<Id>,
        visit_node: F,
        visit_ancestor: G,
        not_found: Err,
    ) -> Result<(), Err>
    where
        F: Fn(&mut Node<Id, T>) -> Result<bool, Err>,
        G: Fn(&mut Node<Id, T>),
    {
        if let Some(index) = self.nodes_by_id.get(&id) {
            let (parent_id, parent_index) = {
                let node = &mut self.nodes[index.0];
                if !visit_node(node)? {
                    return Ok(());
                }
                (node.parent_id.clone(), node.parent_index)
            };

            self.visit_ancestors(parent_id, parent_index, visit_ancestor);
            Ok(())
        } else {
            Err(not_found)
        }
    }

    fn visit_ancestors<F>(
        &mut self,
        mut parent_id: Option<ElementId<Id>>,
        mut parent_index: Option<NodeIdx>,
        f: F,
    ) where
        F: Fn(&mut Node<Id, T>),
    {
        let mut saw_root = false;
        loop {
            let parent = match (parent_id, parent_index) {
                (Some(_), Some(idx)) => &mut self.nodes[idx.0],
                (None, _) => {
                    saw_root = true;
                    &mut self.root
                }
                // TODO should we have errors in this case, it shouldn't really happen i think
                _ => unreachable!(),
            };

            f(parent);
            if saw_root {
                return;
            }

            parent_id = parent.parent_id.clone();
            parent_index = parent.parent_index;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| !n.effect_deleted)
    }

    pub fn iter_prepare(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| n.prepare_state == PrepareState::Inserted)
    }

    fn iter_impl<F>(&self, is_inserted: F) -> impl Iterator<Item = &T>
    where
        F: Fn(&Node<Id, T>) -> bool,
    {
        // we make a few optimizations here to limit memory use:
        // - push the next tree walk operation onto the stack rather than iterating through everything at once
        // - use a smallvec to avoid heap allocation for shallow trees
        enum Todo<'b, Id, T> {
            Traverse(&'b Node<Id, T>),
            TraverseSiblings(&'b [NodeRef<Id>], usize),
            Yield(&'b Node<Id, T>),
        }
        let mut stack: SmallVec<[Todo<'_, Id, T>; 16]> = smallvec![Todo::Traverse(&self.root)];
        std::iter::from_fn(move || {
            loop {
                let top = stack.pop()?;
                match top {
                    Todo::Traverse(node) => {
                        stack.push(Todo::Yield(node));
                        if !node.left_children.is_empty() {
                            stack.push(Todo::TraverseSiblings(&node.left_children, 0));
                        }
                    }
                    Todo::TraverseSiblings(element_ids, idx) => {
                        let n = element_ids.len();
                        if idx < n {
                            if idx + 1 < n {
                                stack.push(Todo::TraverseSiblings(element_ids, idx + 1));
                            }

                            stack.push(Todo::Traverse(&self.nodes[element_ids[idx].index.0]));
                        }
                    }
                    Todo::Yield(node) => {
                        if !node.right_children.is_empty() {
                            stack.push(Todo::TraverseSiblings(&node.right_children, 0));
                        }
                        if let Some(ref content) = node.content
                            && is_inserted(node)
                        {
                            return Some(content);
                        }
                    }
                }
            }
        })
    }
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Node<Id, T> {}

fn insert_child<Id: PartialEq + PartialOrd + Eq + Ord>(
    children: &mut Vec<NodeRef<Id>>,
    id: NodeRef<Id>,
) {
    children.push(id);
    children.sort_by(|a, b| a.id.cmp(&b.id));
}
