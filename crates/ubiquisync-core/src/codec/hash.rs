use thiserror::Error;

use crate::{
    codec::consts::{ENTRY_TYPE_OP_BATCH, ENTRY_TYPE_USE_KEY},
    crypto::{CipherError, EntryCipher, Hash},
    log_entry::{OpBatch, OpaqueBytes},
};

struct OpaqueOpBatchHasher {
    hasher: blake3::Hasher,
    entry_index: u64,
    next_op_index: u64,
    num_ops: u64,
}

struct PlaintextOpBatchHasher<'a> {
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
    #[error("chiper error {0}")]
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

    pub fn append_opaque_op(&mut self, opaque_op_bytes: &[u8]) -> Result<(), EntryHashError> {
        let op_hash = blake3::derive_key(DOMAIN_SLOT_HASH, opaque_op_bytes);
        self.append_op_hash(&op_hash)
    }

    pub fn append_expunged_op(&mut self, op_hash: &[u8; 32]) -> Result<(), EntryHashError> {
        self.append_op_hash(op_hash)
    }

    fn append_op_hash(&mut self, op_hash: &[u8; 32]) -> Result<(), EntryHashError> {
        if self.next_op_index >= self.num_ops {
            return Err(EntryHashError::OutOfRangeOp);
        }
        self.next_op_index += 1;
        self.hasher.update(op_hash);
        Ok(())
    }

    pub fn finalize(self) -> Result<blake3::Hash, EntryHashError> {
        if self.next_op_index != self.num_ops {
            return Err(EntryHashError::OpCountMismatch);
        }
        Ok(self.hasher.finalize())
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

    pub fn append_op(&mut self, canonical_bytes: &[u8]) -> Result<(), EntryHashError> {
        if let Some(cipher) = self.cipher {
            let bytes = cipher.encrypt_op(
                self.hasher.entry_index,
                self.hasher.next_op_index,
                canonical_bytes,
            )?;
            self.hasher.append_opaque_op(bytes.as_slice())
        } else {
            self.hasher.append_opaque_op(canonical_bytes)
        }
    }

    pub fn append_expunged_op(&mut self, hash: &Hash) -> Result<(), EntryHashError> {
        self.hasher.append_expunged_op(hash)
    }

    pub fn finalize(self) -> Result<blake3::Hash, EntryHashError> {
        self.hasher.finalize()
    }
}

pub trait OpBatchHashMethod {
    type Bytes<'a>;

    fn hash(&self, entry_idx: u64, op_batch: OpBatch<Self::Bytes<'_>, Self::Bytes<'_>>) -> Hash;
}

pub struct OpaqueOpBatchHashMethod;

impl OpBatchHashMethod for OpaqueOpBatchHashMethod {
    type Bytes<'a> = OpaqueBytes<'a>;

    fn hash(&self, entry_idx: u64, op_batch: OpBatch<Self::Bytes<'_>, Self::Bytes<'_>>) -> Hash {
        todo!()
    }
}

pub fn hash_use_key(entry_index: u64, fingerprint: &Hash) -> Hash {
    let mut hasher = blake3::Hasher::new_derive_key(DOMAIN_ENTRY_HASH);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[ENTRY_TYPE_USE_KEY]);
    hasher.update(fingerprint);
    hasher.finalize().into()
}
