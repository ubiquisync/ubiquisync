use std::collections::HashMap;

use crate::{codec::error::CodecError, uuid::Uuid};

/// Encodes one log entry's body: accumulates bytes while feeding a rolling
/// blake3 hash, and deduplicates UUIDs against a dictionary shared across the
/// segment's entries.
pub struct EntryBufferWriter<'a> {
    buf: DualWireCanonicalWriter,
    uuid_dict: &'a mut HashMap<Uuid, u32>,
}

impl<'a> EntryBufferWriter<'a> {
    /// Create a writer that shares `uuid_dict` for UUID dictionary compression
    /// across the entries of one segment.
    pub fn new(uuid_dict: &'a mut HashMap<Uuid, u32>) -> Self {
        Self {
            buf: DualWireCanonicalWriter::new(),
            uuid_dict,
        }
    }

    /// Append a single raw byte.
    pub fn write_byte(&mut self, b: u8) {
        self.buf.append(&[b]);
    }

    /// Append `v` as an unsigned varint (7 data bits per byte, little-endian).
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

    /// Append a length-prefixed byte string: a varint length, then the bytes.
    pub fn write_blob(&mut self, data: &[u8]) {
        self.write_varint(data.len() as u64);
        self.buf.append(data);
    }

    /// Append a `u16` in little-endian order.
    pub fn write_u16_le(&mut self, v: u16) {
        self.buf.append(&v.to_le_bytes());
    }

    /// Append a signed integer as a zigzag-encoded varint.
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
        self.buf._append_canonical_only(data);

        if let Some(id) = self.uuid_dict.get(data) {
            // Known UUID — write dict reference varint to buf only.
            self._write_varint_without_hash(*id as u64);
        } else {
            // First occurrence — write 0 sentinel + raw bytes to buf,
            // and register in the dictionary for future references.
            self.buf._append_wire_only(&[0]);
            self.buf._append_wire_only(data);
            let id = self.uuid_dict.len() as u32 + 1; // IDs start at 1; 0 is the inline sentinel.
            self.uuid_dict.insert(*data, id);
        }
    }

    fn _write_varint_without_hash(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf._append_wire_only(&[byte]);
                break;
            } else {
                self.buf._append_wire_only(&[byte | 0x80]);
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
        self.buf._append_canonical_only(&current.to_le_bytes());
        self._write_varint_without_hash(delta);
        Ok(())
    }

    /// Finalize the entry: appends truncated blake3 hash as integrity
    /// check bytes and returns the encoded buffer plus the full hash.
    pub fn finalize(self) -> (Vec<u8>, Vec<u8>) {
        self.buf.finalize()
    }
}

struct DualWireCanonicalWriter {
    wire_bytes: Vec<u8>,
    canonical_bytes: Vec<u8>,
}

impl DualWireCanonicalWriter {
    fn new() -> Self {
        Self {
            wire_bytes: Vec::new(),
            canonical_bytes: Vec::new(),
        }
    }

    fn append(&mut self, data: &[u8]) {
        self.wire_bytes.extend_from_slice(data);
        self.canonical_bytes.extend_from_slice(data);
    }

    /// Write to buf only — skips the hasher. Used for dictionary-compressed
    /// encodings where the canonical hash sees different bytes than the buf.
    fn _append_wire_only(&mut self, data: &[u8]) {
        self.wire_bytes.extend_from_slice(data);
    }

    /// Update the hasher only — skips the buf. Used to feed canonical
    /// content (e.g. raw UUID bytes) into the hash without writing them.
    fn _append_canonical_only(&mut self, data: &[u8]) {
        self.canonical_bytes.extend_from_slice(data);
    }

    /// Returns the accumulated wire and canonical bytes respectively.
    fn finalize(self) -> (Vec<u8>, Vec<u8>) {
        (self.wire_bytes, self.canonical_bytes)
    }
}
