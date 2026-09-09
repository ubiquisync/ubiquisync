//! Wire codec for the table op vocabulary.
//!
//! Wire shapes are derived from the [type-encoded IDs](crate::id): a
//! [`TableId`] carries its PK column count and per-column types, and a
//! [`ColumnId`] carries its column type — so the decoder knows how to read
//! each value without a schema lookup.

use std::borrow::Borrow;
use std::str::Utf8Error;

use thiserror::Error;
use ubiquisync_core::codec::{ReadError, Reader, Writer};
use ubiquisync_core::ids::ContainerId;
use ubiquisync_sql::op::{OpCodec, OpDecodeError, OpEncodeError};

use crate::col_type::ColType;
use crate::id::{ColumnId, TableId};
use crate::op::{ColumnSet, Delete, Op, Upsert, Value};

pub(crate) struct Codec {
    container_id: ContainerId,
}

impl OpCodec<Op> for Codec {
    fn encode(
        &self,
        op: &Op,
    ) -> Result<
        (
            ContainerId,
            Vec<ubiquisync_core::bytes::PlaintextBytes<'static>>,
        ),
        ubiquisync_sql::op::OpEncodeError,
    > {
        let mut w = Writer::new();
        encode_one_op(&mut w, op).map_err(|e| OpEncodeError::Invalid(Box::new(e)))?;
        Ok((self.container_id, vec![w.finalize().into()]))
    }

    fn decode(
        &self,
        container_id: &ContainerId,
        ops: &[ubiquisync_core::bytes::PlaintextBytes],
    ) -> Result<Op, ubiquisync_sql::op::OpDecodeError> {
        self.do_decode(container_id, ops)
            .map_err(|e| OpDecodeError::Invalid(Box::new(e)))
    }
}

impl Codec {
    pub(crate) fn new(container_id: ContainerId) -> Self {
        Self { container_id }
    }

    fn do_decode(
        &self,
        container_id: &ContainerId,
        ops: &[ubiquisync_core::bytes::PlaintextBytes],
    ) -> Result<Op, DecodeOpError> {
        if self.container_id != *container_id {
            return Err(DecodeOpError::WrongContainer {
                actual: *container_id,
                expected: self.container_id,
            });
        }
        if ops.len() != 1 {
            return Err(DecodeOpError::InvalidOpCount(ops.len()));
        }
        let mut r = Reader::new(ops[0].borrow());
        let op = decode_one_op(&mut r)?;
        if !r.is_empty() {
            return Err(DecodeOpError::TrailingBytes);
        }
        Ok(op)
    }
}

// ── Op tags ──────────────────────────────────────────────────────────────────

/// Insert-or-merge a table row.
const TAG_UPSERT: u8 = 0;
/// Soft-delete a table row.
const TAG_DELETE: u8 = 1;

// ── Text validation ──────────────────────────────────────────────────────────

/// Reject text carrying an embedded ASCII NUL. The protocol forbids `\0` in Text
/// because SQLite stores it while Postgres rejects it, so an unchecked NUL is a
/// value one backend physically cannot hold — a silent divergence. Enforced on
/// both encode and decode, for Text PKs and Text columns. (Strict UTF-8 is
/// already guaranteed: encode starts from a `String`, decode goes through
/// `String::from_utf8`.)
fn check_text_for_nul(s: &str) -> Result<(), TextContainsNul> {
    if s.as_bytes().contains(&0u8) {
        return Err(TextContainsNul);
    }
    Ok(())
}

struct TextContainsNul;

fn encode_one_op(w: &mut Writer, op: &Op) -> Result<(), EncodeOpError> {
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
    w: &mut Writer,
    table_id: TableId,
    pkey: &[Value],
) -> Result<(), EncodeOpError> {
    w.write_le_u16(table_id.into());
    write_pk(w, table_id, pkey)
}

pub(crate) fn encode_one_value(w: &mut Writer, e: &Upsert) -> Result<(), EncodeOpError> {
    w.write_var_u64(e.sets.len() as u64);
    for set in &e.sets {
        w.write_byte(set.column_id.into());
        write_col_value(w, set.column_id, &set.value)?;
    }
    w.write_var_u64(e.nulls.len() as u64);
    for col_id in &e.nulls {
        w.write_byte((*col_id).into());
    }
    Ok(())
}

