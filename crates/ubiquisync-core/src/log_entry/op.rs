use std::any::Any;

use crate::{BoxedError, ids::ContainerId};

pub trait EncodableOp: Any + Sync + Send {
    /// Encode the op. The encoded op bytes must be non-empty.
    fn encode(&self) -> (ContainerId, Vec<u8>);

    /// Defines what actor the op is attributed to which restricts where and how it can appear
    /// in server and device logs. Server ops can only occur in server logs and whne user
    /// ops occur in server logs they must always occur in a batch with server_user_id defined.
    /// Device and server ops should never be interpreted as emitted directly by a user.
    fn attribution(&self) -> OpAttribution {
        OpAttribution::User
    }
}

pub trait Op: EncodableOp + Sized + Clone {
    fn decode(container_id: &ContainerId, bytes: &[u8]) -> Result<Self, BoxedError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpAttribution {
    /// Attributed to a user. In server logs, must include server_user_id.
    User,
    /// Attributed to a device only, must not appear in server logs.
    DeviceOnly,
    /// Attributed to a server, must only appear in server logs.
    ServerOnly,
    ///
    DeviceOrServer,
}
