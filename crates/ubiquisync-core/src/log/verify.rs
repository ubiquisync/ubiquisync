use thiserror::Error;

use crate::{
    crypto::{CipherInfo, CipherKeyResolver, SignatureVerifyError, VerifyingKey},
    log::{
        ChainHash, ChainHashError, EntryBody, LogEntry, LogHashContext, OpaqueLogEntry,
        PlaintextLogEntry, SegmentCipherError, entries_to_opaque,
    },
};

/// Verifies the signatures in the entries against the computed hash
/// starting from head_chain.
///
/// The computed hash is checked against `expected_prev_chain` at whatever
/// height this `ChainHash` is provided for. `expected_prev_chain` may
/// equal head_chain or maybe be at some size after that for the cases when
/// we want to start reading mid-segment.
pub fn verify_opaque<'a: 'b, 'b>(
    verifying_key: &VerifyingKey,
    seed: &LogHashContext,
    head_chain: &ChainHash,
    head_cipher: &mut Option<CipherInfo>,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
) -> Result<Vec<ChainHash>, LogVerifyError> {
    let mut ch = *head_chain;
    let mut chain_hashes = vec![];
    for e in entries {
        match e {
            LogEntry::Signature(signature) => {
                verifying_key.verify_signature(&ch.sign_bytes(seed), signature)?;
            }
            e @ LogEntry::IndexedEntry(_) => {
                ch = ch.next(e, seed, head_cipher)?;
                chain_hashes.push(ch);
                if let LogEntry::IndexedEntry(EntryBody::UseKey(ci)) = e {
                    *head_cipher = Some(*ci)
                }
            }
        }
    }
    Ok(chain_hashes)
}

pub async fn verify_plaintext<'a: 'b, 'b>(
    verifying_key: &VerifyingKey,
    seed: &LogHashContext,
    head_cipher: &mut Option<CipherInfo>,
    head_chain: &ChainHash,
    key_resolver: &dyn CipherKeyResolver,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
) -> Result<Vec<ChainHash>, LogVerifyError> {
    let mut chain_hashes = vec![];
    for (e, h) in entries_to_opaque(seed, head_cipher, head_chain, key_resolver, entries).await? {
        match e {
            LogEntry::Signature(signature) => {
                verifying_key.verify_signature(&h.sign_bytes(seed), &signature)?
            }
            LogEntry::IndexedEntry(_) => chain_hashes.push(h),
        }
    }
    Ok(chain_hashes)
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
