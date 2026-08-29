use thiserror::Error;

use crate::{
    crypto::{
        CipherError, EntryCipher, Hash256, Hash256Suite, Key256Fingerprint, mmr::MmrAccumulator,
    },
    log_entry::{
        EntryBody, GenericLogEntry, MmrUpdateError, OpBatchHasher, OpaqueBytes, OpaqueLogEntry,
        PlaintextBytes, PlaintextLogEntry, mmr_append_entry_hashed,
    },
};

struct OpBatchHashState {
    entry_idx: u64,
    hasher: OpBatchHasher,
    last_hash: Hash256,
}

pub fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: Option<&EntryCipher>,
    seed: &Hash256,
    prev_chain_hash: &Hash256,
) -> Result<(OpaqueLogEntry<'a>, Option<Hash256>), CipherError> {
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    entry_idx,
                    last_hash: *prev_chain_hash,
                    hasher: OpBatchHasher::new(*seed, entry_idx, op_batch.ops.len()),
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
        Ok((
            e2,
            maybe_hash_state.map(|st| {
                let leaf_hash = st.hasher.finalize();
                let mut chain_hasher = Hash256Suite::Sha256
                    .new_tagged_hasher(crate::crypto::TaggedHashDomain::ChainHash);
                chain_hasher.update(prev_chain_hash);
                chain_hasher.update(&leaf_hash);
                chain_hasher.finalize()
            }),
        ))
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

pub fn to_plaintext<'a>(
    entry: &OpaqueLogEntry<'a>,
    cipher: Option<&EntryCipher>,
    seed: &Hash256,
    last_entry_hash: &Hash256,
) -> Result<(PlaintextLogEntry<'a>, Option<Hash256>), CipherError> {
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    entry_idx,
                    last_hash: *last_entry_hash,
                    hasher: OpBatchHasher::new(*seed, entry_idx, op_batch.ops.len()),
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

#[derive(Error, Debug)]
pub enum SegmentCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),

    /// When processing a well-defined "segment", it is an error for the cipher key or suite to change
    /// mid-segment or to transition from an unencrypted to encrypted segment.
    /// A plaintext segment should be batch encryptable with a single cipher suite.
    #[error("cipher changed to {0:?} mid-segment")]
    CipherChanged(Key256Fingerprint),

    #[error("MMR update error: {0}")]
    MmrUpdateError(#[from] MmrUpdateError),
}

pub fn segment_to_opaque<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
    seed: Hash256,
    last_entry_hash: &Hash256,
) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
    scan_only_entries(segment_to_opaque_and_hashes(
        cipher,
        entries,
        seed,
        last_entry_hash,
    ))
}

pub fn segment_to_opaque_with_mmr<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
    last_entry_hash: &Hash256,
    mmr: &mut MmrAccumulator,
) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
    scan_with_mmr(
        segment_to_opaque_and_hashes(cipher, entries, *mmr.seed(), last_entry_hash),
        mmr,
    )
}

fn segment_to_opaque_and_hashes<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
    seed: Hash256,
    last_entry_hash: &Hash256,
) -> impl Iterator<Item = Result<(OpaqueLogEntry<'a>, Option<Hash256>), SegmentCipherError>> {
    let mut last_entry_hash = *last_entry_hash;
    entries.map(move |e| {
        check_use_key(cipher, &e)?;
        let (e2, maybe_hash) = to_opaque(&e, cipher, &seed, &last_entry_hash)?;
        let maybe_hash = maybe_hash.or_else(|| e2.hash(&seed)); // get the hash for UseKey entries too
        if let Some(hash) = maybe_hash {
            last_entry_hash = hash;
        }
        Ok((e2, maybe_hash))
    })
}

pub fn segment_to_plaintext<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
    seed: Hash256,
    last_entry_hash: &Hash256,
) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
    scan_only_entries(segment_to_plaintext_and_hashes(
        cipher,
        entries,
        seed,
        last_entry_hash,
    ))
}

pub fn segment_to_plaintext_with_mmr<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
    last_entry_hash: &Hash256,
    mmr: &mut MmrAccumulator,
) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
    scan_with_mmr(
        segment_to_plaintext_and_hashes(cipher, entries, *mmr.seed(), last_entry_hash),
        mmr,
    )
}

fn segment_to_plaintext_and_hashes<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
    seed: Hash256,
    last_entry_hash: &Hash256,
) -> impl Iterator<Item = Result<(PlaintextLogEntry<'a>, Option<Hash256>), SegmentCipherError>> {
    let mut last_entry_hash = *last_entry_hash;
    entries.map(move |e| {
        check_use_key(cipher, &e)?;
        let (e2, maybe_hash) = to_plaintext(&e, cipher, &seed, &last_entry_hash)?;
        let maybe_hash = maybe_hash.or_else(|| e.hash(&seed)); // get the hash for UseKey entries too
        if let Some(hash) = maybe_hash {
            last_entry_hash = hash;
        }
        Ok((e2, maybe_hash))
    })
}

fn scan_only_entries<E>(
    it: impl Iterator<Item = Result<(E, Option<Hash256>), SegmentCipherError>>,
) -> impl Iterator<Item = Result<E, SegmentCipherError>> {
    it.scan(false, |failed, r| {
        if *failed {
            return None;
        }
        match r {
            Ok((e2, _)) => Some(Ok(e2)),
            Err(e) => {
                *failed = true;
                Some(Err(e))
            }
        }
    })
}

fn scan_with_mmr<O: std::fmt::Debug, H: std::fmt::Debug>(
    it: impl Iterator<Item = Result<(GenericLogEntry<O, H>, Option<Hash256>), SegmentCipherError>>,
    mmr: &mut MmrAccumulator,
) -> impl Iterator<Item = Result<GenericLogEntry<O, H>, SegmentCipherError>> {
    it.scan(false, move |failed, r| {
        if *failed {
            return None;
        }
        match r {
            Ok((e2, maybe_hash)) => match mmr_append_entry_hashed(mmr, &e2, maybe_hash) {
                Ok(_) => Some(Ok(e2)),
                Err(e) => {
                    *failed = true;
                    Some(Err(SegmentCipherError::MmrUpdateError(e)))
                }
            },
            Err(e) => {
                *failed = true;
                Some(Err(e))
            }
        }
    })
}

fn check_use_key<E: std::fmt::Debug, H: std::fmt::Debug>(
    cipher: Option<&EntryCipher>,
    e: &GenericLogEntry<E, H>,
) -> Result<(), SegmentCipherError> {
    let GenericLogEntry::IndexedEntry {
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
