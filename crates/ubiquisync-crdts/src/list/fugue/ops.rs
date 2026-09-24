use crate::list::fugue::{
    Delete, EffectError, ElementId, Insert, List, Node, NodeBase, NodeIdx, NodeRef, Op,
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
                mut parent_id,
                mut side,
                // TODO FugueMax
                right_origin: _,
                content,
            }) => {
                let len = content.len();
                for c in content {
                    self.insert(id.clone(), parent_id, side, c);
                    parent_id = Some(id.clone());
                    id = ElementId {
                        op_id: id.op_id,
                        index: id.index + 1,
                    };
                    side = Side::Right;
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

    fn insert(
        &mut self,
        id: ElementId<Id>,
        parent_id: Option<ElementId<Id>>,
        side: Side,
        content: T,
    ) {
        let mut node = Node {
            id,
            parent_id: parent_id.clone(),
            parent_index: None,
            next_sibling: None,
            side,
            content,
            effect_deleted: false,
            prepare_state: PrepareState::UnInserted,
            base: NodeBase {
                first_left_child: None,
                first_right_child: None,
                effect_size: 1,
                prepare_size: 0,
            },
        };

        if self.pending_delete.remove(&id) {
            node.effect_deleted = true;
            node.base.effect_size = 0;
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
                    Side::Left => node.base.first_left_child = self.insert_child(node.base.first_left_child, pending_id),
                    Side::Right => node.base.first_right_child= self.insert_child(node.base.first_right_child, pending_id),
                }
                node.base.effect_size += pending_node.base.effect_size;
            }
        }
        let effect_delta = node.base.effect_size;

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
                Side::Left => self.insert_child(&mut parent.left_children, node_ref),
                Side::Right => self.insert_child(&mut parent.right_children, node_ref),
            }
            self.visit_ancestors(parent_id, parent_index, |parent| {
                parent.effect_size += effect_delta;
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
        F: Fn(&mut NodeBase),
    {
        loop {
            let parent = match (parent_id, parent_index) {
                (Some(_), Some(idx)) => &mut self.nodes[idx.0],
                (None, None) => {
                    f(&mut self.root);
                    return;
                }
                (Some(_), None) => {
                    // this is okay, we just have hit a pending parent state which will be addressed when the parent is inserted
                    return;
                }
                (None, Some(_)) => {
                    unreachable!("there should never be an index set for an empty parent (root)")
                }
            };

            f(&mut parent.base);
            parent_id = parent.parent_id.clone();
            parent_index = parent.parent_index;
        }
    }

    fn insert_child(
        &mut self,
        mut first_child: Option<NodeIdx>,
        id: NodeRef<Id>,
    ) -> Option<NodeIdx> {
        todo!()
        first_child
    }
}
