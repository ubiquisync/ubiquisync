//! Wire codec for the table op vocabulary.
//!
//! This is the op-specific half of the segment codec: it implements the core
//! [`Op`](ubiquisync_core::codec::Op) trait for this crate's
//! [`Op`](crate::op::Op) enum, encoding and decoding op bodies dispatched by
//! tag. The generic envelope framing (segment header, timestamp deltas, UUID
//! dictionary compression, blake3 trailer, expungement) lives in
//! [`ubiquisync_core::codec`] and drives this code through the trait.
//!
//! Wire shapes are derived from the [type-encoded IDs](crate::id): a
//! [`TableId`] carries its PK column count and per-column types, and a
//! [`ColumnId`] carries its column type — so the decoder knows how to read
//! each value without a schema lookup.

use std::io::BufRead;

use ubiquisync_core::codec::{CodecError, EntryBufferReader, EntryBufferWriter};

use crate::id::{ColType, ColumnId, PkColType, TableId};
use crate::op::{ColValue, ColumnUpdate, Delete, Op, PkValue, Upsert};

// ── Op tags ──────────────────────────────────────────────────────────────────
// Tag 0xFF is reserved by the core codec for expunged entries; op tags must
// avoid it. See `ubiquisync_core::codec::TAG_EXPUNGED`.

/// Insert-or-merge a table row.
pub const TAG_UPSERT: u8 = 0;
/// Soft-delete a table row.
pub const TAG_DELETE: u8 = 1;

// The core codec trait is also named `Op`; fully-qualify it here so it reads
// distinctly from this crate's concrete `Op` enum (no `use` to avoid a clash).
impl ubiquisync_core::codec::Op for Op {
    fn decode<R: BufRead>(tag: u8, r: &mut EntryBufferReader<R>) -> Result<Self, CodecError> {
        decode_one_op(tag, r)
    }

    fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError> {
        encode_one_op(w, self);
        Ok(())
    }
}

// ── Encode ─────────────────────────────────────────────────────────────────

fn encode_one_op(w: &mut EntryBufferWriter, op: &Op) {
    match op {
        Op::Upsert(e) => {
            w.write_byte(TAG_UPSERT);
            w.write_u16_le(e.table_id.into());
            write_pk(w, e.table_id, &e.primary_key);
            w.write_varint(e.updates.len() as u64);
            for cu in &e.updates {
                w.write_byte(cu.column_id.into());
                write_col_value(w, cu.column_id, &cu.value);
            }
            w.write_varint(e.nulls.len() as u64);
            for col_id in &e.nulls {
                w.write_byte((*col_id).into());
            }
        }
        Op::Delete(e) => {
            w.write_byte(TAG_DELETE);
            w.write_u16_le(e.table_id.into());
            write_pk(w, e.table_id, &e.primary_key);
        }
    }
}

fn write_pk(w: &mut EntryBufferWriter, table_id: TableId, pk: &[PkValue]) {
    let pk_count = table_id.pk_count();

    for i in 0..pk_count {
        match &pk[i] {
            PkValue::Bytes(b) => w.write_blob(b),
            PkValue::Uuid(u) => w.write_uuid(u),
            PkValue::Text(s) => w.write_blob(s.as_bytes()),
            PkValue::I64(n) => w.write_zigzag(*n),
        }
    }
}

fn write_col_value(w: &mut EntryBufferWriter, col_id: ColumnId, value: &ColValue) {
    match col_id.col_type() {
        ColType::Bytes => match value {
            ColValue::Bytes(b) => w.write_blob(b),
            _ => unreachable!(),
        },
        ColType::Text => match value {
            ColValue::Text(s) => w.write_blob(s.as_bytes()),
            _ => unreachable!(),
        },
        ColType::I64 => match value {
            ColValue::I64(n) => w.write_zigzag(*n),
            _ => unreachable!(),
        },
        ColType::Uuid => match value {
            ColValue::Uuid(u) => w.write_uuid(u),
            _ => unreachable!(),
        },
        ColType::MaxI64 => match value {
            ColValue::I64(n) => w.write_zigzag(*n),
            _ => unreachable!(),
        },
    }
}

// ── Decode ─────────────────────────────────────────────────────────────────

fn decode_one_op<R: BufRead>(tag: u8, r: &mut EntryBufferReader<R>) -> Result<Op, CodecError> {
    match tag {
        TAG_UPSERT => {
            let table_id = TableId::from(r.read_u16_le()?);
            let primary_key = read_pk(r, table_id)?;
            let update_count = r.read_varint()? as usize;
            let mut updates = Vec::with_capacity(update_count);
            for _ in 0..update_count {
                let column_id = read_column_id(r)?;
                let value = read_col_value(r, column_id)?;
                updates.push(ColumnUpdate { column_id, value });
            }
            let null_count = r.read_varint()? as usize;
            let mut nulls = Vec::with_capacity(null_count);
            for _ in 0..null_count {
                nulls.push(read_column_id(r)?);
            }
            Ok(Op::Upsert(Upsert {
                table_id,
                primary_key,
                updates,
                nulls,
            }))
        }
        TAG_DELETE => {
            let table_id = TableId::from(r.read_u16_le()?);
            let primary_key = read_pk(r, table_id)?;
            Ok(Op::Delete(Delete {
                table_id,
                primary_key,
            }))
        }
        other => Err(CodecError::UnknownTag(other)),
    }
}

