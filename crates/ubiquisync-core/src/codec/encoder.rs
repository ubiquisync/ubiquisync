use std::{collections::HashMap, io::Write};

use crate::{
    codec::{
        consts::{FLAG_DEVICE, FLAG_SERVER},
        error::CodecError,
        hash::{PlaintextOpBatchHasher, hash_use_key},
        op::Op,
        writer::EntryBufferWriter,
    },
    crypto::{EntryCipher, MmrAccumulator},
    log_entry::LogEntry,
};

/// Streaming encoder for one segment: writes the header on construction, then
/// appends entries, carrying the cross-entry state (timestamp base, UUID
/// dictionary) that delta- and dictionary-encoding need.
pub struct Encoder<E, W> {
    sink: W,
    last_timestamp: u64,
    server_mode: bool,
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
    pub fn new(
        mut sink: W,
        next_entry_index: u64,
        last_timestamp: u64,
        magic: &[u8],
        server_mode: bool,
    ) -> Result<Self, CodecError> {
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
        todo!()
        // Ok(Self {
        //     sink,
        //     last_timestamp,
        //     server_mode,
        //     next_entry_index,
        //     _phantom: std::marker::PhantomData,
        // })
    }

    /// Mutable access to the underlying writer (e.g. for fsync).
    pub fn sink_mut(&mut self) -> &mut W {
        &mut self.sink
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
    pub fn encode_entry(&mut self, entry: LogEntry<E>) -> Result<(), CodecError> {
        match entry {
            crate::log_entry::GenericLogEntry::IndexedEntry { idx, entry } => {
                if idx != self.mmr.size() {
                    todo!("error")
                }
                match entry {
                    crate::log_entry::EntryBody::OpBatch(op_batch) => {
                        let mut writer = EntryBufferWriter::new();
                        writer.write_u64_le(op_batch.header.timestamp.raw());
                        if self.server_mode {
                            if let Some(server_user_id) = op_batch.header.server_user_id {
                                writer.write_byte(1);
                                writer.write_uuid(&server_user_id);
                            } else {
                                writer.write_byte(0);
                            }
                        }
                        let header_bytes = writer.finalize();
                        let mut hasher = PlaintextOpBatchHasher::new(
                            &self.cipher,
                            &header_bytes,
                            idx,
                            op_batch.ops.len() as u64,
                        )?;
                        for op in op_batch.ops.iter() {
                            match op {
                                crate::log_entry::OpOrExpunge::Op(op) => {
                                    let mut writer = EntryBufferWriter::new();
                                    op.encode(&mut writer)?;
                                    let bytes = writer.finalize();
                                    hasher.append_op(&bytes);
                                }
                                crate::log_entry::OpOrExpunge::Expunge(hash) => {
                                    hasher.append_expunged_op(hash)?;
                                }
                            }
                        }
                        let hash = hasher.finalize()?;
                        self.mmr.append(hash.as_bytes());
                        // TODO actually encode bytes to writer
                    }
                    crate::log_entry::EntryBody::UseKey(fingerprint) => {
                        let hash = hash_use_key(idx, fingerprint);
                        self.mmr.append(hash.as_bytes());
                        // TODO actually encode bytes to writer
                    }
                }
            }
            crate::log_entry::GenericLogEntry::Expunged {
                start_idx,
                end_idx,
                cover,
            } => {
                if start_idx != self.next_entry_index {
                    todo!("error")
                }
            }
            crate::log_entry::GenericLogEntry::Signature { height, signatures } => {}
        }
        Ok(())
        // // Order must match decoder: op (tag + body) → timestamp → server_user_id
        // op.encode(&mut writer)?;
        // let raw_timestamp = timestamp.raw();
        // writer.write_delta(raw_timestamp, self.last_timestamp)?;
        // if self.server_mode {
        //     match server_user_id {
        //         Some(server_user_id) => writer.write_uuid(&server_user_id),
        //         None => return Err(CodecError::MissingUserId),
        //     }
        // }
        // let (bytes, _) = writer.finalize();
        // self.sink.write_all(&bytes)?;
        // // Commit cross-entry state only now that the bytes are written.
        // self.last_timestamp = raw_timestamp;
        // self.entry_index += 1;
        // self.size += bytes.len();
        // Ok(self.entry_index)
    }
}
