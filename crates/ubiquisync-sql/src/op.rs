use thiserror::Error;
use ubiquisync_core::{bytes::PlaintextBytes, ids::ContainerId};

use crate::BoxError;

pub trait OpCodec<Op> {
    fn encode(&self, op: &Op)
    -> Result<(ContainerId, Vec<PlaintextBytes<'static>>), OpEncodeError>;
    fn decode(
        &self,
        container_id: &ContainerId,
        ops: &[PlaintextBytes],
    ) -> Result<Op, OpDecodeError>;
}

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
