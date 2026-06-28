use std::collections::HashMap;

use ubiquisync_core::codec::{
    CodecError, EntryBufferReader, EntryBufferWriter, IndexableOp, OpIndexEntry, Reader,
};

use crate::{
    codec::{
        TAG_DELETE, TAG_UPSERT, decode_one_key, decode_one_value, encode_one_key, encode_one_value,
    },
    id::{ColumnId, TableId},
    op::{ColumnSet, Delete, Op, Upsert, Value},
};

impl IndexableOp for Op {
    fn to_index_entry(&self) -> Result<OpIndexEntry, CodecError> {
        match self {
            Op::Upsert(upsert) => Ok(OpIndexEntry {
                tag: TAG_UPSERT,
                key: encode_index_key(upsert.table_id, &upsert.primary_key)?,
                value: encode_index_value(&upsert)?,
            }),
            Op::Delete(delete) => Ok(OpIndexEntry {
                tag: TAG_DELETE,
                key: encode_index_key(delete.table_id, &delete.primary_key)?,
                value: vec![],
            }),
        }
    }

    fn from_index_parts(tag: u8, key: &[u8], value: &[u8]) -> Result<Self, CodecError> {
        match tag {
            TAG_UPSERT => {
                let (table_id, primary_key) = decode_index_key(key)?;
                let (sets, nulls) = decode_index_value(value)?;
                Ok(Op::Upsert(Upsert {
                    table_id,
                    primary_key,
                    sets,
                    nulls,
                }))
            }
            TAG_DELETE => {
                let (table_id, primary_key) = decode_index_key(key)?;
                Ok(Op::Delete(Delete {
                    table_id,
                    primary_key,
                }))
            }
            other => Err(CodecError::UnknownTag(other)),
        }
    }
}

pub fn encode_index_key(table_id: TableId, pkey: &[Value]) -> Result<Vec<u8>, CodecError> {
    // TODO actually doing hashing is unnecessary but it lets us reuse our existing codec
    // in the future we could remove hashing
    let mut dict = HashMap::new();
    let mut w = EntryBufferWriter::new(&mut dict);
    encode_one_key(&mut w, table_id, pkey)?;
    let (res, _) = w.finalize();
    Ok(res)
}

fn encode_index_value(e: &Upsert) -> Result<Vec<u8>, CodecError> {
    // TODO actually doing hashing is unnecessary but it lets us reuse our existing codec
    // in the future we could remove hashing
    let mut dict = HashMap::new();
    let mut w = EntryBufferWriter::new(&mut dict);
    encode_one_value(&mut w, e)?;
    let (res, _) = w.finalize();
    Ok(res)
}

pub fn decode_index_key(key: &[u8]) -> Result<(TableId, Vec<Value>), CodecError> {
    let mut dict = HashMap::new();
    let mut r = EntryBufferReader::new(&mut Reader::new(key), &mut dict);
    Ok(decode_one_key(&mut r)?)
}

pub fn decode_index_value(key: &[u8]) -> Result<(Vec<ColumnSet>, Vec<ColumnId>), CodecError> {
    let mut dict = HashMap::new();
    let mut r = EntryBufferReader::new(&mut Reader::new(key), &mut dict);
    Ok(decode_one_value(&mut r)?)
}
