use thiserror::Error;

use crate::{
    codec::{
        EntryHashError, OpBatchHashMethod, OpaqueOpBatchHashMethod, PlaintextOpBatchHashMethod,
        hash_use_key,
    },
    crypto::{
        EncryptionKeyRing, EntryCipher, Hash, PubKey, SignatureVerificationError,
        mmr::{MmrAccumulator, MmrState},
    },
    ids::{ContainerId, PeerId},
    log_entry::{CipherInfo, GenericLogEntry, OpaqueLogEntry, PlaintextLogEntry},
};

pub struct Verifier<'a> {
    signing_key: PubKey,
    peer_id: PeerId,
    container_id: ContainerId,
    active_cipher: Option<CipherInfo>,
    mmr: MmrAccumulator,
    signed_size: u64,
    keyring: &'a EncryptionKeyRing,
}

impl<'a> Verifier<'a> {
    pub fn new(
        signing_key: PubKey,
        peer_id: PeerId,
        container_id: ContainerId,
        active_cipher: Option<CipherInfo>,
        mmr: MmrAccumulator,
        signed_size: u64,
        keyring: &'a EncryptionKeyRing,
    ) -> Self {
        Self {
            signing_key,
            peer_id,
            container_id,
            active_cipher,
            mmr,
            signed_size,
            keyring,
        }
    }

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

    fn process_generic<B: std::fmt::Debug, M: OpBatchHashMethod<B>>(
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
            GenericLogEntry::SealBranch {
                signature,
                start,
                end,
                ack_until,
            } => todo!(),
        }
        Ok(())
    }

    pub fn signed_size(&self) -> u64 {
        self.signed_size
    }

    pub fn mmr_state(&self) -> &MmrState {
        self.mmr.state()
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
