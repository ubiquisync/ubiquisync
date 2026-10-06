use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use ubiquisync_core::{
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::RootKey256Fingerprint,
    ids::PeerId,
    pack::{PackFileId, PackRef},
};

use crate::{codeable_col_repr, def_table, def_table_with_auto_id};

def_table_with_auto_id!(remotes as __replica_remotes (id) => {});
def_table_with_auto_id!(topics as __pack_topics (id) => {
    topic: String,
});

def_table!(topic_state as __pack_topic_state (remote_id: i64, topic_id: i64) => {
    dirty: bool,
});

def_table!(published as __pack_published (
    remote_id: i64, // TODO ref remotes
    stream_id: i64, // TODO ref streams
) => {
    published_size: u64 // TODO default 0
});

def_table!(pack_read_state as __pack_read_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
    peer_id: i64, // TODO ref peers
) => {
    state: super::RemoteTopicReadState,
});

codeable_col_repr!(RemoteTopicReadState);

#[derive(Debug, Clone, Default)]
pub struct RemoteTopicReadState {
    pub consumed: HashSet<PackFileId>,
    pub blocked: HashMap<PackRef, BlockedPackInfo>,
}

#[derive(Debug, Clone)]
pub struct BlockedPackInfo {
    pub file: PackFileId,
    pub read_timestamps: Range<u64>,
    pub attempts: u64,

    // Set to the list of missing parents when any segment does not have an anchor (meaning we have a missing dependency).
    pub pending_parents: HashSet<PackRef>,
    /// Set when we are missing keys for decoding some segment with outer encryption.
    pub need_keys: HashSet<RootKey256Fingerprint>,
    /// Any case where we hit some decode error which suggests MAYBE a software upgrade is needed (could also mean corrupt data).
    pub needs_upgrade: bool,
    /// The metadata for this segment did not match the actual segment body.
    pub bad_metadata: bool,
    /// Some decode error which most likely means that retrying will continue to fail.
    /// If this error is the only error blocked condition after retrying, it means
    /// we should probably permanently mark this pack as failed.
    pub decode_error: bool,
    /// The body hash did not match the expect hash. This could mean the file
    /// wasn't fully transferred yet, so we should retry. If there are repeated
    /// errors we can mark this pack as failed, but for now we'll retry later.
    pub bad_body_hash: bool,
    /// The header signature failed verification. Possibly a partially synced file,
    /// so we retry with backoff.
    pub bad_signature: bool,
    /// The header could not be decoded. Possibly a partially synced file,
    /// so we retry with backoff.
    pub corrupt_header: bool,
    // List of peers referenced in peer data for whom we cannot find an init entry.
    pub missing_peers: Vec<PeerId>,
}

impl RemoteTopicReadState {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}
