use std::ops::Range;

use alloc::borrow::Borrow;

use crate::{
    codec::{
        reader::{ReadError, Reader},
        writer::Writer,
    },
    crypto::{Hash256, Key256Fingerprint, Signature},
    log_entry::{LogDecodeError, LogEncodeError, OpBatch, OpHeader, OpaqueBytes, PlaintextBytes},
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
        cover: Vec<Hash256>,
        // needed if we ever want to support a non-MRAE cipher
        last_leaf_hash: Hash256,
    },
    Signature {
        size: u64,
        signature: Signature,
    },
    // DelegatePubKey {
    //     pubkey: VerifyingKey,
    //     valid_range: Range<u64>,
    //     signature: Signature,
    // },
    // DelegateSignature {
    //     pubkey: VerifyingKey,
    //     signature: Signature,
    // },
    Unknown(UnknownEntryType),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct UnknownEntryType {
    idx: Option<u64>,

    entry_type: u8,
    bytes: Vec<u8>,
    // TODO we need some encrypted flag too otherwise we can't verify hashes of encrypted entries!!
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct EntryRef {
    pub hash: Hash256,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct CipherInfo {
    pub cipher_suite: u8,
    pub fingerprint: Key256Fingerprint,
}

impl<B: alloc::fmt::Debug, H: alloc::fmt::Debug> GenericLogEntry<B, H> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError>
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
                writer.write_range(range)?;
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
            GenericLogEntry::Unknown(unknown_entry_type) => {
                assert_eq!(
                    unknown_entry_type.idx.is_some(),
                    unknown_entry_type.entry_type & 0x80 != 0,
                    "indexed unknown entry type should have its top bit set to 1"
                );
                writer.write_byte(unknown_entry_type.entry_type);
                writer.write_len_prefixed(&unknown_entry_type.bytes);
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
                let range = reader.read_range()?;
                let cover_len = reader.read_var_usize()?;
                // TODO check cover len (easy to determine max size)
                // NOTE don't reserve capacity in the vec to prevent out-of-memory attacks!
                let mut cover = vec![];
                for _ in 0..cover_len {
                    cover.push(reader.read_array()?);
                }
                Self::Expunged { range, cover }
            }
            unknown => {
                let bytes = reader.read_len_prefixed()?;
                let idx = if unknown & 0x80 != 0 {
                    Some(next_entry_index)
                } else {
                    None
                };
                GenericLogEntry::Unknown(UnknownEntryType {
                    idx,
                    entry_type: unknown,
                    bytes: bytes.into(),
                })
            }
        })
    }

    pub(crate) fn next_entry_index(&self) -> Option<u64> {
        match self {
            GenericLogEntry::IndexedEntry { idx, .. } => Some(*idx + 1),
            GenericLogEntry::Expunged { range, .. } => Some(range.end),
            GenericLogEntry::Signature { size, .. } => Some(*size),
            GenericLogEntry::Unknown(UnknownEntryType { idx, .. }) => idx.map(|x| x + 1),
        }
    }

    #[cfg(test)]
    /// Just used for testing to be able to roundtrip random data.
    fn end_index(&self) -> Option<u64> {
        match self {
            GenericLogEntry::IndexedEntry { idx, .. } => Some(*idx),
            GenericLogEntry::Expunged { range, .. } => Some(range.end),
            GenericLogEntry::Signature { size, .. } => Some(*size),
            GenericLogEntry::Unknown(UnknownEntryType { idx, .. }) => *idx,
        }
    }
}

const ENTRY_TYPE_OP_BATCH: u8 = 0x00;
const ENTRY_TYPE_USE_KEY: u8 = 0x01;
const ENTRY_TYPE_SIGNATURE: u8 = 0x02;
const ENTRY_TYPE_EXPUNGED: u8 = 0x03;
const MAX_ENTRY_TYPE_V1: u8 = ENTRY_TYPE_EXPUNGED;

impl CipherInfo {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte(self.cipher_suite);
        writer.write_array(&self.fingerprint.0);
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        let cipher_suite = reader.read_byte()?;
        Ok(CipherInfo {
            cipher_suite,
            fingerprint: Key256Fingerprint(reader.read_array()?),
        })
    }
}

impl EntryRef {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_var_u64(self.index);
        writer.write_array(&self.hash);
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, ReadError> {
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
        let idx = entry.end_index();
        let decoded = GenericLogEntry::decode(&mut r, idx.unwrap_or(1)).unwrap();
        assert_eq!(entry, decoded);
        assert_eq!(idx, decoded.end_index());
    }
}
