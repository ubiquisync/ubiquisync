use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    bytes::OpaqueBytes,
    codec::{ReadError, Reader, Writer},
    crypto::{CipherInfo, CipherKeyResolver, Hash256, Hasher, TaggedHashDomain, new_tagged_hasher},
    ids::LogId,
    log::{
        EntryBody, LogEntry, OpBatch, OpOrExpunge, OpaqueLogEntry, PlaintextLogEntry,
        SegmentCipherError, entries_to_opaque,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct ChainHash {
    pub hash: Hash256,
    #[cfg_attr(feature = "proptest", strategy(0u64..1<<24))]
    pub size: u64,
}

#[derive(Error, Debug)]
pub enum ChainHashError {
    #[error("chain size overflowed u64")]
    SizeOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainSeed {
    log_id: LogId,
    hash: Hash256,
}

impl ChainHash {
    pub fn empty(seed: &ChainSeed) -> Self {
        Self {
            hash: seed.hash,
            size: 0,
        }
    }

    fn add_one(
        &self,
        entry_hash: &Hash256,
        active_cipher: &Option<CipherInfo>,
    ) -> Result<Self, ChainHashError> {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::ChainHash);
        hasher.update(&self.hash);
        hasher.update(entry_hash);
        // We bind the active cipher so that claims about which cipher is used
        // are authoratative. We always use the latest cipher is the current entry
        // is a UseKey entry, not the prior cipher.
        // Without this, it would be feasible to package an opaque segment as a plaintext
        // segment and claim that it is actually a plaintext segment with no cipher at all
        // which is totally wrong. This is somewhat redundant because the caller could
        // figure out which segment should be active, but because plaintext segments package
        // their cipher, including this in the hash ensures that the segment is self-contained
        // and that cipher claims in segment headers are verifiable.
        // We could include this in the entry hash itself, but then a segment of all expunged
        // entries could contain any cipher claim and it would be unverifiable - it's an edge
        // case for sure, but this approach is slightly more correct.
        if let Some(ci) = active_cipher {
            hasher.update(&[1]);
            hasher.update(&[ci.cipher_suite]);
            hasher.update(&ci.fingerprint.0);
        } else {
            hasher.update(&[0]);
        }
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
        active_cipher: &Option<CipherInfo>,
    ) -> Result<Self, ChainHashError> {
        let mut active_cipher = *active_cipher;
        match entry {
            LogEntry::IndexedEntry(entry) => {
                let entry_hash = precomputed_hash
                    .unwrap_or_else(|| entry.hash(seed, self.size, &mut active_cipher));
                Ok(self.add_one(&entry_hash, &active_cipher)?)
            }
            LogEntry::Signature(_) => Ok(*self),
        }
    }

    pub async fn compute_next_plaintext<'a: 'b, 'b>(
        &self,
        seed: &ChainSeed,
        head_cipher: &mut Option<CipherInfo>,
        key_resolver: &dyn CipherKeyResolver,
        entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
    ) -> Result<Self, SegmentCipherError> {
        let opaque = entries_to_opaque(seed, head_cipher, self, key_resolver, entries).await?;
        if opaque.is_empty() {
            // TODO: maybe this should be a hard error, but if there are no entries we can validly just clone self
            return Ok(*self);
        }
        // we take the last chain hash in the segment
        Ok(opaque.last().expect("non-empty entries").1)
    }

    pub fn compute_next_opaque<'a: 'b, 'b>(
        &self,
        seed: &ChainSeed,
        active_cipher: &mut Option<CipherInfo>,
        entries: impl Iterator<Item = &'a OpaqueLogEntry<'a>>,
    ) -> Result<Self, ChainHashError> {
        let mut h: ChainHash = *self;
        for e in entries {
            h = h.next(e, None, seed, active_cipher)?;
        }
        Ok(h)
    }

    pub fn sign_bytes(&self, seed: &ChainSeed) -> Hash256 {
        let mut hasher = new_tagged_hasher(TaggedHashDomain::LogSignBytes);
        hasher.update(&seed.hash);
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
        let hash = hasher.finalize();
        Self {
            hash,
            log_id: *log_id,
        }
    }

    pub fn hash(&self) -> &Hash256 {
        &self.hash
    }

    pub fn log_id(&self) -> &LogId {
        &self.log_id
    }
}

impl<'a> EntryBody<OpaqueBytes<'a>> {
    pub fn hash(
        &self,
        seed: &ChainSeed,
        entry_index: u64,
        active_cipher: &mut Option<CipherInfo>,
    ) -> Hash256 {
        match self {
            EntryBody::OpBatch(op_batch) => op_batch.hash(seed, entry_index),
            EntryBody::UseKey(cipher_info) => {
                *active_cipher = Some(*cipher_info);
                hash_use_key(seed, entry_index, cipher_info)
            }
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
        hasher.update(&seed.hash);
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
        slot_hasher.update(&self.seed.hash);
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
    hasher.update(&seed.hash);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[cipher_info.cipher_suite]);
    hasher.update(&cipher_info.fingerprint.0);
    hasher.finalize()
}
