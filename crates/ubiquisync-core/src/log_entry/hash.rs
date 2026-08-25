use std::{borrow::Borrow, ops::Range};

use thiserror::Error;

use crate::{
    crypto::{
        Hash256, Hash256Suite, Hasher, TaggedHashDomain,
        mmr::{InvalidCoverError, MmrAccumulator},
    },
    ids::LogId,
    log_entry::{
        CipherInfo, EntryBody, GenericLogEntry, OpBatch, OpOrExpunge, OpaqueBytes, OpaqueLogEntry,
    },
};

pub struct RootInfo {
    pub root_hash: Hash256,
    pub size: u64,
}

pub fn root_info(mmr: &MmrAccumulator) -> RootInfo {
    let root_hash = mmr.root();
    let size = mmr.size();
    RootInfo { root_hash, size }
}

pub fn mmr_seed(log_id: &LogId) -> Hash256 {
    let mut hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::MmrSeed);
    hasher.update(&log_id.peer_id.0);
    hasher.update(&log_id.container_id.0);
    hasher.finalize()
}

impl RootInfo {
    pub fn sign_bytes(&self, log_id: &LogId) -> Hash256 {
        let mut hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::MmrSignBytes);
        hasher.update(&log_id.peer_id.0);
        hasher.update(&log_id.container_id.0);
        hasher.update(&self.size.to_le_bytes());
        hasher.update(&self.root_hash);
        hasher.finalize()
    }
}

#[derive(Error, Debug)]
pub enum MmrUpdateError {
    #[error("expected entry {size}, got {idx}")]
    OutOfOrderEntry { size: u64, idx: u64 },
    #[error("invalid expunge range: {range:?}, size {size}")]
    InvalidExpungeRange { size: u64, range: Range<u64> },
    #[error("invalid expunge cover: {0}")]
    InvalidExpungeCover(#[from] InvalidCoverError),
}

pub fn update_mmr<'a>(
    acc: &mut MmrAccumulator,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
) -> Result<(), MmrUpdateError> {
    for e in entries {
        mmr_append_entry(acc, &e)?;
    }
    Ok(())
}

fn mmr_append_entry(
    acc: &mut MmrAccumulator,
    entry: &OpaqueLogEntry,
) -> Result<(), MmrUpdateError> {
    match entry {
        GenericLogEntry::IndexedEntry { idx, entry } => {
            let size = acc.size();
            if *idx != size {
                return Err(MmrUpdateError::OutOfOrderEntry { size, idx: *idx });
            }
            let leaf = entry.hash(acc.seed(), *idx);
            acc.append(&leaf);
        }
        GenericLogEntry::Expunged { range, cover, .. } => {
            let size = acc.size();
            if range.is_empty() || range.start != size {
                return Err(MmrUpdateError::InvalidExpungeRange {
                    size,
                    range: range.clone(),
                });
            }
            acc.advance_with_cover(range.end, cover)?;
        }
        _ => {}
    }
    Ok(())
}

impl<'a> OpaqueLogEntry<'a> {
    pub fn hash(&self, seed: &Hash256) -> Option<Hash256> {
        match self {
            GenericLogEntry::IndexedEntry { idx, entry } => Some(entry.hash(seed, *idx)),
            GenericLogEntry::Expunged { last_leaf_hash, .. } => Some(*last_leaf_hash),
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
        let mut hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::LogEntryOpBatch);
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
        let mut slot_hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::OpBatchSlot);
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
    let mut hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::LogEntryUseKey);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[cipher_info.cipher_suite]);
    hasher.update(&cipher_info.fingerprint.0);
    hasher.finalize().into()
}

const HASH_SUITE: Hash256Suite = Hash256Suite::Sha256;
