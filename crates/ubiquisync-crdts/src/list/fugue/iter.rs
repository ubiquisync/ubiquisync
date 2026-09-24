use crate::list::fugue::{List, Node, NodeIdx, PrepareState};

impl<Id: Clone, T> List<Id, T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| !n.effect_deleted)
    }

    pub fn iter_prepare(&self) -> impl Iterator<Item = &T> {
        self.iter_impl(|n| n.prepare_state == PrepareState::Inserted)
    }

    fn iter_impl<'a, F>(&'a self, is_inserted: F) -> impl Iterator<Item = &'a T> + 'a
    where
        F: Fn(&Node<Id, T>) -> bool + 'a,
    {
        self.node_iter(None)
            .filter(move |n| is_inserted(*n))
            .map(|n| &n.content)
    }

    pub(crate) fn node_iter<'a>(&'a self, start: Option<NodeIdx>) -> Iter<'a, Id, T> {
        Iter {
            list: self,
            next: start
                .map(|idx| &self.nodes[idx.0])
                .or(self.root.first_right_child.map(|i| self.leftmost(i))),
        }
    }

    fn leftmost(&self, idx: NodeIdx) -> &Node<Id, T> {
        let mut node = &self.nodes[idx.0];
        loop {
            if let Some(left_id) = node.first_left_child {
                node = &self.nodes[left_id.0];
            } else {
                return node;
            }
        }
    }
}

pub(crate) struct Iter<'a, Id, T> {
    list: &'a List<Id, T>,
    next: Option<&'a Node<Id, T>>,
}

impl<'a, Id: Clone, T> Iterator for Iter<'a, Id, T> {
    type Item = &'a Node<Id, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let leftmost = |next_id: NodeIdx| Some(self.list.leftmost(next_id));

        let cur = self.next?;
        if let Some(right_idx) = cur.base.first_right_child {
            // first we traverse to our left-most right child, if there is one
            self.next = leftmost(right_idx);
        } else if let Some(sib_idx) = cur.next_sibling {
            // otherwise we traverse our next sibling, if there is one
            self.next = leftmost(sib_idx);
        } else {
            // otherwise, we go up to our parent
            match cur.parent {
                super::Parent::Root => {
                    self.next = None;
                }
                super::Parent::Node { side, .. } => {
                    if let Some(parent_idx) = cur.parent_index {
                        let mut parent = self.list.node(parent_idx);
                        match side {
                            // if we were on the left side of the parent then the parent is next
                            super::Side::Left => self.next = Some(parent),
                            // if we were on the right side of the parent, then we go its siblings or parents
                            super::Side::Right => {
                                // here we need to loop until we find something
                                loop {
                                    if let Some(sib_idx) = parent.next_sibling {
                                        // parent had a sibling, so we take its left-most child
                                        self.next = leftmost(sib_idx);
                                        break;
                                    } else {
                                        match parent.parent {
                                            // parent doesn't have a sibling, and we were the last right child,
                                            // so we need to go to its parent
                                            super::Parent::Root => {
                                                self.next = None;
                                                break;
                                            }
                                            super::Parent::Node { side, .. } => {
                                                if let Some(parent_parent_idx) = parent.parent_index
                                                {
                                                    let parent_parent =
                                                        self.list.node(parent_parent_idx);
                                                    match side {
                                                        super::Side::Left => {
                                                            // if we were on the left of parent's parent, then it's next
                                                            self.next = Some(parent_parent);
                                                            break;
                                                        }
                                                        super::Side::Right => {
                                                            // we were the last right child of this parent, so we need
                                                            // to continue traversing up the tree
                                                            parent = parent_parent;
                                                            continue;
                                                        }
                                                    }
                                                } else {
                                                    self.next = None;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        self.next = None;
                    }
                }
            }
        }
        Some(cur)
    }
}
