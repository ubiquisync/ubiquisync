use crate::{crypto::PubKey, uuid::Uuid};
use core::ops::Range;

pub enum Effect {
    // 64 byte limit on keys and values
    // internal kv storage for capabilities, claims, pending ops, etc.
    SetKV {
        key: Vec<u8>,
        value: Vec<u8>,
        // relative to the op's hlc
        ttl: Option<u64>,
    },
    DeleteKV(Vec<u8>),
    AdmitDevice {
        device_id: Uuid,
        user_id: Uuid,
    },
    RemoveUser(Uuid),
    RemoveDevice(Uuid),
    // union join permission, not deny
    // first set is a shrinking of permission - containers default to workspace read/write
    SetContainerPermission {
        container_id: Uuid,
        principal: Principal,
        permission: Permission,
    },
    AddEdge {
        parent: Parent,
        child: Child,
    },
    RemoveEdge {
        parent: Parent,
        child: Child,
    },
    SetServerScope {
        server_id: Uuid,
        scope: ServerScope,
    },
    AddRecoveryKey {
        principal: Principal,
        pub_key: PubKey,
    },
    RemoveRecoveryKey {
        principal: Principal,
        // derived from PubKey deterministically
        key_id: Uuid,
    },
    // 64 byte string limit
    SetProtocol(String),
    // max maybe 256 entries?
    Expunge(Vec<ExpungeTarget>),
}

pub enum Permission {
    None,
    Read,
    Write,
}

pub enum Principal {
    Workspace,
    User(Uuid),
    Group(Uuid),
}

pub enum Parent {
    Workspace,
    Group(Uuid),
}

pub enum Child {
    Group(Uuid),
    User(Uuid),
}

pub enum ServerScope {
    All,
    Containers(Vec<Uuid>),
    None,
}

pub struct ExpungeTarget {
    pub container_id: Uuid,
    pub peer_id: Uuid,
    // max maybe 256?
    pub entry_ranges: Vec<Range<u64>>,
    // max maybe 256?
    pub entry_ops: Vec<(u64, Range<u64>)>,
}
