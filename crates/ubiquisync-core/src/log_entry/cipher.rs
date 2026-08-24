use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    crypto::{CipherError, EntryCipher, Key256Fingerprint},
    log_entry::{OpaqueBytes, OpaqueLogEntry, PlaintextBytes, PlaintextLogEntry},
};

pub fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: Option<&EntryCipher>,
) -> Result<OpaqueLogEntry<'a>, CipherError> {
    if let Some(cipher) = cipher {
        entry.transform(
            |entry_idx, h| Ok(cipher.encrypt_header(entry_idx, h.borrow())?.into()),
            |entry_idx, op_idx, op, _| {
                Ok(cipher.encrypt_op(entry_idx, op_idx, op.borrow())?.into())
            },
        )
    } else {
        entry.transform(
            |_, h| Ok(OpaqueBytes(h.0.clone())),
            |_, _, op, _| Ok(OpaqueBytes(op.0.clone())),
        )
    }
}

pub fn to_plaintext<'a>(
    entry: &OpaqueLogEntry<'a>,
    cipher: Option<&EntryCipher>,
) -> Result<PlaintextLogEntry<'a>, CipherError> {
    if let Some(cipher) = cipher {
        entry.transform(
            |entry_idx, h| Ok(cipher.decrypt_header(entry_idx, h.borrow())?.into()),
            |entry_idx, op_idx, op, _| {
                Ok(cipher.decrypt_op(entry_idx, op_idx, op.borrow())?.into())
            },
        )
    } else {
        entry.transform(
            |_, h| Ok(PlaintextBytes(h.0.clone())),
            |_, _, op, _| Ok(PlaintextBytes(op.0.clone())),
        )
    }
}

#[derive(Error, Debug)]
pub enum EntryCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),
    #[error("cipher changed to {0:?} mid-segment")]
    CipherChanged(Key256Fingerprint),
}

pub fn segment_to_opaque<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, EntryCipherError>> {
    entries.map(move |e| {
        let e2 = to_opaque(&e, cipher)?;
        check_use_key(cipher, &e2)?;
        Ok(e2)
    })
}

pub fn segment_to_plaintext<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, EntryCipherError>> {
    entries.map(move |e| {
        check_use_key(cipher, &e)?;
        let e2 = to_plaintext(&e, cipher)?;
        Ok(e2)
    })
}

fn check_use_key(cipher: Option<&EntryCipher>, e: &OpaqueLogEntry) -> Result<(), EntryCipherError> {
    match e {
        super::GenericLogEntry::IndexedEntry { entry, .. } => match entry {
            super::EntryBody::UseKey(cipher_info) => {
                if let Some(cipher) = cipher {
                    if &cipher_info.fingerprint != cipher.key_fingerprint() {
                        return Err(EntryCipherError::CipherChanged(cipher_info.fingerprint));
                    }
                } else {
                    return Err(EntryCipherError::CipherChanged(cipher_info.fingerprint));
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}
