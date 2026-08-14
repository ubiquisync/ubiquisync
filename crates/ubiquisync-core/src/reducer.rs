use std::any::Any;

use thiserror::Error;

use crate::{
    ContainerId, PeerId,
    codec::op::Op,
    log_entry::{OpBatch, PlaintextBytes, PlaintextOpBatch},
};

pub trait ReducerResolver {
    fn resolve_reducer(&self, container_id: &ContainerId) -> Option<&dyn DynReducer>;
}

pub trait Reducer {
    type Op: Op + Any + 'static;

    fn deliver_ops(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[IndexedOpBatch],
    ) -> Result<(), DeliverError>;
}

#[derive(Error, Debug)]
#[error("deliver error")]
pub struct DeliverError;

pub struct IndexedOpBatch<'a> {
    pub index: u64,
    pub batch: PlaintextOpBatch<'a>, // TODO decode header in advance
}

pub struct ReducerWrapper<R: Reducer> {
    reducer: R,
}

pub trait DynReducer {
    fn deliver(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[IndexedOpBatch],
    ) -> Result<Vec<OpIndexData>, DynReducerError>;
}

pub struct OpIndexData {
    pub entry_idx: u64,
    pub op_idx: u64,
    pub index_key: Vec<u8>,
}

#[derive(Error, Debug)]
pub enum DynReducerError {
    #[error("downcast error")]
    DowncastError,
    #[error("deliver error")]
    DeliverError(#[from] DeliverError),
}

impl<R: Reducer> DynReducer for ReducerWrapper<R> {
    fn deliver(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[IndexedOpBatch],
    ) -> Result<Vec<OpIndexData>, DynReducerError> {
        todo!()
    }
}
