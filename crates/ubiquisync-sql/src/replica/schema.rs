use num_enum::{IntoPrimitive, TryFromPrimitive};
use ubiquisync_core::{
    codec::{ReadError, Reader, Writer},
    crypto::{CipherInfo, RootKey256Fingerprint, Signature},
    ids::{ContainerId, PeerId},
    log::ChainHash,
};

use crate::{
    codeable_col_repr, db::ColRepr, def_table, def_table_with_auto_id, enum_col_repr,
    try_from_into_col_repr,
};

def_table_with_auto_id!(peers (id) => {
    peer_id: [u8; 32], // TODO UNIQUE
    commitment_bytes: Vec<u8>,
    signature: super::Signature
});

def_table_with_auto_id!(streams (id) => {
   peer_id: i64, // TODO ref peers
   container_id: [u8;16],
   head_size: u64, // TODO default 0
   head_hash: [u8; 32], // TODO could be non-null and default to seed
   head_cipher: Option<super::CipherInfo>,
   head_status: Option<super::HeadStatus>,
   commit_size: u64, // TODO default 0
   commit_cipher: Option<super::CipherInfo>,
   commit_status: super::CommitStatus, // TODO default 0
   commit_status_data: Option<super::CommitStatusData>,
   parent_id: Option<i64>, // TODO ref streams
   fork_idx: Option<u64>,
   fork_hash: Option<[u8;32]>,
   // TODO CHECK(commit_size <= head_size)
});

#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy)]
#[repr(i64)]
pub enum CommitStatus {
    Ok = 0,
    NeedKey = 1,
    HLCForwardSkew = 2,
    NeedPeerCommit = 3,
    IncompatibleSoftware = 4,
    CantDecodeOp = 5,
    Frozen = 6,
}

enum_col_repr!(CommitStatus);
try_from_into_col_repr!([u8; 32], Vec<u8>);
codeable_col_repr!(CipherInfo);
codeable_col_repr!(CommitStatusData);
codeable_col_repr!(HeadStatus);
codeable_col_repr!(Signature);

// TODO: CREATE UNIQUE INDEX streams_root ON streams(peer_id, container_id) WHERE parent_id IS NULL;
// TODO: we might also want a unique on (parent_id, fork_idx, fork_hash) to avoid races

def_table!(segments (stream_id: i64, end_size: u64) => { // TODO ref streams
    start_idx: u64,
    body: Vec<u8>,
    // WITH ROWID!
});

pub enum HeadStatus {
    Ok,
}

pub enum CommitStatusData {
    Other,
    NeedKey(RootKey256Fingerprint),
    HLCForwardSkew,
    NeedPeerCommit {
        peer_id: PeerId,
        // None if same container
        container_id: Option<ContainerId>,
        head: ChainHash,
    },
    IncompatibleSoftware(SoftwareIncompatibilityInfo),
}

pub enum SoftwareIncompatibilityInfo {
    UnknownEntryType(u8),
    UnknownOpType(u8),
    UnknownCipherSuite(u8),
}

impl HeadStatus {
    pub fn encode(&self, writer: &mut Writer) {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}

impl CommitStatusData {
    pub fn encode(&self, writer: &mut Writer) {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}

impl SoftwareIncompatibilityInfo {
    pub fn encode(&self, writer: &mut Writer) {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}
