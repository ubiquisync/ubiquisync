//! State operations — the core mutation types applied by the reducer.
//!
//! An [`Op`] is a single atomic state change. It is the payload inside a
//! [`LogEntry`](crate::log_entry::LogEntry) in the state log: the application
//! layer constructs `Op` values, the log layer wraps them with timestamp and
//! attribution metadata, and the merge reducer applies them to local storage.
//!
//! Variants cover the protocol's two table domains: **system tables**
//! (compile-time schema, type-encoded IDs — see [`crate::sys_id`]) and
//! **user-defined tables** (runtime schema, UUID-addressed). Collaborative
//! rich-text documents have their own op vocabulary in [`crate::docs::op`].

use crate::sys_id::{SysColumnId, SysTableId};
use crate::uuid::Uuid;

/// A single state mutation. Variants cover system tables (compile-time
/// schema) and user-defined tables (runtime schema, UUID-addressed).
#[derive(Debug, Clone)]
pub enum Op {
    /// Insert or merge a system table row.
    SysUpsert(SysUpsert),
    /// Soft-delete a system table row.
    SysDelete(SysDelete),
    /// Insert or merge a user-defined table row.
    UsrUpsert(UsrUpsert),
    /// Set entity→table affinity. Used for entity creation and move-row.
    UsrSetTable(UsrSetTable),
    /// Soft-delete an entity.
    UsrDelete(UsrDelete),
    /// Set or delete a user-defined join table entry.
    UsrUpdateJoin(UsrUpdateJoin),
}

// ── System table operations ─────────────────────────────────────────────────

/// Inserts or merges a row in a system table. Merge strategy (LWW or
/// max-wins) is determined by each column's type bits in its [`SysColumnId`].
/// Timestamp comes from the enclosing [`LogEntry`](crate::log_entry::LogEntry).
#[derive(Debug, Clone)]
pub struct SysUpsert {
    pub table_id: SysTableId,
    /// PK values identifying the row. Count and per-column wire encoding are
    /// determined by the table ID's PK shape bits — each value's variant must
    /// match the corresponding [`PkColType`](crate::sys_id::PkColType)
    /// positionally.
    pub primary_key: Vec<SysPkValue>,
    /// Columns to update with new values. The column ID's type bits determine
    /// wire encoding and merge strategy.
    pub updates: Vec<SysColumnUpdate>,
    /// Columns to set to SQL NULL. All non-PK columns are implicitly nullable.
    pub nulls: Vec<SysColumnId>,
}

/// A system table primary key value. One variant per
/// [`PkColType`](crate::sys_id::PkColType) — PK values are row identity:
/// they are compared, never merged.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SysPkValue {
    /// Raw byte data (length-prefixed on wire).
    Bytes(Vec<u8>),
    /// 16-byte UUID (fixed-width on wire).
    Uuid(Uuid),
    /// UTF-8 text (length-prefixed on wire). Strict UTF-8, no embedded NUL,
    /// compared as raw bytes — see the sys table protocol's text rules.
    Text(String),
    /// Signed 64-bit integer (zigzag varint on wire).
    I64(i64),
}

/// Soft-deletes a system table row by advancing `__deleted_ts`. LWW — a later
/// timestamp always wins; an earlier timestamp is silently ignored.
/// Timestamp comes from the enclosing [`LogEntry`](crate::log_entry::LogEntry).
#[derive(Debug, Clone)]
pub struct SysDelete {
    pub table_id: SysTableId,
    /// PK values identifying the row (see [`SysUpsert::primary_key`]).
    pub primary_key: Vec<SysPkValue>,
}

/// A typed column value. The variant is determined by the column ID's type bits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SysColValue {
    /// Raw byte data (length-prefixed on wire).
    Bytes(Vec<u8>),
    /// UTF-8 text (length-prefixed on wire, same encoding as Bytes).
    Text(String),
    /// 16-byte UUID (fixed-width on wire).
    Uuid(Uuid),
    /// Integer data. Covers I64 (LWW) and MaxI64 (max-wins) columns —
    /// merge strategy determined by column ID.
    I64(i64),
}

/// A system column ID paired with the value to write.
#[derive(Debug, Clone)]
pub struct SysColumnUpdate {
    pub column_id: SysColumnId,
    pub value: SysColValue,
}

// ── User-defined table operations ───────────────────────────────────────────

/// Inserts or merges a row in a user-defined table. Each column merges
/// independently, LWW by the enclosing entry's timestamp — concurrent edits
/// to different columns of the same row both survive.
///
/// `id` is the row's entity UUID; the row only becomes visible once the
/// entity's table affinity is set via [`UsrSetTable`].
#[derive(Debug, Clone)]
pub struct UsrUpsert {
    pub table_id: Uuid,
    pub id: Uuid,
    pub values: Vec<UsrColValue>,
}

/// A user-table column UUID paired with the value to write.
/// `None` sets the column to NULL (all user columns are nullable).
#[derive(Debug, Clone)]
pub struct UsrColValue {
    pub col_id: Uuid,
    pub value: Option<UsrValue>,
}

/// A user-table cell value. Deliberately only two shapes:
///
/// - **Text** carries every user-facing scalar — strings, numbers, dates,
///   booleans, URLs. Richer scalar types are a view-time concern (formatting
///   plus lightweight validation layered on top by the application), so a
///   column can be retyped — number → plain text → date — without rewriting
///   rows or coordinating a migration across peers.
/// - **Uuid** carries references — row links, select/multi-select option
///   IDs — values that point at other synced objects and are not
///   meaningfully retypeable.
///
/// The type travels with each value (a tag on the wire), not with the
/// column, which is what makes retyping free at the protocol level.
#[derive(Debug, Clone, PartialEq)]
pub enum UsrValue {
    Text(String),
    Uuid(Uuid),
}

impl From<&str> for UsrValue {
    fn from(s: &str) -> Self {
        UsrValue::Text(s.to_string())
    }
}

impl PartialEq<&str> for UsrValue {
    fn eq(&self, other: &&str) -> bool {
        matches!(self, UsrValue::Text(s) if s == *other)
    }
}

/// Sets or clears one edge in a user-defined join table — the protocol's
/// many-to-many primitive (row relations, multi-select membership). The edge
/// is keyed by `(left_row_id, right_row_id)` within the join table; set and
/// delete merge LWW by the enclosing entry's timestamp.
#[derive(Debug, Clone)]
pub struct UsrUpdateJoin {
    pub join_table_id: Uuid,
    pub left_row_id: Uuid,
    pub right_row_id: Uuid,
    /// `true` removes the edge, `false` sets it.
    pub delete: bool,
}

/// Sets an entity's table affinity, LWW by the enclosing entry's timestamp.
/// Entities are global UUIDs; pointing one at a table is what creates a row
/// there, and re-pointing it is how a row moves between tables.
#[derive(Debug, Clone)]
pub struct UsrSetTable {
    pub entity_id: Uuid,
    pub table_id: Uuid,
}

/// Soft-deletes an entity by advancing its delete tombstone. An entity is
/// deleted while its tombstone is newer than its table affinity — a later
/// [`UsrSetTable`] un-deletes it.
#[derive(Debug, Clone)]
pub struct UsrDelete {
    pub entity_id: Uuid,
}
