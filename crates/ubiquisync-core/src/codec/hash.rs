use crate::{
    codec::consts::ENTRY_TYPE_OP_BATCH,
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
    next_op_index: u64,
    num_ops: u64,
}
const DOMAIN_ENTRY_HASH = "ubiquisync/v1/entry-hash";
const DOMAIN_SLOT_HASH = "ubiquisync/v1/slot-hash";


impl<'a> OpBatchHasher<'a> {
    pub fn new(
        cipher: Option<&'a mut EntryCipher>,
        encrypted_header_bytes: &[u8],
        entry_index: u64,
        num_ops: u64,
    ) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key(DOMAIN_ENTRY_HASH);
        hasher.update(ENTRY_TYPE_OP_BATCH); // use as domain separator
        let header_hash = blake3::derive_key(DOMAIN_SLOT_HASH, encrypted_header_bytes);
        hasher.update(header_hash.as_bytes());
        hasher.update(&num_ops.to_le_bytes()[..]);
        Self {
            cipher,
            entry_index,
            hasher,
            next_op_index: 0,
            num_ops,
        }
    }

    pub fn append_plaintext_op(&mut self, canonical_bytes: &[u8]) -> Result<(), Error> {
        if let Some(ref cipher) = self.cipher {
            let bytes = cipher.encrypt_op(self.entry_index, self.next_op_index, canonical_bytes)?;
            self.append_opaque_op(bytes.as_slice())
        } else {
            self.append_opaque_op(canonical_bytes)
        }
    }

    pub fn append_opaque_op(&mut self, opaque_op_bytes: &[u8]) -> Result<(), Error> {
        let op_hash = blake3::hash(opaque_op_bytes);
        self.append_op_hash(op_hash.as_bytes())
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

    pub fn finalize(self) -> blake3::Hash {
        self.hasher.finalize()
    }
}
