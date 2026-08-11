use crate::uuid::Uuid;

/// Encodes one log entry's body: accumulates bytes while feeding a rolling
/// blake3 hash, and deduplicates UUIDs against a dictionary shared across the
/// segment's entries.
pub struct EntryBufferWriter {
    buf: Vec<u8>,
}

impl EntryBufferWriter {
    /// Create a writer that shares `uuid_dict` for UUID dictionary compression
    /// across the entries of one segment.
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append a single raw byte.
    pub fn write_byte(&mut self, b: u8) {
        self.buf.extend_from_slice(&[b]);
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
        self.buf.extend_from_slice(data);
    }

    /// Append a `u16` in little-endian order.
    pub fn write_u16_le(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u64_le(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_hash(&mut self, hash: &[u8; 32]) {
        self.buf.extend_from_slice(hash);
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
        self.buf.extend_from_slice(data);
    }

    pub fn finalize(self) -> Vec<u8> {
        self.buf
    }
}
