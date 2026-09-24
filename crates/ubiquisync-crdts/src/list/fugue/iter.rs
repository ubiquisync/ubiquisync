use smallvec::{SmallVec, smallvec};

use crate::list::fugue::{List, Node, NodeRef, PrepareState};

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
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
        let iter = Iter {
            list: self,
            stack: smallvec![IterStep {
                node: &self.root,
                sibling_idx: None
            }],
        };
        iter.filter(move |n| is_inserted(*n))
            .filter_map(|n| n.content.as_ref())
    }
}

struct Iter<'a, Id, T> {
    list: &'a List<Id, T>,
    stack: SmallVec<[IterStep<'a, Id, T>; 16]>,
}

struct IterStep<'a, Id, T> {
    node: &'a Node<Id, T>,
    // none only for root
    sibling_idx: Option<usize>,
}

impl<'a, Id, T> Iterator for Iter<'a, Id, T> {
    type Item = &'a Node<Id, T>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(cur) = self.stack.pop() {
                if let Some(parent) = self.stack.last().map(|n| n.node) {
                    if let Some(sibling_idx) = cur.sibling_idx {
                        let mut push_next_sibling = |children: &[NodeRef<Id>]| {
                            let next_sibling_idx = sibling_idx + 1;
                            if next_sibling_idx < children.len() {
                                let next_sibling =
                                    &self.list.nodes[children[next_sibling_idx].index.0];
                                self.stack.push(IterStep {
                                    node: next_sibling,
                                    sibling_idx: Some(next_sibling_idx),
                                });
                            }
                        };
                        match cur.node.side {
                            super::Side::Left => push_next_sibling(&parent.left_children),
                            super::Side::Right => push_next_sibling(&parent.right_children),
                        }
                    } else {
                        unreachable!("parented node should have sibling index")
                    }
                }

                if let Some(right_id) = cur.node.right_children.first() {
                    let mut cur = &self.list.nodes[right_id.index.0];
                    loop {
                        self.stack.push(IterStep {
                            node: cur,
                            sibling_idx: Some(0),
                        });
                        if let Some(left_id) = cur.left_children.first() {
                            cur = &self.list.nodes[left_id.index.0]
                        } else {
                            break;
                        }
                    }
                }
                return Some(cur.node);
            } else {
                return None;
            }
        }
    }
}
