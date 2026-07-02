use std::{collections::HashMap, io::Write};

use crate::{
    codec::{
        consts::{FLAG_DEVICE, FLAG_SERVER},
        error::CodecError,
        op::Op,
        writer::EntryBufferWriter,
    },
    hlc::Timestamp,
    uuid::Uuid,
};

/// Streaming encoder for one segment: writes the header on construction, then
/// appends entries, carrying the cross-entry state (timestamp base, UUID
/// dictionary) that delta- and dictionary-encoding need.
pub struct Encoder<E, W> {
    sink: W,
    last_timestamp: u64,
    uuids: HashMap<Uuid, u32>,
    server_mode: bool,
    entry_index: usize,
    size: usize,
    _phantom: std::marker::PhantomData<E>,
}

impl<E: Op, W: Write> Encoder<E, W> {
    /// Create a new encoder.
    ///
    /// `magic` is the segment's leading identity bytes — it is **not** defined
    /// by this library. Each application must supply its own stable, app-unique
    /// value (the same bytes its decoder expects) so that one app's segments are
    /// never mistaken for another's when they share a sync location. Use a
    /// distinct value per app, and prefer same-length magics across apps so one
    /// cannot be a prefix of another.
    pub fn new(mut sink: W, magic: &[u8], server_mode: bool) -> Result<Self, CodecError> {
        // An empty magic gives zero app isolation — the decoder would compare
        // zero bytes and accept any header.
        if magic.is_empty() {
            return Err(CodecError::BadMagic);
        }
        // Write segment header
        sink.write_all(magic)?;
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
    ///
    /// The UUID dictionary and last timestamp are persistent, cross-entry
    /// state; they are committed only once the entry's bytes are written. A
    /// failure partway through leaves the encoder exactly as it was, so a
    /// partly-built entry can never leave behind a UUID definition (or an
    /// advanced clock) that the flushed bytes don't account for.
    pub fn encode_entry(
        &mut self,
        op: &E,
        timestamp: Timestamp,
        server_user_id: Option<Uuid>,
    ) -> Result<usize, CodecError> {
        let dict_len_before = self.uuids.len();
        let result = self.try_encode_entry(op, timestamp, server_user_id);
        if result.is_err() {
            // Roll back UUID definitions registered before the failure. IDs are
            // handed out sequentially from 1, so any id past the pre-call count
            // belongs to this aborted entry.
            self.uuids.retain(|_, id| (*id as usize) <= dict_len_before);
        }
        result
    }

    fn try_encode_entry(
        &mut self,
        op: &E,
        timestamp: Timestamp,
        server_user_id: Option<Uuid>,
    ) -> Result<usize, CodecError> {
        let mut writer = EntryBufferWriter::new(&mut self.uuids);
        // Order must match decoder: op (tag + body) → timestamp → server_user_id
        op.encode(&mut writer)?;
        let raw_timestamp = timestamp.raw();
        writer.write_delta(raw_timestamp, self.last_timestamp)?;
        if self.server_mode {
            match server_user_id {
                Some(server_user_id) => writer.write_uuid(&server_user_id),
                None => return Err(CodecError::MissingUserId),
            }
        }
        let (bytes, _) = writer.finalize();
        self.sink.write_all(&bytes)?;
        // Commit cross-entry state only now that the bytes are written.
        self.last_timestamp = raw_timestamp;
        self.entry_index += 1;
        self.size += bytes.len();
        Ok(self.entry_index)
    }

    /// Total entry bytes written so far (the segment header is not counted),
    /// for deciding when to roll over to a new segment.
    pub fn size(&self) -> usize {
        self.size
    }
}
