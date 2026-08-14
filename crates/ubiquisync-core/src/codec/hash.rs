use std::borrow::{Borrow, Cow};

use thiserror::Error;

use crate::{
    codec::consts::{ENTRY_TYPE_OP_BATCH, ENTRY_TYPE_USE_KEY},
    crypto::{CipherError, CipherSuite, EntryCipher, Hash},
    log_entry::{OpBatch, OpOrExpunge, OpaqueBytes, OpaqueOpBatch, PlaintextBytes},
};

pub struct OpaqueOpBatchHasher {
    hasher: blake3::Hasher,
    entry_index: u64,
    next_op_index: u64,
    num_ops: u64,
}

pub struct PlaintextOpBatchHasher<'a> {
    hasher: OpaqueOpBatchHasher,
    cipher: &'a Option<EntryCipher>,
}

const DOMAIN_ENTRY_HASH: &str = "ubiquisync/v1/entry-hash";
const DOMAIN_SLOT_HASH: &str = "ubiquisync/v1/slot-hash";

#[derive(Error, Debug)]
pub enum EntryHashError {
    #[error("op count range exceeded")]
    OutOfRangeOp,
    #[error("op count mismatch")]
    OpCountMismatch,
    #[error("cipher error {0}")]
    CipherError(#[from] CipherError),
}

impl OpaqueOpBatchHasher {
    pub fn new(opaque_header_bytes: &[u8], entry_index: u64, num_ops: u64) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key(DOMAIN_ENTRY_HASH);
        hasher.update(&entry_index.to_le_bytes());
        hasher.update(&[ENTRY_TYPE_OP_BATCH]);
        let header_hash = blake3::derive_key(DOMAIN_SLOT_HASH, opaque_header_bytes);
        hasher.update(&header_hash);
        hasher.update(&num_ops.to_le_bytes()[..]);
        Self {
            entry_index,
            hasher,
            next_op_index: 0,
            num_ops,
        }
    }

    fn append_op_hash(&mut self, op_hash: &[u8; 32]) -> Result<(), EntryHashError> {
        if self.next_op_index >= self.num_ops {
            return Err(EntryHashError::OutOfRangeOp);
        }
        self.next_op_index += 1;
        self.hasher.update(op_hash);
        Ok(())
    }
}

impl OpaqueOpBatchHasher {
    fn append_op_bytes<'b>(
        &mut self,
        opaque_op_bytes: &'b OpaqueBytes<'_>,
    ) -> Result<(), EntryHashError> {
        let op_bytes = opaque_op_bytes.borrow();
        let op_hash = blake3::derive_key(DOMAIN_SLOT_HASH, op_bytes);
        self.append_op_hash(&op_hash)
    }

    fn append_expunged_op(&mut self, op_hash: &[u8; 32]) -> Result<(), EntryHashError> {
        self.append_op_hash(op_hash)
    }

    fn finalize(self) -> Result<Hash, EntryHashError> {
        if self.next_op_index != self.num_ops {
            return Err(EntryHashError::OpCountMismatch);
        }
        Ok(self.hasher.finalize().into())
    }
}

impl<'a> PlaintextOpBatchHasher<'a> {
    pub fn new<'b>(
        cipher: &'a Option<EntryCipher>,
        plaintext_header_bytes: &'b [u8],
        entry_index: u64,
        num_ops: u64,
    ) -> Result<(Self, OpaqueBytes<'b>), EntryHashError> {
        let (hasher, opaque_header) = if let Some(cipher) = cipher {
            let bytes = cipher.encrypt_header(entry_index, plaintext_header_bytes)?;
            (
                OpaqueOpBatchHasher::new(bytes.as_slice(), entry_index, num_ops),
                Cow::Owned(bytes),
            )
        } else {
            (
                OpaqueOpBatchHasher::new(plaintext_header_bytes, entry_index, num_ops),
                Cow::Borrowed(plaintext_header_bytes),
            )
        };
        Ok((Self { cipher, hasher }, OpaqueBytes(opaque_header)))
    }
}

