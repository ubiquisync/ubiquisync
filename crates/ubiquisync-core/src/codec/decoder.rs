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

pub fn decode_op_header(header_bytes: &PlaintextBytes) -> Result<OpHeader, DecodeError> {
    let mut reader = Reader::new(header_bytes.borrow());
    let timestamp = Timestamp::from_raw(reader.reader_le_u64()?);
    // NOTE: all the remaining header bytes are for the server user id which is len delimited at the layer above this
    let server_user_id_bytes = reader.unwrap();
    let n = server_user_id_bytes.len();
    if n == 0 {
        return Ok(OpHeader {
            server_user_id: None,
            timestamp,
        });
    } else if n != 16 {
        // for now only handle UUIDs
        return Err(DecodeError::Other(format!(
            "expected server_user_id of length 16, got {n}"
        )));
    } else {
        return Ok(OpHeader {
            server_user_id: Some(server_user_id_bytes.try_into().unwrap()), // length already checked above
            timestamp,
        });
    }
}
