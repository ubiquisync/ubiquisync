use std::collections::HashMap;

use thiserror::Error;

use crate::{
    codec::{
        EntryHashError, OpBatchHashMethod, OpaqueOpBatchHashMethod, PlaintextOpBatchHashMethod,
        hash_use_key,
    },
    crypto::{
        EncryptionKeyRing, EntryCipher, Hash, PubKey, SignatureVerificationError,
        mmr::{MmrAccumulator, MmrError},
    },
    log_entry::{CipherInfo, GenericLogEntry, OpaqueLogEntry, PlaintextLogEntry},
    reducer::{DynReducer, ReducerResolver},
    storage::{Storage, StorageError},
    uuid::Uuid,
};

pub struct Processor<S: Storage> {
    storage: S,
    keyring: EncryptionKeyRing,
    reducers: Box<dyn ReducerResolver>,
}

impl<S: Storage> Processor<S> {
    pub fn process_plaintext(
        &mut self,
        container_id: &Uuid,
        peer_id: &Uuid,
        entries: &[PlaintextLogEntry],
    ) -> Result<(), ProcessorError> {
        if entries.is_empty() {
            return Err(ProcessorError::EmptyEntries);
        }

        match entries[entries.len() - 1] {
            GenericLogEntry::Signature { size, signature } => {}
            _ => return Err(ProcessorError::ExpectedSignature),
        }

        let receive_state = self.storage.get_receive_state(container_id, peer_id)?;
        let size = receive_state.mmr_state.size;
        let peer_info = self.storage.get_peer_info(peer_id)?;
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
            for entry in entries.iter() {
                verifier.process_plaintext(entry)?;
                let decoded = entry.transform(|op| reducer.op_parser().decode(op.borrow()), |header| )
            }
        } else {
            let mut opaque_entries = vec![];
            for entry in entries.iter() {
                let opaque_entry = verifier.process_plaintext(entry)?;
                opaque_entries.push(opaque_entry);
            }
        }
        todo!()
    }

    pub fn process_opaque(
        &mut self,
        container_id: &Uuid,
        peer_id: &Uuid,
        entries: &[OpaqueLogEntry],
    ) -> Result<(), ProcessorError> {
        todo!()
    }
}

#[derive(Error, Debug)]
pub enum ProcessorError {
    #[error("storage error: {0}")]
    StorageError(#[from] StorageError),
    #[error("MMR error: {0}")]
    MmrError(#[from] MmrError),
    #[error("verification error: {0}")]
    VerificationError(#[from] VerificationError),
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
