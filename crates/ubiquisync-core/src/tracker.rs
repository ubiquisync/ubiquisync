use std::{borrow::Borrow, collections::HashMap};

use thiserror::Error;

use crate::{
    codec::{
        EntryHashError, OpBatchHashMethod, OpaqueOpBatchHashMethod, PlaintextOpBatchHashMethod,
        decoder::{DecodeError, decode_op_header},
        hash_use_key,
    },
    crypto::{
        EncryptionKeyRing, EntryCipher, Hash, PubKey, SignatureVerificationError,
        mmr::{MmrAccumulator, MmrError},
    },
    hlc::{HlcService, wall_ms},
    log_entry::{CipherInfo, GenericLogEntry, OpaqueLogEntry, PlaintextLogEntry},
    reducer::{DynReducer, DynReducerError, IndexedOpBatch, ReducerResolver},
    storage::{Batch, LogEntries, Storage},
    uuid::Uuid,
};

pub struct Processor<S: Storage> {
    storage: S,
    hlc: HlcService<S>,
    keyring: EncryptionKeyRing,
    reducers: Box<dyn ReducerResolver>,
}

impl<S: Storage> Processor<S> {
    pub fn process_plaintext(
        &mut self,
        container_id: &Uuid,
        peer_id: &Uuid,
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
        let mmr = MmrAccumulator::new(
            &peer_info.genesis_hash(),
            container_id,
            receive_state.mmr_state,
        )?;
        let mut verifier = Verifier {
            signing_key: peer_info.signing_pub_key(),
            peer_id: *peer_id,
            container_id: *container_id,
            active_cipher: receive_state.active_cipher,
            mmr,
            signed_size: size,
            keyring: &self.keyring,
        };
        if let Some(reducer) = self.reducers.resolve_reducer(container_id) {
            let mut processable_batches = vec![];
            let mut indexable_entries = vec![];
            let local_wall_ms = wall_ms();
            for entry in entries.iter() {
                verifier.process_plaintext(entry)?;
                let decoded = entry.transform(
                    |op| reducer.op_parser().decode(op.borrow()),
                    decode_op_header,
                )?;
                let indexable = decoded.transform(|op| op.to_index_parts(), |h| Ok(h.clone()))?;
                match decoded {
                    GenericLogEntry::IndexedEntry { entry, idx } => match entry {
                        crate::log_entry::EntryBody::OpBatch(op_batch) => {
                            // TODO observe hlc timestamp - need to refactor the HLC service a bit and figure out what to do on a skew error
                            processable_batches.push(IndexedOpBatch {
                                index: idx,
                                batch: op_batch,
                            });
                        }
                        _ => {}
                    },
                    _ => {}
                }
                indexable_entries.push((indexable, None)); // TODO get digest from verifier
            }
            let mut storage_batch = self.storage.new_batch();
            storage_batch.add_log_entries(LogEntries {
                container_id: *container_id,
                peer_id: *peer_id,
                processed_idx: Some(verifier.signed_size),
                decoded_entries: indexable_entries,
                opaque_entries: vec![],
                received_mmr_state: verifier.mmr.state().clone(),
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
                opaque_entries.push(opaque_entry);
            }
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
    ReducerError(#[from] DynReducerError),
    #[error("last entry in a batch must be a signature")]
    ExpectedSignature,
    #[error("empty entries")]
    EmptyEntries,
}

pub struct Verifier<'a> {
    signing_key: PubKey,
    peer_id: Uuid,
    container_id: Uuid,
    active_cipher: Option<CipherInfo>,
    mmr: MmrAccumulator,
    signed_size: u64,
    keyring: &'a EncryptionKeyRing,
}

impl<'a> Verifier<'a> {
    pub fn process_plaintext<'b>(
        &mut self,
        entry: &PlaintextLogEntry<'b>,
    ) -> Result<OpaqueLogEntry<'b>, VerificationError> {
        // TODO cache the key if we've already retrieved it previously
        let maybe_cipher = if let Some(cipher) = &self.active_cipher {
            let key = self.keyring.get_key(&cipher.fingerprint).ok_or(
                VerificationError::EncryptionKeyNotFound {
                    fingerprint: cipher.fingerprint,
                },
            )?;
            Some(EntryCipher::new(key, &self.peer_id, &self.container_id))
        } else {
            None
        };
        let hash_method = PlaintextOpBatchHashMethod(&maybe_cipher);
        self.process_generic(entry, &hash_method)?;
        todo!("reconstruct opaque from hasher return")
    }

    pub fn process_opaque(&mut self, entry: &OpaqueLogEntry) -> Result<(), VerificationError> {
        self.process_generic(entry, &OpaqueOpBatchHashMethod)
    }

    fn process_generic<B, M: OpBatchHashMethod<B>>(
        &mut self,
        entry: &GenericLogEntry<B, B>,
        hash_method: &M,
    ) -> Result<(), VerificationError> {
        match entry {
            crate::log_entry::GenericLogEntry::IndexedEntry { idx, entry } => {
                let expected_idx = self.mmr.size();
                let idx = *idx;
                if idx != expected_idx {
                    return Err(VerificationError::OutOfOrderLogEntry {
                        expected: expected_idx,
                        actual: idx,
                    });
                }

                let entry_hash = match entry {
                    crate::log_entry::EntryBody::OpBatch(op_batch) => {
                        let (hash, _opaque_log_entry) = hash_method.hash(idx, op_batch)?;
                        hash
                    }
                    crate::log_entry::EntryBody::UseKey(CipherInfo {
                        cipher_suite,
                        fingerprint,
                    }) => {
                        self.active_cipher = Some(CipherInfo {
                            cipher_suite: *cipher_suite,
                            fingerprint: *fingerprint,
                        });
                        hash_use_key(idx, *cipher_suite, fingerprint)
                    }
                };

                self.mmr.append(&entry_hash);
            }
            crate::log_entry::GenericLogEntry::Expunged {
                start_idx,
                end_idx,
                cover,
            } => todo!(),
            crate::log_entry::GenericLogEntry::Signature { size, signature } => {
                let expected_size = self.mmr.size();
                let size = *size;
                if size != expected_size {
                    return Err(VerificationError::OutOfOrderSignature {
                        expected: expected_size,
                        actual: size,
                    });
                }

                let sign_bytes = self.mmr.sign_bytes();
                self.signing_key.verify_signature(&sign_bytes, signature)?;
                self.signed_size = size;
            }
        }
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum VerificationError {
    #[error("expected log index: {expected}, got {actual}")]
    OutOfOrderLogEntry { expected: u64, actual: u64 },
    #[error("entry hash error: {0}")]
    HashError(#[from] EntryHashError),
    #[error("expected log index: {expected}, got {actual}")]
    OutOfOrderSignature { expected: u64, actual: u64 },
    #[error("signature verification error: {0}")]
    SignatureVerificationError(#[from] SignatureVerificationError),
    #[error("encryption key not found")]
    EncryptionKeyNotFound { fingerprint: Hash },
}
