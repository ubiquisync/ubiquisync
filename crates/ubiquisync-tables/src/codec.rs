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

use crate::col_type::ColType;
use crate::id::{ColumnId, TableId};
use crate::op::{ColumnSet, Delete, Op, Upsert, Value};
use ubiquisync_core::codec::{CodecError, EntryBufferReader, EntryBufferWriter};

// ── Op tags ──────────────────────────────────────────────────────────────────
// Tag 0xFF is reserved by the core codec for expunged entries; op tags must
// avoid it. See `ubiquisync_core::codec::TAG_EXPUNGED`.

/// Insert-or-merge a table row.
pub const TAG_UPSERT: u8 = 0;
/// Soft-delete a table row.
pub const TAG_DELETE: u8 = 1;

// Fully-qualified: the core trait and this crate's enum are both named `Op`.
impl ubiquisync_core::codec::Op for Op {
    fn decode<R: BufRead>(tag: u8, r: &mut EntryBufferReader<R>) -> Result<Self, CodecError> {
        decode_one_op(tag, r)
    }

    fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError> {
        encode_one_op(w, self)
    }
}

// ── Text validation ──────────────────────────────────────────────────────────

/// Reject text carrying an embedded NUL. The protocol forbids `\0` in Text
/// because SQLite stores it while Postgres rejects it, so an unchecked NUL is a
/// value one backend physically cannot hold — a silent divergence. Enforced on
/// both encode and decode, for Text PKs and Text columns. (Strict UTF-8 is
/// already guaranteed: encode starts from a `String`, decode goes through
/// `String::from_utf8`.)
fn check_text(s: &str) -> Result<(), CodecError> {
    if s.as_bytes().contains(&0u8) {
        return Err(CodecError::TextContainsNul);
    }
    Ok(())
}

// ── Encode ─────────────────────────────────────────────────────────────────

fn encode_one_op(w: &mut EntryBufferWriter, op: &Op) -> Result<(), CodecError> {
    match op {
        Op::Upsert(e) => {
            w.write_byte(TAG_UPSERT);
            encode_one_key(w, e.table_id, &e.primary_key)?;
            encode_one_value(w, e)
        }
        Op::Delete(e) => {
            w.write_byte(TAG_DELETE);
            encode_one_key(w, e.table_id, &e.primary_key)
        }
    }
}

pub(crate) fn encode_one_key(
    w: &mut EntryBufferWriter,
    table_id: TableId,
    pkey: &[Value],
) -> Result<(), CodecError> {
    w.write_u16_le(table_id.into());
    write_pk(w, table_id, pkey)
}

pub(crate) fn encode_one_value(w: &mut EntryBufferWriter, e: &Upsert) -> Result<(), CodecError> {
    w.write_varint(e.sets.len() as u64);
    for set in &e.sets {
        w.write_byte(set.column_id.into());
        write_col_value(w, set.column_id, &set.value)?;
    }
    w.write_varint(e.nulls.len() as u64);
    for col_id in &e.nulls {
        w.write_byte((*col_id).into());
    }
    Ok(())
}

pub(crate) fn write_pk(
    w: &mut EntryBufferWriter,
    table_id: TableId,
    pk: &[Value],
) -> Result<(), CodecError> {
    let pk_count = table_id.pk_count();
    // A wrong number of PK values is a caller error.
    if pk.len() != pk_count {
        return Err(CodecError::PkCountMismatch {
            expected: pk_count,
            got: pk.len(),
        });
    }

    // Encode each PK value per the type the table ID declares — not the variant
    // the caller happened to pass — so the bytes always match what the decoder
    // reads back. A variant that disagrees with the declared type is a caller
    // error (it would otherwise serialize with the wrong wire shape).
    for (i, v) in pk.iter().enumerate() {
        match (table_id.pk_col_type(i), v) {
            (ColType::Bytes, Value::Bytes(b)) => w.write_blob(b),
            (ColType::Uuid, Value::Uuid(u)) => w.write_uuid(u),
            (ColType::Text, Value::Text(s)) => {
                check_text(s)?;
                w.write_blob(s.as_bytes());
            }
            (ColType::I64, Value::I64(n)) => w.write_zigzag(*n),
            _ => return Err(CodecError::PkValueMismatch),
        }
    }
    Ok(())
}

