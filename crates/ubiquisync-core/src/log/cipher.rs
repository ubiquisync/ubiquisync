use thiserror::Error;

use crate::{
    bytes::{OpaqueBytes, PlaintextBytes},
    crypto::{
        CipherError, CipherInfo, CipherKeyResolveError, CipherKeyResolver, EntryCipher, Hash256,
        SlotCipher,
    },
    log::{
        ChainHash, ChainHashError, ChainSeed, LogValidationError, OpBatchHasher, OpaqueLogEntry,
        PlaintextLogEntry,
    },
};

#[derive(Error, Debug)]
pub enum SegmentCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),

    #[error("chain update error: {0}")]
    ChainHashError(#[from] ChainHashError),

    #[error("log validation error: {0}")]
    LogValidation(#[from] LogValidationError),

    #[error("key resolve error: {0}")]
    KeyResolve(#[from] CipherKeyResolveError),
}

/// Head cipher is the cipher at the start of the segment.
/// It is unnecessary to pass this if the segment starts with UseKey
/// and doing so will result in unnecessarily resolving the head cipher key.
pub async fn entries_to_opaque<'a: 'b, 'b>(
    seed: &ChainSeed,
    head_cipher: &mut Option<CipherInfo>,
    head_chain: &ChainHash,
    key_resolver: &dyn CipherKeyResolver,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
) -> Result<Vec<(OpaqueLogEntry<'a>, ChainHash)>, SegmentCipherError> {
    let mut head_chain = *head_chain;
    let mut entry_cipher = if let Some(ci) = head_cipher {
        Some(EntryCipher::resolve(ci, seed.log_id(), key_resolver).await?)
    } else {
        None
    };
    let mut res = vec![];
    for e in entries {
        let (e2, maybe_hash) = to_opaque(e, &entry_cipher, seed, &head_chain)?;
        head_chain = head_chain.next(&e2, maybe_hash, seed, head_cipher)?;
        check_cipher_change(head_cipher, &mut entry_cipher, seed, key_resolver).await?;
        res.push((e2, head_chain));
    }
    Ok(res)
}

/// Head cipher is the cipher at the start of the segment.
/// It is unnecessary to pass this if the segment starts with UseKey
/// and doing so will result in unnecessarily resolving the head cipher key.
pub async fn entries_to_plaintext<'a: 'b, 'b>(
    seed: &ChainSeed,
    head_cipher: &mut Option<CipherInfo>,
    head_chain: &ChainHash,
    key_resolver: &dyn CipherKeyResolver,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
) -> Result<Vec<(PlaintextLogEntry<'a>, ChainHash)>, SegmentCipherError> {
    let mut head_chain = *head_chain;
    let mut entry_cipher = if let Some(ci) = head_cipher {
        Some(EntryCipher::resolve(ci, seed.log_id(), key_resolver).await?)
    } else {
        None
    };
    let mut res = vec![];
    for e in entries {
        let (e2, maybe_hash) = to_plaintext(e, &entry_cipher, seed, &head_chain)?;
        head_chain = head_chain.next(e, maybe_hash, seed, head_cipher)?;
        check_cipher_change(head_cipher, &mut entry_cipher, seed, key_resolver).await?;
        res.push((e2, head_chain));
    }
    Ok(res)
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
    let entry_index = prev_chain.size;
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry.transform(
            entry_index,
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
            entry_index,
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
) -> Result<(PlaintextLogEntry<'a>, Option<Hash256>), SegmentCipherError> {
    let entry_index = prev_chain.size;
    if let Some(cipher) = cipher {
        let (e2, maybe_hash_state) = entry
            .transform(
                entry_index,
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
            )
            .map_err(SegmentCipherError::CipherError)?;
        e2.validate()?;
        Ok((e2, maybe_hash_state.map(|st| st.hasher.finalize())))
    } else {
        let (e2, _) = entry
            .transform(
                entry_index,
                |_, _| Ok(()),
                |s, _| Ok(PlaintextBytes(s.0.clone())),
                |_, _| Ok(()),
            )
            .map_err(SegmentCipherError::CipherError)?;
        e2.validate()?;
        Ok((e2, None))
    }
}

async fn check_cipher_change(
    head_ci: &Option<CipherInfo>,
    cipher: &mut Option<EntryCipher>,
    seed: &ChainSeed,
    key_resolver: &dyn CipherKeyResolver,
) -> Result<(), SegmentCipherError> {
    if let Some(ci) = head_ci {
        if let Some(cipher) = cipher
            && cipher.cipher_info() == *ci
        {
            return Ok(());
        }

        *cipher = Some(EntryCipher::resolve(ci, seed.log_id(), key_resolver).await?);
    }
    Ok(())
}
