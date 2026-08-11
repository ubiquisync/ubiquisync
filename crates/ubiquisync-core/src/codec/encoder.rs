// use std::{collections::HashMap, io::Write};

// use crate::{
//     codec::{
//         consts::{FLAG_DEVICE, FLAG_SERVER},
//         error::CodecError,
//         hash::{PlaintextOpBatchHasher, hash_use_key},
//         op::Op,
//         writer::EntryBufferWriter,
//     },
//     crypto::{EntryCipher, MmrAccumulator},
//     log_entry::LogEntry,
// };

// /// Streaming encoder for one segment: writes the header on construction, then
// /// appends entries, carrying the cross-entry state (timestamp base, UUID
// /// dictionary) that delta- and dictionary-encoding need.
// pub struct Encoder<E, W> {
//     sink: W,
//     last_timestamp: u64,
//     server_mode: bool,
//     _phantom: std::marker::PhantomData<E>,
// }

// impl<E: Op, W: Write> Encoder<E, W> {
//     /// Create a new encoder.
//     ///
//     /// `magic` is the segment's leading identity bytes — it is **not** defined
//     /// by this library. Each application must supply its own stable, app-unique
//     /// value (the same bytes its decoder expects) so that one app's segments are
//     /// never mistaken for another's when they share a sync location. Use a
//     /// distinct value per app, and prefer same-length magics across apps so one
//     /// cannot be a prefix of another.
//     pub fn new(
//         mut sink: W,
//         next_entry_index: u64,
//         last_timestamp: u64,
//         magic: &[u8],
//         server_mode: bool,
//     ) -> Result<Self, CodecError> {
//         // An empty magic gives zero app isolation — the decoder would compare
//         // zero bytes and accept any header.
//         if magic.is_empty() {
//             return Err(CodecError::BadMagic);
//         }
//         // Write segment header
//         sink.write_all(magic)?;
//         if server_mode {
//             sink.write_all(&[FLAG_SERVER])?;
//         } else {
//             sink.write_all(&[FLAG_DEVICE])?;
//         }
//         todo!()
//         // Ok(Self {
//         //     sink,
//         //     last_timestamp,
//         //     server_mode,
//         //     next_entry_index,
//         //     _phantom: std::marker::PhantomData,
//         // })
//     }

//     /// Mutable access to the underlying writer (e.g. for fsync).
//     pub fn sink_mut(&mut self) -> &mut W {
//         &mut self.sink
//     }

//     /// Encode one log entry. Takes parts by ref/value rather than a full
//     /// `LogEntry<E>` so callers iterating `&[E]` can avoid cloning each op
//     /// just to satisfy a `&LogEntry<E>` argument.
//     ///
//     /// The UUID dictionary and last timestamp are persistent, cross-entry
//     /// state; they are committed only once the entry's bytes are written. A
//     /// failure partway through leaves the encoder exactly as it was, so a
//     /// partly-built entry can never leave behind a UUID definition (or an
//     /// advanced clock) that the flushed bytes don't account for.
//     pub fn encode_entry(&mut self, entry: LogEntry<E>) -> Result<(), CodecError> {
//         match entry {
//             crate::log_entry::GenericLogEntry::IndexedEntry { idx, entry } => {
//                 if idx != self.mmr.size() {
//                     todo!("error")
//                 }
//                 match entry {
//                     crate::log_entry::EntryBody::OpBatch(op_batch) => {
//                         let mut writer = EntryBufferWriter::new();
//                         writer.write_u64_le(op_batch.header.timestamp.raw());
//                         if self.server_mode {
//                             if let Some(server_user_id) = op_batch.header.server_user_id {
//                                 writer.write_byte(1);
//                                 writer.write_uuid(&server_user_id);
//                             } else {
//                                 writer.write_byte(0);
//                             }
//                         }
//                         let header_bytes = writer.finalize();
//                         let mut hasher = PlaintextOpBatchHasher::new(
//                             &self.cipher,
//                             &header_bytes,
//                             idx,
//                             op_batch.ops.len() as u64,
//                         )?;
//                         for op in op_batch.ops.iter() {
//                             match op {
//                                 crate::log_entry::OpOrExpunge::Op(op) => {
//                                     let mut writer = EntryBufferWriter::new();
//                                     op.encode(&mut writer)?;
//                                     let bytes = writer.finalize();
//                                     hasher.append_op(&bytes);
//                                 }
//                                 crate::log_entry::OpOrExpunge::Expunge(hash) => {
//                                     hasher.append_expunged_op(hash)?;
//                                 }
//                             }
//                         }
//                         let hash = hasher.finalize()?;
//                         self.mmr.append(hash.as_bytes());
//                         // TODO actually encode bytes to writer
//                     }
//                     crate::log_entry::EntryBody::UseKey(fingerprint) => {
//                         let hash = hash_use_key(idx, fingerprint);
//                         self.mmr.append(hash.as_bytes());
//                         // TODO actually encode bytes to writer
//                     }
//                 }
//             }
//             crate::log_entry::GenericLogEntry::Expunged {
//                 start_idx,
//                 end_idx,
//                 cover,
//             } => {
//                 if start_idx != self.next_entry_index {
//                     todo!("error")
//                 }
//             }
//             crate::log_entry::GenericLogEntry::Signature { height, signatures } => {}
//         }
//         Ok(())
//         // // Order must match decoder: op (tag + body) → timestamp → server_user_id
//         // op.encode(&mut writer)?;
//         // let raw_timestamp = timestamp.raw();
//         // writer.write_delta(raw_timestamp, self.last_timestamp)?;
//         // if self.server_mode {
//         //     match server_user_id {
//         //         Some(server_user_id) => writer.write_uuid(&server_user_id),
//         //         None => return Err(CodecError::MissingUserId),
//         //     }
//         // }
//         // let (bytes, _) = writer.finalize();
//         // self.sink.write_all(&bytes)?;
//         // // Commit cross-entry state only now that the bytes are written.
//         // self.last_timestamp = raw_timestamp;
//         // self.entry_index += 1;
//         // self.size += bytes.len();
//         // Ok(self.entry_index)
//     }
// }