#[derive(Error, Debug)]
pub enum EncodeOpError {
    #[error("wrong number of PK values: expected {expected}, got {got}")]
    PkCountMismatch { expected: usize, got: usize },
    #[error("PK value mismatch")]
    PkValueMismatch,
    #[error("text contains NUL")]
    TextContainsNul,
    #[error("column value mismatch")]
    ColumnValueMismatch,
}

fn write_str(w: &mut Writer, s: &str) -> Result<(), EncodeOpError> {
    check_text_for_nul(s).map_err(|_| EncodeOpError::TextContainsNul)?;
    w.write_len_prefixed(s.as_bytes());
    Ok(())
}

pub(crate) fn write_pk(
    w: &mut Writer,
    table_id: TableId,
    pk: &[Value],
) -> Result<(), EncodeOpError> {
    let pk_count = table_id.pk_count();
    // A wrong number of PK values is a caller error.
    if pk.len() != pk_count {
        return Err(EncodeOpError::PkCountMismatch {
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
            (ColType::Bytes, Value::Bytes(b)) => w.write_len_prefixed(b),
            (ColType::Uuid, Value::Uuid(u)) => w.write_array(u),
            (ColType::Text, Value::Text(s)) => write_str(w, s)?,
            (ColType::I64, Value::I64(n)) => w.write_zigzag_i64(*n),
            _ => return Err(EncodeOpError::PkValueMismatch),
        }
    }
    Ok(())
}

pub(crate) fn write_col_value(
    w: &mut Writer,
    col_id: ColumnId,
    value: &Value,
) -> Result<(), EncodeOpError> {
    // The value variant must match the column ID's declared type; a mismatch
    // is a caller error.
    match col_id.col_type() {
        ColType::Bytes => match value {
            Value::Bytes(b) => w.write_len_prefixed(b),
            _ => return Err(EncodeOpError::ColumnValueMismatch),
        },
        ColType::Text => match value {
            Value::Text(s) => write_str(w, s)?,
            _ => return Err(EncodeOpError::ColumnValueMismatch),
        },
        ColType::I64 => match value {
            Value::I64(n) => w.write_zigzag_i64(*n),
            _ => return Err(EncodeOpError::ColumnValueMismatch),
        },
        ColType::Uuid => match value {
            Value::Uuid(u) => w.write_array(u),
            _ => return Err(EncodeOpError::ColumnValueMismatch),
        },
    }
    Ok(())
}

#[derive(Error, Debug)]
pub(crate) enum DecodeOpError {
    #[error("read error: {0}")]
    Read(#[from] ReadError),
    #[error("unknown op tag {0}")]
    UnknownTag(u8),
    #[error("length too large {0}")]
    LengthTooLarge(u64),
    #[error("utf conversion: {0}")]
    UtfConversion(#[from] Utf8Error),
    #[error("text contains NUL")]
    TextContainsNul,
    #[error("wrong container id {actual:?}, expected {expected:?}")]
    WrongContainer {
        actual: ContainerId,
        expected: ContainerId,
    },
    #[error("invalid op count {0} expected 1")]
    InvalidOpCount(usize),
    #[error("trailing bytes")]
    TrailingBytes,
}

fn decode_one_op(r: &mut Reader) -> Result<Op, DecodeOpError> {
    match r.read_byte()? {
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
        other => Err(DecodeOpError::UnknownTag(other)),
    }
}

pub(crate) fn decode_one_key(r: &mut Reader) -> Result<(TableId, Vec<Value>), DecodeOpError> {
    let table_id = TableId::from(r.read_le_u16()?);
    let primary_key = read_pk(r, table_id)?;
    Ok((table_id, primary_key))
}

pub(crate) fn decode_one_value(
    r: &mut Reader,
) -> Result<(Vec<ColumnSet>, Vec<ColumnId>), DecodeOpError> {
    // Counts come from untrusted bytes. Convert with try_into (not `as`,
    // which truncates on 32-bit targets and would mis-decode), and don't
    // pre-allocate to them — the Vec grows as entries are actually
    // decoded, so a too-large count just fails fast on the first absent
    // column rather than OOM-ing up front.
    let set_raw = r.read_var_u64()?;
    let set_count: usize = set_raw
        .try_into()
        .map_err(|_| DecodeOpError::LengthTooLarge(set_raw))?;
    let mut sets = Vec::new();
    for _ in 0..set_count {
        let column_id = read_column_id(r)?;
        let value = read_col_value(r, column_id)?;
        sets.push(ColumnSet { column_id, value });
    }
    let null_raw = r.read_var_u64()?;
    let null_count: usize = null_raw
        .try_into()
        .map_err(|_| DecodeOpError::LengthTooLarge(null_raw))?;
    let mut nulls = Vec::new();
    for _ in 0..null_count {
        nulls.push(read_column_id(r)?);
    }
    Ok((sets, nulls))
}

fn read_str<'a>(r: &mut Reader<'a>) -> Result<&'a str, DecodeOpError> {
    let s = str::from_utf8(r.read_len_prefixed()?)?;
    check_text_for_nul(s).map_err(|_| DecodeOpError::TextContainsNul)?;
    Ok(s)
}

fn read_string(r: &mut Reader) -> Result<String, DecodeOpError> {
    Ok(read_str(r)?.into())
}

fn read_pk(r: &mut Reader, table_id: TableId) -> Result<Vec<Value>, DecodeOpError> {
    let pk_count = table_id.pk_count();
    let mut pk = Vec::with_capacity(pk_count);
    for i in 0..pk_count {
        pk.push(match table_id.pk_col_type(i) {
            ColType::Bytes => Value::Bytes(r.read_len_prefixed()?.into()),
            ColType::Uuid => Value::Uuid(r.read_array()?),
            ColType::Text => Value::Text(read_string(r)?),
            ColType::I64 => Value::I64(r.read_zigzag_i64()?),
        });
    }
    Ok(pk)
}

fn read_col_value(r: &mut Reader, col_id: ColumnId) -> Result<Value, DecodeOpError> {
    match col_id.col_type() {
        ColType::Text => Ok(Value::Text(read_string(r)?)),
        ColType::Bytes => Ok(Value::Bytes(r.read_len_prefixed()?.into())),
        ColType::Uuid => Ok(Value::Uuid(r.read_array()?)),
        ColType::I64 => Ok(Value::I64(r.read_zigzag_i64()?)),
    }
}

fn read_column_id(r: &mut Reader) -> Result<ColumnId, DecodeOpError> {
    // Every byte is a valid column ID: the 2-bit type field admits all four
    // `ColType` values, so this only fails if the byte itself can't be read.
    Ok(ColumnId::from(r.read_byte()?))
}

#[cfg(test)]
mod tests {
    use test_case::test_case;
    use test_strategy::proptest;
    use ubiquisync_core::bytes::PlaintextBytes;

    use super::*;

    #[proptest]
    fn roundtrip_op(op: Op) {
        let codec = Codec::new(CONTAINER_ID_0);
        let (cid, buf) = codec.encode(&op).unwrap();
        let decoded = codec.decode(&cid, &buf).unwrap();
        assert_eq!(op, decoded);
    }

    #[test_case(upsert(TABLE_TEXT, &[text("a\0b")], &[])
        => matches EncodeOpError::TextContainsNul;
        "nul in text pk")]
    #[test_case(upsert(TABLE_TEXT, &[text("k")], &[(COL_TEXT, text("a\0b"))])
        => matches EncodeOpError::TextContainsNul;
        "nul in text column")]
    #[test_case(upsert(TABLE_TEXT_I64, &[text("k")], &[])
        => matches EncodeOpError::PkCountMismatch { expected: 2, got: 1 };
        "pk count mismatch")]
    #[test_case(delete(TABLE_TEXT, &[text("k"), Value::I64(1)])
        => matches EncodeOpError::PkCountMismatch { expected: 1, got: 2 };
        "pk count mismatch on delete")]
    #[test_case(upsert(TABLE_TEXT, &[Value::I64(1)], &[])
        => matches EncodeOpError::PkValueMismatch;
        "pk value type mismatch")]
    #[test_case(upsert(TABLE_TEXT, &[text("k")], &[(COL_I64, text("x"))])
        => matches EncodeOpError::ColumnValueMismatch;
        "column value type mismatch")]
    fn encode_rejects(op: Op) -> EncodeOpError {
        let mut w = Writer::new();
        encode_one_op(&mut w, &op).unwrap_err()
    }

    #[test_case(|w| { w.write_byte(0x7E); }
        => matches DecodeOpError::UnknownTag(0x7E);
        "unknown tag")]
    #[test_case(|w| { w.write_byte(TAG_DELETE); w.write_le_u16(TABLE_TEXT.raw()); w.write_len_prefixed(b"a\0b"); }
        => matches DecodeOpError::TextContainsNul;
        "nul in text pk")]
    #[test_case(|w| { w.write_byte(TAG_UPSERT); w.write_le_u16(TABLE_TEXT.raw()); w.write_len_prefixed(b"k");
                      w.write_var_u64(1); w.write_byte(COL_TEXT.into()); w.write_len_prefixed(b"a\0b"); w.write_var_u64(0); }
        => matches DecodeOpError::TextContainsNul;
        "nul in text column")]
    #[test_case(|w| { w.write_byte(TAG_DELETE); w.write_le_u16(TABLE_TEXT.raw()); w.write_len_prefixed(&[0xFF]); }
        => matches DecodeOpError::UtfConversion(_);
        "invalid utf8 in text pk")]
    #[test_case(|w| { w.write_byte(TAG_DELETE); w.write_le_u16(TABLE_TEXT.raw()); w.write_var_u64(u64::MAX); }
        => matches DecodeOpError::Read(ReadError::UnexpectedEof);
        "bogus blob length does not allocate")]
    #[test_case(|w| { w.write_byte(TAG_DELETE); w.write_le_u16(TABLE_TEXT.raw()); }
        => matches DecodeOpError::Read(ReadError::UnexpectedEof);
        "truncated entry")]
    fn decode_rejects(build: fn(&mut Writer)) -> DecodeOpError {
        let mut w = Writer::new();
        build(&mut w);
        let buf = w.finalize();
        decode_one_op(&mut Reader::new(&buf)).unwrap_err()
    }

    #[test_case(ContainerId([1; 16]), vec![one_good_slot()] =>
        matches DecodeOpError::WrongContainer { .. };
        "wrong container")]
    #[test_case(CONTAINER_ID_0, vec![]
        => matches DecodeOpError::InvalidOpCount(0);
        "zero ops")]
    #[test_case(CONTAINER_ID_0, vec![one_good_slot(), one_good_slot()]
        => matches DecodeOpError::InvalidOpCount(2);
        "two ops")]
    #[test_case(CONTAINER_ID_0, vec![with_trailing_bytes(one_good_slot())]
        => matches DecodeOpError::TrailingBytes;
        "trailing bytes")]
    fn codec_rejects(container: ContainerId, ops: Vec<PlaintextBytes<'static>>) -> DecodeOpError {
        Codec::new(CONTAINER_ID_0)
            .do_decode(&container, &ops)
            .unwrap_err()
    }

    const CONTAINER_ID_0: ContainerId = ContainerId([0; 16]);
    const TABLE_TEXT: TableId = TableId::new(&[ColType::Text], 1);
    const TABLE_TEXT_I64: TableId = TableId::new(&[ColType::Text, ColType::I64], 1);
    const COL_TEXT: ColumnId = ColumnId::new(0, ColType::Text);
    const COL_I64: ColumnId = ColumnId::new(1, ColType::I64);

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn upsert(table_id: TableId, pk: &[Value], sets: &[(ColumnId, Value)]) -> Op {
        Op::Upsert(Upsert {
            table_id,
            primary_key: pk.to_vec(),
            sets: sets
                .iter()
                .map(|(column_id, value)| ColumnSet {
                    column_id: *column_id,
                    value: value.clone(),
                })
                .collect(),
            nulls: vec![],
        })
    }

    fn delete(table_id: TableId, pk: &[Value]) -> Op {
        Op::Delete(Delete {
            table_id,
            primary_key: pk.to_vec(),
        })
    }

    fn one_good_slot() -> PlaintextBytes<'static> {
        let (_, mut slots) = Codec::new(CONTAINER_ID_0)
            .encode(&delete(TABLE_TEXT, &[text("k")]))
            .unwrap();
        slots.remove(0)
    }

    fn with_trailing_bytes(slot: PlaintextBytes<'static>) -> PlaintextBytes<'static> {
        let mut bytes = slot.0.into_owned();
        bytes.extend_from_slice(&[0xDE, 0xAD]);
        bytes.into()
    }
}
