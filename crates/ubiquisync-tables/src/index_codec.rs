use std::collections::HashMap;
use std::io::BufRead;

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
                value: encode_index_value(upsert)?,
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
    // Reuse the wire codec for the body bytes, but take them without the
    // integrity-hash trailer `finalize` would append — the op-log column
    // stores raw key bytes and the entry hash already lives in the log.
    let mut dict = HashMap::new();
    let mut w = EntryBufferWriter::new(&mut dict);
    encode_one_key(&mut w, table_id, pkey)?;
    Ok(w.into_bytes())
}

fn encode_index_value(e: &Upsert) -> Result<Vec<u8>, CodecError> {
    // See `encode_index_key`: body bytes only, no integrity-hash trailer.
    let mut dict = HashMap::new();
    let mut w = EntryBufferWriter::new(&mut dict);
    encode_one_value(&mut w, e)?;
    Ok(w.into_bytes())
}

pub fn decode_index_key(key: &[u8]) -> Result<(TableId, Vec<Value>), CodecError> {
    let mut dict = HashMap::new();
    let mut r = Reader::new(key);
    let decoded = {
        let mut ebr = EntryBufferReader::new(&mut r, &mut dict);
        decode_one_key(&mut ebr)?
    };
    reject_trailing(&mut r)?;
    Ok(decoded)
}

pub fn decode_index_value(value: &[u8]) -> Result<(Vec<ColumnSet>, Vec<ColumnId>), CodecError> {
    let mut dict = HashMap::new();
    let mut r = Reader::new(value);
    let decoded = {
        let mut ebr = EntryBufferReader::new(&mut r, &mut dict);
        decode_one_value(&mut ebr)?
    };
    reject_trailing(&mut r)?;
    Ok(decoded)
}

/// An index blob must decode to exactly its bytes. Leftover bytes signal a
/// mis-decode — a wrong column, appended garbage, or decoder drift — so reject
/// them rather than silently ignoring them. (These blobs carry no integrity
/// hash; the authoritative log does.)
fn reject_trailing<R: BufRead>(r: &mut Reader<R>) -> Result<(), CodecError> {
    if r.is_eof()? {
        Ok(())
    } else {
        Err(CodecError::TrailingBytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::col_type::ColType;

    fn col(index: u8, ct: ColType) -> ColumnId {
        ColumnId::new().with_index(index).with_col_type(ct)
    }

    // Upsert exercising a composite PK plus one column of every wire shape, and
    // a nulled column.
    fn rich_upsert() -> Op {
        Op::Upsert(Upsert {
            table_id: TableId::new(&[ColType::Text, ColType::I64], 3),
            primary_key: vec![Value::Text("café".into()), Value::I64(7)],
            sets: vec![
                ColumnSet {
                    column_id: col(1, ColType::Bytes),
                    value: Value::Bytes(vec![0xDE, 0xAD]),
                },
                ColumnSet {
                    column_id: col(2, ColType::I64),
                    value: Value::I64(-42),
                },
                ColumnSet {
                    column_id: col(3, ColType::Uuid),
                    value: Value::Uuid([0x20; 16]),
                },
            ],
            nulls: vec![col(4, ColType::Text)],
        })
    }

    fn delete() -> Op {
        Op::Delete(Delete {
            table_id: TableId::new(&[ColType::Uuid], 2),
            primary_key: vec![Value::Uuid([0xE0; 16])],
        })
    }

    // Upsert whose PK *and* a column value carry the *same* UUID. `key` and
    // `value` are encoded (and decoded) with independent dictionaries, so the
    // value's UUID must serialize inline rather than as a back-reference into
    // the key's dictionary — this op only round-trips if that holds.
    fn shared_uuid_upsert() -> Op {
        let u = [0x5A; 16];
        Op::Upsert(Upsert {
            table_id: TableId::new(&[ColType::Uuid], 9),
            primary_key: vec![Value::Uuid(u)],
            sets: vec![ColumnSet {
                column_id: col(1, ColType::Uuid),
                value: Value::Uuid(u),
            }],
            nulls: vec![],
        })
    }

    /// Goal: an op survives the split into `(tag, key, value)` and back, for
    /// both variants — `from_index_parts` is the exact inverse of
    /// `to_index_entry`. Includes an op that shares a UUID across the
    /// key/value boundary to pin the independent-dictionary behavior.
    #[test]
    fn index_entry_round_trip() {
        for op in [rich_upsert(), delete(), shared_uuid_upsert()] {
            let e = op.to_index_entry().unwrap();
            let back = Op::from_index_parts(e.tag, &e.key, &e.value).unwrap();
            assert_eq!(op, back, "round trip for {op:?}");
        }
    }

    /// Goal: the `key`/`value` bytes are the raw op body only — no integrity
    /// hash trailer is appended (a 4-byte blake3 prefix used to leak in via
    /// `finalize`).
    #[test]
    fn index_bytes_have_no_hash_trailer() {
        // Single Bytes PK "pk1": 2 (table id u16) + 1 (blob len) + 3 (bytes).
        let key = encode_index_key(
            TableId::new(&[ColType::Bytes], 1),
            &[Value::Bytes(b"pk1".to_vec())],
        )
        .unwrap();
        assert_eq!(key.len(), 6, "key must not carry a 4-byte hash trailer");

        // Empty upsert body: one zero varint for `sets`, one for `nulls`.
        let value = encode_index_value(&Upsert {
            table_id: TableId::new(&[ColType::Bytes], 1),
            primary_key: vec![Value::Bytes(b"pk1".to_vec())],
            sets: vec![],
            nulls: vec![],
        })
        .unwrap();
        assert_eq!(value.len(), 2, "value must not carry a 4-byte hash trailer");
    }

    /// Goal: the `key` is pure row identity — an upsert and a delete addressing
    /// the same `(table, primary key)` produce byte-identical keys, and the
    /// delete's `value` is empty.
    #[test]
    fn key_is_identity_only() {
        let table = TableId::new(&[ColType::Uuid], 2);
        let pk = vec![Value::Uuid([0xE0; 16])];
        let upsert = Op::Upsert(Upsert {
            table_id: table,
            primary_key: pk.clone(),
            sets: vec![ColumnSet {
                column_id: col(1, ColType::I64),
                value: Value::I64(1),
            }],
            nulls: vec![],
        })
        .to_index_entry()
        .unwrap();
        let delete = delete().to_index_entry().unwrap();
        assert_eq!(upsert.key, delete.key, "key depends only on table + PK");
        assert!(delete.value.is_empty(), "delete carries no payload");
    }

    /// Goal: an unknown tag is rejected rather than silently mis-decoded.
    #[test]
    fn from_index_parts_rejects_unknown_tag() {
        let err = Op::from_index_parts(0x7E, &[], &[]).unwrap_err();
        assert!(matches!(err, CodecError::UnknownTag(0x7E)), "got {err:?}");
    }

    /// Goal: trailing bytes after a valid key/value are rejected, not silently
    /// ignored — they signal a mis-decode.
    #[test]
    fn decode_rejects_trailing_bytes() {
        let e = rich_upsert().to_index_entry().unwrap();

        let mut key = e.key.clone();
        key.push(0xAB);
        assert!(
            matches!(decode_index_key(&key), Err(CodecError::TrailingBytes)),
            "trailing key byte must be rejected"
        );

        let mut value = e.value.clone();
        value.push(0xAB);
        assert!(
            matches!(decode_index_value(&value), Err(CodecError::TrailingBytes)),
            "trailing value byte must be rejected"
        );
    }
}
