use crate::{hlc::Timestamp, uuid::Uuid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtlOp {
    CommitContainers {
        container_heads: Vec<CommitInfo>,
        merkle_root: Option<MerkleRoot>,
    },
    ObservePeers {
        peer_heads: Vec<CommitInfo>,
    },
    SetDeviceName(String), // TODO change to device meta (could include some other details)
    Join {
        workspace_id: Uuid,
        user_id: Uuid,
    },
    RemoveSelf,
    ShareKey {
        fingerprint: Uuid,
        containers: Vec<Uuid>, // empty vec specifies that this is a default workspace key
        wraps: Vec<KeyWrap>,
    },
    AuthOp {
        opaque_bytes: Vec<u8>,
    },
    SetAuthPolicy {
        cel: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyWrap {
    pub device_id: Uuid,
    pub cipher: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: Uuid,
    pub height: u64,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleRoot {
    pub version: u8,
    pub root: Vec<u8>,
}
