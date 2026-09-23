use itertools::Itertools;

use crate::list::fugue::{Insert, InsertPosition, List, Node, NodeRef, Op, Side, View};

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
    pub fn create_insert(&self, view: View, offset: usize, content: Vec<T>) -> Insert<Id, T> {
        let pos = self.find_insert_position(view, offset);
        Insert {
            parent_id: pos.parent.map(|r| r.id),
            side: pos.side,
            right_origin: pos.right_origin.map(|r| r.id),
            content,
        }
    }

    pub fn create_deletes(&self, view: View, offset: usize, count: usize) -> Vec<Op<Id, T>> {
        let mut deletes = vec![];
        for i in 0..count {
            if let Some(node_ref) = self.find_before_offset(view, offset + i + 1) {
                deletes.push(Op::Delete {
                    id: node_ref.id.clone(),
                    count: 1,
                });
            } else {
                // TODO error
            }
        }
        deletes
    }

    fn find_insert_position(&self, view: View, offset: usize) -> InsertPosition<Id> {
        let left_origin = self.find_before_offset(view, offset);
        let right_origin = self.successor(view, &left_origin);
        if self
            .not_uninserted_children(view, &self.resolve(&left_origin).right_children)
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
        }
    }

    fn resolve(&self, n: &Option<NodeRef<Id>>) -> &Node<Id, T> {
        match n {
            Some(n) => &self.nodes[n.index.0],
            None => &self.root,
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
            // TODO: question: if we are inserting into the start of a non-empty document, is root still left origin?
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

    /// Finds the first node inserted or deleted node to the right of n.
    /// Basically we want to traverse logically to the right for the left-most node
    /// that is to the right of this node (if any)
    fn successor(&self, view: View, n: &Option<NodeRef<Id>>) -> Option<NodeRef<Id>> {
        let node = &self.resolve(n);
        if let Some(right) = node
            .right_children
            .iter()
            .find(|c| self.nodes[c.index.0].not_uninserted(view))
        {
            Some(self.leftmost(view, right.clone()))
        } else {
            if let Some(n) = n {
                // try ascending to the parent and then this node's next sibling's left most
                self.parent_next_sibling(node, view, n)
            } else {
                // we are at the root and the root is empty
                None
            }
        }
    }

    fn parent_next_sibling(
        &self,
        node: &Node<Id, T>,
        view: View,
        id: &NodeRef<Id>,
    ) -> Option<NodeRef<Id>> {
        // check if alrady at root, if we are return None because we can't ascend any further
        node.content.as_ref()?;

        // find this node's parent and that parents ref
        let (parent, parent_ref) = match (&node.parent_id, node.parent_index) {
            (Some(id), Some(index)) => (
                &self.nodes[index.0],
                Some(NodeRef {
                    id: id.clone(),
                    index,
                }),
            ),
            (None, None) => (&self.root, None),
            _ => unreachable!(),
        };

        match node.side {
            // if this node is on the left side of the parent,
            // find it's next sibling and then return that next sibling's leftmost node
            // or return the parent itself
            Side::Left => {
                if let Some(sib) = self.next_sibling(&parent.left_children, view, id) {
                    Some(self.leftmost(view, sib))
                } else {
                    parent_ref
                }
            }
            // if this node is on the right side of the parent
            // find it's next sibling and then return that next sibling's leftmost node
            // or return the parent's next sibling (there is nothing to the right under this parent
            // so we assume if there is a node to the right it is somwhere in the traversal where
            // the parent is on the left side of some other node)
            Side::Right => {
                if let Some(sib) = self.next_sibling(&parent.right_children, view, id) {
                    Some(self.leftmost(view, sib))
                } else {
                    if let Some(ref parent_ref) = parent_ref {
                        self.parent_next_sibling(parent, view, parent_ref)
                    } else {
                        // we are already at the root so we can't go any higher
                        None
                    }
                }
            }
        }
    }

    /// Finds the left most inserted or deleted child of n or n
    fn leftmost(&self, view: View, mut n: NodeRef<Id>) -> NodeRef<Id> {
        loop {
            if let Some(child) = self
                .not_uninserted_children(view, &self.nodes[n.index.0].left_children)
                .next()
            {
                n = child.clone();
            } else {
                return n.clone();
            }
        }
    }

    fn not_uninserted_children<'a>(
        &self,
        view: View,
        children: &'a [NodeRef<Id>],
    ) -> impl Iterator<Item = &'a NodeRef<Id>> {
        children
            .iter()
            .filter(move |c| self.nodes[c.index.0].not_uninserted(view))
    }

    fn next_sibling(
        &self,
        children: &[NodeRef<Id>],
        view: View,
        id: &NodeRef<Id>,
    ) -> Option<NodeRef<Id>> {
        self.not_uninserted_children(view, children)
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
}