impl<'a> PlaintextOpBatchHasher<'a> {
    fn append_op_bytes<'b>(
        &mut self,
        canonical_bytes: &'b PlaintextBytes<'_>,
    ) -> Result<OpaqueBytes<'b>, EntryHashError> {
        if let Some(cipher) = self.cipher {
            let bytes = cipher.encrypt_op(
                self.hasher.entry_index,
                self.hasher.next_op_index,
                &canonical_bytes.0,
            )?;
            self.hasher
                .append_op_bytes(&OpaqueBytes(Cow::Borrowed(bytes.as_slice())))?;
            Ok(OpaqueBytes(Cow::Owned(bytes)))
        } else {
            let bytes = canonical_bytes.borrow();
            self.hasher
                .append_op_bytes(&OpaqueBytes(Cow::Borrowed(bytes)))?;
            Ok(OpaqueBytes(Cow::Borrowed(bytes)))
        }
    }

    fn append_expunged_op(&mut self, hash: &Hash) -> Result<(), EntryHashError> {
        self.hasher.append_expunged_op(hash)
    }

    fn finalize(self) -> Result<Hash, EntryHashError> {
        Ok(self.hasher.finalize()?.into())
    }
}

pub trait OpBatchHashMethod<B: std::fmt::Debug> {
    fn hash<'b>(
        &self,
        entry_idx: u64,
        op_batch: &'b OpBatch<B, B>,
    ) -> Result<(Hash, Cow<'b, OpaqueOpBatch<'b>>), EntryHashError>;
}

pub struct OpaqueOpBatchHashMethod;

impl OpBatchHashMethod<OpaqueBytes<'_>> for OpaqueOpBatchHashMethod {
    fn hash<'b>(
        &self,
        entry_idx: u64,
        op_batch: &'b OpBatch<OpaqueBytes<'_>, OpaqueBytes<'_>>,
    ) -> Result<(Hash, Cow<'b, OpaqueOpBatch<'b>>), EntryHashError> {
        let mut hasher = OpaqueOpBatchHasher::new(
            op_batch.header.borrow(),
            entry_idx,
            op_batch.ops.len() as u64,
        );
        for op in op_batch.ops.iter() {
            match op {
                crate::log_entry::OpOrExpunge::Op(bytes) => {
                    hasher.append_op_bytes(bytes)?;
                }
                crate::log_entry::OpOrExpunge::Expunge(hash) => hasher.append_expunged_op(hash)?,
            }
        }
        let hash = hasher.finalize()?;
        Ok((hash, Cow::Borrowed(op_batch)))
    }
}

pub struct PlaintextOpBatchHashMethod<'a>(pub &'a Option<EntryCipher>);

impl<'a> OpBatchHashMethod<PlaintextBytes<'_>> for PlaintextOpBatchHashMethod<'a> {
    fn hash<'b>(
        &self,
        entry_idx: u64,
        op_batch: &'b OpBatch<PlaintextBytes<'_>, PlaintextBytes<'_>>,
    ) -> Result<(Hash, Cow<'b, OpaqueOpBatch<'b>>), EntryHashError> {
        let (mut hasher, opaque_header_bytes) = PlaintextOpBatchHasher::new(
            self.0,
            op_batch.header.borrow(),
            entry_idx,
            op_batch.ops.len() as u64,
        )?;
        let mut opaque_ops = vec![];
        for op in op_batch.ops.iter() {
            match op {
                crate::log_entry::OpOrExpunge::Op(bytes) => {
                    let opaque_bytes = hasher.append_op_bytes(bytes)?;
                    opaque_ops.push(OpOrExpunge::Op(opaque_bytes));
                }
                crate::log_entry::OpOrExpunge::Expunge(hash) => {
                    hasher.append_expunged_op(hash)?;
                    opaque_ops.push(OpOrExpunge::Expunge(*hash));
                }
            }
        }
        let hash = hasher.finalize()?;
        let opaque_batch = OpBatch {
            header: opaque_header_bytes,
            ops: opaque_ops,
        };
        Ok((hash, Cow::Owned(opaque_batch)))
    }
}

pub fn hash_use_key(entry_index: u64, cipher_suite: CipherSuite, fingerprint: &Hash) -> Hash {
    let mut hasher = blake3::Hasher::new_derive_key(DOMAIN_ENTRY_HASH);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[ENTRY_TYPE_USE_KEY]);
    hasher.update(&[cipher_suite.into()]);
    hasher.update(fingerprint);
    hasher.finalize().into()
}
