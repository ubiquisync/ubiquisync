use crate::{
    crypto::{Hash, VerifyingKey, Signature, mmr::MmrState},
    ids::{ContainerId, PeerId},
    log_entry::{CipherInfo, OpaqueLogEntry, PlaintextLogEntry},
    uuid::Uuid,
};

pub trait Storage {
    type Batch: Batch;
    type Error;

    fn new_batch(&self) -> Self::Batch;
    fn commit_batch(&self, batch: Self::Batch) -> Result<(), Self::Error>;

    /// Load the last persisted clock state, or `None` for a fresh store.
    fn load_hlc(&self) -> Result<Option<u64>, Self::Error>;

    fn get_peer_info(&self, peer_id: &PeerId) -> Result<PeerInfo, Self::Error>;
    fn get_receive_state(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
    ) -> Result<ReceiveState, Self::Error>;
    // TODO methods to get unprocessed and uncommitted entries in order to retry later
}

pub trait Batch {
    fn add_hlc_update(&mut self, raw: u64);
    fn add_log_entries(&mut self, log_entries: LogEntries<'_>);
}

pub struct LogEntries<'a> {
    pub container_id: ContainerId,
    pub peer_id: PeerId,
    /// Updates the processed index when we have both decrypted entries and processed their HLC
    /// this must be less than or equal to the last entry in decoded_entries if it is set at all.
    pub processed_idx: Option<u64>,
    pub decoded_entries: Vec<(PlaintextLogEntry<'a>, Option<Hash>)>, // TODO we want to preserve the entries but still index header data (hlc & server user id)
    /// Start index for received entries must be last index in received entries + 1, or empty.
    pub opaque_entries: Vec<(OpaqueLogEntry<'a>, Option<Hash>)>,
    /// This will update both the received index and peaks
    /// Size must match the last log index we have seen in plaintext_entries or decode_entries + 1
    pub received_mmr_state: MmrState,
}

pub struct ReceiveState {
    pub mmr_state: MmrState,
    pub active_cipher: Option<CipherInfo>,
}

pub struct PeerInfo {
    pub peer_id: PeerId,
    pub genesis_bytes: Vec<u8>,
    pub genesis_signature: Signature,
}

impl PeerInfo {
    pub fn genesis_hash(&self) -> Hash {
        blake3::derive_key(DOMAIN_PEER_HASH, &self.genesis_bytes)
    }

    pub fn signing_pub_key(&self) -> VerifyingKey {
        todo!()
    }
}

const DOMAIN_PEER_HASH: &str = "ubiquisync/v1/peer-hash";
