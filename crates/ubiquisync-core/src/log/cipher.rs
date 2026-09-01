use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, OpaqueBytes, PlaintextBytes},
    crypto::{CipherError, EntryCipher, Hash256, RootKey256Fingerprint, SlotCipher},
    log::{
        ChainHash, ChainHashError, ChainSeed, EntryBody, LogEntry, OpBatchHasher, OpaqueLogEntry,
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
    CipherChanged(RootKey256Fingerprint),

    #[error("chain update error: {0}")]
    ChainHashError(#[from] ChainHashError),
}

pub fn segment_to_opaque<'a: 'b, 'b>(
    cipher: &Option<EntryCipher>,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
) -> Result<(Vec<OpaqueLogEntry<'a>>, ChainHash), SegmentCipherError> {
    let mut res = vec![];
    let mut cur_chain = *prev_chain;
    for e in segment_to_opaque_iter(cipher, entries, seed, prev_chain) {
        let (e, chain) = e?;
        res.push(e);
        cur_chain = chain;
    }
    Ok((res, cur_chain))
}

/// Converts a segment of log entries from plaintext (not encrypted) to opaque (possibly encrypted)
/// while updating the chain hash along the way.
/// Entries are encrypted depending on whether or not a cipher is passed in.
/// The provided [Cipher] MUST match whatever cipher was declared by the latest `UseKey` entry in the log (if any).
/// It is an error for the segment to change its cipher mid-stream. Cipher changes MUST result in separate
/// segments with respect to encryption/decryption
pub fn segment_to_opaque_iter<'a: 'b, 'b>(
    cipher: &Option<EntryCipher>,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
) -> impl Iterator<Item = Result<(OpaqueLogEntry<'a>, ChainHash), SegmentCipherError>> {
    let mut cur_chain = *prev_chain;
    entries.scan(false, move |failed, e| {
        if *failed {
            return None;
        }
        let res = (|| {
            check_use_key(cipher, e)?;
            let (e2, maybe_hash) = to_opaque(e, cipher, seed, &cur_chain)?;
            cur_chain = cur_chain.next(&e2, maybe_hash, seed)?;
            Ok((e2, cur_chain))
        })();
        if res.is_err() {
            *failed = true
        }
        Some(res)
    })
}

pub fn segment_to_plaintext<'a: 'b, 'b>(
    cipher: &Option<EntryCipher>,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
) -> Result<(Vec<PlaintextLogEntry<'a>>, ChainHash), SegmentCipherError> {
    let mut res = vec![];
    let mut cur_chain = *prev_chain;
    for e in segment_to_plaintext_iter(cipher, entries, seed, prev_chain) {
        let (e, chain) = e?;
        res.push(e);
        cur_chain = chain;
    }
    Ok((res, cur_chain))
}

/// Converts a segment of log entries from opaque (possibly encrypted) to plaintext (not encrypted)
/// while updating the chain hash along the way.
/// This function has the same behavior as [segment_to_plaintext] with regards to ciphers.
pub fn segment_to_plaintext_iter<'a: 'b, 'b>(
    cipher: &Option<EntryCipher>,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
) -> impl Iterator<Item = Result<(PlaintextLogEntry<'a>, ChainHash), SegmentCipherError>> {
    let mut cur_chain = *prev_chain;
    entries.scan(false, move |failed, e| {
        if *failed {
            return None;
        }
        let res = (|| {
            let (e2, maybe_hash) = to_plaintext(e, cipher, seed, &cur_chain)?;
            check_use_key(cipher, &e2)?;
            cur_chain = cur_chain.next(&e, maybe_hash, seed)?;
            Ok((e2, cur_chain))
        })();
        if res.is_err() {
            *failed = true
        }
        Some(res)
    })
}

struct OpBatchHashState {
    hasher: OpBatchHasher,
    slot_cipher: SlotCipher,
    last_hash: Hash256,
}

fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: &Option<EntryCipher>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
) -> Result<(OpaqueLogEntry<'a>, Option<Hash256>), CipherError> {
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    last_hash: prev_chain.hash,
                    hasher: OpBatchHasher::new(seed, entry_idx, op_batch.ops.len()),
                    slot_cipher: cipher.slot_cipher(entry_idx),
                })
            },
            |slot, st| {
                let slot_cipher = st.slot_cipher.encrypt_slot(&st.last_hash, slot)?;
                st.last_hash = st.hasher.hash_slot(&slot_cipher);
                Ok(slot_cipher)
            },
            |expunge_hash, st| {
                st.last_hash = *expunge_hash;
                st.hasher.hash_expunge(expunge_hash);
                st.slot_cipher.skip_slot();
                Ok(())
            },
        )?;
        Ok((e2, maybe_hash_state.map(|st| st.hasher.finalize())))
    } else {
        let (e2, _) = entry.transform(
            |_, _| Ok(()),
            |s, _| Ok(OpaqueBytes(s.0.clone())),
            |_, _| Ok(()),
        )?;
        Ok((e2, None))
    }
}

fn to_plaintext<'a>(
    entry: &OpaqueLogEntry<'a>,
    cipher: &Option<EntryCipher>,
    seed: &ChainSeed,
    prev_chain: &ChainHash,
) -> Result<(PlaintextLogEntry<'a>, Option<Hash256>), CipherError> {
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    last_hash: prev_chain.hash,
                    hasher: OpBatchHasher::new(seed, entry_idx, op_batch.ops.len()),
                    slot_cipher: cipher.slot_cipher(entry_idx),
                })
            },
            |slot_cipher, st| {
                let op = st.slot_cipher.decrypt_slot(&st.last_hash, slot_cipher)?;
                st.last_hash = st.hasher.hash_slot(slot_cipher);
                Ok(op)
            },
            |expunge_hash, st| {
                st.last_hash = *expunge_hash;
                st.hasher.hash_expunge(expunge_hash);
                st.slot_cipher.skip_slot();
                Ok(())
            },
        )?;
        Ok((e2, maybe_hash_state.map(|st| st.hasher.finalize())))
    } else {
        let (e2, _) = entry.transform(
            |_, _| Ok(()),
            |s, _| Ok(PlaintextBytes(s.0.clone())),
            |_, _| Ok(()),
        )?;
        Ok((e2, None))
    }
}

// TODO: we actually should be able to accomodate key changes mid segment
fn check_use_key<E: BytesWrapper>(
    cipher: &Option<EntryCipher>,
    e: &LogEntry<E>,
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
    use crate::crypto::Hash256;
    use crate::crypto::{EntryCipher, EntryCipherSuite, RootKey256};
    use crate::ids::LogId;
    #[cfg(test)]
    use crate::log::ChainSeed;
    use crate::log::cipher::{to_opaque, to_plaintext};
    use crate::log::segment::tests::LogEntries;
    use crate::log::{ChainHash, segment_to_opaque};
    use crate::log::{LogEntry, segment_to_plaintext};

    #[proptest]
    fn test_entry_cipher(
        entry: LogEntry<PlaintextBytes<'static>>,
        key: Option<[u8; 32]>,
        log_id: LogId,
    ) {
        let cipher = if let Some(key) = key {
            let key = RootKey256::new(SecretBox::new(Box::new(key)));
            let container_key = key.container_key(&log_id.container_id);
            Some(EntryCipher::new(
                EntryCipherSuite::ChaCha20,
                container_key,
                &log_id,
            ))
        } else {
            None
        };
        let seed = ChainSeed::new(&log_id);
        let chain_hash = ChainHash::empty(&seed);
        let (opaque, hash1) = to_opaque(&entry, &cipher, &seed, &chain_hash).unwrap();
        let (plaintext, hash2) = to_plaintext(&opaque, &cipher, &seed, &chain_hash).unwrap();
        assert_eq!(entry, plaintext);
        assert_eq!(hash1, hash2);
        if let LogEntry::IndexedEntry {
            idx,
            entry: crate::log::EntryBody::OpBatch(batch),
        } = opaque
            && cipher.is_some()
        {
            let hash = batch.hash(&seed, idx);
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
            let key = RootKey256::new(SecretBox::new(Box::new(key)));
            let container_key = key.container_key(&log_id.container_id);
            let cipher = EntryCipher::new(EntryCipherSuite::ChaCha20, container_key, &log_id);
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

        let seed = ChainSeed::new(&log_id);
        let start_chain = ChainHash {
            size: start_idx,
            hash: prev_hash,
        };
        let (opaque, end_chain) =
            segment_to_opaque(&cipher, entries.iter(), &seed, &start_chain).unwrap();
        let (plaintext, end_chain2) =
            segment_to_plaintext(&cipher, opaque.iter(), &seed, &start_chain).unwrap();
        assert_eq!(end_chain, end_chain2);
        assert_eq!(entries, plaintext);
    }
}
