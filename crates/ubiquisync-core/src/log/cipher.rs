use thiserror::Error;

use crate::{
    bytes::{OpaqueBytes, PlaintextBytes},
    crypto::{Cipher, CipherError, EntryCipher, Hash256, Key256Fingerprint},
    log::{
        ChainHash, ChainHashError, EntryBody, LogEntry, OpBatchHasher, OpaqueLogEntry,
        PlaintextLogEntry,
    },
};

struct OpBatchHashState {
    entry_idx: u64,
    hasher: OpBatchHasher,
    last_hash: Hash256,
}

fn cipher_entry<I: std::fmt::Debug, O: std::fmt::Debug>(
    entry: &LogEntry<I, I>,
    cipher: Option<&Cipher>,
    chain_hash: &ChainHash,
) -> Result<(LogEntry<O, O>, Option<Hash256>), CipherError>
where
    Cipher: EntryCipher<I, O>,
{
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            |entry_idx, op_batch| {
                Ok(OpBatchHashState {
                    entry_idx,
                    last_hash: chain_hash.hash(),
                    hasher: OpBatchHasher::new(chain_hash.seed(), entry_idx, op_batch.ops.len()),
                })
            },
            |header, st| {
                let header_cipher = cipher.cipher_header(st.entry_idx, &st.last_hash, header)?;
                st.last_hash = st.hasher.hash_header(&header_cipher);
                Ok(header_cipher)
            },
            |op_idx, op, st| {
                let op_cipher = cipher.cipher_op(st.entry_idx, op_idx, &st.last_hash, op)?;
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
    cipher_entry(entry, cipher, chain_hash)
}

fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: Option<&Cipher>,
    chain_hash: &ChainHash,
) -> Result<(OpaqueLogEntry<'a>, Option<Hash256>), CipherError> {
    cipher_entry(entry, cipher, chain_hash)
}

// #[derive(Error, Debug)]
// pub enum SegmentCipherError {
//     #[error("cipher error {0}")]
//     CipherError(#[from] CipherError),

//     /// When processing a well-defined "segment", it is an error for the cipher key or suite to change
//     /// mid-segment or to transition from an unencrypted to encrypted segment.
//     /// A plaintext segment should be batch encryptable with a single cipher suite.
//     #[error("cipher changed to {0:?} mid-segment")]
//     CipherChanged(Key256Fingerprint),

//     #[error("chain update error: {0}")]
//     ChainHashError(#[from] ChainHashError),
// }

// pub fn segment_to_opaque<'a>(
//     cipher: Option<&Cipher>,
//     entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
//     seed: Hash256,
//     last_entry_hash: &Hash256,
// ) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
//     scan_only_entries(segment_to_opaque_and_hashes(
//         cipher,
//         entries,
//         seed,
//         last_entry_hash,
//     ))
// }

// pub fn segment_to_opaque_with_chain_hash<'a>(
//     cipher: Option<&Cipher>,
//     entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
//     chain_hash: &mut ChainHash,
// ) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
//     scan_with_hash(
//         segment_to_opaque_and_hashes(cipher, entries, chain_hash),
//         chain_hash,
//     )
// }

// fn segment_to_opaque_and_hashes<'a>(
//     cipher: Option<&Cipher>,
//     entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
//     chain_hash: &mut ChainHash,
// ) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
//     entries.map(move |e| {
//         check_use_key(cipher, &e)?;
//         let (e2, maybe_hash) = to_opaque(&e, cipher, chain_hash)?;
//         // get a hash whether or not we got one from the converting to opaque
//         let maybe_hash = maybe_hash.or_else(|| e2.hash(chain_hash.seed()));
//         chain_hash.update(&e2, maybe_hash);
//         Ok(e2)
//     })
// }

// pub fn segment_to_plaintext<'a>(
//     cipher: Option<&Cipher>,
//     entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
//     seed: Hash256,
//     last_entry_hash: &Hash256,
// ) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
//     scan_only_entries(segment_to_plaintext_and_hashes(
//         cipher,
//         entries,
//         seed,
//         last_entry_hash,
//     ))
// }

// pub fn segment_to_plaintext_with_mmr<'a>(
//     cipher: Option<&Cipher>,
//     entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
//     last_entry_hash: &Hash256,
//     mmr: &mut MmrAccumulator,
// ) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
//     scan_with_mmr(
//         segment_to_plaintext_and_hashes(cipher, entries, *mmr.seed(), last_entry_hash),
//         mmr,
//     )
// }

// fn segment_to_plaintext_and_hashes<'a>(
//     cipher: Option<&Cipher>,
//     entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
//     chain_hash: &mut ChainHash,
// ) -> impl Iterator<Item = Result<(PlaintextLogEntry<'a>, Option<Hash256>), SegmentCipherError>> {
//     entries.map(move |e| {
//         check_use_key(cipher, &e)?;
//         let (e2, maybe_hash) = to_plaintext(&e, cipher, &chain_hash.hash)?;
//         // get a hash whether or not we got one from the converting to opaque
//         let maybe_hash = maybe_hash.or_else(|| e2.hash(chain_hash.seed()));
//         Ok((e2, maybe_hash))
//     })
// }

// fn scan_only_entries<E>(
//     it: impl Iterator<Item = Result<(E, Option<Hash256>), SegmentCipherError>>,
// ) -> impl Iterator<Item = Result<E, SegmentCipherError>> {
//     it.scan(false, |failed, r| {
//         if *failed {
//             return None;
//         }
//         match r {
//             Ok((e2, _)) => Some(Ok(e2)),
//             Err(e) => {
//                 *failed = true;
//                 Some(Err(e))
//             }
//         }
//     })
// }

// fn scan_with_hash<O: std::fmt::Debug, H: std::fmt::Debug>(
//     it: impl Iterator<Item = Result<(LogEntry<O, H>, Option<Hash256>), SegmentCipherError>>,
//     chain_hash: &mut ChainHash,
// ) -> impl Iterator<Item = Result<LogEntry<O, H>, SegmentCipherError>> {
//     it.scan(false, move |failed, r| {
//         if *failed {
//             return None;
//         }
//         match r {
//             Ok((e2, maybe_hash)) => match chain_hash.update(&e2, maybe_hash) {
//                 Ok(_) => Some(Ok(e2)),
//                 Err(e) => {
//                     *failed = true;
//                     Some(Err(SegmentCipherError::ChainHashError(e)))
//                 }
//             },
//             Err(e) => {
//                 *failed = true;
//                 Some(Err(e))
//             }
//         }
//     })
// }

// fn check_use_key<E: std::fmt::Debug, H: std::fmt::Debug>(
//     cipher: Option<&Cipher>,
//     e: &LogEntry<E, H>,
// ) -> Result<(), SegmentCipherError> {
//     let LogEntry::IndexedEntry {
//         entry: EntryBody::UseKey(cipher_info),
//         ..
//     } = e
//     else {
//         return Ok(());
//     };

//     if let Some(cipher) = cipher
//         && cipher_info == &cipher.cipher_info()
//     {
//         // only okay if fingerprint and cipher suite match
//         return Ok(());
//     };
//     Err(SegmentCipherError::CipherChanged(cipher_info.fingerprint))
// }
