use crate::{codec::op::IndexableOp, log_entry::OpBatch, uuid::Uuid};

pub trait Reducer {
    type Op: IndexableOp;

    fn deliver_ops(&self, container_id: &Uuid, peer_id: &Uuid, batches: &[OpBatch<Self::Op>]);
}
