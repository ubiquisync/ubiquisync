use crate::{
    codec::PlaintextOpBatchHasher, crypto::{MmrAccumulator, MmrState}, log_entry::{LogEntry, OpaqueLogEntry}, uuid::Uuid
};

pub trait TrackerStorage {
    fn received_state(&self, peer_id: Uuid, container_id: Uuid) -> MmrAccumulator;
    fn advance_mmr(&self, peer_id: Uuid, container_id: Uuid, state: &MmrState);
    fn receive_entry(&self, peer_id: Uuid, container_id: Uuid, entry_bytes: &[u8]);
}

pub struct Tracker<Op> {
    storage: Box<dyn TrackerStorage>,
}

impl<Op> Tracker<Op> {
    fn receive_entry(&self, peer_id: Uuid, container_id: Uuid, entry: LogEntry<Op>) {
        let mmr = self.storage.received_state(peer_id, container_id);
        let hasher = PlaintextOpBatchHasher::new(todo!(), )
    }

    fn receive_opaque_entry(&self, peer_id: Uuid, container_id: Uuid, entry: OpaqueLogEntry) {}
}
