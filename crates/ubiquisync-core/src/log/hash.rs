use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    bytes::OpaqueBytes,
    codec::{ReadError, Reader, Writer},
    crypto::{CipherInfo, EntryCipher, Hash256, Hasher, TaggedHashDomain, new_tagged_hasher},
    ids::LogId,
    log::{
        EntryBody, LogEntry, OpBatch, OpOrExpunge, OpaqueLogEntry, PlaintextLogEntry,
        SegmentCipherError, entries_to_opaque,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainHash {
    pub hash: Hash256,
    pub size: u64,
}

#[derive(Error, Debug)]
pub enum ChainHashError {
    #[error("chain size overflowed u64")]
    SizeOverflow,
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

    fn add_one(&self, entry_hash: &Hash256) -> Result<Self, ChainHashError> {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::ChainHash);
        hasher.update(&self.hash);
        hasher.update(entry_hash);
        let hash = hasher.finalize();
        let size = self
            .size
            .checked_add(1)
            .ok_or(ChainHashError::SizeOverflow)?;
        Ok(Self { hash, size })
    }

    /// Updates the chain has with the next entry and a possibly already computed hash (from encryption/decryption processing).
    pub(crate) fn next(
        &self,
        entry: &OpaqueLogEntry,
        precomputed_hash: Option<Hash256>,
        seed: &ChainSeed,
    ) -> Result<Self, ChainHashError> {
        match entry {
            LogEntry::IndexedEntry(entry) => {
                let entry_hash = precomputed_hash.unwrap_or_else(|| entry.hash(seed, self.size));
                Ok(self.add_one(&entry_hash)?)
            }
            LogEntry::Signature(_) => Ok(*self),
        }
    }

    pub fn compute_next_plaintext<'a: 'b, 'b>(
        &self,
        seed: &ChainSeed,
        cipher: &Option<EntryCipher>,
        entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
    ) -> Result<Self, SegmentCipherError> {
        let (_, h) = entries_to_opaque(cipher, seed, self, entries)?;
        Ok(h)
    }

    pub fn compute_next_opaque<'a: 'b, 'b>(
        &self,
        seed: &ChainSeed,
        entries: impl Iterator<Item = &'a OpaqueLogEntry<'a>>,
    ) -> Result<Self, ChainHashError> {
        let mut h: ChainHash = *self;
        for e in entries {
            h = h.next(e, None, seed)?;
        }
        Ok(h)
    }

    pub fn sign_bytes(&self, seed: &ChainSeed) -> Hash256 {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::LogSignBytes);
        hasher.update(&seed.0);
        hasher.update(&self.size.to_le_bytes());
        hasher.update(&self.hash);
        hasher.finalize()
    }

    pub fn encode(&self, w: &mut Writer) {
        w.write_var_u64(self.size);
        w.write_array(&self.hash);
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        let size = r.read_var_u64()?;
        let hash = r.read_array()?;
        Ok(Self { size, hash })
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
            EntryBody::UseKey(cipher_info) => hash_use_key(seed, entry_index, cipher_info),
            EntryBody::Expunged(hash) => *hash,
        }
    }
}

impl<'a> OpBatch<OpaqueBytes<'a>> {
    pub fn hash(&self, seed: &ChainSeed, entry_idx: u64) -> Hash256 {
        let mut hasher = OpBatchHasher::new(seed, entry_idx, self.ops.len());
        hasher.hash_slot(&self.timestamp);
        if !self.server_attested_user_id.0.is_empty() {
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

fn hash_use_key(seed: &ChainSeed, entry_index: u64, cipher_info: &CipherInfo) -> Hash256 {
    let mut hasher = new_tagged_hasher(TaggedHashDomain::LogEntryUseKey);
    hasher.update(&seed.0);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[cipher_info.cipher_suite]);
    hasher.update(&cipher_info.fingerprint.0);
    hasher.finalize()
}
