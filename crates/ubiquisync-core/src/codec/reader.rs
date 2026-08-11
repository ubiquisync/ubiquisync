use crate::{codec::error::CodecError, uuid::Uuid};
use std::io::BufRead;

/// A thin [`BufRead`] wrapper that the codec reads a segment from, tracking
/// position only enough to answer [`is_eof`](Self::is_eof).
pub struct EntryBufferReader<R> {
    reader: R,
}

impl<R: BufRead> EntryBufferReader<R> {
    /// Wrap an underlying [`BufRead`] source.
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    /// Whether the underlying source has no more bytes.
    pub fn is_eof(&mut self) -> Result<bool, CodecError> {
        Ok(self.reader.fill_buf()?.is_empty())
    }

    pub(super) fn read_varint(&mut self) -> Result<u64, CodecError> {
        let (x, _, _) = self.read_varint_parts()?;
        Ok(x)
    }

    /// Returns the decoded value plus the raw on-wire bytes.
    /// A u64 varint is at most 10 bytes, so they go in a fixed stack
    /// buffer — `len` is how many are valid. No allocation.
    pub(super) fn read_varint_parts(&mut self) -> Result<(u64, [u8; 10], usize), CodecError> {
        let mut bytes = [0u8; 10];
        let mut len = 0;
        let mut result = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.read_byte()?.ok_or(CodecError::UnexpectedEof)?;
            bytes[len] = byte;
            len += 1;
            // On the 10th byte (shift=63), only bit 0 is valid — higher
            // bits or a continuation flag would overflow u64. This also caps
            // the loop at 10 bytes, so `bytes[len]` never indexes past the end.
            if shift == 63 && byte > 1 {
                return Err(CodecError::VarIntOverflow);
            }
            result |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                if len > 1 && byte == 0 {
                    return Err(CodecError::NonMinimalVarint);
                }
                return Ok((result, bytes, len));
            }
            shift += 7;
        }
    }

    pub(super) fn read_byte(&mut self) -> Result<Option<u8>, CodecError> {
        let mut buf: [u8; 1] = [0; 1];
        let read = self.reader.read(&mut buf)?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(buf[0]))
    }

    pub(super) fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), CodecError> {
        self.reader.read_exact(buf).map_err(|e| match e.kind() {
            // A short read is truncation — surface the dedicated EOF error for
            // consistency with the rest of the decoder, not a generic Io.
            std::io::ErrorKind::UnexpectedEof => CodecError::UnexpectedEof,
            _ => CodecError::Io(e),
        })
    }

    pub fn read_vec(&mut self, len: usize) -> Result<Vec<u8>, CodecError> {
        // Grow the buffer with the bytes actually delivered rather than
        // pre-allocating an on-wire length we haven't validated — a corrupt or
        // hostile blob length must not OOM the process before we hit EOF.
        let mut buf = Vec::new();
        let mut remaining = len;
        let mut chunk = [0u8; 8192];
        while remaining > 0 {
            let want = remaining.min(chunk.len());
            let n = self.reader.read(&mut chunk[..want])?;
            if n == 0 {
                return Err(CodecError::UnexpectedEof);
            }
            buf.extend_from_slice(&chunk[..n]);
            remaining -= n;
        }
        Ok(buf)
    }

    pub fn must_read_byte(&mut self) -> Result<u8, CodecError> {
        let b = self.read_byte()?;
        b.ok_or(CodecError::UnexpectedEof)
    }

    /// Read a length-prefixed byte string (varint length, then the bytes).
    pub fn read_blob(&mut self) -> Result<Vec<u8>, CodecError> {
        let len = self.read_varint()?;
        // try_into, not `as`: on 32-bit targets (e.g. wasm32) `as usize` would
        // truncate a bogus 64-bit length and mis-decode instead of rejecting.
        let len: usize = len
            .try_into()
            .map_err(|_| CodecError::LengthTooLarge(len))?;
        self.read_vec(len)
    }

    /// Read a zigzag-encoded signed varint.
    pub fn read_zigzag(&mut self) -> Result<i64, CodecError> {
        let encoded = self.read_varint()?;
        Ok(((encoded >> 1) as i64) ^ -((encoded & 1) as i64))
    }

    /// Read a little-endian `u16`.
    pub fn read_u16_le(&mut self) -> Result<u16, CodecError> {
        let mut x = [0; 2];
        self.read_exact(&mut x[..])?;
        Ok(u16::from_le_bytes(x))
    }

    /// Read a little-endian `u64`.
    pub fn read_u64_le(&mut self) -> Result<u64, CodecError> {
        let mut x = [0; 8];
        self.read_exact(&mut x[..])?;
        Ok(u64::from_le_bytes(x))
    }

    pub fn read_uuid(&mut self) -> Result<Uuid, CodecError> {
        let mut res: Uuid = [0; 16];
        self.read_exact(&mut res[..])?;
        Ok(res)
    }

    pub fn read_hash(&mut self) -> Result<[u8; 32], CodecError> {
        let mut res = [0; 32];
        self.read_exact(&mut res[..])?;
        Ok(res)
    }
}

