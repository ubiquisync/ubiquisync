use std::collections::HashMap;

use itertools::Itertools;
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

struct InsertPosition<Id> {
    parent: Option<NodeRef<Id>>,
    side: Side,
    right_origin: Option<NodeRef<Id>>,
}

#[derive(Debug, Clone, Copy)]
enum View {
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
            effect_size: 1,
            prepare_state: PrepareState::Inserted,
            prepare_size: 1,
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
                node.effect_size += pending_node.effect_size;
                node.prepare_size += pending_node.prepare_size;
            }
        }
        let effect_delta = node.effect_size;
        let prepare_delta = node.prepare_size;

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
                node.effect_size = node.effect_size.checked_sub(1).expect("non-zero");
                Ok(true)
            },
            |parent| parent.effect_size = parent.effect_size.checked_sub(1).expect("non-zero"),
            EffectError::NodeNotFound,
        )
    }

    fn prepare_insert(&mut self, id: ElementId<Id>) -> Result<(), PrepareError> {
        self.visit_node_and_ancestors(
            id,
            |node| match node.prepare_state {
                PrepareState::UnInserted => {
                    node.prepare_state = PrepareState::Inserted;
                    node.prepare_size += 1;
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
                    node.prepare_size = node.prepare_size.checked_sub(1).expect("non-zero");
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
                if delete_count == 0 {
                    // only update parents when this is the first tracked delete
                    node.prepare_size = node.prepare_size.checked_sub(1).expect("non-zero");
                    Ok(true)
                } else {
                    Ok(false)
                }
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
                    node.prepare_size += 1;
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

    fn resolve(&self, n: &Option<NodeRef<Id>>) -> &Node<Id, T> {
        match n {
            Some(n) => &self.nodes[n.index.0],
            None => &self.root,
        }
    }

    fn find_insert_position(&self, view: View, offset: usize) -> InsertPosition<Id> {
        let left_origin = self.find_before_offset(view, offset);
        let right_origin = self.successor(view, &left_origin);
        if self.resolve(&left_origin).right_children.is_empty() {
            InsertPosition {
                parent: left_origin,
                side: Side::Right,
                right_origin,
            }
        } else {
            InsertPosition {
                parent: right_origin,
                side: Side::Left,
                right_origin: None,
            }
        }
    }

    /// Finds the node right before the offset position, so for 0, it will return root,
    /// and 1 will return the first element
    fn find_before_offset(&self, view: View, offset: usize) -> Option<NodeRef<Id>> {
        if offset > self.root.size(view) {
            todo!("out of range")
        }

        if offset == 0 {
            // root
            return None;
        }

        let mut cur = None;
        let mut cur_node = &self.root;
        let mut remaining = offset - 1;
        'outer: loop {
            // TODO where to put unreachable's to ensure loop terminates
            for c in cur_node.left_children.iter() {
                let node = &self.nodes[c.index.0];
                let n = node.size(view);
                if remaining < n {
                    cur = Some(c.clone());
                    cur_node = node;
                    continue 'outer;
                }
                remaining -= n;
            }

            if cur_node.content.is_some() && cur_node.inserted(view) {
                if remaining == 0 {
                    return cur;
                }
                remaining -= 1;
            }

            for c in cur_node.right_children.iter() {
                let node = &self.nodes[c.index.0];
                let n = node.size(view);
                if remaining < n {
                    cur = Some(c.clone());
                    cur_node = node;
                    continue 'outer;
                }
                remaining -= n;
            }
            unreachable!("looped without doing anything")
        }
    }

    fn successor(&self, view: View, n: &Option<NodeRef<Id>>) -> Option<NodeRef<Id>> {
        let Some(n) = n else {
            return None;
        };

        let node = &self.nodes[n.index.0];
        if let Some(right) = node.right_children.first() {
            Some(self.leftmost(view, right.clone()))
        } else {
            self.parent_right_sibling(node, view, n)
        }
    }

    fn leftmost(&self, view: View, mut n: NodeRef<Id>) -> NodeRef<Id> {
        loop {
            if let Some(child) = self.nodes[n.index.0]
                .left_children
                .iter()
                .find(|c| self.nodes[c.index.0].not_uninserted(view))
            {
                n = child.clone();
            } else {
                return n.clone();
            }
        }
    }

    fn parent_right_sibling(
        &self,
        node: &Node<Id, T>,
        view: View,
        id: &NodeRef<Id>,
    ) -> Option<NodeRef<Id>> {
        if node.content.is_none() {
            // already at root
            return None;
        }
        let parent = match (&node.parent_id, node.parent_index) {
            (Some(_), Some(idx)) => &self.nodes[idx.0],
            (None, None) => &self.root,
            (None, Some(_)) => unreachable!(),
            (Some(_), None) => unreachable!(),
        };
        self.right_sibling(&parent.left_children, view, id)
            .or(self.right_sibling(&parent.right_children, view, id))
    }

    fn right_sibling(
        &self,
        children: &[NodeRef<Id>],
        view: View,
        id: &NodeRef<Id>,
    ) -> Option<NodeRef<Id>> {
        children
            .iter()
            .filter(|c| self.nodes[c.index.0].not_uninserted(view))
            .tuple_windows()
            .filter_map(|(l, sib)| {
                if l.id == id.id {
                    Some(sib.clone())
                } else {
                    None
                }
            })
            .next()
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
                (None, None) => {
                    saw_root = true;
                    &mut self.root
                }
                (Some(_), None) => {
                    // this is okay, we just have hit a pending parent state which will be addressed when the parent is inserted
                    return;
                }
                (None, Some(_)) => {
                    unreachable!("there should never be an index set for an empty parent (root)")
                }
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

fn insert_child<Id: PartialEq + PartialOrd + Eq + Ord>(
    children: &mut Vec<NodeRef<Id>>,
    id: NodeRef<Id>,
) {
    children.push(id);
    children.sort_by(|a, b| a.id.cmp(&b.id));
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
