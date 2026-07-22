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
    SetDeviceName(String),
    Join {
        workspace_id: Uuid,
        user_id: Uuid,
    },
    AdmitDevice {
        device_id: Uuid,
        user_id: Uuid,
    },
    AdmitServer {
        server_id: Uuid,
    },
    Grant {
        user_id: Uuid,
        capability: u64,
        value: u64,
    },
    Revoke {
        user_id: Uuid,
        capability: u64,
    },
    RemoveDevice {
        device_id: Uuid,
        ctl_cut: u64,
        hlc_cut: Timestamp,
    }, // users always remove their own devices, doesn't apply to servers
    RemoveUser {
        user_id: Uuid,
    },
    RemoveServer {
        server_id: Uuid,
        ctl_cut: u64,
        hlc_cut: Timestamp,
    },
    SetPolicy {
        policy: Policy,
        cel: String,
    },
    ShareKey {
        device_id: Uuid,
        fingerprint: [u8; 16],
        cipher: Vec<u8>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    AdmitUser, // applies when admitting a user with a different user id than the admitter (different user id)
    AdmitDevice, // applies when admitting a device to a user's own account - could be used for MFA (same user id)
    Grant,
    Revoke,
    RemoveUser,
    RemoveDevice, // applies when removing a user's own device - could be used for MFA
    AdmitServer,
    RemoveServer,
    SetPolicy,
}
