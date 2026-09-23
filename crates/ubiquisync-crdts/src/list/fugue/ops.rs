use std::ops::AddAssign;

use crate::list::fugue::{
    EffectError, ElementId, Insert, List, Node, NodeIdx, NodeRef, Op, PrepareError, PrepareState,
    Side,
};

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
    pub fn apply_op(&mut self, op: Op<Id, T>) -> Result<(), EffectError> {
        match op {
            Op::Insert {
                mut id,
                insert:
                    Insert {
                        mut parent_id,
                        mut side,
                        // TODO FugueMax
                        right_origin: _,
                        content,
                    },
            } => {
                for c in content {
                    self.insert(id.clone(), parent_id, side, c);
                    parent_id = Some(id.clone());
                    id = ElementId {
                        op_id: id.op_id,
                        index: id.index + 1,
                    };
                    side = Side::Right;
                }
            }
            Op::Delete { mut id, count } => {
                for _ in 0..count {
                    self.delete(id.clone())?;
                    id.index += 1;
                }
            }
        }
        Ok(())
    }

    pub fn prepare_advance(&mut self, op: Op<Id, T>) -> Result<(), PrepareError> {
        match op {
            Op::Insert {
                mut id,
                insert: Insert { content, .. },
            } => {
                for _ in content {
                    self.prepare_insert(id.clone())?;
                    id.index += 1;
                }
            }
            Op::Delete { mut id, count } => {
                for _ in 0..count {
                    self.prepare_delete(id.clone())?;
                    id.index += 1;
                }
            }
        }
        Ok(())
    }

    pub fn prepare_retract(&mut self, op: Op<Id, T>) -> Result<(), PrepareError> {
        match op {
            Op::Insert {
                mut id,
                insert: Insert { content, .. },
                ..
            } => {
                for _ in content {
                    self.prepare_uninsert(id.clone())?;
                    id.index += 1;
                }
            }
            Op::Delete { mut id, count } => {
                for _ in 0..count {
                    self.prepare_undelete(id.clone())?;
                    id.index += 1;
                }
            }
        }
        Ok(())
    }

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

        if let Some(n) = self.pending_delete.remove(&id) {
            node.effect_deleted = true;
            node.effect_size = 0;
            node.prepare_state = PrepareState::Deleted(n);
            node.prepare_size = 0;
        }

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
        match self.visit_node_and_ancestors(
            id.clone(),
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
            || Err(EffectError::NodeNotFound),
        ) {
            Err(EffectError::NodeNotFound) => {
                self.pending_delete.entry(id).or_default().add_assign(1);
                Ok(())
            }
            res => res,
        }
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
            || Err(PrepareError::NodeNotFound),
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
            || Err(PrepareError::NodeNotFound),
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
            || Err(PrepareError::NodeNotFound),
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
            || Err(PrepareError::NodeNotFound),
        )
    }

    fn visit_node_and_ancestors<F, G, H, Err>(
        &mut self,
        id: ElementId<Id>,
        visit_node: F,
        visit_ancestor: G,
        not_found: H,
    ) -> Result<(), Err>
    where
        F: Fn(&mut Node<Id, T>) -> Result<bool, Err>,
        G: Fn(&mut Node<Id, T>),
        H: Fn() -> Result<(), Err>,
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
            not_found()
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
}

fn insert_child<Id: PartialEq + PartialOrd + Eq + Ord>(
    children: &mut Vec<NodeRef<Id>>,
    id: NodeRef<Id>,
) {
    children.push(id);
    children.sort_by(|a, b| a.id.cmp(&b.id));
}
