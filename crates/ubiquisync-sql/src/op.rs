use ubiquisync_core::ids::ContainerId;

pub trait Op {
    fn encode(&self) -> (ContainerId, Vec<u8>);
}
