use std::borrow::Cow;

use thiserror::Error;

use crate::{
    codec::consts::{ENTRY_TYPE_OP_BATCH, ENTRY_TYPE_USE_KEY},
    crypto::{CipherError, EntryCipher, Hash},
    log_entry::{OpBatch, OpaqueBytes, PlaintextBytes},
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

impl OpBatchHasher<OpaqueBytes<'_>> for OpaqueOpBatchHasher {
    fn append_op_bytes(&mut self, opaque_op_bytes: &OpaqueBytes<'_>) -> Result<(), EntryHashError> {
        let op_hash = blake3::derive_key(DOMAIN_SLOT_HASH, &opaque_op_bytes.0);
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
    pub fn new(
        cipher: &'a Option<EntryCipher>,
        plaintext_header_bytes: &[u8],
        entry_index: u64,
        num_ops: u64,
    ) -> Result<Self, EntryHashError> {
        let hasher = if let Some(cipher) = cipher {
            let bytes = cipher.encrypt_header(entry_index, plaintext_header_bytes)?;
            OpaqueOpBatchHasher::new(bytes.as_slice(), entry_index, num_ops)
        } else {
            OpaqueOpBatchHasher::new(plaintext_header_bytes, entry_index, num_ops)
        };
        Ok(Self { cipher, hasher })
    }
}

impl<'a> OpBatchHasher<PlaintextBytes<'_>> for PlaintextOpBatchHasher<'a> {
    fn append_op_bytes(
        &mut self,
        canonical_bytes: &PlaintextBytes<'_>,
    ) -> Result<(), EntryHashError> {
        if let Some(cipher) = self.cipher {
            let bytes = cipher.encrypt_op(
                self.hasher.entry_index,
                self.hasher.next_op_index,
                &canonical_bytes.0,
            )?;
            self.hasher
                .append_op_bytes(&OpaqueBytes(Cow::Borrowed(bytes.as_slice())))
        } else {
            self.hasher
                .append_op_bytes(&OpaqueBytes(Cow::Borrowed(&canonical_bytes.0)))
        }
    }

    fn append_expunged_op(&mut self, hash: &Hash) -> Result<(), EntryHashError> {
        self.hasher.append_expunged_op(hash)
    }

    fn finalize(self) -> Result<Hash, EntryHashError> {
        Ok(self.hasher.finalize()?.into())
    }
}

pub trait OpBatchHashMethod<B> {
    type Hasher: OpBatchHasher<B>;

    fn hasher(
        &self,
        entry_idx: u64,
        op_batch: &OpBatch<B, B>,
    ) -> Result<Self::Hasher, EntryHashError>;
}

pub trait OpBatchHasher<B> {
    fn append_op_bytes(&mut self, bytes: &B) -> Result<(), EntryHashError>;
    fn append_expunged_op(&mut self, hash: &Hash) -> Result<(), EntryHashError>;
    fn finalize(self) -> Result<Hash, EntryHashError>;
}

pub struct OpaqueOpBatchHashMethod;

impl OpBatchHashMethod<OpaqueBytes<'_>> for OpaqueOpBatchHashMethod {
    type Hasher = OpaqueOpBatchHasher;

    fn hasher(
        &self,
        entry_idx: u64,
        op_batch: &OpBatch<OpaqueBytes<'_>, OpaqueBytes<'_>>,
    ) -> Result<Self::Hasher, EntryHashError> {
        Ok(OpaqueOpBatchHasher::new(
            &op_batch.header.0,
            entry_idx,
            op_batch.ops.len() as u64,
        ))
    }
}

pub struct PlaintextOpBatchHashMethod<'a>(pub &'a Option<EntryCipher>);

impl<'a> OpBatchHashMethod<PlaintextBytes<'_>> for PlaintextOpBatchHashMethod<'a> {
    type Hasher = PlaintextOpBatchHasher<'a>;

    fn hasher(
        &self,
        entry_idx: u64,
        op_batch: &OpBatch<PlaintextBytes<'_>, PlaintextBytes<'_>>,
    ) -> Result<Self::Hasher, EntryHashError> {
        Ok(PlaintextOpBatchHasher::new(
            self.0,
            &op_batch.header.0,
            entry_idx,
            op_batch.ops.len() as u64,
        )?)
    }
}

pub fn hash_op_batch<B, M: OpBatchHashMethod<B>>(
    entry_idx: u64,
    batch: &OpBatch<B, B>,
    method: &M,
) -> Result<Hash, EntryHashError> {
    let mut hasher = method.hasher(entry_idx, &batch)?;
    for op in batch.ops.iter() {
        match op {
            crate::log_entry::OpOrExpunge::Op(bytes) => hasher.append_op_bytes(bytes)?,
            crate::log_entry::OpOrExpunge::Expunge(hash) => hasher.append_expunged_op(hash)?,
        }
    }
    hasher.finalize()
}

pub fn hash_use_key(entry_index: u64, fingerprint: &Hash) -> Hash {
    let mut hasher = blake3::Hasher::new_derive_key(DOMAIN_ENTRY_HASH);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[ENTRY_TYPE_USE_KEY]);
    hasher.update(fingerprint);
    hasher.finalize().into()
}