pub(crate) fn write_col_value(
    w: &mut EntryBufferWriter,
    col_id: ColumnId,
    value: &Value,
) -> Result<(), CodecError> {
    // The value variant must match the column ID's declared type; a mismatch
    // is a caller error.
    match col_id.col_type() {
        ColType::Bytes => match value {
            Value::Bytes(b) => w.write_blob(b),
            _ => return Err(CodecError::ColumnValueMismatch),
        },
        ColType::Text => match value {
            Value::Text(s) => {
                check_text(s)?;
                w.write_blob(s.as_bytes());
            }
            _ => return Err(CodecError::ColumnValueMismatch),
        },
        ColType::I64 => match value {
            Value::I64(n) => w.write_zigzag(*n),
            _ => return Err(CodecError::ColumnValueMismatch),
        },
        ColType::Uuid => match value {
            Value::Uuid(u) => w.write_uuid(u),
            _ => return Err(CodecError::ColumnValueMismatch),
        },
    }
    Ok(())
}

// ── Decode ─────────────────────────────────────────────────────────────────

fn decode_one_op<R: BufRead>(tag: u8, r: &mut EntryBufferReader<R>) -> Result<Op, CodecError> {
    match tag {
        TAG_UPSERT => {
            let (table_id, primary_key) = decode_one_key(r)?;
            let (sets, nulls) = decode_one_value(r)?;
            Ok(Op::Upsert(Upsert {
                table_id,
                primary_key,
                sets,
                nulls,
            }))
        }
        TAG_DELETE => {
            let (table_id, primary_key) = decode_one_key(r)?;
            Ok(Op::Delete(Delete {
                table_id,
                primary_key,
            }))
        }
        other => Err(CodecError::UnknownTag(other)),
    }
}

pub(crate) fn decode_one_key<'a, R: BufRead>(
    r: &mut EntryBufferReader<'a, R>,
) -> Result<(TableId, Vec<Value>), CodecError> {
    let table_id = TableId::from(r.read_u16_le()?);
    let primary_key = read_pk(r, table_id)?;
    Ok((table_id, primary_key))
}

pub(crate) fn decode_one_value<'a, R: BufRead>(
    r: &mut EntryBufferReader<'a, R>,
) -> Result<(Vec<ColumnSet>, Vec<ColumnId>), CodecError> {
    // Counts come from untrusted bytes. Convert with try_into (not `as`,
    // which truncates on 32-bit targets and would mis-decode), and don't
    // pre-allocate to them — the Vec grows as entries are actually
    // decoded, so a too-large count just fails fast on the first absent
    // column rather than OOM-ing up front.
    let set_raw = r.read_varint()?;
    let set_count: usize = set_raw
        .try_into()
        .map_err(|_| CodecError::LengthTooLarge(set_raw))?;
    let mut sets = Vec::new();
    for _ in 0..set_count {
        let column_id = read_column_id(r)?;
        let value = read_col_value(r, column_id)?;
        sets.push(ColumnSet { column_id, value });
    }
    let null_raw = r.read_varint()?;
    let null_count: usize = null_raw
        .try_into()
        .map_err(|_| CodecError::LengthTooLarge(null_raw))?;
    let mut nulls = Vec::new();
    for _ in 0..null_count {
        nulls.push(read_column_id(r)?);
    }
    Ok((sets, nulls))
}

fn read_pk<R: BufRead>(
    r: &mut EntryBufferReader<R>,
    table_id: TableId,
) -> Result<Vec<Value>, CodecError> {
    let pk_count = table_id.pk_count();
    let mut pk = Vec::with_capacity(pk_count);
    for i in 0..pk_count {
        pk.push(match table_id.pk_col_type(i) {
            ColType::Bytes => Value::Bytes(r.read_blob()?),
            ColType::Uuid => Value::Uuid(r.read_uuid()?),
            ColType::Text => {
                let s = String::from_utf8(r.read_blob()?)?;
                check_text(&s)?;
                Value::Text(s)
            }
            ColType::I64 => Value::I64(r.read_zigzag()?),
        });
    }
    Ok(pk)
}

