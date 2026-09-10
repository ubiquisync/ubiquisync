use thiserror::Error;

use crate::{
    crypto::{EntryCipher, SignatureVerifyError, VerifyingKey},
    log::{
        ChainHash, ChainHashError, ChainSeed, LogEntry, OpaqueLogEntry, PlaintextLogEntry,
        SegmentCipherError, entries_to_opaque_iter,
    },
};

pub fn verify_opaque<'a: 'b, 'b>(
    verifying_key: &VerifyingKey,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
) -> Result<ChainHash, LogVerifyError> {
    let mut ch = *prev_chain;
    for e in entries {
        match e {
            LogEntry::Signature(signature) => {
                verifying_key.verify_signature(&ch.sign_bytes(seed), &signature)?;
            }
            e @ LogEntry::IndexedEntry(_) => ch = ch.next(e, None, seed)?,
        }
    }
    Ok(ch)
}

pub fn verify_plaintext<'a: 'b, 'b>(
    verifying_key: &VerifyingKey,
    cipher: &Option<EntryCipher>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
) -> Result<ChainHash, LogVerifyError> {
    let mut ch = *prev_chain;
    for e in entries_to_opaque_iter(cipher, seed, &prev_chain, entries) {
        let (e, h) = e?;
        match e {
            LogEntry::Signature(signature) => {
                verifying_key.verify_signature(&h.sign_bytes(seed), &signature)?;
            }
            _ => {}
        }
        ch = h;
    }
    Ok(ch)
}

#[derive(Error, Debug)]
pub enum LogVerifyError {
    #[error("cipher error: {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("signature error: {0}")]
    Signature(#[from] SignatureVerifyError),
    #[error("chain hash error: {0}")]
    ChainHash(#[from] ChainHashError),
}
