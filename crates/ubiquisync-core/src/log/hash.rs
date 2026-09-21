use thiserror::Error;

use crate::{
    bytes::OpaqueBytes,
    codec::{ReadError, Reader, Writer},
    crypto::{CipherInfo, CipherKeyResolver, Hash256, Hasher, TaggedHashDomain, new_tagged_hasher},
    ids::LogId,
    log::{
        EntryBody, LogEntry, OpEntry, OpaqueLogEntry, PlaintextLogEntry, SegmentCipherError,
        entries_to_opaque,
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

#[derive(Debug, Clone)]
pub struct LogHashContext {
    log_id: LogId,
    op_hasher: Hasher,
    sign_bytes_hasher: Hasher,
}

// returns a hasher instance that should have pre-computed exactly 1 SHA256 block (64 bytes)
// from tag (16 bytes) + peer_id (32 bytes) + container_id (16 bytes)
fn log_id_hasher(tag: TaggedHashDomain, log_id: &LogId) -> Hasher {
    let mut hasher = new_tagged_hasher(tag);
    hasher.update(&log_id.peer_id.0);
    hasher.update(&log_id.container_id.0);
    hasher
}

impl ChainHash {
    pub fn empty(seed: &LogHashContext) -> Self {
        Self {
            hash: log_id_hasher(TaggedHashDomain::ChainSeed, &seed.log_id).finalize(),
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
        seed: &LogHashContext,
        active_cipher: &mut Option<CipherInfo>,
    ) -> Result<Self, ChainHashError> {
        match entry {
            LogEntry::IndexedEntry(entry) => {
                let entry_hash = entry.hash(seed, self.size, active_cipher);
                Ok(self.add_one(&entry_hash, active_cipher)?)
            }
            LogEntry::Signature(_) => Ok(*self),
        }
    }

    pub async fn compute_next_plaintext<'a: 'b, 'b>(
        &self,
        seed: &LogHashContext,
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
        seed: &LogHashContext,
        active_cipher: &mut Option<CipherInfo>,
        entries: impl Iterator<Item = &'a OpaqueLogEntry<'a>>,
    ) -> Result<Self, ChainHashError> {
        let mut h: ChainHash = *self;
        for e in entries {
            h = h.next(e, seed, active_cipher)?;
        }
        Ok(h)
    }

    pub fn sign_bytes(&self, seed: &LogHashContext) -> Hash256 {
        let mut hasher = seed.sign_bytes_hasher.clone();
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

impl LogHashContext {
    pub fn new(log_id: &LogId) -> Self {
        let op_hasher = log_id_hasher(TaggedHashDomain::LogOp, log_id);
        let sign_bytes_hasher = log_id_hasher(TaggedHashDomain::LogSign, log_id);
        Self {
            log_id: *log_id,
            op_hasher,
            sign_bytes_hasher,
        }
    }

    pub fn log_id(&self) -> &LogId {
        &self.log_id
    }
}

impl<'a> EntryBody<OpaqueBytes<'a>, OpaqueBytes<'a>> {
    pub fn hash(
        &self,
        seed: &LogHashContext,
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

impl<'a> OpEntry<OpaqueBytes<'a>, OpaqueBytes<'a>> {
    pub fn hash(&self, seed: &LogHashContext, entry_idx: u64) -> Hash256 {
        let mut hasher = seed.op_hasher.clone();
        hasher.update_varint(entry_idx);
        hasher.update_len_prefixed(&self.timestamp.0);
        hasher.update_len_prefixed(&self.server_attested_user_id.0);
        hasher.update_len_prefixed(&self.op.0);
        hasher.finalize()
    }
}

fn hash_use_key(seed: &LogHashContext, entry_index: u64, cipher_info: &CipherInfo) -> Hash256 {
    // we don't cache this hasher because it's expected to be much less frequent
    // if we wanted to, we could use a LazyCell, but probably not needed
    let mut hasher = log_id_hasher(TaggedHashDomain::LogUseKey, &seed.log_id);
    hasher.update_varint(entry_index);
    hasher.update(&[cipher_info.cipher_suite]);
    hasher.update(&cipher_info.fingerprint.0);
    hasher.finalize()
}
