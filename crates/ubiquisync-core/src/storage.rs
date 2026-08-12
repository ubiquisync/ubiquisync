use crate::{
    crypto::mmr::MmrState,
    log_entry::{OpaqueLogEntry, PlaintextLogEntry},
    uuid::Uuid,
};

pub trait Batch {
    fn add_hlc_update(&mut self, raw: u64);
    fn add_log_entries(&mut self, log_entries: LogEntries<'_>);
}

pub struct LogId {
    pub container_id: Uuid,
    pub peer_id: Uuid,
}

pub struct LogEntries<'a> {
    pub log_id: LogId,
    // size must match last index + 1 in received_entries
    pub received_mmr_state: MmrState,
    pub received_entries: Vec<OpaqueLogEntry<'a>>,
    // start index for processed_entries must be last index in received entries + 1
    pub processed_entries: Vec<PlaintextLogEntry<'a>>,
}
