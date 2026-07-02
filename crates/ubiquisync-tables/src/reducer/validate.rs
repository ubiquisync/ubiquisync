//! Op validation against the types encoded in its table/column IDs.
//!
//! The reducer trusts nothing about an op's shape: ops are decoded from the log,
//! possibly authored by another peer. Before an op reaches schema reconciliation
//! or SQL generation, [`validate_upsert`]/[`validate_delete`] reject any op whose
//! values don't match the declared types, whose primary key has the wrong arity
//! or types, or that names a column more than once (which would otherwise emit a
//! duplicate column into the generated `INSERT`).

use std::collections::HashSet;

use crate::error::TablesError;
use crate::id::{ColumnId, TableId};
use crate::op::{Delete, Upsert, Value};

/// Reject an [`Upsert`] whose PK, column values, or column set is malformed.
pub(crate) fn validate_upsert(upsert: &Upsert) -> Result<(), TablesError> {
    validate_pk(upsert.table_id, &upsert.primary_key)?;

    // Every written column — value or NULL — must be named at most once, or the
    // generated INSERT would list a duplicate column.
    let mut seen = HashSet::new();
    for set in &upsert.sets {
        check_value_type(set.column_id, &set.value)?;
        insert_unique(&mut seen, set.column_id)?;
    }
    for &null_col in &upsert.nulls {
        insert_unique(&mut seen, null_col)?;
    }
    Ok(())
}

/// Reject a [`Delete`] whose primary key is malformed.
pub(crate) fn validate_delete(delete: &Delete) -> Result<(), TablesError> {
    validate_pk(delete.table_id, &delete.primary_key)
}

/// The primary key must have exactly one value per PK slot, each matching that
/// slot's declared type.
fn validate_pk(table_id: TableId, primary_key: &[Value]) -> Result<(), TablesError> {
    let expected = table_id.pk_count();
    if primary_key.len() != expected {
        return Err(invalid(format!(
            "expected {expected} primary key value(s), got {}",
            primary_key.len()
        )));
    }
    for (i, value) in primary_key.iter().enumerate() {
        let want = table_id.pk_col_type(i);
        if value.col_type() != want {
            return Err(invalid(format!(
                "primary key {i} expected {want:?}, got {:?}",
                value.col_type()
            )));
        }
    }
    Ok(())
}

fn check_value_type(column_id: ColumnId, value: &Value) -> Result<(), TablesError> {
    let want = column_id.col_type();
    if value.col_type() != want {
        return Err(invalid(format!(
            "column {column_id:?} expected {want:?}, got {:?}",
            value.col_type()
        )));
    }
    Ok(())
}

fn insert_unique(seen: &mut HashSet<ColumnId>, column_id: ColumnId) -> Result<(), TablesError> {
    if seen.insert(column_id) {
        Ok(())
    } else {
        Err(invalid(format!("column {column_id:?} referenced more than once")))
    }
}

fn invalid(msg: String) -> TablesError {
    TablesError::InvalidOp(msg)
}