//#[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::codec::writer::EntryBufferWriter;

//     const UUID_A: Uuid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
//     const UUID_B: Uuid = [
//         0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE,
//         0xAF,
//     ];
//     const UUID_C: Uuid = [0xFF; 16];

//     /// Goal: Verify that all primitive types round-trip correctly through
//     /// the writer and reader, including UUID dictionary dedup and blake3
//     /// hash verification.
//     ///
//     /// Given: Two entries written sequentially sharing a UUID dictionary,
//     ///        each containing every primitive type (byte, varint, blob,
//     ///        u16_le, zigzag, uuid), with 3 distinct UUIDs where UUID_A
//     ///        appears in both entries to exercise dict dedup.
//     /// When:  Both entries are read back with a fresh reader-side UUID dict.
//     /// Then:  All values match, hashes match, and the second occurrence of
//     ///        UUID_A is decoded from the dict (not inline).
//     #[test]
//     fn roundtrip_two_entries_all_types() {
//         let mut write_uuid_dict: HashMap<Uuid, u32> = HashMap::new();
//         let blob_data = b"hello ubiquisync";

//         // Simulate HLC timestamps: first entry has two timestamps (e.g. created_at, updated_at),
//         // second entry has one. Gaps: 1 second, sub-ms counter bump, 30 seconds.
//         let ts1: u64 = 1_700_000_000_000 << 16; // ~2023, counter=0
//         let ts2: u64 = ts1 + 1; // same ms, counter=1
//         let ts3: u64 = ts1 + (30_000 << 16); // 30 seconds later

//         // ── Write entry 1: all types including 2 delta timestamps ──
//         let mut w1 = EntryBufferWriter::new(&mut write_uuid_dict);
//         w1.write_byte(0x42);
//         w1.write_varint(123456789);
//         w1.write_blob(blob_data);
//         w1.write_u16_le(0xBEEF);
//         w1.write_zigzag(-99);
//         w1.write_delta(ts1, 0).unwrap(); // first timestamp, last=0
//         w1.write_delta(ts2, ts1).unwrap(); // counter bump, delta=1
//         w1.write_uuid(&UUID_A);
//         w1.write_uuid(&UUID_B);
//         let (buf1, hash1) = w1.finalize();

//         // ── Write entry 2: UUID_A dict hit, 1 delta timestamp, edge cases ──
//         let mut w2 = EntryBufferWriter::new(&mut write_uuid_dict);
//         w2.write_byte(0x00);
//         w2.write_varint(0); // edge case: zero
//         w2.write_blob(b""); // edge case: empty blob
//         w2.write_u16_le(0x0000);
//         w2.write_zigzag(i64::MIN);
//         w2.write_delta(ts3, ts2).unwrap(); // 30 second gap
//         w2.write_uuid(&UUID_A); // dict hit — should be smaller on wire
//         w2.write_uuid(&UUID_C);
//         let (buf2, hash2) = w2.finalize();

