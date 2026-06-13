use std::collections::HashMap;

use crate::{codec::error::CodecError, uuid::Uuid};

pub struct EntryBufferWriter<'a> {
    buf: HashWriter,
    uuid_dict: &'a mut HashMap<Uuid, u32>,
}

impl<'a> EntryBufferWriter<'a> {
    pub fn new(uuid_dict: &'a mut HashMap<Uuid, u32>) -> Self {
        Self {
            buf: HashWriter::new(),
            uuid_dict,
        }
    }

    pub fn write_byte(&mut self, b: u8) {
        self.buf.append(&[b]);
    }

    pub fn write_varint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.write_byte(byte);
                break;
            } else {
                self.write_byte(byte | 0x80);
            }
        }
    }

    pub fn write_blob(&mut self, data: &[u8]) {
        self.write_varint(data.len() as u64);
        self.buf.append(data);
    }

    pub fn write_u16_le(&mut self, v: u16) {
        self.buf.append(&v.to_le_bytes());
    }

    pub fn write_zigzag(&mut self, n: i64) {
        let encoded = ((n << 1) ^ (n >> 63)) as u64;
        self.write_varint(encoded);
    }

    /// Write a UUID using dictionary compression. The canonical hash
    /// always sees the raw 16 bytes regardless of whether the buffer
    /// gets a dict reference or an inline literal.
    pub fn write_uuid(&mut self, data: &Uuid) {
        // Hash the raw UUID bytes — canonical content identity must be
        // independent of dictionary state.
        self.buf._hash_without_append(data);

        if let Some(id) = self.uuid_dict.get(data) {
            // Known UUID — write dict reference varint to buf only.
            self._write_varint_without_hash(*id as u64);
        } else {
            // First occurrence — write 0 sentinel + raw bytes to buf,
            // and register in the dictionary for future references.
            self.buf._append_without_hash(&[0]);
            self.buf._append_without_hash(data);
            let id = self.uuid_dict.len() as u32 + 1; // IDs start at 1; 0 is the inline sentinel.
            self.uuid_dict.insert(data.clone(), id);
        }
    }

    fn _write_varint_without_hash(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf._append_without_hash(&[byte]);
                break;
            } else {
                self.buf._append_without_hash(&[byte | 0x80]);
            }
        }
    }

    /// Write a delta-encoded timestamp. Timestamps within a segment must
    /// be monotonically non-decreasing.
    pub fn write_delta(&mut self, current: u64, last: u64) -> Result<(), CodecError> {
        if current < last {
            return Err(CodecError::NonMonotonicDelta);
        }
        let delta = current - last;
        self.buf._hash_without_append(&current.to_le_bytes());
        self._write_varint_without_hash(delta);
        Ok(())
    }

    /// Finalize the entry: appends truncated blake3 hash as integrity
    /// check bytes and returns the encoded buffer plus the full hash.
    pub fn finalize(self) -> (Vec<u8>, blake3::Hash) {
        self.buf.finalize()
    }
}

struct HashWriter {
    buf: Vec<u8>,
    hasher: blake3::Hasher,
}

impl HashWriter {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            hasher: blake3::Hasher::new(),
        }
    }

    fn append(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        self.hasher.update(data);
    }

    /// Write to buf only — skips the hasher. Used for dictionary-compressed
    /// encodings where the canonical hash sees different bytes than the buf.
    fn _append_without_hash(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Update the hasher only — skips the buf. Used to feed canonical
    /// content (e.g. raw UUID bytes) into the hash without writing them.
    fn _hash_without_append(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    fn finalize(self) -> (Vec<u8>, blake3::Hash) {
        let hash = self.hasher.finalize();
        // append last 4 bytes of hash as CRC check
        let mut buf = self.buf;
        buf.extend_from_slice(&hash.as_bytes()[..4]);
        (buf, hash)
    }
}
