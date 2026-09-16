use thiserror::Error;

use crate::{
    crypto::{CipherInfo, CipherKeyResolver, SignatureVerifyError, VerifyingKey},
    log::{
        ChainHash, ChainHashError, ChainSeed, LogEntry, OpaqueLogEntry, PlaintextLogEntry,
        SegmentCipherError, entries_to_opaque,
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

pub async fn verify_plaintext<'a: 'b, 'b>(
    verifying_key: &VerifyingKey,
    seed: &ChainSeed,
    head_cipher: &mut Option<CipherInfo>,
    head_chain: &ChainHash,
    key_resolver: &dyn CipherKeyResolver,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
) -> Result<ChainHash, LogVerifyError> {
    let mut ch = *head_chain;
    for (e, h) in entries_to_opaque(seed, head_cipher, head_chain, key_resolver, entries).await? {
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