//         // Entry 2's UUID_A should be a dict reference (varint 1 = 1 byte),
//         // not inline (1 + 16 = 17 bytes). Sanity check that buf2 is smaller.
//         assert!(
//             buf2.len() < buf1.len(),
//             "entry 2 should be smaller due to UUID_A dict hit"
//         );

//         // Hashes should differ — different content.
//         assert_ne!(hash1, hash2);

//         // ── Read both entries back ──────────────────────────────────────────
//         let mut combined = Vec::new();
//         combined.extend_from_slice(&buf1);
//         combined.extend_from_slice(&buf2);

//         let mut reader = EntryBufferReader::new(combined.as_slice());
//         let mut read_uuid_dict: HashMap<u32, Uuid> = HashMap::new();

//         // ── Read entry 1 ──
//         {
//             let mut r1 = EntryBufferReader::new(&mut reader, &mut read_uuid_dict);
//             assert_eq!(r1.read_byte().unwrap(), 0x42);
//             assert_eq!(r1.read_varint().unwrap(), 123456789);
//             assert_eq!(r1.read_blob().unwrap(), blob_data);
//             assert_eq!(r1.read_u16_le().unwrap(), 0xBEEF);
//             assert_eq!(r1.read_zigzag().unwrap(), -99);
//             assert_eq!(r1.read_delta(0).unwrap(), ts1); // first timestamp
//             assert_eq!(r1.read_delta(ts1).unwrap(), ts2); // counter bump
//             assert_eq!(r1.read_uuid().unwrap(), UUID_A);
//             assert_eq!(r1.read_uuid().unwrap(), UUID_B);
//             let read_hash1 = r1.finalize().unwrap();
//             assert_eq!(read_hash1, hash1);
//         }

//         // ── Read entry 2 ──
//         {
//             let mut r2 = EntryBufferReader::new(&mut reader, &mut read_uuid_dict);
//             assert_eq!(r2.read_byte().unwrap(), 0x00);
//             assert_eq!(r2.read_varint().unwrap(), 0);
//             assert_eq!(r2.read_blob().unwrap(), b"" as &[u8]);
//             assert_eq!(r2.read_u16_le().unwrap(), 0x0000);
//             assert_eq!(r2.read_zigzag().unwrap(), i64::MIN);
//             assert_eq!(r2.read_delta(ts2).unwrap(), ts3); // 30 second gap
//             assert_eq!(r2.read_uuid().unwrap(), UUID_A); // from dict
//             assert_eq!(r2.read_uuid().unwrap(), UUID_C);
//             let read_hash2 = r2.finalize().unwrap();
//             assert_eq!(read_hash2, hash2);
//         }

//         // Reader-side dict should have all 3 UUIDs.
//         assert_eq!(read_uuid_dict.len(), 3);
//     }

//     /// Goal: a varint with a long run of continuation bytes (more than a u64
//     /// can hold) errors with `VarIntOverflow` rather than spinning past the
//     /// 10-byte maximum or overflowing the shift.
//     ///
//     /// Given: ten `0x80` bytes — every byte keeps the continuation bit set, so
//     ///        the value never terminates within u64's range.
//     /// When:  reading a varint.
//     /// Then:  the 10th byte (shift 63) is rejected as `VarIntOverflow`.
//     #[test]
//     fn read_varint_rejects_overflow() {
//         let data = [0x80u8; 10];
//         let mut reader = EntryBufferReader::new(data.as_slice());
//         assert!(matches!(
//             reader.read_varint_parts(),
//             Err(CodecError::VarIntOverflow)
//         ));
//     }
// }