fn read_pk<R: BufRead>(
    r: &mut EntryBufferReader<R>,
    table_id: TableId,
) -> Result<Vec<PkValue>, CodecError> {
    let pk_count = table_id.pk_count();
    let mut pk = Vec::with_capacity(pk_count);
    for i in 0..pk_count {
        pk.push(match table_id.pk_col_type(i) {
            PkColType::Bytes => PkValue::Bytes(r.read_blob()?),
            PkColType::Uuid => PkValue::Uuid(r.read_uuid()?),
            PkColType::Text => PkValue::Text(String::from_utf8(r.read_blob()?)?),
            PkColType::I64 => PkValue::I64(r.read_zigzag()?),
        });
    }
    Ok(pk)
}

fn read_col_value<R: BufRead>(
    r: &mut EntryBufferReader<R>,
    col_id: ColumnId,
) -> Result<ColValue, CodecError> {
    match col_id.col_type() {
        ColType::Text => Ok(ColValue::Text(String::from_utf8(r.read_blob()?)?)),
        ColType::Bytes => Ok(ColValue::Bytes(r.read_blob()?)),
        ColType::Uuid => Ok(ColValue::Uuid(r.read_uuid()?)),
        ColType::I64 | ColType::MaxI64 => Ok(ColValue::I64(r.read_zigzag()?)),
    }
}

