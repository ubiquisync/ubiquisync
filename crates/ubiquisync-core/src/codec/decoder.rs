use std::collections::HashMap;
use std::io::BufRead;
use std::marker::PhantomData;

use crate::codec::{
    consts::{FLAG_SERVER, MAGIC, TAG_EXPUNGED},
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
    pub fn new(read: R) -> Result<Option<Self>, CodecError> {
        let mut reader = Reader::new(read);
        if reader.is_eof()? {
            return Ok(None);
        }
        let mut magic = [0; 2];
        reader.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(CodecError::BadSegmentMagic);
        }
        let flags = reader.read_byte()?.ok_or(CodecError::UnexpectedEof)?;
        let server_mode = flags & 0x01 == FLAG_SERVER;
        Ok(Some(Self {
            buf: reader,
            last_timestamp: 0,
            uuids: HashMap::default(),
            server_mode,
            _phantom: PhantomData,
        }))
    }

    pub fn decode_entry(&mut self) -> Result<Option<DecodedEntry<E>>, CodecError> {
        if self.buf.is_eof()? {
            return Ok(None);
        }
        let mut reader = EntryBufferReader::new(&mut self.buf, &mut self.uuids);
        let tag = reader.read_byte()?;
        if tag == TAG_EXPUNGED {
            // Expunged entries are just TAG + 32-byte blake3 hash of the
            // original entry. No CRC suffix, no timestamp delta, no
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
        self.last_timestamp = timestamp;
        let user_id = if self.server_mode {
            Some(reader.read_uuid()?)
        } else {
            None
        };
        reader.finalize()?;
        Ok(Some(DecodedEntry::LogEntry(LogEntry {
            user_id,
            timestamp: Timestamp::from_raw(timestamp),
            op: e,
        })))
    }

    // Decodes all entries and returns any error.
    // Entries that were decoded are still returned even if there was an error.
    pub fn decode_all(buf: R) -> (Option<DecodedLogs<E>>, Option<CodecError>) {
        match Self::new(buf) {
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
