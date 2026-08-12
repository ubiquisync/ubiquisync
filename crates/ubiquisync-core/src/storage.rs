use crate::{
    codec::op::OpIndexEntry,
    crypto::mmr::MmrState,
    log_entry::{GenericLogEntry, OpHeader, OpaqueLogEntry},
    uuid::Uuid,
};

pub trait Storage {
    type Batch: Batch;

    fn new_batch(&self) -> Self::Batch;
    fn commit_batch(&self, batch: Self::Batch) -> Result<(), StorageError>;
    // TODO methods to get unprocessed and uncommitted entries in order to retry later
}

// TODO define generic storage error
pub struct StorageError;

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
    /// Updates the processed index when we have both decrypted entries and processed their HLC
    /// this must be less than or equal to the last entry in decoded_entries if it is set at all.
    pub processed_idx: Option<u64>,
    pub decoded_entries: Vec<GenericLogEntry<OpHeader, Vec<OpIndexEntry>>>,
    /// Start index for received entries must be last index in received entries + 1, or empty.
    pub opaque_entries: Vec<OpaqueLogEntry<'a>>,
    /// This will update both the received index and peaks
    /// Size must match the last log index we have seen in plaintext_entries or decode_entries + 1
    pub received_mmr_state: MmrState,
}
