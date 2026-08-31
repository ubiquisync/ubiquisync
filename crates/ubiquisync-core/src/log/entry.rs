use alloc::borrow::Borrow;

use crate::{
    bytes::{OpaqueBytes, PlaintextBytes},
    codec::{Reader, Writer},
    crypto::{CipherInfo, Hash256, Signature},
    log::{LogDecodeError, LogEncodeError, OpBatch},
};

/// One decoded entry: a live log entry or an expunged-entry marker.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum LogEntry<Op: std::fmt::Debug, H: std::fmt::Debug> {
    IndexedEntry { idx: u64, entry: EntryBody<Op, H> },
    Expunged { end_size: u64, end_hash: Hash256 },
    Signature { size: u64, signature: Signature },
    Unknown(UnknownEntry),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum UnknownEntry {
    Indexed {
        idx: u64,
        #[cfg_attr(test, strategy(0..=0x1Fu8))]
        entry_type: u8,
        bytes: Vec<u8>,
        encrypted: bool,
    },
    Unindexed {
        #[cfg_attr(test, strategy(0..=0x3Fu8))]
        entry_type: u8,
        bytes: Vec<u8>,
    },
}

/// Log entry where op and header are encoded as canonical hash bytes (may be encrypted)
pub type OpaqueLogEntry<'a> = LogEntry<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type OpaqueOpBatch<'a> = OpBatch<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type PlaintextLogEntry<'a> = LogEntry<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type PlaintextOpBatch<'a> = OpBatch<PlaintextBytes<'a>, PlaintextBytes<'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EntryBody<Op: std::fmt::Debug, H: std::fmt::Debug> {
    OpBatch(OpBatch<Op, H>),
    /// Declares the fingerprint for the encryption key being used from
    /// this point forward until the next UseKey op changes the key.
    /// MUST NOT be expunged.
    UseKey(CipherInfo),
}

impl<B: alloc::fmt::Debug, H: alloc::fmt::Debug> LogEntry<B, H> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError>
    where
        B: Borrow<[u8]>,
        H: Borrow<[u8]>,
    {
        match self {
            LogEntry::IndexedEntry { idx: _, entry } => match entry {
                EntryBody::OpBatch(op_batch) => {
                    writer.write_byte(ENTRY_TYPE_OP_BATCH);
                    op_batch.encode(writer)?;
                }
                EntryBody::UseKey(cipher_info) => {
                    writer.write_byte(ENTRY_TYPE_USE_KEY);
                    cipher_info.encode(writer);
                }
            },
            LogEntry::Expunged { end_size, end_hash } => {
                writer.write_byte(ENTRY_TYPE_EXPUNGED);
                writer.write_var_u64(*end_size);
                writer.write_array(end_hash);
            }
            LogEntry::Signature { size: _, signature } => {
                // NOTE: size is inferred by the last entry, it's the callers responsibility to verify before encoding
                writer.write_byte(ENTRY_TYPE_SIGNATURE);
                signature.encode(writer);
            }
            LogEntry::Unknown(unknown) => {
                unknown.encode(writer);
            }
        }
        Ok(())
    }

    pub fn decode<'a>(
        reader: &mut Reader<'a>,
        next_entry_index: u64,
    ) -> Result<Self, LogDecodeError>
    where
        B: From<&'a [u8]>,
        H: From<&'a [u8]>,
    {
        let entry_type = reader.read_byte()?;
        Ok(match entry_type {
            ENTRY_TYPE_OP_BATCH => Self::IndexedEntry {
                idx: next_entry_index,
                // TODO max op length
                entry: EntryBody::OpBatch(OpBatch::decode(reader)?),
            },
            ENTRY_TYPE_SIGNATURE => Self::Signature {
                size: next_entry_index,
                signature: Signature::decode(reader)
                    .map_err(LogDecodeError::from_sig_decode_err)?,
            },
            ENTRY_TYPE_USE_KEY => Self::IndexedEntry {
                idx: next_entry_index,
                entry: EntryBody::UseKey(CipherInfo::decode(reader)?),
            },
            ENTRY_TYPE_EXPUNGED => {
                let end_size = reader.read_var_u64()?;
                let end_hash = reader.read_array()?;
                Self::Expunged { end_size, end_hash }
            }
            unknown => {
                if unknown & 0x80 == 0x80 {
                    // entry is "forward-compatible" so we can at least transparently hash and encrypt it
                    Self::Unknown(UnknownEntry::decode(unknown, reader, next_entry_index)?)
                } else {
                    return Err(LogDecodeError::UndecodableEntryType(unknown));
                }
            }
        })
    }

    pub(crate) fn next_entry_index(&self) -> Option<u64> {
        match self {
            LogEntry::IndexedEntry { idx, .. } => Some(*idx + 1),
            LogEntry::Expunged { end_size, .. } => Some(*end_size),
            LogEntry::Signature { size, .. } => Some(*size),
            LogEntry::Unknown(UnknownEntry::Indexed { idx, .. }) => Some(*idx + 1),
            _ => None,
        }
    }

    #[cfg(test)]
    /// Just used for testing to be able to roundtrip random data.
    fn end_index(&self) -> Option<u64> {
        match self {
            LogEntry::IndexedEntry { idx, .. } => Some(*idx),
            LogEntry::Expunged { end_size, .. } => Some(*end_size),
            LogEntry::Signature { size, .. } => Some(*size),
            LogEntry::Unknown(UnknownEntry::Indexed { idx, .. }) => Some(*idx),
            _ => None,
        }
    }
}

