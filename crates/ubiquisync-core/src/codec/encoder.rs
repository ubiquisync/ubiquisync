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

    // fn encode_entry<B: Borrow<[u8]>>(
    //     &mut self,
    //     entry: GenericLogEntry<B, B>,
    // ) -> Result<(), EncodeError> {
    //     match entry {
    //         GenericLogEntry::IndexedEntry { idx, entry } => {
    //             if idx != self.next_entry_index {
    //                 return Err(EncodeError::UnexpectedIndex {
    //                     expected: self.next_entry_index,
    //                     actual: idx,
    //                 });
    //             }
    //             match entry {
    //                 crate::log_entry::EntryBody::OpBatch(op_batch) => {
    //                     self.write_byte(ENTRY_TYPE_OP_BATCH)?;
    //                     let header: &[u8] = op_batch.header.borrow();
    //                     self.write_var_u64(header.len() as u64)?;
    //                     self.write_bytes(header)?;
    //                     self.write_var_u64(op_batch.ops.len() as u64)?;
    //                     for op in op_batch.ops.iter() {
    //                         match op {
    //                             crate::log_entry::OpOrExpunge::Op(op_bytes) => {
    //                                 let op_bytes: &[u8] = op_bytes.borrow();
    //                                 let len = op_bytes.len();
    //                                 if len == 0 {
    //                                     return Err(EncodeError::EmptyOp);
    //                                 }
    //                                 self.write_var_u64(len as u64)?;
    //                                 self.write_bytes(op_bytes)?;
    //                             }
    //                             crate::log_entry::OpOrExpunge::Expunge(hash) => {
    //                                 self.write_byte(SENTINEL_EXPUNGED)?;
    //                                 self.write_bytes(&hash[..])?
    //                             }
    //                         }
    //                     }
    //                 }
    //                 crate::log_entry::EntryBody::UseKey(key) => {
    //                     self.write_byte(ENTRY_TYPE_USE_KEY)?;
    //                     self.write_bytes(&key[..])?
    //                 }
    //             }
    //             self.next_entry_index += 1;
    //         }
    //         GenericLogEntry::Expunged {
    //             start_idx,
    //             end_idx,
    //             cover,
    //         } => {
    //             if start_idx != self.next_entry_index {
    //                 return Err(EncodeError::UnexpectedIndex {
    //                     expected: self.next_entry_index,
    //                     actual: start_idx,
    //                 });
    //             }
    //             if end_idx <= start_idx {
    //                 return Err(EncodeError::InvalidExpungeRecord);
    //             }
    //             let span = end_idx - start_idx;
    //             self.write_byte(ENTRY_TYPE_EXPUNGED)?;
    //             self.write_var_u64(span)?;
    //             let n = cover.len();
    //             if n == 0 {
    //                 return Err(EncodeError::InvalidExpungeRecord);
    //             }
    //             self.write_var_u64(n as u64)?;
    //             for hash in cover.iter() {
    //                 self.write_bytes(&hash[..])?;
    //             }
    //             self.next_entry_index = end_idx;
    //         }
    //         GenericLogEntry::Signature { size, signature } => {
    //             if size != self.next_entry_index {
    //                 return Err(EncodeError::UnexpectedIndex {
    //                     expected: self.next_entry_index,
    //                     actual: size,
    //                 });
    //             }
    //             match signature {
    //                 crate::crypto::Signature::Ed25519(sig) => {
    //                     self.write_byte(SIG_ED25519)?;
    //                     self.write_bytes(&sig[..])?;
    //                 }
    //                 crate::crypto::Signature::P256(sig) => {
    //                     self.write_byte(SIG_P256)?;
    //                     self.write_bytes(&sig[..])?;
    //                 }
    //             }
    //         }
    //     }
    //     Ok(())
    // }
}
