use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    bytes::OpaqueBytes,
    crypto::{CipherInfo, Hash256, Hasher, TaggedHashDomain, new_tagged_hasher},
    ids::LogId,
    log::{EntryBody, LogEntry, OpBatch, OpOrExpunge, OpaqueLogEntry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainHash {
    pub hash: Hash256,
    pub size: u64,
}

#[derive(Error, Debug)]
pub enum ChainHashError {
    #[error("expected entry {size}, got {idx}")]
    OutOfOrderEntry { size: u64, idx: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainSeed(Hash256);

impl ChainHash {
    pub fn empty(seed: &ChainSeed) -> Self {
        Self {
            hash: seed.0,
            size: 0,
        }
    }

    fn add_one(&self, entry_hash: &Hash256) -> Self {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::ChainHash);
        hasher.update(&self.hash);
        hasher.update(entry_hash);
        let hash = hasher.finalize();
        let size = self.size + 1;
        Self { hash, size }
    }

    /// Updates the chain has with the next entry and a possibly already computed hash (from encryption/decryption processing).
    pub(crate) fn next(
        &self,
        entry: &OpaqueLogEntry,
        precomputed_hash: Option<Hash256>,
        seed: &ChainSeed,
    ) -> Result<ChainHash, ChainHashError> {
        match entry {
            LogEntry::IndexedEntry { idx, entry } => {
                let size = self.size;
                if *idx != size {
                    return Err(ChainHashError::OutOfOrderEntry { size, idx: *idx });
                }
                let entry_hash = precomputed_hash.unwrap_or_else(|| entry.hash(seed, *idx));
                Ok(self.add_one(&entry_hash))
            }
            LogEntry::Signature { .. } => Ok(*self),
        }
    }

    pub fn sign_bytes(&self, seed: &ChainSeed) -> Hash256 {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::LogSignBytes);
        hasher.update(&seed.0);
        hasher.update(&self.size.to_le_bytes());
        hasher.update(&self.hash);
        hasher.finalize()
    }
}

impl ChainSeed {
    pub fn new(log_id: &LogId) -> Self {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::ChainSeed);
        hasher.update(&log_id.peer_id.0);
        hasher.update(&log_id.container_id.0);
        let seed = hasher.finalize();
        Self(seed)
    }
}

impl<'a> EntryBody<OpaqueBytes<'a>> {
    pub fn hash(&self, seed: &ChainSeed, entry_index: u64) -> Hash256 {
        match self {
            EntryBody::OpBatch(op_batch) => op_batch.hash(seed, entry_index),
            // TODO should we add LogId coordinates to hash_use_key?
            EntryBody::UseKey(cipher_info) => hash_use_key(entry_index, cipher_info),
            EntryBody::Expunged(hash) => *hash,
        }
    }
}

impl<'a> OpBatch<OpaqueBytes<'a>> {
    pub fn hash(&self, seed: &ChainSeed, entry_idx: u64) -> Hash256 {
        let mut hasher = OpBatchHasher::new(seed, entry_idx, self.ops.len());
        hasher.hash_slot(&self.timestamp);
        if self.server_attested_user_id.0.len() > 0 {
            hasher.hash_slot(&self.server_attested_user_id);
        }
        for e in self.ops.iter() {
            match e {
                OpOrExpunge::Op(e) => {
                    hasher.hash_slot(e);
                }
                OpOrExpunge::Expunge(h) => hasher.hash_expunge(h),
            }
        }
        hasher.finalize()
    }
}

pub(crate) struct OpBatchHasher {
    hasher: Hasher,
    entry_idx: u64,
    seed: ChainSeed,
    slot_idx: u64,
}

impl OpBatchHasher {
    pub(crate) fn new(seed: &ChainSeed, entry_idx: u64, num_ops: usize) -> Self {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::LogEntryOpBatch);
        hasher.update(&seed.0);
        hasher.update(&entry_idx.to_le_bytes());
        let num_ops = num_ops as u64;
        hasher.update(&num_ops.to_le_bytes());
        Self {
            hasher,
            entry_idx,
            seed: *seed,
            slot_idx: 0,
        }
    }

    pub(crate) fn hash_expunge(&mut self, h: &Hash256) {
        self.slot_idx += 1;
        self.hasher.update(h);
    }

    pub(crate) fn hash_slot(&mut self, bytes: &OpaqueBytes) -> Hash256 {
        let mut slot_hasher = new_tagged_hasher(TaggedHashDomain::OpBatchSlot);
        slot_hasher.update(&self.seed.0);
        slot_hasher.update(&self.entry_idx.to_le_bytes());
        slot_hasher.update(&self.slot_idx.to_le_bytes());
        slot_hasher.update(bytes.borrow());
        let h = slot_hasher.finalize();
        self.slot_idx += 1;
        self.hasher.update(&h);
        h
    }

    pub(crate) fn finalize(self) -> Hash256 {
        self.hasher.finalize()
    }
}

// TODO should we anchor this with seed too?
fn hash_use_key(entry_index: u64, cipher_info: &CipherInfo) -> Hash256 {
    let mut hasher = new_tagged_hasher(TaggedHashDomain::LogEntryUseKey);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[cipher_info.cipher_suite]);
    hasher.update(&cipher_info.fingerprint.0);
    hasher.finalize()
}
