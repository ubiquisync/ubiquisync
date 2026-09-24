use crate::list::fugue::{
    Delete, EffectError, ElementId, Insert, List, Node, NodeBase, NodeIdx, NodeRef, Op, Parent,
    PrepareError, PrepareOp, PrepareState, Side,
};

impl<Id: Clone + std::hash::Hash + PartialEq + PartialOrd + Eq + Ord, T> List<Id, T> {
    pub fn apply_op(
        &mut self,
        mut id: ElementId<Id>,
        op: Op<Id, T>,
    ) -> Result<PrepareOp<Id>, EffectError> {
        match op {
            Op::Insert(Insert {
                mut parent,
                // TODO FugueMax
                right_origin: _,
                content,
            }) => {
                let len = content.len();
                for c in content {
                    self.insert(id.clone(), parent, c);
                    let next_id = ElementId {
                        op_id: id.op_id.clone(),
                        index: id.index + 1,
                    };
                    parent = Parent::Node {
                        id,
                        side: Side::Right,
                    };
                    id = next_id;
                }
                Ok(PrepareOp::Insert {
                    id,
                    count: len as u64,
                })
            }
            Op::Delete(deletes) => {
                for Delete { id, count } in deletes.iter() {
                    let mut id = id.clone();
                    for _ in 0..*count {
                        self.delete(id.clone())?;
                        id.index += 1;
                    }
                }
                Ok(PrepareOp::Delete(deletes))
            }
        }
    }

