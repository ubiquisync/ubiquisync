use std::collections::HashMap;
use std::io::BufRead;
use std::marker::PhantomData;

use crate::codec::{
    consts::{FLAG_DEVICE, FLAG_SERVER, TAG_EXPUNGED},
    error::CodecError,
    op::Op,
    reader::{EntryBufferReader, Reader},
};
use crate::hlc::Timestamp;
use crate::log_entry::LogEntry;
use crate::uuid::Uuid;

pub struct Decoder<E, R> {
    buf: Reader<R>,
    last_timestamp: u64,
    uuids: HashMap<u32, Uuid>,
    server_mode: bool,
    _phantom: PhantomData<E>,
}

pub struct DecodedLogs<E> {
    pub entries: Vec<DecodedEntry<E>>,
    pub uuid_dict: HashMap<Uuid, u32>,
    pub last_timestamp: u64,
    pub server_mode: bool,
}

#[derive(Clone)]
pub enum DecodedEntry<E> {
    LogEntry(LogEntry<E>),
    Expunged(blake3::Hash),
}

impl<E: Op, R: BufRead> Decoder<E, R> {
    /// `magic` is the app-supplied segment identity the encoder wrote (see
    /// [`Encoder::new`](crate::codec::Encoder::new)). A segment whose leading
    /// bytes don't match is rejected as foreign with [`CodecError::BadSegmentMagic`].
    pub fn new(read: R, magic: &[u8]) -> Result<Option<Self>, CodecError> {
        if magic.is_empty() {
            return Err(CodecError::EmptyMagic);
        }
        let mut reader = Reader::new(read);
        if reader.is_eof()? {
            return Ok(None);
        }
        let mut got = vec![0u8; magic.len()];
        reader.read_exact(&mut got)?;
        if got.as_slice() != magic {
            return Err(CodecError::BadSegmentMagic);
        }
        let flags = reader.read_byte()?.ok_or(CodecError::UnexpectedEof)?;
        // Strict: a flag byte that isn't exactly a known mode signals a format
        // we don't understand — reject it rather than masking and silently
        // treating unknown values as device mode.
        let server_mode = match flags {
            FLAG_DEVICE => false,
            FLAG_SERVER => true,
            other => return Err(CodecError::UnknownSegmentFlags(other)),
        };
        Ok(Some(Self {
            buf: reader,
            last_timestamp: 0,
            uuids: HashMap::default(),
            server_mode,
            _phantom: PhantomData,
        }))
    }

    pub fn decode_entry(&mut self) -> Result<Option<DecodedEntry<E>>, CodecError> {
        let dict_len_before = self.uuids.len();
        let result = self.try_decode_entry();
        if result.is_err() {
            // Roll back UUID definitions registered by the failed entry. IDs are
            // assigned sequentially from 1, so any id past the pre-call count
            // came from this entry; dropping them keeps the dictionary (handed
            // back by decode_all for encoder reuse) consistent with only the
            // entries that decoded successfully.
            self.uuids.retain(|id, _| (*id as usize) <= dict_len_before);
        }
        result
    }

    fn try_decode_entry(&mut self) -> Result<Option<DecodedEntry<E>>, CodecError> {
        if self.buf.is_eof()? {
            return Ok(None);
        }
        let mut reader = EntryBufferReader::new(&mut self.buf, &mut self.uuids);
        let tag = reader.read_byte()?;
        if tag == TAG_EXPUNGED {
            // Expunged entries are just TAG + 32-byte blake3 hash of the
            // original entry. No integrity-check suffix, no timestamp delta, no
            // finalize() — the hash itself is the integrity mechanism.
            // last_timestamp is intentionally not updated; segment
            // rewriting recalculates deltas around expunged gaps.
            let hash_bytes = reader.read_bytes(32)?;
            let hash =
                blake3::Hash::from_slice(&hash_bytes).map_err(|_| CodecError::CorruptedLogFile)?;
            return Ok(Some(DecodedEntry::Expunged(hash)));
        }
        let e = E::decode(tag, &mut reader)?;
        let timestamp = reader.read_delta(self.last_timestamp)?;
        let user_id = if self.server_mode {
            Some(reader.read_uuid()?)
        } else {
            None
        };
        reader.finalize()?;
        // Commit cross-entry state only after the integrity check passes.
        self.last_timestamp = timestamp;
        Ok(Some(DecodedEntry::LogEntry(LogEntry {
            user_id,
            timestamp: Timestamp::from_raw(timestamp),
            op: e,
        })))
    }

    // Decodes all entries and returns any error.
    // Entries that were decoded are still returned even if there was an error.
    pub fn decode_all(buf: R, magic: &[u8]) -> (Option<DecodedLogs<E>>, Option<CodecError>) {
        match Self::new(buf, magic) {
            Ok(Some(mut decoder)) => {
                let mut entries = Vec::new();
                let mut err = None;
                loop {
                    match decoder.decode_entry() {
                        Ok(Some(entry)) => entries.push(entry),
                        Ok(None) => break,
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }

                // Invert the UUID dict (id→uuid → uuid→id) for encoder reuse.
                let mut uuid_dict = HashMap::new();
                for (id, uuid) in decoder.uuids.into_iter() {
                    uuid_dict.insert(uuid, id);
                }

                (
                    Some(DecodedLogs {
                        entries,
                        uuid_dict,
                        last_timestamp: decoder.last_timestamp,
                        server_mode: decoder.server_mode,
                    }),
                    err,
                )
            }
            Ok(None) => (None, None),
            Err(e) => (None, Some(e)),
        }
    }
}