impl UnknownEntry {
    fn encode(&self, writer: &mut Writer) {
        match self {
            UnknownEntry::Indexed {
                entry_type,
                bytes,
                encrypted,
                ..
            } => {
                let mut entry_type = *entry_type | 0x80; // forward-compatible flag
                if *encrypted {
                    entry_type |= 0x20; // encrypted flag
                }
                writer.write_byte(entry_type);
                writer.write_len_prefixed(bytes);
            }
            UnknownEntry::Unindexed { entry_type, bytes } => {
                let mut entry_type = *entry_type | 0x80; // forward-compatible flag
                entry_type |= 0x40; // unindexed flag
                writer.write_byte(entry_type);
                writer.write_len_prefixed(bytes);
            }
        }
    }

    // NOTE: does not read the entry type byte but expects it to be passed in by the caller
    fn decode<'a>(
        mut entry_type: u8,
        reader: &mut Reader<'a>,
        next_entry_index: u64,
    ) -> Result<Self, LogDecodeError> {
        assert_eq!(entry_type & 0x80, 0x80);
        let bytes = reader.read_len_prefixed()?;
        Ok(if entry_type & 0x40 == 0x40 {
            entry_type &= 0x3F; // clear top 2 flag bits
            // unindexed
            Self::Unindexed {
                entry_type,
                bytes: bytes.into(),
            }
        } else {
            // indexed
            let encrypted = entry_type & 0x20 == 0x20;
            entry_type &= 0x1F; // clear top 3 flag bits
            Self::Indexed {
                idx: next_entry_index,
                entry_type,
                bytes: bytes.into(),
                encrypted,
            }
        })
    }
}

const ENTRY_TYPE_OP_BATCH: u8 = 0x00;
const ENTRY_TYPE_USE_KEY: u8 = 0x01;
const ENTRY_TYPE_SIGNATURE: u8 = 0x02;
const ENTRY_TYPE_EXPUNGED: u8 = 0x03;

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    use crate::log::LogEntry;
    #[cfg(test)]
    use crate::{
        bytes::OpaqueBytes,
        codec::{Reader, Writer},
    };

    #[proptest]
    fn test_round_trip(entry: LogEntry<OpaqueBytes<'static>, OpaqueBytes<'static>>) {
        let mut w = Writer::new();
        entry.encode(&mut w).unwrap();
        let res = w.finalize();

        let mut r = Reader::new(&res);
        let idx = entry.end_index();
        let decoded = LogEntry::decode(&mut r, idx.unwrap_or(1)).unwrap();
        assert_eq!(entry, decoded);
        assert_eq!(idx, decoded.end_index());
    }
}
