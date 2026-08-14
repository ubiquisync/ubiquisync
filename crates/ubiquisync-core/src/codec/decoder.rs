use std::borrow::Borrow;

use thiserror::Error;

use crate::{
    codec::{
        consts::{
            ENTRY_TYPE_EXPUNGED, ENTRY_TYPE_OP_BATCH, ENTRY_TYPE_SIGNATURE, ENTRY_TYPE_USE_KEY,
            SIG_ED25519, SIG_P256,
        },
        reader::{ReadError, Reader},
    },
    crypto::HASH_SIZE,
    hlc::Timestamp,
    log_entry::{GenericLogEntry, OpHeader, OpOrExpunge, PlaintextBytes},
};

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("unexpected entry type: {0}")]
    UnexpectedEntryType(u8),
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("other: {0}")]
    Other(String),
}

// fn decode_one<'a, B: From<&'a [u8]>>(
//     reader: &mut Reader<'a>,
// ) -> Result<GenericLogEntry<B, B>, DecodeError> {
//     let entry_type = reader.read_byte()?;
//     match entry_type {
//         ENTRY_TYPE_OP_BATCH => {
//             let header_len = reader.read_usize()?;
//             let header_bytes = reader.read_slice(header_len)?;
//             let num_ops = reader.read_usize()?;
//             let mut ops: Vec<OpOrExpunge<B>> = Vec::new();
//             // NOTE: don't reserve a count in the Vec because this could be an out-of-memory attack surfaces if the number is large
//             for _ in 0..num_ops {
//                 let op_len = reader.read_usize()?;
//                 if op_len == 0 {
//                     let hash = reader.read_slice(HASH_SIZE)?;
//                     ops.push(OpOrExpunge::Expunge(hash.try_into().unwrap()));
//                 } else {
//                     let op_bytes = reader.read_slice(op_len)?;
//                     ops.push(OpOrExpunge::Op(op_bytes.into()))
//                 }
//             }
//             Ok(GenericLogEntry::IndexedEntry {
//                 idx: 0, // TODO
//                 entry: crate::log_entry::EntryBody::OpBatch(crate::log_entry::OpBatch {
//                     header: header_bytes.into(),
//                     ops,
//                 }),
//             })
//         }
//         ENTRY_TYPE_SIGNATURE => {
//             let size = reader.read_var_u64()?;
//             let sig_type = reader.read_byte()?;
//             match sig_type {
//                 SIG_ED25519 => {}
//                 SIG_P256 => {}
//                 _ => {}
//             }
//         }
//         ENTRY_TYPE_USE_KEY => {}
//         ENTRY_TYPE_EXPUNGED => {}
//         _ => return Err(DecodeError::UnexpectedEntryType(entry_type)),
//     }
// }
