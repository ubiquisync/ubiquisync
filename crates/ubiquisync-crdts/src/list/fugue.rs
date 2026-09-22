use std::collections::HashMap;

pub struct ElementId<Id> {
    pub op_id: Id,
    pub index: u64,
}

pub struct List<Id, T> {
    root: Node<Id, T>,
    nodes_by_id: HashMap<Id, Node<Id, T>>,
    // nodes who are missing their parent, keyed by the parent ID
    // and point to the list of unparented nodes
    pending_parent: HashMap<Id, Vec<Id>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<Id, T> {
    Insert {
        parent_id: Option<Id>,
        side: Side,
        right_origin: Option<Id>,
        content: Vec<T>,
    },
    Delete {
        id: Id,
        count: u32,
    },
}

struct Node<Id, T> {
    parent_id: Option<Id>,
    side: Side,
    left_children: Vec<Id>,
    right_children: Vec<Id>,
    content: Vec<T>,
    size: usize,
    is_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl<Id: Clone + std::cmp::Eq + std::hash::Hash, T> List<Id, T> {
    pub fn new() -> Self {
        Self {
            root: Node {
                parent_id: None,
                left_children: vec![],
                right_children: vec![],
                content: vec![],
                size: 0,
                side: Side::Right,
                is_deleted: false,
            },
            nodes_by_id: HashMap::new(),
            pending_parent: HashMap::new(),
        }
    }

    pub fn apply(&mut self, id: Id, op: Op<Id, T>) {
        match op {
            Op::Insert {
                parent_id,
                side,
                right_origin,
                content,
            } => {
                // TODO
                let mut node = Node {
                    parent_id: parent_id.clone(),
                    side,
                    left_children: vec![],
                    right_children: vec![],
                    content,
                    size: 0,
                    is_deleted: false,
                };
                // first see if any pending entries were waiting for this one
                if let Some(pending) = self.pending_parent.remove(&id) {
                    for n in pending {
                        if let Some(pn) = self.nodes_by_id.get(&n) {
                            match pn.side {
                                Side::Left => node.left_children.push(n),
                                Side::Right => node.right_children.push(n),
                            }
                            if !pn.is_deleted {
                                node.size += pn.size + pn.content.len();
                            }
                        }
                    }
                }

                let parent = match parent_id {
                    Some(parent_id) => {
                        // check if we can resolve this entry's parent
                        if let Some(parent) = self.nodes_by_id.get_mut(&parent_id) {
                            parent
                        } else {
                            // add to pending id list
                            self.pending_parent.entry(parent_id).or_default().push(id);
                            return;
                        }
                    }
                    None => &mut self.root,
                };
                match node.side {
                    Side::Left => parent.left_children.push(id),
                    Side::Right => parent.right_children.push(id),
                }
                parent.size += node.content.len();
            }
            Op::Delete { id, count } => todo!(),
        }
    }
}