    pub fn prepare_advance(&mut self, op: &PrepareOp<Id>) -> Result<(), PrepareError> {
        match op {
            PrepareOp::Insert { id, count } => {
                let mut id = id.clone();
                for _ in 0..*count {
                    self.prepare_insert(id.clone())?;
                    id.index += 1;
                }
            }
            PrepareOp::Delete(deletes) => {
                for Delete { id, count } in deletes {
                    let mut id = id.clone();
                    for _ in 0..*count {
                        self.prepare_delete(id.clone())?;
                        id.index += 1;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn prepare_retract(&mut self, op: &PrepareOp<Id>) -> Result<(), PrepareError> {
        match op {
            PrepareOp::Insert { id, count } => {
                let mut id = id.clone();
                for _ in 0..*count {
                    self.prepare_uninsert(id.clone())?;
                    id.index += 1;
                }
            }
            PrepareOp::Delete(deletes) => {
                for Delete { id, count } in deletes {
                    let mut id = id.clone();
                    for _ in 0..*count {
                        self.prepare_undelete(id.clone())?;
                        id.index += 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn insert(&mut self, id: ElementId<Id>, parent: Parent<Id>, content: T) {
        let index = NodeIdx(self.nodes.len());
        let node_ref = NodeRef {
            id: id.clone(),
            index,
        };

        let mut node = Node {
            node_ref: node_ref.clone(),
            parent: parent.clone(),
            parent_index: None,
            next_sibling: None,
            content,
            effect_deleted: false,
            prepare_state: PrepareState::UnInserted,
            first_left_child: None,
            base: NodeBase {
                first_right_child: None,
                effect_size: 1,
                prepare_size: 0,
            },
        };

        if self.pending_delete.remove(&id) {
            node.effect_deleted = true;
            node.base.effect_size = 0;
        }

        // first see if any pending entries were waiting for this one
        if let Some(pending) = self.pending_parent.remove(&id) {
            for pending_id in pending {
                let pidx = pending_id.index;
                self.node_mut(pidx).parent_index = Some(index);
                let pending_node_parent = &self.node(pidx).parent;
                match pending_node_parent {
                    Parent::Root => unreachable!(),
                    Parent::Node { side, .. } => match side {
                        Side::Left => {
                            node.first_left_child =
                                self.insert_child(node.first_left_child, pending_id)
                        }
                        Side::Right => {
                            node.base.first_right_child =
                                self.insert_child(node.base.first_right_child, pending_id)
                        }
                    },
                }
                node.base.effect_size += self.node(pidx).base.effect_size;
            }
        }
        let effect_delta = node.base.effect_size;

        // add node now that we're mostly done mutating it
        // we can't do it with push_mut earlier, because we need a mutable ref self.node below for parent
        self.nodes.push(node);
        self.nodes_by_id.insert(id, index);

        let parent_index = match &parent {
            Parent::Root => {
                self.root.first_right_child =
                    self.insert_child(self.root.first_right_child, node_ref);
                None
            }
            Parent::Node { id, side } => {
                if let Some(parent_index) = self.nodes_by_id.get(&id).cloned() {
                    self.nodes[index.0].parent_index = Some(parent_index);
                    let parent_node = &self.nodes[parent_index.0];
                    match side {
                        Side::Left => {
                            self.node_mut(parent_index).first_left_child =
                                self.insert_child(parent_node.first_left_child, node_ref)
                        }
                        Side::Right => {
                            self.node_mut(parent_index).base.first_right_child =
                                self.insert_child(parent_node.base.first_right_child, node_ref)
                        }
                    }
                    Some(parent_index)
                } else {
                    // add to pending id list
                    self.pending_parent
                        .entry(id.clone())
                        .or_default()
                        .push(node_ref);
                    return;
                }
            }
        };
        self.visit_ancestors(parent.clone(), parent_index, |parent| {
            parent.effect_size += effect_delta;
        });
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
                node.base.effect_size = node.base.effect_size.checked_sub(1).expect("non-zero");
                Ok(true)
            },
            |parent| parent.effect_size = parent.effect_size.checked_sub(1).expect("non-zero"),
            || Err(EffectError::NodeNotFound),
        ) {
            Err(EffectError::NodeNotFound) => {
                self.pending_delete.insert(id);
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
                    node.base.prepare_size += 1;
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
                    node.base.prepare_size =
                        node.base.prepare_size.checked_sub(1).expect("non-zero");
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
                    node.base.prepare_size =
                        node.base.prepare_size.checked_sub(1).expect("non-zero");
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
                    node.base.prepare_size += 1;
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
        G: Fn(&mut NodeBase),
        H: Fn() -> Result<(), Err>,
    {
        if let Some(index) = self.nodes_by_id.get(&id) {
            let node = &mut self.nodes[index.0];
            if !visit_node(node)? {
                return Ok(());
            }
            // need to stop borrowing mutably
            let node = &self.nodes[index.0];

            self.visit_ancestors(node.parent.clone(), node.parent_index, visit_ancestor);
            Ok(())
        } else {
            not_found()
        }
    }

    fn visit_ancestors<F>(
        &mut self,
        mut parent: Parent<Id>,
        mut parent_index: Option<NodeIdx>,
        f: F,
    ) where
        F: Fn(&mut NodeBase),
    {
        loop {
            match parent {
                Parent::Root => {
                    f(&mut self.root);
                    return;
                }
                Parent::Node { .. } => {
                    if let Some(idx) = parent_index {
                        let parent_node = &mut self.nodes[idx.0];
                        f(&mut parent_node.base);
                        parent = parent_node.parent.clone();
                        parent_index = parent_node.parent_index;
                    } else {
                        // parent is pending
                        return;
                    }
                }
            }
        }
    }

    fn insert_child(&mut self, first_child: Option<NodeIdx>, id: NodeRef<Id>) -> Option<NodeIdx> {
        let new_id = id.id;
        let new_idx = id.index;
        if let Some(first) = first_child {
            let mut prev = None;
            let mut next = first;
            loop {
                if new_id < self.node(next).node_ref.id {
                    self.node_mut(new_idx).next_sibling = Some(next);
                    if let Some(prev) = prev {
                        self.node_mut(prev).next_sibling = Some(new_idx);
                        return Some(first);
                    } else {
                        // new first child
                        return Some(new_idx);
                    }
                }
                prev = Some(next);
                if let Some(next_next) = self.node(next).next_sibling {
                    next = next_next;
                } else {
                    // insert at end
                    self.node_mut(next).next_sibling = Some(new_idx);
                    return Some(first);
                }
            }
        } else {
            // first child
            Some(new_idx)
        }
    }
}

impl<Id, T> List<Id, T> {
    pub(crate) fn node(&self, idx: NodeIdx) -> &Node<Id, T> {
        &self.nodes[idx.0]
    }

    pub(crate) fn node_mut(&mut self, idx: NodeIdx) -> &mut Node<Id, T> {
        &mut self.nodes[idx.0]
    }
}
