use std::any::Any;

use thiserror::Error;

use crate::{
    codec::op::Op,
    log_entry::{OpBatch, PlaintextBytes},
    uuid::Uuid,
};

pub trait ReducerResolver {
    fn resolve_reducer(&self, container_id: &Uuid) -> Option<&dyn DynReducer>;
}

pub trait Reducer {
    type Op: Op + Any + 'static;

    fn deliver_ops(
        &self,
        container_id: &Uuid,
        peer_id: &Uuid,
        batches: &[IndexedOpBatch<Self::Op>],
    ) -> Result<(), DeliverError>;
}

#[derive(Error, Debug)]
#[error("deliver error")]
pub struct DeliverError;

pub struct IndexedOpBatch<O> {
    pub index: u64,
    pub batch: OpBatch<O>,
}

pub struct ReducerWrapper<R: Reducer> {
    reducer: R,
}

pub trait DynReducer {
    fn deliver(
        &self,
        container_id: &Uuid,
        peer_id: &Uuid,
        batches: &[IndexedOpBatch<PlaintextBytes>],
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
        container_id: &Uuid,
        peer_id: &Uuid,
        batches: &[IndexedOpBatch<PlaintextBytes>],
    ) -> Result<Vec<OpIndexData>, DynReducerError> {
        todo!()
    }
}
