//! Table operations — the core mutation types applied by the reducer.
//!
//! An [`Op`] is a single atomic state change. It is the payload inside a
//! [`LogEntry`](ubiquisync_core::log_entry::LogEntry) in the table log: the
//! application layer constructs `Op` values, the log layer wraps them with
//! timestamp and attribution metadata, and the merge reducer applies them to
//! local storage.
//!
//! Tables have a compile-time schema with type-encoded IDs — see
//! [`crate::id`].

use crate::id::{ColumnId, TableId};
use ubiquisync_core::uuid::Uuid;

/// A single state mutation against a table (compile-time schema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Insert or merge a table row.
    Upsert(Upsert),
    /// Soft-delete a table row.
    Delete(Delete),
}

// ── Table operations ─────────────────────────────────────────────────────────

/// Inserts or merges a row in a table. Every column merges last-writer-wins,
/// keyed by the timestamp from the enclosing
/// [`LogEntry`](ubiquisync_core::log_entry::LogEntry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upsert {
    pub table_id: TableId,
    /// PK values identifying the row. Count and per-column wire encoding are
    /// determined by the table ID's PK shape bits — each value's variant must
    /// match the corresponding [`pk_col_type`](crate::id::TableId::pk_col_type)
    /// positionally.
    pub primary_key: Vec<Value>,
    /// Columns to set with new values. The column ID's type bits determine
    /// the wire encoding.
    pub sets: Vec<ColumnSet>,
    /// Columns to set to SQL NULL. All non-PK columns are implicitly nullable.
    pub nulls: Vec<ColumnId>,
}

/// A typed value — used both as a primary-key component and as a column value.
/// The variant is fixed positionally by the table's PK shape (for PK values)
/// or by the column ID's type bits (for column values). PK values are row
/// identity (compared, never merged); column values merge last-writer-wins.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    /// Raw byte data (length-prefixed on wire).
    Bytes(Vec<u8>),
    /// 16-byte UUID (fixed-width on wire).
    Uuid(Uuid),
    /// UTF-8 text (length-prefixed on wire). Strict UTF-8, no embedded NUL,
    /// compared as raw bytes — see the table protocol's text rules.
    Text(String),
    /// Signed 64-bit integer (zigzag varint on wire).
    I64(i64),
}

/// Soft-deletes a table row by advancing `__deleted_ts`. LWW — a later
/// timestamp always wins; an earlier timestamp is silently ignored.
/// Timestamp comes from the enclosing
/// [`LogEntry`](ubiquisync_core::log_entry::LogEntry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delete {
    pub table_id: TableId,
    /// PK values identifying the row (see [`Upsert::primary_key`]).
    pub primary_key: Vec<Value>,
}

/// A column ID paired with the value to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSet {
    pub column_id: ColumnId,
    pub value: Value,
}
