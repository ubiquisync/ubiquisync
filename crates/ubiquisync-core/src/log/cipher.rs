use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, OpaqueBytes, PlaintextBytes},
    crypto::{CipherError, CipherInfo, CipherKeyResolveError, CipherKeyResolver, EntryCipher},
    hlc::Timestamp,
    log::{
        ChainHash, ChainHashError, LogHashContext, LogValidationError, OpEntry, OpaqueLogEntry,
        PlaintextLogEntry,
    },
};

#[derive(Error, Debug)]
pub enum SegmentCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),

    #[error("chain update error: {0}")]
    ChainHashError(#[from] ChainHashError),

    #[error("key resolve error: {0}")]
    KeyResolve(#[from] CipherKeyResolveError),

    #[error("invalid timestamp")]
    InvalidTimestamp,

    #[error("log validation failed: {0}")]
    Validation(#[from] LogValidationError),
}

/// Head cipher is the cipher at the start of the segment.
/// It is unnecessary to pass this if the segment starts with UseKey
/// and doing so will result in unnecessarily resolving the head cipher key.
pub async fn entries_to_opaque<'a: 'b, 'b>(
    seed: &LogHashContext,
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
    seed: &LogHashContext,
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

fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: &Option<EntryCipher>,
    prev_chain: &ChainHash,
) -> Result<OpaqueLogEntry<'a>, CipherError> {
    if let Some(cipher) = cipher {
        entry.transform(
            |OpEntry {
                 timestamp,
                 server_attested_user_id,
                 op,
             }| {
                let mut slot_cipher = cipher.slot_cipher(prev_chain);
                let timestamp = slot_cipher
                    .encrypt_slot(&PlaintextBytes::from(&timestamp.raw().to_le_bytes()[..]));
                slot_cipher.add_context(timestamp.borrow());
                let server_attested_user_id = if !server_attested_user_id.is_empty() {
                    slot_cipher.encrypt_slot(server_attested_user_id)
                } else {
                    Default::default()
                };
                slot_cipher.add_context(server_attested_user_id.borrow());
                let op = slot_cipher.encrypt_slot(op);
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
    let e: Result<PlaintextLogEntry<'a>, SegmentCipherError> = if let Some(cipher) = cipher {
        entry.transform(|opaque| {
            let mut slot_cipher = cipher.slot_cipher(prev_chain);
            let timestamp: PlaintextBytes<'_> = slot_cipher.decrypt_slot(&opaque.timestamp);
            let timestamp: u64 = u64::from_le_bytes(
                Borrow::<[u8]>::borrow(&timestamp)
                    .try_into()
                    .map_err(|_| SegmentCipherError::InvalidTimestamp)?,
            );
            slot_cipher.add_context(opaque.timestamp.borrow());
            let server_attested_user_id = if !opaque.server_attested_user_id.is_empty() {
                slot_cipher.decrypt_slot(&opaque.server_attested_user_id)
            } else {
                PlaintextBytes::default()
            };
            slot_cipher.add_context(opaque.server_attested_user_id.borrow());
            let op = slot_cipher.decrypt_slot(&opaque.op);
            Ok(OpEntry {
                timestamp: Timestamp::from_raw(timestamp),
                server_attested_user_id,
                op,
            })
        })
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
    };
    let e = e?;
    e.validate()?;
    Ok(e)
}

async fn check_cipher_change(
    head_ci: &Option<CipherInfo>,
    cipher: &mut Option<EntryCipher>,
    seed: &LogHashContext,
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
