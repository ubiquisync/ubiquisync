use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    bytes::OpaqueBytes,
    crypto::{Hash256, Hasher, TaggedHashDomain, new_tagged_hasher},
    ids::LogId,
    log::{CipherInfo, EntryBody, LogEntry, OpBatch, OpOrExpunge, OpaqueLogEntry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainHash {
    seed: Hash256,
    hash: Hash256,
    size: u64,
}

#[derive(Error, Debug)]
pub enum ChainHashError {
    #[error("expected entry {size}, got {idx}")]
    OutOfOrderEntry { size: u64, idx: u64 },
    #[error("invalid expunge size {expunge_size} at {size}")]
    InvalidExpunge { size: u64, expunge_size: u64 },
}

impl ChainHash {
    pub fn new_seed(log_id: &LogId) -> Self {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::ChainSeed);
        hasher.update(&log_id.peer_id.0);
        hasher.update(&log_id.container_id.0);
        let seed = hasher.finalize();
        Self {
            seed,
            hash: seed,
            size: 0,
        }
    }

    pub fn from_existing(log_id: &LogId, size: u64, hash: Hash256) -> Self {
        let mut res = Self::new_seed(log_id);
        res.size = size;
        res.hash = hash;
        res
    }

    pub fn sign_bytes(&self, log_id: &LogId) -> Hash256 {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::LogSignBytes);
        hasher.update(&log_id.peer_id.0);
        hasher.update(&log_id.container_id.0);
        hasher.update(&self.size.to_le_bytes());
        hasher.update(&self.hash);
        hasher.finalize()
    }

    pub(crate) fn update<O: std::fmt::Debug, H: std::fmt::Debug>(
        &mut self,
        entry: &LogEntry<O, H>,
        maybe_hash: Option<Hash256>,
    ) -> Result<(), ChainHashError> {
        match entry {
            LogEntry::IndexedEntry { idx, entry } => {
                let size = self.size();
                if *idx != size {
                    return Err(ChainHashError::OutOfOrderEntry { size, idx: *idx });
                }
                let entry_hash = maybe_hash
                    .expect("state machine error, caller should have produced a hash here");
                let mut hasher = new_tagged_hasher(TaggedHashDomain::ChainHash);
                hasher.update(self.hash);
                hasher.update(entry_hash);
                self.hash = hasher.finalize()
            }
            LogEntry::Expunged { end_size, end_hash } => {
                let size = self.size;
                if end_size <= size {
                    return Err(ChainHashError::InvalidExpunge {
                        size,
                        expunge_size: end_size,
                    });
                }
                self.size = end_size;
                self.hash = end_hash;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn hash(&self) -> &Hash256 {
        &self.hash
    }

    pub fn seed(&self) -> &Hash256 {
        &self.seed
    }
}

// pub fn update_mmr<'a>(
//     acc: &mut MmrAccumulator,
//     entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
// ) -> Result<(), MmrUpdateError> {
//     for e in entries {
//         mmr_append_entry(acc, &e)?;
//     }
//     Ok(())
// }

// fn mmr_append_entry(
//     acc: &mut MmrAccumulator,
//     entry: &OpaqueLogEntry,
// ) -> Result<(), MmrUpdateError> {
//     mmr_append_entry_hashed(acc, entry, entry.hash(acc.seed()))
// }

impl<'a> OpaqueLogEntry<'a> {
    pub fn hash(&self, seed: &Hash256) -> Option<Hash256> {
        match self {
            LogEntry::IndexedEntry { idx, entry } => Some(entry.hash(seed, *idx)),
            _ => None,
        }
    }
}

impl<'a> EntryBody<OpaqueBytes<'a>, OpaqueBytes<'a>> {
    pub fn hash(&self, seed: &Hash256, entry_index: u64) -> Hash256 {
        match self {
            EntryBody::OpBatch(op_batch) => op_batch.hash(*seed, entry_index),
            // TODO should we add LogId coordinates to hash_use_key?
            EntryBody::UseKey(cipher_info) => hash_use_key(entry_index, cipher_info),
        }
    }
}

impl<'a> OpBatch<OpaqueBytes<'a>, OpaqueBytes<'a>> {
    pub fn hash(&self, seed: Hash256, entry_idx: u64) -> Hash256 {
        let mut hasher = OpBatchHasher::new(seed, entry_idx, self.ops.len());
        hasher.hash_header(&self.header);
        for (i, e) in self.ops.iter().enumerate() {
            match e {
                OpOrExpunge::Op(e) => {
                    hasher.hash_op(i as u64, e);
                }
                OpOrExpunge::Expunge(h) => hasher.hash_expunge(i as u64, h),
            }
        }
        hasher.finalize()
    }
}

pub(crate) struct OpBatchHasher {
    hasher: Hasher,
    entry_idx: u64,
    seed: Hash256,
}

impl OpBatchHasher {
    pub(crate) fn new(seed: Hash256, entry_idx: u64, num_ops: usize) -> Self {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::LogEntryOpBatch);
        hasher.update(&seed[..]);
        hasher.update(&entry_idx.to_le_bytes());
        let num_ops = num_ops as u64;
        hasher.update(&num_ops.to_le_bytes());
        Self {
            hasher,
            entry_idx,
            seed,
        }
    }

    pub(crate) fn hash_header(&mut self, bytes: &OpaqueBytes) -> Hash256 {
        self.hash_slot(0, bytes)
    }

    pub(crate) fn hash_op(&mut self, op_idx: u64, o: &OpaqueBytes) -> Hash256 {
        self.hash_slot((op_idx + 1) as u64, o)
    }

    pub(crate) fn hash_expunge(&mut self, _op_idx: u64, h: &Hash256) {
        self.hasher.update(h);
    }

    fn hash_slot(&mut self, slot_idx: u64, bytes: &OpaqueBytes) -> Hash256 {
        let mut slot_hasher = new_tagged_hasher(TaggedHashDomain::OpBatchSlot);
        slot_hasher.update(&self.seed);
        slot_hasher.update(&self.entry_idx.to_le_bytes());
        slot_hasher.update(&slot_idx.to_le_bytes());
        slot_hasher.update(bytes.borrow());
        let h = slot_hasher.finalize();
        self.hasher.update(&h);
        h
    }

    pub(crate) fn finalize(self) -> Hash256 {
        self.hasher.finalize()
    }
}

fn hash_use_key(entry_index: u64, cipher_info: &CipherInfo) -> Hash256 {
    let mut hasher = new_tagged_hasher(TaggedHashDomain::LogEntryUseKey);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[cipher_info.cipher_suite]);
    hasher.update(&cipher_info.fingerprint.0);
    hasher.finalize().into()
}
