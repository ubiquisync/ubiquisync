//! Change events the reducer emits for downstream observers.
//!
//! When the reducer applies an op that actually mutates state, it produces a
//! [`ChangeEvent`] describing what changed, in terms of the *user-facing* names
//! of a declared `TableSchema`. Ops against tables
//! the reducer only knows by surrogate ID (no declared schema) produce no event,
//! since there is no user-facing name to report. A [`WatchTarget`] names the
//! granularity at which an observer subscribes.

use crate::id::{ColumnId, TableId};
use crate::op::Value;

/// A materialized state change, ready to dispatch to observers.
#[derive(Debug, Clone)]
pub enum ChangeEvent {
    /// A row was inserted or had one or more columns updated.
    Upsert(UpsertEvent),
    /// A row was soft-deleted (tombstoned).
    Delete(DeleteEvent),
}

/// What an observer subscribes to: either a whole table or a single row within
/// it.
#[derive(Clone, Eq, PartialEq, Hash)]
pub enum WatchTarget {
    /// Every change to any row of the table.
    Table(TableId),
    /// Changes to the one row identified by these primary-key values.
    TableRow(TableId, Vec<Value>),
}

/// An insert or column update on a named table.
#[derive(Debug, Clone)]
pub struct UpsertEvent {
    /// The table the row belongs to.
    pub table_id: TableId,
    /// The table's user-facing name.
    pub table_name: String,
    /// PK values identifying the affected row.
    pub primary_key: Vec<Value>,
    /// The columns this op actually changed (won LWW for). Empty when the op
    /// touched the row but changed no observable column value — e.g. inserting a
    /// row into a table that has only a primary key.
    pub changed_columns: Vec<ColumnValue>,
}

/// One column's new value within an [`UpsertEvent`].
#[derive(Debug, Clone)]
pub struct ColumnValue {
    /// The column that changed.
    pub column_id: ColumnId,
    /// The column's user-facing name.
    pub name: String,
    /// The new value, or `None` if the column was set to SQL NULL.
    pub value: Option<Value>,
}

/// A soft-delete (tombstone) on a named table.
#[derive(Debug, Clone)]
pub struct DeleteEvent {
    /// The table the row belonged to.
    pub table_id: TableId,
    /// The table's user-facing name.
    pub table_name: String,
    /// PK values identifying the deleted row.
    pub primary_key: Vec<Value>,
}