fn read_column_id<R: BufRead>(r: &mut EntryBufferReader<R>) -> Result<ColumnId, CodecError> {
    let raw = r.read_byte()?;
    ColumnId::try_from_raw(raw).ok_or(CodecError::InvalidColumnType(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ubiquisync_core::codec::{DecodedEntry, Decoder, Encoder};
    use ubiquisync_core::hlc::Timestamp;
    use ubiquisync_core::log_entry::LogEntry;
    use ubiquisync_core::uuid::Uuid;

    // ── Test UUIDs ──────────────────────────────────────────────────────
    const ROW_UUID_1: Uuid = [0x20; 16];
    const PK_UUID: Uuid = [0xE0; 16];
    const USER_1: Uuid = [0xF0; 16];
    const USER_2: Uuid = [0xF1; 16];

    // ── Tables exercising different PK shapes ────────────────────────────
    fn table_bytes_pk() -> TableId {
        TableId::new(&[PkColType::Bytes], 1)
    }
    fn table_uuid_pk() -> TableId {
        TableId::new(&[PkColType::Uuid], 2)
    }
    // Composite PK exercising the Text and I64 PK column types.
    fn table_text_i64_pk() -> TableId {
        TableId::new(&[PkColType::Text, PkColType::I64], 3)
    }

    // ── One ColumnId per col type ────────────────────────────────────────
    fn col_bytes() -> ColumnId {
        ColumnId::new().with_index(0).with_col_type(ColType::Bytes)
    }
    fn col_text() -> ColumnId {
        ColumnId::new().with_index(1).with_col_type(ColType::Text)
    }
    fn col_i64() -> ColumnId {
        ColumnId::new().with_index(2).with_col_type(ColType::I64)
    }
    fn col_uuid() -> ColumnId {
        ColumnId::new().with_index(3).with_col_type(ColType::Uuid)
    }
    fn col_max_i64() -> ColumnId {
        ColumnId::new().with_index(4).with_col_type(ColType::MaxI64)
    }

    // ── Test timestamps (HLC-style) ─────────────────────────────────────
    const TS1: u64 = 1_700_000_000_000 << 16;
    const TS2: u64 = TS1 + (1_000 << 16); // 1s later
    const TS3: u64 = TS2 + (5_000 << 16); // 5s later

    /// Build test entries exercising both op types, all 5 col types, nulls,
    /// and PK shapes covering every PK column type (Bytes, Uuid, Text, I64),
    /// including a composite PK. In server mode, entries alternate user
    /// attribution.
    fn make_test_entries(server_mode: bool) -> Vec<LogEntry<Op>> {
        let user = |u| if server_mode { Some(u) } else { None };
        vec![
            // Upsert with Bytes PK and all 5 col types + nulls.
            LogEntry {
                user_id: user(USER_1),
                timestamp: Timestamp::from_raw(TS1),
                op: Op::Upsert(Upsert {
                    table_id: table_bytes_pk(),
                    primary_key: vec![PkValue::Bytes(b"pk1".to_vec())],
                    updates: vec![
                        ColumnUpdate {
                            column_id: col_bytes(),
                            value: ColValue::Bytes(vec![0xDE, 0xAD]),
                        },
                        ColumnUpdate {
                            column_id: col_text(),
                            value: ColValue::Text("hello".into()),
                        },
                        ColumnUpdate {
                            column_id: col_i64(),
                            value: ColValue::I64(-42),
                        },
                        ColumnUpdate {
                            column_id: col_uuid(),
                            value: ColValue::Uuid(ROW_UUID_1),
                        },
                        ColumnUpdate {
                            column_id: col_max_i64(),
                            value: ColValue::I64(999),
                        },
                    ],
                    nulls: vec![col_text(), col_i64()],
                }),
            },
            // Upsert with a composite (Text, I64) PK and no nulls.
            LogEntry {
                user_id: user(USER_2),
                timestamp: Timestamp::from_raw(TS2),
                op: Op::Upsert(Upsert {
                    table_id: table_text_i64_pk(),
                    primary_key: vec![PkValue::Text("café".into()), PkValue::I64(7)],
                    updates: vec![ColumnUpdate {
                        column_id: col_uuid(),
                        value: ColValue::Uuid(ROW_UUID_1),
                    }],
                    nulls: vec![],
                }),
            },
            // Delete with UUID PK.
            LogEntry {
                user_id: user(USER_1),
                timestamp: Timestamp::from_raw(TS3),
                op: Op::Delete(Delete {
                    table_id: table_uuid_pk(),
                    primary_key: vec![PkValue::Uuid(PK_UUID)],
                }),
            },
        ]
    }

    /// Assert decoded entries match originals field-by-field.
    fn assert_entries_eq(decoded: &[DecodedEntry<Op>], expected: &[LogEntry<Op>]) {
        assert_eq!(decoded.len(), expected.len(), "entry count mismatch");
        for (i, (got, want)) in decoded.iter().zip(expected).enumerate() {
            match got {
                DecodedEntry::LogEntry(got) => {
                    assert_eq!(got.user_id, want.user_id, "entry {i}: user_id");
                    assert_eq!(got.timestamp, want.timestamp, "entry {i}: timestamp");
                    assert_eq!(
                        format!("{:?}", got.op),
                        format!("{:?}", want.op),
                        "entry {i}: op"
                    );
                }
                DecodedEntry::Expunged(_) => panic!("entry {i}: unexpected Expunged"),
            }
        }
    }

    /// Goal: Verify full encode→decode round-trip in device mode.
    ///
    /// Given: entries covering Upsert (all col types + nulls), a composite
    ///        (Text, I64) PK, and Delete, written with no user_id.
    /// When:  Encoded to a buffer and decoded back.
    /// Then:  All entries, timestamps, and ops match exactly.
    #[test]
    fn roundtrip_device_mode() {
        let entries = make_test_entries(false);
        let buf = std::io::Cursor::new(Vec::new());

        let mut encoder = Encoder::new(buf, false).unwrap();
        for entry in &entries {
            encoder
                .encode_entry(&entry.op, entry.timestamp, entry.user_id)
                .unwrap();
        }
        let buf = encoder.sink_mut().get_ref().clone();

        let (decoded, err) = Decoder::<Op, &[u8]>::decode_all(buf.as_slice());
        assert!(err.is_none(), "decode error: {:?}", err);
        let decoded = decoded.unwrap();
        assert!(!decoded.server_mode);
        assert_eq!(decoded.last_timestamp, TS3);
        assert_entries_eq(&decoded.entries, &entries);
    }

    /// Goal: Verify full encode→decode round-trip in server mode with
    ///       multiple users.
    ///
    /// Given: the same entries attributed to alternating users.
    /// When:  Encoded in server mode and decoded back.
    /// Then:  All entries match including per-entry user_id attribution.
    #[test]
    fn roundtrip_server_mode() {
        let entries = make_test_entries(true);
        let buf = std::io::Cursor::new(Vec::new());

        let mut encoder = Encoder::new(buf, true).unwrap();
        for entry in &entries {
            encoder
                .encode_entry(&entry.op, entry.timestamp, entry.user_id)
                .unwrap();
        }
        let buf = encoder.sink_mut().get_ref().clone();

        let (decoded, err) = Decoder::<Op, &[u8]>::decode_all(buf.as_slice());
        assert!(err.is_none(), "decode error: {:?}", err);
        let decoded = decoded.unwrap();
        assert!(decoded.server_mode);
        assert_eq!(decoded.last_timestamp, TS3);
        assert_entries_eq(&decoded.entries, &entries);

        // Verify user attribution pattern.
        let get_user = |e: &DecodedEntry<Op>| match e {
            DecodedEntry::LogEntry(e) => e.user_id,
            DecodedEntry::Expunged(_) => panic!("unexpected Expunged"),
        };
        assert_eq!(get_user(&decoded.entries[0]), Some(USER_1));
        assert_eq!(get_user(&decoded.entries[1]), Some(USER_2));
        assert_eq!(get_user(&decoded.entries[2]), Some(USER_1));
    }
}
