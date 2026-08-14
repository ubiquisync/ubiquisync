use std::ops::Range;

use alloc::borrow::Borrow;

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::{CipherSuite, Hash, Signature},
    log_entry::{DecodeError, EncodeError, OpBatch, OpHeader, OpaqueBytes, PlaintextBytes},
};

/// One decoded entry: a live log entry or an expunged-entry marker.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum GenericLogEntry<Op: std::fmt::Debug, H: std::fmt::Debug> {
    IndexedEntry {
        idx: u64,
        entry: EntryBody<Op, H>,
    },
    Expunged {
        range: Range<u64>,
        cover: Vec<Hash>,
    },
    Signature {
        size: u64,
        signature: Signature,
    },
    SealBranch {
        signature: Signature,
        start: EntryRef,
        end: EntryRef,
        ack_until: Option<EntryRef>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct EntryRef {
    pub hash: Hash,
    pub index: u64,
}

/// Log entry where op and header are encoded as canonical hash bytes (may be encrypted)
pub type OpaqueLogEntry<'a> = GenericLogEntry<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type OpaqueOpBatch<'a> = OpBatch<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type PlaintextLogEntry<'a> = GenericLogEntry<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type PlaintextOpBatch<'a> = OpBatch<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type LogEntry<Op> = GenericLogEntry<Op, OpHeader>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EntryBody<Op: std::fmt::Debug, H: std::fmt::Debug> {
    OpBatch(OpBatch<Op, H>),
    /// Declares the fingerprint for the encryption key being used from
    /// this point forward until the next UseKey op changes the key.
    /// MUST NOT be expunged.
    UseKey(CipherInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct CipherInfo {
    pub cipher_suite: CipherSuite,
    pub fingerprint: Hash,
}

impl<B: alloc::fmt::Debug, H: alloc::fmt::Debug> GenericLogEntry<B, H> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), EncodeError>
    where
        B: Borrow<[u8]>,
        H: Borrow<[u8]>,
    {
        match self {
            GenericLogEntry::IndexedEntry { idx: _, entry } => match entry {
                EntryBody::OpBatch(op_batch) => {
                    writer.write_byte(ENTRY_TYPE_OP_BATCH);
                    op_batch.encode(writer)?;
                }
                EntryBody::UseKey(cipher_info) => {
                    writer.write_byte(ENTRY_TYPE_USE_KEY);
                    cipher_info.encode(writer);
                }
            },
            GenericLogEntry::Expunged { range, cover } => {
                writer.write_byte(ENTRY_TYPE_EXPUNGED);
                writer.write_var_u64(range.start);
                if range.is_empty() {
                    return Err(EncodeError::InvalidExpungeRange);
                }
                let span = range.end - range.start;
                writer.write_var_u64(span);
                writer.write_var_usize(cover.len());
                for h in cover.iter() {
                    writer.write_array(h);
                }
            }
            GenericLogEntry::Signature { size: _, signature } => {
                // NOTE: size is inferred by the last entry, it's the callers responsibility to verify before encoding
                writer.write_byte(ENTRY_TYPE_SIGNATURE);
                signature.encode(writer);
            }
            GenericLogEntry::SealBranch {
                signature,
                start,
                end,
                ack_until,
            } => {
                // NOTE: we don't validate indexes while encoding because it doesn't affect the encoding format
                if let Some(ack_until) = ack_until {
                    writer.write_byte(ENTRY_TYPE_ACKED_SEAL_BRANCH);
                    ack_until.encode(writer);
                } else {
                    writer.write_byte(ENTRY_TYPE_SEAL_BRANCH);
                }
                start.encode(writer);
                end.encode(writer);
                signature.encode(writer);
            }
        }
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>, next_entry_index: u64) -> Result<Self, DecodeError>
    where
        B: From<&'a [u8]>,
        H: From<&'a [u8]>,
    {
        let entry_type = reader.read_byte()?;
        Ok(match entry_type {
            ENTRY_TYPE_OP_BATCH => Self::IndexedEntry {
                idx: next_entry_index,
                entry: EntryBody::OpBatch(OpBatch::decode(reader)?),
            },
            ENTRY_TYPE_SIGNATURE => Self::Signature {
                size: next_entry_index,
                signature: Signature::decode(reader)?,
            },
            ENTRY_TYPE_USE_KEY => Self::IndexedEntry {
                idx: next_entry_index,
                entry: EntryBody::UseKey(CipherInfo::decode(reader)?),
            },
            ENTRY_TYPE_EXPUNGED => {
                let start = reader.read_var_u64()?;
                let span = reader.read_var_u64()?;
                let cover_len = reader.read_var_usize()?;
                // NOTE don't reserve capacity in the vec to prevent out-of-memory attacks!
                let mut cover = vec![];
                for _ in 0..cover_len {
                    cover.push(reader.read_array()?);
                }
                Self::Expunged {
                    range: Range {
                        start,
                        end: start + span,
                    },
                    cover,
                }
            }
            ENTRY_TYPE_SEAL_BRANCH | ENTRY_TYPE_ACKED_SEAL_BRANCH => {
                let ack_until = if entry_type == ENTRY_TYPE_ACKED_SEAL_BRANCH {
                    Some(EntryRef::decode(reader)?)
                } else {
                    None
                };
                let start = EntryRef::decode(reader)?;
                let end = EntryRef::decode(reader)?;
                let signature = Signature::decode(reader)?;
                Self::SealBranch {
                    signature,
                    start,
                    end,
                    ack_until,
                }
            }
            _ => return Err(DecodeError::UnexpectedEntryType(entry_type)),
        })
    }

    pub fn expected_next_index(&self) -> Option<u64> {
        match self {
            GenericLogEntry::IndexedEntry { idx, .. } => Some(idx + 1),
            GenericLogEntry::Expunged { range, .. } => Some(range.end),
            GenericLogEntry::Signature { size, .. } => Some(*size),
            GenericLogEntry::SealBranch { .. } => None,
        }
    }
}

const ENTRY_TYPE_OP_BATCH: u8 = 0x00;
const ENTRY_TYPE_USE_KEY: u8 = 0x01;
const ENTRY_TYPE_SIGNATURE: u8 = 0x02;
const ENTRY_TYPE_EXPUNGED: u8 = 0x03;
const ENTRY_TYPE_SEAL_BRANCH: u8 = 0x04;
const ENTRY_TYPE_ACKED_SEAL_BRANCH: u8 = 0x05;

impl CipherInfo {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte(self.cipher_suite.into());
        writer.write_array(&self.fingerprint);
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let cipher_suite = reader.read_byte()?.try_into().map_err(
            |e: num_enum::TryFromPrimitiveError<CipherSuite>| {
                DecodeError::UnknownCipherSuite(e.number)
            },
        )?;
        Ok(CipherInfo {
            cipher_suite,
            fingerprint: reader.read_array()?,
        })
    }
}

impl EntryRef {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_var_u64(self.index);
        writer.write_array(&self.hash);
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, DecodeError> {
        let index = reader.read_var_u64()?;
        Ok(Self {
            index,
            hash: reader.read_array()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    #[cfg(test)]
    use crate::codec::{reader::Reader, writer::Writer};
    use crate::log_entry::{GenericLogEntry, OpaqueBytes};

    #[proptest]
    fn test_round_trip(entry: GenericLogEntry<OpaqueBytes<'static>, OpaqueBytes<'static>>) {
        let mut w = Writer::new();
        entry.encode(&mut w).unwrap();
        let res = w.finalize();

        let mut r = Reader::new(&res);
        let next_index = entry.expected_next_index();
        let decoded = GenericLogEntry::decode(&mut r, next_index.unwrap_or(1)).unwrap();
        assert_eq!(entry, decoded);
        assert_eq!(next_index, decoded.expected_next_index());
    }
}
