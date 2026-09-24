use thiserror::Error;

use crate::{
    list::fugue::{Delete, Insert, InsertPosition, List, Node, NodeBase, NodeIdx, Parent, Side},
    walker::View,
};

#[derive(Error, Debug)]
pub enum LogicalOpError {
    #[error("out of range")]
    OutOfRange,
}

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
    pub fn create_insert(
        &self,
        view: View,
        offset: usize,
        content: Vec<T>,
    ) -> Result<Insert<Id, T>, LogicalOpError> {
        let pos = self.find_insert_position(view, offset)?;
        Ok(Insert {
            parent: pos
                .parent
                .map(|p| Parent::Node {
                    id: self.node(p).node_ref.id.clone(),
                    side: pos.side,
                })
                .unwrap_or(Parent::Root),
            right_origin: pos.right_origin.map(|r| self.node(r).node_ref.id.clone()),
            content,
        })
    }

    pub fn create_delete(
        &self,
        view: View,
        offset: usize,
        count: usize,
    ) -> Result<Vec<Delete<Id>>, LogicalOpError> {
        let mut deletes = vec![];
        for i in 0..count {
            if let Some(idx) = self.find_before_offset(view, offset + i + 1)? {
                deletes.push(Delete {
                    id: self.node(idx).node_ref.id.clone(),
                    count: 1,
                });
            } else {
                return Err(LogicalOpError::OutOfRange);
            }
        }
        Ok(deletes)
    }

    fn find_insert_position(
        &self,
        view: View,
        offset: usize,
    ) -> Result<InsertPosition, LogicalOpError> {
        let left_origin = self.find_before_offset(view, offset)?;
        let right_origin = self.node_iter(left_origin).nth(1).map(|n| n.node_ref.index);

        let left_base = if let Some(left_origin) = left_origin {
            &self.node(left_origin).base
        } else {
            &self.root
        };

        Ok(
            if left_base
                .right_children(self)
                .filter(|n| n.not_uninserted(view))
                .next()
                .is_none()
            {
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
            },
        )
    }

    // fn resolve(&self, n: &Option<NodeRef<Id>>) -> &Node<Id, T> {
    //     match n {
    //         Some(n) => &self.nodes[n.index.0],
    //         None => &self.root,
    //     }
    // }

    /// Finds the node right before the offset position, so for 0, it will return root,
    /// and 1 will return the first element
    fn find_before_offset(
        &self,
        view: View,
        offset: usize,
    ) -> Result<Option<NodeIdx>, LogicalOpError> {
        if offset > self.root.size(view) {
            return Err(LogicalOpError::OutOfRange);
        }

        if offset == 0 {
            // root
            return Ok(None);
        }

        let mut target = offset - 1;

        let Some(mut idx) =
            self.search_in_children(view, self.root.right_children(self), &mut target)
        else {
            unreachable!("offset should have been out of range")
        };

        loop {
            match self.search_in_node(view, idx, &mut target) {
                FindNodeResult::Found(found) => return Ok(Some(found)),
                FindNodeResult::SubTree(subtree) => idx = subtree,
                FindNodeResult::NotFound => unreachable!("loop not making progress"),
            }
        }
    }

    // /// Finds the first node inserted or deleted node to the right of n.
    // /// Basically we want to traverse logically to the right for the left-most node
    // /// that is to the right of this node (if any)
    // fn successor(&self, view: View, n: &Option<NodeRef<Id>>) -> Option<NodeRef<Id>> {
    //     let node = &self.resolve(n);
    //     if let Some(right) = node
    //         .right_children
    //         .iter()
    //         .find(|c| self.nodes[c.index.0].not_uninserted(view))
    //     {
    //         Some(self.leftmost(view, right.clone()))
    //     } else {
    //         if let Some(n) = n {
    //             // try ascending to the parent and then this node's next sibling's left most
    //             self.parent_next_sibling(node, view, n)
    //         } else {
    //             // we are at the root and the root is empty
    //             None
    //         }
    //     }
    // }

    // fn parent_next_sibling(
    //     &self,
    //     node: &Node<Id, T>,
    //     view: View,
    //     id: &NodeRef<Id>,
    // ) -> Option<NodeRef<Id>> {
    //     // check if alrady at root, if we are return None because we can't ascend any further
    //     node.content.as_ref()?;

    //     // find this node's parent and that parents ref
    //     let (parent, parent_ref) = match (&node.parent_id, node.parent_index) {
    //         (Some(id), Some(index)) => (
    //             &self.nodes[index.0],
    //             Some(NodeRef {
    //                 id: id.clone(),
    //                 index,
    //             }),
    //         ),
    //         (None, None) => (&self.root, None),
    //         _ => unreachable!(),
    //     };

    //     match node.side {
    //         // if this node is on the left side of the parent,
    //         // find it's next sibling and then return that next sibling's leftmost node
    //         // or return the parent itself
    //         Side::Left => {
    //             if let Some(sib) = self.next_sibling(&parent.left_children, view, id) {
    //                 Some(self.leftmost(view, sib))
    //             } else {
    //                 parent_ref
    //             }
    //         }
    //         // if this node is on the right side of the parent
    //         // find it's next sibling and then return that next sibling's leftmost node
    //         // or return the parent's next sibling (there is nothing to the right under this parent
    //         // so we assume if there is a node to the right it is somwhere in the traversal where
    //         // the parent is on the left side of some other node)
    //         Side::Right => {
    //             if let Some(sib) = self.next_sibling(&parent.right_children, view, id) {
    //                 Some(self.leftmost(view, sib))
    //             } else {
    //                 if let Some(ref parent_ref) = parent_ref {
    //                     self.parent_next_sibling(parent, view, parent_ref)
    //                 } else {
    //                     // we are already at the root so we can't go any higher
    //                     None
    //                 }
    //             }
    //         }
    //     }
    // }

    // /// Finds the left most inserted or deleted child of n or n
    // fn leftmost(&self, view: View, mut n: NodeRef<Id>) -> NodeRef<Id> {
    //     loop {
    //         if let Some(child) = self
    //             .not_uninserted_children(view, &self.nodes[n.index.0].left_children)
    //             .next()
    //         {
    //             n = child.clone();
    //         } else {
    //             return n.clone();
    //         }
    //     }
    // }

    // fn not_uninserted_children<'a>(
    //     &self,
    //     view: View,
    //     children: &'a [NodeRef<Id>],
    // ) -> impl Iterator<Item = &'a NodeRef<Id>> {
    //     children
    //         .iter()
    //         .filter(move |c| self.nodes[c.index.0].not_uninserted(view))
    // }

    // fn next_sibling(
    //     &self,
    //     children: &[NodeRef<Id>],
    //     view: View,
    //     id: &NodeRef<Id>,
    // ) -> Option<NodeRef<Id>> {
    //     self.not_uninserted_children(view, children)
    //         .tuple_windows()
    //         .filter_map(|(l, sib)| {
    //             if l.id == id.id {
    //                 Some(sib.clone())
    //             } else {
    //                 None
    //             }
    //         })
    //         .next()
    // }

    fn search_in_node<'a>(
        &'a self,
        view: View,
        idx: NodeIdx,
        target: &mut usize,
    ) -> FindNodeResult {
        let node = self.node(idx);
        if let Some(cidx) = self.search_in_children(view, node.left_children(self), target) {
            return FindNodeResult::SubTree(cidx);
        }
        if node.inserted(view) {
            if *target == 0 {
                return FindNodeResult::Found(idx);
            }
            *target -= 1;
        }
        if let Some(cidx) = self.search_in_children(view, node.base.right_children(self), target) {
            return FindNodeResult::SubTree(cidx);
        }
        FindNodeResult::NotFound
    }

    fn search_in_children<'a>(
        &'a self,
        view: View,
        children: impl Iterator<Item = &'a Node<Id, T>>,
        target: &mut usize,
    ) -> Option<NodeIdx> {
        for node in children {
            let n = node.size(view);
            if *target < n {
                return Some(node.node_ref.index);
            } else {
                *target -= n;
            }
        }
        None
    }
}

enum FindNodeResult {
    Found(NodeIdx),
    SubTree(NodeIdx),
    NotFound,
}

impl NodeBase {
    pub(crate) fn right_children<'a, Id, T>(
        &'a self,
        list: &'a List<Id, T>,
    ) -> impl Iterator<Item = &'a Node<Id, T>> {
        let mut right = self.first_right_child;
        std::iter::from_fn(move || {
            if let Some(idx) = right {
                let node = list.node(idx);
                right = node.next_sibling;
                Some(node)
            } else {
                None
            }
        })
    }
}

impl<Id, T> Node<Id, T> {
    pub(crate) fn left_children<'a>(
        &'a self,
        list: &'a List<Id, T>,
    ) -> impl Iterator<Item = &'a Node<Id, T>> + 'a {
        let mut left = self.first_left_child;
        std::iter::from_fn(move || {
            if let Some(idx) = left {
                let node = list.node(idx);
                left = node.next_sibling;
                Some(node)
            } else {
                None
            }
        })
    }
}