use std::{borrow::Borrow, io::Write};

use thiserror::Error;

use crate::{
    codec::{
        consts::{
            ENTRY_TYPE_EXPUNGED, ENTRY_TYPE_OP_BATCH, ENTRY_TYPE_USE_KEY, SENTINEL_EXPUNGED,
            SIG_ED25519, SIG_P256,
        },
        varint::{MAX_VAR_U64_SIZE, encode_var_u64},
    },
    log_entry::GenericLogEntry,
};

struct Encoder {
    write: Box<dyn Write>,
    next_entry_index: u64,
}

#[derive(Error, Debug)]
enum EncodeError {
    #[error("unexpected index {actual}, expected {expected}")]
    UnexpectedIndex { expected: u64, actual: u64 },
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("empty op")]
    EmptyOp,
    #[error("invalid expunge record")]
    InvalidExpungeRecord,
}

impl Encoder {
    fn write_byte(&mut self, x: u8) -> Result<(), EncodeError> {
        self.write.write(&[x])?;
        Ok(())
    }

    fn write_var_u64(&mut self, x: u64) -> Result<(), EncodeError> {
        let mut buf = [0; MAX_VAR_U64_SIZE];
        let res = encode_var_u64(x, &mut buf);
        self.write.write(res)?;
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        self.write.write(bytes)?;
        Ok(())
    }

    fn encode_entry<B: Borrow<[u8]>>(
        &mut self,
        entry: GenericLogEntry<B, B>,
    ) -> Result<(), EncodeError> {
        match entry {
            GenericLogEntry::IndexedEntry { idx, entry } => {
                if idx != self.next_entry_index {
                    return Err(EncodeError::UnexpectedIndex {
                        expected: self.next_entry_index,
                        actual: idx,
                    });
                }
                match entry {
                    crate::log_entry::EntryBody::OpBatch(op_batch) => {
                        self.write_byte(ENTRY_TYPE_OP_BATCH)?;
                        let header: &[u8] = op_batch.header.borrow();
                        self.write_var_u64(header.len() as u64)?;
                        self.write_bytes(header)?;
                        self.write_var_u64(op_batch.ops.len() as u64)?;
                        for op in op_batch.ops.iter() {
                            match op {
                                crate::log_entry::OpOrExpunge::Op(op_bytes) => {
                                    let op_bytes: &[u8] = op_bytes.borrow();
                                    let len = op_bytes.len();
                                    if len == 0 {
                                        return Err(EncodeError::EmptyOp);
                                    }
                                    self.write_var_u64(len as u64)?;
                                    self.write_bytes(op_bytes)?;
                                }
                                crate::log_entry::OpOrExpunge::Expunge(hash) => {
                                    self.write_byte(SENTINEL_EXPUNGED)?;
                                    self.write_bytes(&hash[..])?
                                }
                            }
                        }
                    }
                    crate::log_entry::EntryBody::UseKey(key) => {
                        self.write_byte(ENTRY_TYPE_USE_KEY)?;
                        self.write_bytes(&key[..])?
                    }
                }
                self.next_entry_index += 1;
            }
            GenericLogEntry::Expunged {
                start_idx,
                end_idx,
                cover,
            } => {
                if start_idx != self.next_entry_index {
                    return Err(EncodeError::UnexpectedIndex {
                        expected: self.next_entry_index,
                        actual: start_idx,
                    });
                }
                if end_idx <= start_idx {
                    return Err(EncodeError::InvalidExpungeRecord);
                }
                let span = end_idx - start_idx;
                self.write_byte(ENTRY_TYPE_EXPUNGED)?;
                self.write_var_u64(span)?;
                let n = cover.len();
                if n == 0 {
                    return Err(EncodeError::InvalidExpungeRecord);
                }
                self.write_var_u64(n as u64)?;
                for hash in cover.iter() {
                    self.write_bytes(&hash[..])?;
                }
                self.next_entry_index = end_idx;
            }
            GenericLogEntry::Signature { size, signature } => {
                if size != self.next_entry_index {
                    return Err(EncodeError::UnexpectedIndex {
                        expected: self.next_entry_index,
                        actual: size,
                    });
                }
                match signature {
                    crate::crypto::Signature::Ed25519(sig) => {
                        self.write_byte(SIG_ED25519)?;
                        self.write_bytes(&sig[..])?;
                    }
                    crate::crypto::Signature::P256(sig) => {
                        self.write_byte(SIG_P256)?;
                        self.write_bytes(&sig[..])?;
                    }
                }
            }
        }
        Ok(())
    }
}
