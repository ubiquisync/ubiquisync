use thiserror::Error;

use crate::{
    codec::{
        EntryHashError, OpBatchHashMethod, OpaqueOpBatchHashMethod, PlaintextOpBatchHashMethod,
        hash_use_key,
    },
    crypto::{
        EncryptionKeyRing, EntryCipher, Hash, PubKey, SignatureVerificationError,
        mmr::MmrAccumulator,
    },
    log_entry::{GenericLogEntry, OpaqueLogEntry, PlaintextLogEntry},
    uuid::Uuid,
};
// use crate::{
//     codec::PlaintextOpBatchHasher, crypto::{MmrAccumulator, MmrState}, log_entry::{LogEntry, OpaqueLogEntry}, uuid::Uuid
// };

// pub trait TrackerStorage {
//     fn received_state(&self, peer_id: Uuid, container_id: Uuid) -> MmrAccumulator;
//     fn advance_mmr(&self, peer_id: Uuid, container_id: Uuid, state: &MmrState);
//     fn receive_entry(&self, peer_id: Uuid, container_id: Uuid, entry_bytes: &[u8]);
// }

// pub struct Tracker<Op> {
//     storage: Box<dyn TrackerStorage>,
// }

// impl<Op> Tracker<Op> {
//     fn receive_entry(&self, peer_id: Uuid, container_id: Uuid, entry: LogEntry<Op>) {
//         let mmr = self.storage.received_state(peer_id, container_id);
//         let hasher = PlaintextOpBatchHasher::new(todo!(), )
//     }

//     fn receive_opaque_entry(&self, peer_id: Uuid, container_id: Uuid, entry: OpaqueLogEntry) {}
// }

pub struct Verifier<'a> {
    signing_key: PubKey,
    peer_id: Uuid,
    container_id: Uuid,
    active_key: Option<Hash>,
    mmr: MmrAccumulator,
    signed_size: u64,
    keyring: &'a mut EncryptionKeyRing,
}

impl<'a> Verifier<'a> {
    pub fn process_plaintext(
        &mut self,
        entry: &PlaintextLogEntry,
    ) -> Result<(), VerificationError> {
        // TODO cache the key if we've already retrieved it previously
        let maybe_cipher = if let Some(fingerprint) = self.active_key {
            let key = self
                .keyring
                .get_key(&fingerprint)
                .ok_or(VerificationError::EncryptionKeyNotFound { fingerprint })?;
            Some(EntryCipher::new(key, &self.peer_id, &self.container_id))
        } else {
            None
        };
        let hash_method = PlaintextOpBatchHashMethod(&maybe_cipher);
        self.process_generic(entry, &hash_method)
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
                    crate::log_entry::EntryBody::UseKey {
                        cipher_suite,
                        fingerprint,
                    } => {
                        self.active_key = Some(*fingerprint);
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
