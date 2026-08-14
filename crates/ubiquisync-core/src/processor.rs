use std::borrow::Borrow;
use std::sync::Mutex;

use thiserror::Error;

use crate::hlc::Hlc;
use crate::ids::ContainerId;
use crate::ids::PeerId;
use crate::log_entry::GenericLogEntry;
use crate::log_entry::OpaqueLogEntry;
use crate::reducer::ReducerOpBatch;
use crate::verifier::VerificationError;
use crate::verifier::Verifier;
use crate::{
    codec::decoder::{DecodeError, decode_op_header},
    crypto::{
        EncryptionKeyRing,
        mmr::{MmrAccumulator, MmrError},
    },
    hlc::wall_ms,
    log_entry::PlaintextLogEntry,
    reducer::{ReducerError, ReducerResolver},
    storage::{Batch, LogEntries, Storage},
    uuid::Uuid,
};

pub struct Processor<S: Storage> {
    storage: S,
    hlc: Mutex<Hlc>,
    keyring: EncryptionKeyRing,
    reducers: Box<dyn ReducerResolver>,
}

impl<S: Storage> Processor<S> {
    pub fn process_plaintext(
        &mut self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        entries: &[PlaintextLogEntry],
    ) -> Result<(), ProcessorError<S::Error>> {
        if entries.is_empty() {
            return Err(ProcessorError::EmptyEntries);
        }

        match entries[entries.len() - 1] {
            GenericLogEntry::Signature { size, signature } => {}
            _ => return Err(ProcessorError::ExpectedSignature),
        }

        let receive_state = self
            .storage
            .get_receive_state(container_id, peer_id)
            .map_err(ProcessorError::StorageError)?;
        let size = receive_state.mmr_state.size;
        let peer_info = self
            .storage
            .get_peer_info(peer_id)
            .map_err(ProcessorError::StorageError)?;
        let mmr = MmrAccumulator::new(&peer_info.peer_id, container_id, receive_state.mmr_state)?;
        let mut verifier = Verifier::new(
            peer_info.signing_pub_key(),
            *peer_id,
            *container_id,
            receive_state.active_cipher,
            mmr,
            size,
            &self.keyring,
        );
        if let Some(reducer) = self.reducers.resolve_reducer(container_id) {
            let mut processable_batches = vec![];
            let mut decoded_entries = vec![];
            let local_wall_ms = wall_ms();
            let mut hlc = self.hlc.lock().unwrap_or_else(|e| e.into_inner());
            for entry in entries.iter() {
                verifier.process_plaintext(entry)?;
                decoded_entries.push((entry.clone(), None)); // TODO get digest from verifier
                match entry {
                    GenericLogEntry::IndexedEntry { entry, idx } => match entry {
                        crate::log_entry::EntryBody::OpBatch(op_batch) => {
                            let header = decode_op_header(op_batch.header.borrow())?;
                            match hlc.observe(header.timestamp, local_wall_ms) {
                                Ok(_) => {
                                    let ops = op_batch
                                        .ops
                                        .iter()
                                        .filter_map(|o| match o {
                                            crate::log_entry::OpOrExpunge::Op(op) => {
                                                Some(op.clone())
                                            }
                                            crate::log_entry::OpOrExpunge::Expunge(_) => None,
                                        })
                                        .collect();
                                    processable_batches.push(ReducerOpBatch {
                                        header: header.clone(),
                                        ops,
                                    });
                                }

                                Err(_) => todo!("handle skew error and don't send to reducer!!!"),
                            };
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            let mut storage_batch = self.storage.new_batch();
            storage_batch.add_log_entries(LogEntries {
                container_id: *container_id,
                peer_id: *peer_id,
                processed_idx: Some(verifier.signed_size()),
                decoded_entries,
                opaque_entries: vec![],
                received_mmr_state: verifier.mmr_state().clone(),
            });
            // TODO hlc update
            self.storage
                .commit_batch(storage_batch)
                .map_err(ProcessorError::StorageError)?;
            reducer.deliver(container_id, peer_id, &processable_batches)?;
        } else {
            let mut opaque_entries = vec![];
            for entry in entries.iter() {
                let opaque_entry = verifier.process_plaintext(entry)?;
                opaque_entries.push((opaque_entry, None));
            }
            let mut storage_batch = self.storage.new_batch();
            storage_batch.add_log_entries(LogEntries {
                container_id: *container_id,
                peer_id: *peer_id,
                processed_idx: None,
                decoded_entries: vec![],
                opaque_entries,
                received_mmr_state: verifier.mmr_state().clone(),
            });
            self.storage
                .commit_batch(storage_batch)
                .map_err(ProcessorError::StorageError)?;
        }
        Ok(())
    }

    pub fn process_opaque(
        &mut self,
        container_id: &Uuid,
        peer_id: &Uuid,
        entries: &[OpaqueLogEntry],
    ) -> Result<(), ProcessorError<S::Error>> {
        todo!()
    }
}

#[derive(Error, Debug)]
pub enum ProcessorError<StorageError> {
    #[error("storage error: {0}")]
    StorageError(StorageError),
    #[error("MMR error: {0}")]
    MmrError(#[from] MmrError),
    #[error("verification error: {0}")]
    VerificationError(#[from] VerificationError),
    #[error("decode error: {0}")]
    DecodeError(#[from] DecodeError),
    #[error("reducer error: {0}")]
    ReducerError(#[from] ReducerError),
    #[error("last entry in a batch must be a signature")]
    ExpectedSignature,
    #[error("empty entries")]
    EmptyEntries,
}
