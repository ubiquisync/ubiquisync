use thiserror::Error;
use ubiquisync_core::{bytes::PlaintextBytes, ids::ContainerId};

pub trait OpCodec<Op> {
    fn encode(&self, op: &Op)
    -> Result<(ContainerId, Vec<PlaintextBytes<'static>>), OpEncodeError>;
    fn decode(
        &self,
        container_id: &ContainerId,
        ops: &[PlaintextBytes],
    ) -> Result<Op, OpDecodeError>;
}

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Error, Debug)]
pub enum OpEncodeError {
    #[error("invalid op: {0}")]
    Invalid(BoxError),
}

#[derive(Error, Debug)]
pub enum OpDecodeError {
    #[error("invalid op: {0}")]
    Invalid(BoxError),
}
