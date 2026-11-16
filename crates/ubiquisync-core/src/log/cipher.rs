use std::{borrow::Borrow, convert::Into};

use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, OpaqueBytes, PlaintextBytes},
    crypto::{CipherError, CipherInfo, CipherKeyResolveError, CipherKeyResolver, EntryCipher},
    hlc::Timestamp,
    log::{
        ChainHash, ChainHashError, LogHashContext, LogValidationError, OpEntry, OpaqueLogEntry,
        PlaintextLogEntry, segment::ChainMeta,
    },
};

#[derive(Error, Debug, Clone)]
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
) -> Vec<Result<(OpaqueLogEntry<'a>, ChainMeta), SegmentCipherError>> {
    let mut head_chain = *head_chain;
    let mut entry_cipher = if let Some(ci) = head_cipher {
        Some(
            match EntryCipher::resolve(ci, seed.log_id(), key_resolver).await {
                Ok(c) => c,
                Err(e) => return vec![Err(e.into())],
            },
        )
    } else {
        None
    };
    let mut res = vec![];
    let mut next = async |e| {
        let e2 = to_opaque(e, &entry_cipher, &head_chain)?;
        head_chain = head_chain.next(&e2, seed, head_cipher)?;
        check_cipher_change(head_cipher, &mut entry_cipher, seed, key_resolver).await?;
        Ok((
            e2,
            ChainMeta {
                chain_hash: head_chain,
                cipher_info: *head_cipher,
            },
        ))
    };
    for e in entries {
        if res.push_mut(next(e).await).is_err() {
            break;
        }
    }
    res
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
                let timestamp = slot_cipher.encrypt_slot(&PlaintextBytes::from(
                    &Into::<u64>::into(*timestamp).to_le_bytes()[..],
                ));
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
                    timestamp: Into::<u64>::into(*timestamp).to_le_bytes().to_vec().into(),
                    server_attested_user_id: OpaqueBytes(server_attested_user_id.0.clone()),
                    op: OpaqueBytes(op.0.clone()),
                })
            },
        )?)
    }
}

pub(crate) fn to_plaintext<'a>(
    entry: &OpaqueLogEntry<'a>,
    cipher: &Option<EntryCipher>,
    prev_chain: &ChainHash,
) -> Result<PlaintextLogEntry<'a>, SegmentCipherError> {
    let e: Result<PlaintextLogEntry<'a>, SegmentCipherError> = if let Some(cipher) = cipher {
        entry.transform(|opaque| {
            let mut slot_cipher = cipher.slot_cipher(prev_chain);
            let timestamp: PlaintextBytes<'_> = slot_cipher.decrypt_slot(&opaque.timestamp);
            let timestamp = parse_timestamp(Borrow::<[u8]>::borrow(&timestamp))?;
            slot_cipher.add_context(opaque.timestamp.borrow());
            let server_attested_user_id = if !opaque.server_attested_user_id.is_empty() {
                slot_cipher.decrypt_slot(&opaque.server_attested_user_id)
            } else {
                PlaintextBytes::default()
            };
            slot_cipher.add_context(opaque.server_attested_user_id.borrow());
            let op = slot_cipher.decrypt_slot(&opaque.op);
            Ok(OpEntry {
                timestamp,
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
                let timestamp = parse_timestamp(Borrow::<[u8]>::borrow(timestamp))?;
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

fn parse_timestamp(buf: &[u8]) -> Result<Timestamp, SegmentCipherError> {
    TryInto::<[u8; 8]>::try_into(buf)
        .ok()
        .and_then(|b| TryInto::<Timestamp>::try_into(u64::from_le_bytes(b)).ok())
        .ok_or(SegmentCipherError::InvalidTimestamp)
}

pub(crate) async fn check_cipher_change(
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
