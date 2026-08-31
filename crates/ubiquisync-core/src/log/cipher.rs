use thiserror::Error;

use crate::{
    bytes::{OpaqueBytes, PlaintextBytes},
    crypto::{Cipher, CipherError, Hash256, Key256Fingerprint},
    log::{
        ChainHash, ChainHashError, EntryBody, LogEntry, OpBatchHasher, OpaqueLogEntry,
        PlaintextLogEntry,
    },
};

#[derive(Error, Debug)]
pub enum SegmentCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),

    /// When processing a well-defined "segment", it is an error for the cipher key or suite to change
    /// mid-segment or to transition from an unencrypted to encrypted segment.
    /// A plaintext segment should be batch encryptable with a single cipher suite.
    #[error("cipher changed to {0:?} mid-segment")]
    CipherChanged(Key256Fingerprint),

    #[error("chain update error: {0}")]
    ChainHashError(#[from] ChainHashError),
}

/// Converts a segment of log entries from plaintext (not encrypted) to opaque (possibly encrypted)
/// while updating the chain hash along the way.
/// Entries are encrypted depending on whether or not a cipher is passed in.
/// The provided [Cipher] MUST match whatever cipher was declared by the latest `UseKey` entry in the log (if any).
/// It is an error for the segment to change its cipher mid-stream. Cipher changes MUST result in separate
/// segments with respect to encryption/decryption
pub fn segment_to_opaque<'a>(
    cipher: Option<&Cipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
    chain_hash: &mut ChainHash,
) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
    entries.scan(false, move |failed, e| {
        if *failed {
            return None;
        }
        let res = (|| {
            check_use_key(cipher, &e)?;
            let (e2, maybe_hash) = to_opaque(&e, cipher, chain_hash)?;
            chain_hash.update(&e2, maybe_hash)?;
            Ok(e2)
        })();
        if res.is_err() {
            *failed = true
        }
        Some(res)
    })
}

/// Converts a segment of log entries from opaque (possibly encrypted) to plaintext (not encrypted)
/// while updating the chain hash along the way.
/// This function has the same behavior as [segment_to_plaintext] with regards to ciphers.
pub fn segment_to_plaintext<'a>(
    cipher: Option<&Cipher>,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
    chain_hash: &mut ChainHash,
) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
    entries.scan(false, move |failed, e| {
        if *failed {
            return None;
        }
        let res = (|| {
            let (e2, maybe_hash) = to_plaintext(&e, cipher, chain_hash)?;
            check_use_key(cipher, &e2)?;
            chain_hash.update(&e, maybe_hash)?;
            Ok(e2)
        })();
        if res.is_err() {
            *failed = true
        }
        Some(res)
    })
}

struct OpBatchHashState {
    entry_idx: u64,
    hasher: OpBatchHasher,
    last_hash: Hash256,
}

fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: Option<&Cipher>,
    chain_hash: &ChainHash,
) -> Result<(OpaqueLogEntry<'a>, Option<Hash256>), CipherError> {
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    entry_idx,
                    last_hash: *chain_hash.hash(),
                    hasher: OpBatchHasher::new(*chain_hash.seed(), entry_idx, op_batch.ops.len()),
                })
            },
            |header, st| {
                let header_cipher = cipher.encrypt_header(st.entry_idx, &st.last_hash, header)?;
                st.last_hash = st.hasher.hash_header(&header_cipher);
                Ok(header_cipher)
            },
            |op_idx, op, st| {
                let op_cipher = cipher.encrypt_op(st.entry_idx, op_idx, &st.last_hash, op)?;
                st.last_hash = st.hasher.hash_op(op_idx, &op_cipher);
                Ok(op_cipher)
            },
            |op_idx, expunge_hash, st| {
                st.last_hash = *expunge_hash;
                st.hasher.hash_expunge(op_idx, expunge_hash);
                Ok(())
            },
        )?;
        Ok((e2, maybe_hash_state.map(|st| st.hasher.finalize())))
    } else {
        let (e2, _) = entry.transform(
            |_, _| Ok(()),
            |h, _| Ok(OpaqueBytes(h.0.clone())),
            |_, op, _| Ok(OpaqueBytes(op.0.clone())),
            |_, _, _| Ok(()),
        )?;
        Ok((e2, None))
    }
}

fn to_plaintext<'a>(
    entry: &OpaqueLogEntry<'a>,
    cipher: Option<&Cipher>,
    chain_hash: &ChainHash,
) -> Result<(PlaintextLogEntry<'a>, Option<Hash256>), CipherError> {
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    entry_idx,
                    last_hash: *chain_hash.hash(),
                    hasher: OpBatchHasher::new(*chain_hash.seed(), entry_idx, op_batch.ops.len()),
                })
            },
            |header_cipher, st| {
                let header = cipher.decrypt_header(st.entry_idx, &st.last_hash, header_cipher)?;
                st.last_hash = st.hasher.hash_header(header_cipher);
                Ok(header)
            },
            |op_idx, op_cipher, st| {
                let op = cipher.decrypt_op(st.entry_idx, op_idx, &st.last_hash, op_cipher)?;
                st.last_hash = st.hasher.hash_op(op_idx, op_cipher);
                Ok(op)
            },
            |op_idx, expunge_hash, st| {
                st.last_hash = *expunge_hash;
                st.hasher.hash_expunge(op_idx, expunge_hash);
                Ok(())
            },
        )?;
        Ok((e2, maybe_hash_state.map(|st| st.hasher.finalize())))
    } else {
        let (e2, _) = entry.transform(
            |_, _| Ok(()),
            |h, _| Ok(PlaintextBytes(h.0.clone())),
            |_, op, _| Ok(PlaintextBytes(op.0.clone())),
            |_, _, _| Ok(()),
        )?;
        Ok((e2, None))
    }
}

fn check_use_key<E: std::fmt::Debug, H: std::fmt::Debug>(
    cipher: Option<&Cipher>,
    e: &LogEntry<E, H>,
) -> Result<(), SegmentCipherError> {
    let LogEntry::IndexedEntry {
        entry: EntryBody::UseKey(cipher_info),
        ..
    } = e
    else {
        return Ok(());
    };

    if let Some(cipher) = cipher
        && cipher_info == &cipher.cipher_info()
    {
        // only okay if fingerprint and cipher suite match
        return Ok(());
    };
    Err(SegmentCipherError::CipherChanged(cipher_info.fingerprint))
}
