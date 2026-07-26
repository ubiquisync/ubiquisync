use crate::uuid::Uuid;

pub enum Effect {
    SetKV(Vec<u8>, Vec<u8>), // internal kv storage for capabilities, claims, pending ops, etc.
    // TODO should there be some way to set an auto hlc timeout for certain kv data to expire pending invites, etc.?
    DeleteKV(Vec<u8>),
    AdmitUser(Uuid),
    AdmitServer(Uuid),
    AdmitDevice {
        device_id: Uuid,
        server_id: Uuid,
    },
    RemoveUser(Uuid),
    RemoveServer(Uuid),
    RemoveDevice(Uuid),
    SetContainerPermission {
        container_id: Uuid,
        principal: Principal,
        permission: Permission,
    },
    AddToGroup {
        group_id: Uuid,
        principal: Principal,
    }, // TODO Workspace is invalid here, so we probably need another enum, this is just a draft
    RemoveFromGroup {
        group_id: Uuid,
        principal: Principal,
    },
}

pub enum Permission {
    None,
    Read,
    Write,
}

pub enum Principal {
    Workspace,
    User(Uuid),
    Server(Uuid),
    Group(Uuid),
}
