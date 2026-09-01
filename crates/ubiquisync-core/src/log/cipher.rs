use thiserror::Error;

use crate::{
    bytes::{OpaqueBytes, PlaintextBytes},
    crypto::{CipherError, EntryCipher, Hash256, Key256Fingerprint},
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
pub fn segment_to_opaque<'a: 'b, 'b>(
    cipher: &Option<EntryCipher>,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
    chain_hash: &mut ChainHash,
) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
    entries.scan(false, move |failed, e| {
        if *failed {
            return None;
        }
        let res = (|| {
            check_use_key(cipher, e)?;
            let (e2, maybe_hash) = to_opaque(e, cipher, chain_hash)?;
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
pub fn segment_to_plaintext<'a: 'b, 'b>(
    cipher: &Option<EntryCipher>,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
    chain_hash: &mut ChainHash,
) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
    entries.scan(false, move |failed, e| {
        if *failed {
            return None;
        }
        let res = (|| {
            let (e2, maybe_hash) = to_plaintext(e, cipher, chain_hash)?;
            check_use_key(cipher, &e2)?;
            chain_hash.update(e, maybe_hash)?;
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
    cipher: &Option<EntryCipher>,
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
    cipher: &Option<EntryCipher>,
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
    cipher: &Option<EntryCipher>,
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

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use test_strategy::proptest;

    use crate::bytes::PlaintextBytes;
    #[cfg(test)]
    use crate::crypto::Hash256;
    use crate::crypto::{EntryCipher, EntryCipherSuite, Key256};
    use crate::ids::LogId;
    use crate::log::cipher::{to_opaque, to_plaintext};
    #[cfg(test)]
    use crate::log::segment::tests::LogEntries;
    use crate::log::{ChainHash, segment_to_opaque};
    use crate::log::{LogEntry, segment_to_plaintext};

    #[proptest]
    fn test_entry_cipher(
        entry: LogEntry<PlaintextBytes<'static>, PlaintextBytes<'static>>,
        key: Option<[u8; 32]>,
        log_id: LogId,
    ) {
        let cipher = if let Some(key) = key {
            let key = Key256(SecretBox::new(Box::new(key)));
            Some(EntryCipher::new(EntryCipherSuite::ChaCha20, key, &log_id))
        } else {
            None
        };
        let chain_hash = ChainHash::new_seed(&log_id);
        let (opaque, hash1) = to_opaque(&entry, &cipher, &chain_hash).unwrap();
        let (plaintext, hash2) = to_plaintext(&opaque, &cipher, &chain_hash).unwrap();
        assert_eq!(entry, plaintext);
        assert_eq!(hash1, hash2);
        if let LogEntry::IndexedEntry {
            idx,
            entry: crate::log::EntryBody::OpBatch(batch),
        } = opaque
            && cipher.is_some()
        {
            let hash = batch.hash(chain_hash.seed(), idx);
            assert_eq!(hash, hash1.unwrap());
        }
    }

    #[proptest(cases = 10)]
    fn test_segment_cipher(
        entries: LogEntries,
        key: Option<[u8; 32]>,
        log_id: LogId,
        prev_hash: Hash256,
    ) {
        let mut start_idx = entries.start_index;
        let mut entries = entries.entries;
        let cipher = if let Some(key) = key {
            let key = Key256(SecretBox::new(Box::new(key)));
            let cipher = EntryCipher::new(EntryCipherSuite::ChaCha20, key, &log_id);
            if start_idx > 0 {
                // if we're not at the very start, inject a UseKey entry at the beginning with our cipher to test this case
                // random UseKey entries in other places are not valid
                start_idx -= 1;
                entries.insert(
                    0,
                    LogEntry::IndexedEntry {
                        idx: start_idx,
                        entry: crate::log::EntryBody::UseKey(cipher.cipher_info()),
                    },
                )
            }
            Some(cipher)
        } else {
            None
        };
        let mut chain_hash = ChainHash::from_existing(&log_id, start_idx, prev_hash);
        let opaque = segment_to_opaque(&cipher, entries.iter(), &mut chain_hash)
            .map(|e| e.unwrap())
            .collect::<Vec<_>>();
        let mut chain_hash2 = ChainHash::from_existing(&log_id, start_idx, prev_hash);
        let plaintext = segment_to_plaintext(&cipher, opaque.iter(), &mut chain_hash2)
            .map(|e| e.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(chain_hash, chain_hash2);
        assert_eq!(entries, plaintext);
    }
}
