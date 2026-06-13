use std::{
    collections::HashMap,
    io::{Read, Write},
};

use crate::{
    codec::{
        consts::{FLAG_DEVICE, FLAG_SERVER, MAGIC},
        error::CodecError,
        op::Op,
        writer::EntryBufferWriter,
    },
    hlc::Timestamp,
    uuid::Uuid,
};

pub struct Encoder<E, W> {
    sink: W,
    last_timestamp: u64,
    uuids: HashMap<Uuid, u32>,
    server_mode: bool,
    entry_index: usize,
    size: usize,
    _phantom: std::marker::PhantomData<E>,
}

impl<E: Op, W: Write + Read> Encoder<E, W> {
    /// Create a new encoder.
    pub fn new(mut sink: W, server_mode: bool) -> Result<Self, CodecError> {
        // Write segment header
        sink.write_all(&MAGIC)?;
        if server_mode {
            sink.write_all(&[FLAG_SERVER])?;
        } else {
            sink.write_all(&[FLAG_DEVICE])?;
        }
        Ok(Self {
            sink,
            last_timestamp: 0,
            uuids: HashMap::default(),
            server_mode,
            entry_index: 0,
            size: 0,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Mutable access to the underlying writer (e.g. for fsync).
    pub fn sink_mut(&mut self) -> &mut W {
        &mut self.sink
    }

    /// Number of entries written so far in this segment.
    pub fn entry_index(&self) -> usize {
        self.entry_index
    }

    /// Encode one log entry. Takes parts by ref/value rather than a full
    /// `LogEntry<E>` so callers iterating `&[E]` can avoid cloning each op
    /// just to satisfy a `&LogEntry<E>` argument.
    pub fn encode_entry(
        &mut self,
        op: &E,
        timestamp: Timestamp,
        user_id: Option<Uuid>,
    ) -> Result<usize, CodecError> {
        let mut writer = EntryBufferWriter::new(&mut self.uuids);
        // Order must match decoder: op (tag + body) → timestamp → user_id
        op.encode(&mut writer)?;
        let raw_timestamp = timestamp.raw();
        writer.write_delta(raw_timestamp, self.last_timestamp)?;
        self.last_timestamp = raw_timestamp;
        if self.server_mode {
            if let Some(user_id) = user_id {
                writer.write_uuid(&user_id);
            } else {
                return Err(CodecError::MissingUserId);
            }
        }
        let (bytes, _) = writer.finalize();
        self.sink.write_all(&bytes)?;
        self.entry_index += 1;
        self.size += bytes.len();
        Ok(self.entry_index)
    }

    pub fn size(&self) -> usize {
        self.size
    }
}
