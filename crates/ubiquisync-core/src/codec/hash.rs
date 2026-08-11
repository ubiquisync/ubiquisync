use crate::{
    codec::consts::{ENTRY_TYPE_OP_BATCH, ENTRY_TYPE_USE_KEY},
    crypto::{EntryCipher, Error},
};

pub struct OpBatchHasher {
    hasher: blake3::Hasher,
    entry_index: u64,
    next_op_index: u64,
    num_ops: u64,
}

pub struct PlaintextOpBatchHasher<'a> {
    hasher: OpBatchHasher,
    cipher: Option<&'a mut EntryCipher>,
}
const DOMAIN_ENTRY_HASH: &str = "ubiquisync/v1/entry-hash";
const DOMAIN_SLOT_HASH: &str = "ubiquisync/v1/slot-hash";

impl OpBatchHasher {
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

    pub fn append_opaque_op(&mut self, opaque_op_bytes: &[u8]) -> Result<(), Error> {
        let op_hash = blake3::derive_key(DOMAIN_SLOT_HASH, opaque_op_bytes);
        self.append_op_hash(&op_hash)
    }

    pub fn append_expunged_op(&mut self, op_hash: &[u8; 32]) -> Result<(), Error> {
        self.append_op_hash(op_hash)
    }

    fn append_op_hash(&mut self, op_hash: &[u8; 32]) -> Result<(), Error> {
        if self.next_op_index >= self.num_ops {
            return Err(Error::OutOfRangeOp);
        }
        self.next_op_index += 1;
        self.hasher.update(op_hash);
        Ok(())
    }

    pub fn finalize(self) -> Result<blake3::Hash, Error> {
        if self.next_op_index != self.num_ops {
            return Err(Error::OpCountMismatch);
        }
        Ok(self.hasher.finalize())
    }
}

impl<'a> PlaintextOpBatchHasher<'a> {
    pub fn new(
        cipher: Option<&'a mut EntryCipher>,
        plaintext_header_bytes: &[u8],
        entry_index: u64,
        num_ops: u64,
    ) -> Result<Self, Error> {
        let hasher = if let Some(ref cipher) = cipher {
            let bytes = cipher.encrypt_header(entry_index, plaintext_header_bytes)?;
            OpBatchHasher::new(bytes.as_slice(), entry_index, num_ops)
        } else {
            OpBatchHasher::new(plaintext_header_bytes, entry_index, num_ops)
        };
        Ok(Self { cipher, hasher })
    }

    pub fn append_op(&mut self, canonical_bytes: &[u8]) -> Result<(), Error> {
        if let Some(ref cipher) = self.cipher {
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

    pub fn finalize(self) -> Result<blake3::Hash, Error> {
        self.hasher.finalize()
    }
}

pub fn hash_use_key(entry_index: u64, fingerprint: [u8; 32]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new_derive_key(DOMAIN_ENTRY_HASH);
    hasher.update(&entry_index.to_le_bytes());
    hasher.update(&[ENTRY_TYPE_USE_KEY]);
    hasher.update(&fingerprint);
    hasher.finalize()
}
