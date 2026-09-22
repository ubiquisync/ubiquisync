use std::collections::HashMap;

use smallvec::{SmallVec, smallvec};

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
    // TODO we can optimize later and store parent Option<NodeIdx>
    side: Side,
    left_children: Vec<NodeRef<Id>>,
    right_children: Vec<NodeRef<Id>>,
    content: Option<T>,

    effect_deleted: bool,
    effect_size: usize,

    prepare_state: PrepareState,
    prepare_size: usize,
}

enum PrepareState {
    NotInserted,
    Inserted,
    Deleted(u32),
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> Default for List<Id, T> {
    fn default() -> Self {
        Self {
            root: Node {
                parent_id: None,
                left_children: vec![],
                right_children: vec![],
                content: None,
                side: Side::Right,
                effect_deleted: false,
                effect_size: 0,
                prepare_state: PrepareState::NotInserted,
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
            side,
            left_children: vec![],
            right_children: vec![],
            content: Some(content),
            effect_deleted: false,
            effect_size: 0,
            prepare_state: PrepareState::NotInserted,
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
                match pending_node.side {
                    Side::Left => insert_child(&mut node.left_children, pending_id),
                    Side::Right => insert_child(&mut node.right_children, pending_id),
                }
                if !pending_node.effect_deleted {
                    node.effect_size += pending_node.effect_size + 1;
                }
            }
        }

        // add node now that we're done mutating it
        // we can't do it with push_mut earlier, because we need a mutable ref self.node below for parent
        self.nodes.push(node);

        let parent = match parent_id {
            Some(parent_id) => {
                // check if we can resolve this entry's parent
                if let Some(parent_index) = self.nodes_by_id.get(&parent_id) {
                    &mut self.nodes[parent_index.0]
                } else {
                    // add to pending id list
                    self.pending_parent
                        .entry(parent_id)
                        .or_default()
                        .push(node_ref);
                    return;
                }
            }
            None => &mut self.root,
        };
        match side {
            Side::Left => insert_child(&mut parent.left_children, node_ref),
            Side::Right => insert_child(&mut parent.right_children, node_ref),
        }
        parent.effect_size += 1;

        self.nodes_by_id.insert(id, index);
    }

    fn delete(&mut self, id: ElementId<Id>) {
        if let Some(index) = self.nodes_by_id.get(&id) {
            let parent_id = {
                let node = &mut self.nodes[index.0];
                node.effect_deleted = true;
                node.parent_id.clone()
            };

            let parent = if let Some(parent_id) = parent_id {
                if let Some(parent_index) = self.nodes_by_id.get(&parent_id) {
                    &mut self.nodes[parent_index.0]
                } else {
                    return;
                }
            } else {
                &mut self.root
            };

            parent.effect_size.checked_sub(1).expect("non-zero size");
        }
    }

    pub fn iter<F>(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| !n.effect_deleted)
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