fn read_col_value<R: BufRead>(
    r: &mut EntryBufferReader<R>,
    col_id: ColumnId,
) -> Result<Value, CodecError> {
    match col_id.col_type() {
        ColType::Text => {
            let s = String::from_utf8(r.read_blob()?)?;
            check_text(&s)?;
            Ok(Value::Text(s))
        }
        ColType::Bytes => Ok(Value::Bytes(r.read_blob()?)),
        ColType::Uuid => Ok(Value::Uuid(r.read_uuid()?)),
        ColType::I64 => Ok(Value::I64(r.read_zigzag()?)),
    }
}

fn read_column_id<R: BufRead>(r: &mut EntryBufferReader<R>) -> Result<ColumnId, CodecError> {
    // Every byte is a valid column ID: the 2-bit type field admits all four
    // `ColType` values, so this only fails if the byte itself can't be read.
    Ok(ColumnId::from(r.read_byte()?))
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
        TableId::new(&[ColType::Bytes], 1)
    }
    fn table_uuid_pk() -> TableId {
        TableId::new(&[ColType::Uuid], 2)
    }
    // Composite PK exercising the Text and I64 PK column types.
    fn table_text_i64_pk() -> TableId {
        TableId::new(&[ColType::Text, ColType::I64], 3)
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

    // ── Test timestamps (HLC-style) ─────────────────────────────────────
    const TS1: u64 = 1_700_000_000_000 << 16;
    const TS2: u64 = TS1 + (1_000 << 16); // 1s later
    const TS3: u64 = TS2 + (5_000 << 16); // 5s later

    // App-supplied segment magic. Real apps each pick their own distinct value;
    // this stands in for one in the codec tests.
    const TEST_MAGIC: &[u8] = b"TESTMAGIC";

    /// Build test entries exercising both op types, all 4 col types, nulls,
    /// and PK shapes covering every PK column type (Bytes, Uuid, Text, I64),
    /// including a composite PK. In server mode, entries alternate user
    /// attribution.
    fn make_test_entries(server_mode: bool) -> Vec<LogEntry<Op>> {
        let user = |u| if server_mode { Some(u) } else { None };
        vec![
            // Upsert with Bytes PK and all 4 col types + nulls.
            LogEntry {
                user_id: user(USER_1),
                timestamp: Timestamp::from_raw(TS1),
                op: Op::Upsert(Upsert {
                    table_id: table_bytes_pk(),
                    primary_key: vec![Value::Bytes(b"pk1".to_vec())],
                    sets: vec![
                        ColumnSet {
                            column_id: col_bytes(),
                            value: Value::Bytes(vec![0xDE, 0xAD]),
                        },
                        ColumnSet {
                            column_id: col_text(),
                            value: Value::Text("hello".into()),
                        },
                        ColumnSet {
                            column_id: col_i64(),
                            value: Value::I64(-42),
                        },
                        ColumnSet {
                            column_id: col_uuid(),
                            value: Value::Uuid(ROW_UUID_1),
                        },
                        ColumnSet {
                            column_id: col_i64(),
                            value: Value::I64(999),
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
                    primary_key: vec![Value::Text("café".into()), Value::I64(7)],
                    sets: vec![ColumnSet {
                        column_id: col_uuid(),
                        value: Value::Uuid(ROW_UUID_1),
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
                    primary_key: vec![Value::Uuid(PK_UUID)],
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

        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        for entry in &entries {
            encoder
                .encode_entry(&entry.op, entry.timestamp, entry.user_id)
                .unwrap();
        }
        let buf = encoder.sink_mut().get_ref().clone();

        let (decoded, err) = Decoder::<Op, &[u8]>::decode_all(buf.as_slice(), TEST_MAGIC);
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

        let mut encoder = Encoder::new(buf, TEST_MAGIC, true).unwrap();
        for entry in &entries {
            encoder
                .encode_entry(&entry.op, entry.timestamp, entry.user_id)
                .unwrap();
        }
        let buf = encoder.sink_mut().get_ref().clone();

        let (decoded, err) = Decoder::<Op, &[u8]>::decode_all(buf.as_slice(), TEST_MAGIC);
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

    /// Goal: a Text *column* value carrying an embedded NUL is rejected at
    /// encode time.
    ///
    /// Given: an Upsert whose Text column value contains a `\0`.
    /// When:  encoding it.
    /// Then:  encoding fails with `TextContainsNul` — the protocol forbids
    ///        embedded NUL in Text (SQLite stores it, Postgres rejects it).
    #[test]
    fn encode_rejects_embedded_nul_in_text_column() {
        let op = Op::Upsert(Upsert {
            table_id: table_bytes_pk(),
            primary_key: vec![Value::Bytes(b"pk1".to_vec())],
            sets: vec![ColumnSet {
                column_id: col_text(),
                value: Value::Text("bad\0value".into()),
            }],
            nulls: vec![],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        let err = encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap_err();
        assert!(matches!(err, CodecError::TextContainsNul), "got {err:?}");
    }

    /// Goal: a Text *primary key* carrying an embedded NUL is also rejected at
    /// encode time.
    #[test]
    fn encode_rejects_embedded_nul_in_text_pk() {
        let op = Op::Delete(Delete {
            table_id: TableId::new(&[ColType::Text], 4),
            primary_key: vec![Value::Text("bad\0key".into())],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        let err = encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap_err();
        assert!(matches!(err, CodecError::TextContainsNul), "got {err:?}");
    }

    /// Goal: the decoder rejects a Text value with an embedded NUL even in an
    /// otherwise-valid segment — the real defense, since a peer or a corrupt
    /// file can carry bytes our own encoder would never emit.
    ///
    /// Given: a hand-built segment whose single Upsert has a Text column value
    ///        containing `\0`, written through the low-level buffer writer
    ///        (which does not validate) in the exact order the decoder reads.
    /// When:  decoding it.
    /// Then:  `decode_all` surfaces `TextContainsNul`.
    #[test]
    fn decode_rejects_embedded_nul_in_text_column() {
        use std::collections::HashMap;
        use ubiquisync_core::codec::{EntryBufferWriter, FLAG_DEVICE};

        let table = table_bytes_pk();
        let col = col_text();
        let mut dict: HashMap<Uuid, u32> = HashMap::new();
        let mut w = EntryBufferWriter::new(&mut dict);
        w.write_byte(TAG_UPSERT);
        w.write_u16_le(table.into());
        w.write_blob(b"pk1"); // single Bytes PK
        w.write_varint(1); // one column update
        w.write_byte(col.into());
        w.write_blob(b"bad\0value"); // Text column value with an embedded NUL
        w.write_varint(0); // no nulls
        w.write_delta(TS1, 0).unwrap(); // device mode: timestamp, no user id
        let (entry_bytes, _) = w.finalize();

        let mut segment = Vec::new();
        segment.extend_from_slice(TEST_MAGIC);
        segment.push(FLAG_DEVICE);
        segment.extend_from_slice(&entry_bytes);

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(segment.as_slice(), TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::TextContainsNul)),
            "got {err:?}"
        );
    }

    /// Goal: an op whose PK value count disagrees with the table ID's declared
    /// PK shape is rejected at encode time instead of panicking on an index.
    ///
    /// Given: a 2-column (Text, I64) PK table but only one PK value supplied.
    /// When:  encoding it.
    /// Then:  encoding fails with `PkCountMismatch`.
    #[test]
    fn encode_rejects_pk_count_mismatch() {
        let op = Op::Delete(Delete {
            table_id: table_text_i64_pk(),
            primary_key: vec![Value::Text("only-one".into())],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        let err = encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap_err();
        assert!(
            matches!(
                err,
                CodecError::PkCountMismatch {
                    expected: 2,
                    got: 1
                }
            ),
            "got {err:?}"
        );
    }

    /// Goal: a column value whose variant doesn't match the column ID's declared
    /// type is rejected at encode time instead of panicking.
    ///
    /// Given: an I64 column carrying a Text value.
    /// When:  encoding it.
    /// Then:  encoding fails with `ColumnValueMismatch`.
    #[test]
    fn encode_rejects_column_value_type_mismatch() {
        let op = Op::Upsert(Upsert {
            table_id: table_bytes_pk(),
            primary_key: vec![Value::Bytes(b"pk1".to_vec())],
            sets: vec![ColumnSet {
                column_id: col_i64(),
                value: Value::Text("not an int".into()),
            }],
            nulls: vec![],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        let err = encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap_err();
        assert!(
            matches!(err, CodecError::ColumnValueMismatch),
            "got {err:?}"
        );
    }

    /// Goal: a segment claiming an absurd on-wire blob length fails cleanly
    /// instead of pre-allocating (and OOM-ing) that length. The test completing
    /// at all is the real assertion — the old code aborted trying to allocate
    /// `usize::MAX` bytes.
    ///
    /// Given: a hand-built entry whose PK blob declares a length of `u64::MAX`
    ///        with no data behind it.
    /// When:  decoding it.
    /// Then:  `decode_all` surfaces `UnexpectedEof` and the process survives.
    #[test]
    fn decode_rejects_bogus_blob_length_without_oom() {
        use std::collections::HashMap;
        use ubiquisync_core::codec::{EntryBufferWriter, FLAG_DEVICE};

        let table = table_bytes_pk(); // single Bytes PK → first field is a blob
        let mut dict: HashMap<Uuid, u32> = HashMap::new();
        let mut w = EntryBufferWriter::new(&mut dict);
        w.write_byte(TAG_UPSERT);
        w.write_u16_le(table.into());
        w.write_varint(u64::MAX); // bogus PK blob length with no data behind it
        let (entry_bytes, _) = w.finalize();

        let mut segment = Vec::new();
        segment.extend_from_slice(TEST_MAGIC);
        segment.push(FLAG_DEVICE);
        segment.extend_from_slice(&entry_bytes);

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(segment.as_slice(), TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::UnexpectedEof)),
            "got {err:?}"
        );
    }

    /// Goal: a segment whose flags byte is neither device nor server mode is
    /// rejected, rather than silently masked to device mode.
    ///
    /// Given: a well-magicked segment with a flags byte of `0x07`.
    /// When:  decoding it.
    /// Then:  `decode_all` surfaces `UnknownSegmentFlags`.
    #[test]
    fn decode_rejects_unknown_segment_flags() {
        let mut segment = Vec::new();
        segment.extend_from_slice(TEST_MAGIC);
        segment.push(0x07); // not FLAG_DEVICE (0) or FLAG_SERVER (1)

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(segment.as_slice(), TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::UnknownSegmentFlags(0x07))),
            "got {err:?}"
        );
    }

    /// Goal: a PK value whose variant disagrees with the table ID's declared PK
    /// type is rejected at encode time, rather than being written with the
    /// wrong wire shape and mis-decoded.
    ///
    /// Given: a Uuid-PK table but a Text PK value.
    /// When:  encoding it.
    /// Then:  encoding fails with `PkValueMismatch`.
    #[test]
    fn encode_rejects_pk_value_type_mismatch() {
        let op = Op::Delete(Delete {
            table_id: table_uuid_pk(),
            primary_key: vec![Value::Text("not a uuid".into())],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        let err = encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap_err();
        assert!(matches!(err, CodecError::PkValueMismatch), "got {err:?}");
    }

    /// Goal: a failed `encode_entry` leaves the encoder's cross-entry state
    /// (UUID dictionary and last timestamp) exactly as it was, so a later entry
    /// is still correctly encodable and decodable.
    ///
    /// Given: a server-mode encoder; a first entry that registers a UUID in its
    ///        PK at a high timestamp but then fails (no user id in server mode).
    /// When:  a second, valid entry reuses that UUID at an earlier timestamp.
    /// Then:  the segment decodes cleanly to just the second entry. This fails
    ///        under either bug: if the dict isn't rolled back the UUID becomes a
    ///        dangling dict reference (`UnresolvedUuid`); if `last_timestamp`
    ///        advanced, the earlier second timestamp is a `NonMonotonicDelta`.
    #[test]
    fn failed_entry_does_not_corrupt_encoder_state() {
        let table = table_uuid_pk();

        let failed = Op::Upsert(Upsert {
            table_id: table,
            primary_key: vec![Value::Uuid(PK_UUID)],
            sets: vec![],
            nulls: vec![],
        });
        let good = Op::Delete(Delete {
            table_id: table,
            primary_key: vec![Value::Uuid(PK_UUID)],
        });

        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, true).unwrap();

        // First entry registers PK_UUID, then fails: server mode with no user.
        let err = encoder
            .encode_entry(&failed, Timestamp::from_raw(TS2), None)
            .unwrap_err();
        assert!(matches!(err, CodecError::MissingUserId), "got {err:?}");

        // Second entry reuses the UUID at an earlier timestamp, with a user.
        encoder
            .encode_entry(&good, Timestamp::from_raw(TS1), Some(USER_1))
            .unwrap();
        let bytes = encoder.sink_mut().get_ref().clone();

        let (decoded, derr) = Decoder::<Op, &[u8]>::decode_all(bytes.as_slice(), TEST_MAGIC);
        assert!(derr.is_none(), "decode error: {derr:?}");
        let decoded = decoded.unwrap();
        assert_eq!(decoded.entries.len(), 1);
        match &decoded.entries[0] {
            DecodedEntry::LogEntry(e) => {
                assert_eq!(e.user_id, Some(USER_1));
                assert_eq!(e.timestamp, Timestamp::from_raw(TS1));
                assert_eq!(format!("{:?}", e.op), format!("{:?}", good));
            }
            DecodedEntry::Expunged(_) => panic!("unexpected Expunged"),
        }
    }

    /// Goal: the decoder reads an expunged marker (tag + 32-byte hash, no body,
    /// no integrity suffix) as a `DecodedEntry::Expunged`.
    ///
    /// Given: a segment containing a single hand-built expunged marker.
    /// When:  decoding it.
    /// Then:  one `Expunged` entry is produced with no error.
    #[test]
    fn decode_reads_expunged_marker() {
        use ubiquisync_core::codec::{FLAG_DEVICE, TAG_EXPUNGED};

        let mut segment = Vec::new();
        segment.extend_from_slice(TEST_MAGIC);
        segment.push(FLAG_DEVICE);
        segment.push(TAG_EXPUNGED);
        segment.extend_from_slice(&[0xAB; 32]); // 32-byte hash; no body or trailer

        let (decoded, err) = Decoder::<Op, &[u8]>::decode_all(segment.as_slice(), TEST_MAGIC);
        assert!(err.is_none(), "decode error: {err:?}");
        let decoded = decoded.unwrap();
        assert_eq!(decoded.entries.len(), 1);
        assert!(matches!(decoded.entries[0], DecodedEntry::Expunged(_)));
    }

    /// Goal: a segment whose magic doesn't match the expected app magic is
    /// rejected as foreign.
    #[test]
    fn decode_rejects_bad_magic() {
        // Same length as TEST_MAGIC, different bytes.
        let segment = b"WRONGMAGI";
        assert_eq!(segment.len(), TEST_MAGIC.len());

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(segment.as_slice(), TEST_MAGIC);
        assert!(matches!(err, Some(CodecError::BadMagic)), "got {err:?}");
    }

    /// Goal: an unknown op tag is rejected rather than mis-parsed.
    #[test]
    fn decode_rejects_unknown_op_tag() {
        use ubiquisync_core::codec::FLAG_DEVICE;

        let mut segment = Vec::new();
        segment.extend_from_slice(TEST_MAGIC);
        segment.push(FLAG_DEVICE);
        segment.push(0x7E); // neither TAG_UPSERT/TAG_DELETE nor TAG_EXPUNGED

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(segment.as_slice(), TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::UnknownTag(0x7E))),
            "got {err:?}"
        );
    }

    /// Goal: a flipped byte in a valid entry is caught by the integrity check.
    ///
    /// Given: one validly encoded entry with its trailing integrity bytes
    ///        corrupted (content intact, so framing still parses).
    /// When:  decoding it.
    /// Then:  decode fails with `HashMismatch`.
    #[test]
    fn decode_detects_hash_mismatch() {
        let op = Op::Delete(Delete {
            table_id: table_uuid_pk(),
            primary_key: vec![Value::Uuid(PK_UUID)],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap();
        let mut bytes = encoder.sink_mut().get_ref().clone();

        // Corrupt the entry's trailing integrity bytes (last byte of the buffer).
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(bytes.as_slice(), TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::HashMismatch { .. })),
            "got {err:?}"
        );
    }

    /// Goal: an entry truncated mid-body fails cleanly with `UnexpectedEof`
    /// rather than panicking or mis-decoding.
    #[test]
    fn decode_rejects_truncated_entry() {
        let op = Op::Delete(Delete {
            table_id: table_uuid_pk(),
            primary_key: vec![Value::Uuid(PK_UUID)],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap();
        let bytes = encoder.sink_mut().get_ref().clone();

        // Keep the header + tag + one byte, cutting the table id read short.
        let truncated = &bytes[..TEST_MAGIC.len() + 3];
        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(truncated, TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::UnexpectedEof)),
            "got {err:?}"
        );
    }

    /// Goal: truncating only the trailing integrity bytes (body intact) also
    /// surfaces as the dedicated `UnexpectedEof`, not a generic `Io` error.
    #[test]
    fn decode_truncated_trailer_is_unexpected_eof() {
        let op = Op::Delete(Delete {
            table_id: table_uuid_pk(),
            primary_key: vec![Value::Uuid(PK_UUID)],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap();
        let bytes = encoder.sink_mut().get_ref().clone();

        // Drop 2 of the 4 trailing integrity bytes: the body reads fine, then
        // the trailer read comes up short.
        let truncated = &bytes[..bytes.len() - 2];
        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(truncated, TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::UnexpectedEof)),
            "got {err:?}"
        );
    }

    /// Goal: two apps with distinct (same-length) magics reject each other's
    /// segments — the isolation the caller-supplied magic exists to provide.
    #[test]
    fn distinct_magics_reject_each_others_segments() {
        let magic_a: &[u8] = b"APP_A";
        let magic_b: &[u8] = b"APP_B";

        let op = Op::Delete(Delete {
            table_id: table_uuid_pk(),
            primary_key: vec![Value::Uuid(PK_UUID)],
        });
        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, magic_a, false).unwrap();
        encoder
            .encode_entry(&op, Timestamp::from_raw(TS1), None)
            .unwrap();
        let bytes = encoder.sink_mut().get_ref().clone();

        let (_decoded, err) = Decoder::<Op, &[u8]>::decode_all(bytes.as_slice(), magic_b);
        assert!(matches!(err, Some(CodecError::BadMagic)), "got {err:?}");
    }

    /// Goal: an empty magic is rejected at runtime by both the encoder and the
    /// decoder (in all builds) — it would otherwise give zero app isolation.
    #[test]
    fn empty_magic_is_rejected() {
        let buf = std::io::Cursor::new(Vec::new());
        assert!(
            matches!(
                Encoder::<Op, _>::new(buf, b"", false),
                Err(CodecError::BadMagic)
            ),
            "encoder should reject empty magic"
        );

        let (decoded, dec_err) = Decoder::<Op, &[u8]>::decode_all(b"anything".as_slice(), b"");
        assert!(decoded.is_none());
        assert!(
            matches!(dec_err, Some(CodecError::BadMagic)),
            "got {dec_err:?}"
        );
    }

    /// Goal: when an entry fails to decode, the decoder's cross-entry state —
    /// the UUID dictionary and last timestamp that `decode_all` returns for
    /// encoder reuse — reflects only the entries that decoded successfully.
    ///
    /// Given: two valid entries (each registering a distinct PK UUID) with the
    ///        second entry's integrity trailer corrupted.
    /// When:  decoding the segment.
    /// Then:  the good first entry is returned alongside a `HashMismatch`, and
    ///        the returned `last_timestamp` and `uuid_dict` carry only the first
    ///        entry's state — the failed entry's UUID and timestamp are rolled
    ///        back.
    #[test]
    fn decode_failure_rolls_back_cross_entry_state() {
        let table = table_uuid_pk();
        let first = Op::Delete(Delete {
            table_id: table,
            primary_key: vec![Value::Uuid(PK_UUID)],
        });
        let second = Op::Delete(Delete {
            table_id: table,
            primary_key: vec![Value::Uuid(ROW_UUID_1)],
        });

        let buf = std::io::Cursor::new(Vec::new());
        let mut encoder = Encoder::new(buf, TEST_MAGIC, false).unwrap();
        encoder
            .encode_entry(&first, Timestamp::from_raw(TS1), None)
            .unwrap();
        encoder
            .encode_entry(&second, Timestamp::from_raw(TS2), None)
            .unwrap();
        let mut bytes = encoder.sink_mut().get_ref().clone();

        // Corrupt the second entry's trailing integrity byte.
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;

        let (decoded, err) = Decoder::<Op, &[u8]>::decode_all(bytes.as_slice(), TEST_MAGIC);
        assert!(
            matches!(err, Some(CodecError::HashMismatch { .. })),
            "got {err:?}"
        );
        let decoded = decoded.unwrap();
        assert_eq!(decoded.entries.len(), 1);
        // Only the first entry's state survived the failure.
        assert_eq!(decoded.last_timestamp, TS1);
        assert_eq!(decoded.uuid_dict.len(), 1);
        assert!(decoded.uuid_dict.contains_key(&PK_UUID));
    }
}
