use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    crypto::{CipherError, EntryCipher, Hash256, Key256Fingerprint},
    log_entry::{
        EntryBody, GenericLogEntry, OpaqueBytes, OpaqueLogEntry, PlaintextBytes, PlaintextLogEntry,
    },
};

pub fn to_opaque<'a>(
    entry: &PlaintextLogEntry<'a>,
    cipher: Option<&EntryCipher>,
    last_entry_hash: &Hash256,
) -> Result<OpaqueLogEntry<'a>, CipherError> {
    if let Some(cipher) = cipher {
        entry.transform(
            |entry_idx, h| {
                Ok(cipher
                    .encrypt_header(entry_idx, last_entry_hash, h.borrow())?
                    .into())
            },
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
            // TODO header and op hashes
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
pub enum SegmentCipherError {
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),
    /// When processing a well-defined "segment", it is an error for the cipher key or suite to change
    /// mid-segment or to transition from an unencrypted to encrypted segment.
    /// A plaintext segment should be batch encryptable with a single cipher suite.
    #[error("cipher changed to {0:?} mid-segment")]
    CipherChanged(Key256Fingerprint),
}

pub fn segment_to_opaque<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> impl Iterator<Item = Result<OpaqueLogEntry<'a>, SegmentCipherError>> {
    entries.map(move |e| {
        check_use_key(cipher, &e)?;
        Ok(to_opaque(&e, cipher)?)
    })
}

pub fn segment_to_plaintext<'a>(
    cipher: Option<&EntryCipher>,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
) -> impl Iterator<Item = Result<PlaintextLogEntry<'a>, SegmentCipherError>> {
    entries.map(move |e| {
        check_use_key(cipher, &e)?;
        Ok(to_plaintext(&e, cipher)?)
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
        && &cipher_info.fingerprint == cipher.key_fingerprint()
        && cipher_info.cipher_suite == cipher.cipher_suite().into()
    {
        // only okay if fingerprint and cipher suite match
        return Ok(());
    };
    Err(SegmentCipherError::CipherChanged(cipher_info.fingerprint))
}
