use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, OpaqueBytes, PlaintextBytes},
    crypto::{
        CipherError, CipherInfo, CipherKeyResolveError, CipherKeyResolver, EntryCipher, Hash256,
        SlotCipher,
    },
    hlc::Timestamp,
    log::{ChainHash, ChainHashError, ChainSeed, OpEntry, OpaqueLogEntry, PlaintextLogEntry},
};

#[derive(Error, Debug)]
pub enum SegmentCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),

    #[error("chain update error: {0}")]
    ChainHashError(#[from] ChainHashError),

    #[error("invalid timestamp")]
    InvalidTimestamp,

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
        let e2 = to_opaque(e, &entry_cipher, &head_chain)?;
        head_chain = head_chain.next(&e2, seed, head_cipher)?;
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
        let e2 = to_plaintext(e, &entry_cipher, &head_chain)?;
        head_chain = head_chain.next(e, seed, head_cipher)?;
        check_cipher_change(head_cipher, &mut entry_cipher, seed, key_resolver).await?;
        res.push((e2, head_chain));
    }
    Ok(res)
}

struct OpBatchHashState {
    slot_cipher: SlotCipher,
    last_hash: Hash256,
}

fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: &Option<EntryCipher>,
    prev_chain: &ChainHash,
) -> Result<OpaqueLogEntry<'a>, CipherError> {
    let entry_index = prev_chain.size;
    if let Some(cipher) = cipher {
        entry.transform(
            |OpEntry {
                 timestamp,
                 server_attested_user_id,
                 op,
             }| {
                let mut slot_cipher = cipher.slot_cipher(entry_index);
                let timestamp = slot_cipher.encrypt_slot(
                    &prev_chain.hash,
                    &PlaintextBytes::from(&timestamp.raw().to_le_bytes()[..]),
                );
                let server_attested_user_id = if !server_attested_user_id.is_empty() {
                    slot_cipher.encrypt_slot(&prev_chain.hash, server_attested_user_id.borrow())
                } else {
                    OpaqueBytes::default()
                };
                let op = slot_cipher.encrypt_slot(&prev_chain.hash, op.borrow());
                Ok(OpEntry {
                    timestamp,
                    server_attested_user_id,
                    op,
                })
            },
        )
    } else {
        Ok(entry.transform(
            |OpEntry {
                 timestamp,
                 server_attested_user_id,
                 op,
             }| {
                Ok(OpEntry {
                    timestamp: timestamp.raw().to_le_bytes().to_vec().into(),
                    server_attested_user_id: OpaqueBytes(server_attested_user_id.0.clone()),
                    op: OpaqueBytes(op.0.clone()),
                })
            },
        )?)
    }
}

fn to_plaintext<'a>(
    entry: &OpaqueLogEntry<'a>,
    cipher: &Option<EntryCipher>,
    prev_chain: &ChainHash,
) -> Result<PlaintextLogEntry<'a>, SegmentCipherError> {
    let entry_index = prev_chain.size;
    if let Some(cipher) = cipher {
        entry.transform(
            |OpEntry {
                 timestamp,
                 server_attested_user_id,
                 op,
             }| {
                let mut slot_cipher = cipher.slot_cipher(entry_index);
                let timestamp: PlaintextBytes<'_> =
                    slot_cipher.decrypt_slot(&prev_chain.hash, &timestamp);
                let timestamp: u64 = u64::from_le_bytes(
                    Borrow::<[u8]>::borrow(&timestamp)
                        .try_into()
                        .map_err(|_| SegmentCipherError::InvalidTimestamp)?,
                );
                let server_attested_user_id = if !server_attested_user_id.is_empty() {
                    slot_cipher.decrypt_slot(&prev_chain.hash, server_attested_user_id.borrow())
                } else {
                    PlaintextBytes::default()
                };
                let op = slot_cipher.decrypt_slot(&prev_chain.hash, op.borrow());
                Ok(OpEntry {
                    timestamp: Timestamp::from_raw(timestamp),
                    server_attested_user_id,
                    op,
                })
            },
        )
    } else {
        entry.transform(
            |OpEntry {
                 timestamp,
                 server_attested_user_id,
                 op,
             }| {
                let timestamp = Timestamp::from_raw(u64::from_le_bytes(
                    Borrow::<[u8]>::borrow(timestamp)
                        .try_into()
                        .map_err(|_| SegmentCipherError::InvalidTimestamp)?,
                ));
                Ok(OpEntry {
                    timestamp,
                    server_attested_user_id: PlaintextBytes(server_attested_user_id.0.clone()),
                    op: PlaintextBytes(op.0.clone()),
                })
            },
        )
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
