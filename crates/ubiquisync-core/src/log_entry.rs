//! Wire-level log entry: a single op with timestamp and optional
//! server-attested user attribution. This is the unit of encoding/decoding in a
//! segment file.

use crate::hlc::Timestamp;
use crate::uuid::Uuid;

/// A single log entry: one operation plus metadata. This is the unit
/// written to and read from segment files — each entry has its own
/// blake3 hash and can be independently expunged.
///
/// The entry is generic over its op vocabulary `E`: each data domain defines
/// its own op type and carries it in this envelope — for example the table
/// op vocabulary in `ubiquisync-tables`. Any domains in use share one HLC
/// clock domain, so timestamps are causally comparable across them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry<E> {
    /// The **server-attested** user id for this entry. Every entry originates
    /// from *some* user, but this field specifically carries the identity a
    /// server vouched for — it is populated only in server-mode segments, where
    /// the server asserts attribution. `None` in device mode, where attribution
    /// is implicit from the peer directory and no server assertion exists.
    ///
    /// Do not read this as "the author"; read it as "who the server said this
    /// was." It is distinct from a stream's `peer_id` (which stream the entry
    /// came from).
    pub server_user_id: Option<Uuid>,
    /// HLC timestamp — monotonically non-decreasing within a peer's stream.
    /// Entries written in one atomic transaction share a tick, so they are
    /// treated as one logical write by LWW comparisons.
    pub timestamp: Timestamp,
    /// The state mutation.
    pub op: OpBody<E>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpBody<E> {
    App(E),
    Sys(SysOp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysOp {
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
    SetCapability {
        user_id: Uuid,
        capability: u64,
        value: Option<u64>,
    }, // None revokes
    RemoveDevice {
        device_id: Uuid,
        cut_index: u64,
    }, // users always remove their own devices, doesn't apply to servers
    RemoveUser {
        user_id: Uuid,
    },
    RemoveServer {
        server_id: Uuid,
        cut_index: u64,
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

pub enum Policy {
    AdmitUser,
    SetCapability,
    RemoveUser,
    ManageServers,
    SetPolicy,
}
