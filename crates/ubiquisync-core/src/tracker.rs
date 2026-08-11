use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    codec::{EntryHashError, OpBatchHasher, hash_use_key},
    crypto::{EncryptionKeyRing, Hash, PubKey, SignatureVerificationError, mmr::MmrAccumulator},
    log_entry::{OpaqueLogEntry, PlaintextLogEntry},
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
    active_key: Option<Hash>,
    mmr: MmrAccumulator,
    signed_idx: u64,
    keyring: &'a mut EncryptionKeyRing,
}

impl<'a> Verifier<'a> {
    pub fn process_plaintext(&mut self, entry: &PlaintextLogEntry) {}

    pub fn process_opaque(&mut self, entry: &OpaqueLogEntry) -> Result<(), VerificationError> {
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
                        let mut hasher = OpBatchHasher::new(
                            op_batch.header.0.borrow(),
                            idx,
                            op_batch.ops.len() as u64,
                        );
                        for op in op_batch.ops.iter() {
                            match op {
                                crate::log_entry::OpOrExpunge::Op(bytes) => {
                                    hasher.append_opaque_op(bytes.0.borrow())?
                                }
                                crate::log_entry::OpOrExpunge::Expunge(hash) => {
                                    hasher.append_expunged_op(hash)?
                                }
                            }
                        }
                        let hash = hasher.finalize()?;
                        hash
                    }
                    crate::log_entry::EntryBody::UseKey(fingerprint) => {
                        self.active_key = Some(*fingerprint);
                        hash_use_key(idx, fingerprint)
                    }
                };

                self.mmr.append(&entry_hash.as_bytes());
            }
            crate::log_entry::GenericLogEntry::Expunged {
                start_idx,
                end_idx,
                cover,
            } => todo!(),
            crate::log_entry::GenericLogEntry::Signature { height, signature } => {
                let expected_height = self.mmr.size();
                let height = *height;
                if height != expected_height {
                    return Err(VerificationError::OutOfOrderSignature {
                        expected: expected_height,
                        actual: height,
                    });
                }

                let sign_bytes = self.mmr.sign_bytes();
                self.signing_key.verify_signature(&sign_bytes, signature)?;
                self.signed_idx = height;
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
}
