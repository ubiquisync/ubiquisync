//! The table [`Reducer`]: materializes table [ops](crate::op) into LWW SQL
//! writes and emits [change events](crate::watch) for observers.
//!
//! Implements the [`ubiquisync_sql::reducer::Reducer`] three-phase contract —
//! `prepare` reconciles schema (creating/altering surrogate tables) outside the
//! batch, `apply` emits the conditional upsert/delete statements, and
//! `post_apply` turns the committed batch result into a [`ChangeEvent`].

mod delete;
mod schema;
mod upsert;
mod validate;

use crate::error::TablesError;
use crate::id::TableId;
use crate::op::Op;
use crate::physical_schema::PhysicalTableSchema;
use crate::schema::TableSchema;
use crate::watch::ChangeEvent;
use std::collections::HashMap;
use ubiquisync_sql::db::{Db, DbBatch, DbStatementResult, StmtId};

/// Applies table ops to a SQL backend, merging every column last-writer-wins.
///
/// `all_tables` holds the physical (surrogate-named) schema for every table the
/// reducer has touched — the storage all ops are written to. `named_tables`
/// holds the subset the caller declared with user-facing names: these are the
/// application's own tables, exposed for querying as a SQL VIEW over the physical
/// storage using the declared table/column names, and their changes surface as
/// [`ChangeEvent`]s carrying those names. A table seen only by surrogate ID
/// (e.g. one a newer peer defined that this build doesn't model) is still
/// materialized and merged, but has no view and emits no events.
pub struct Reducer {
    prefix: String,
    all_tables: HashMap<TableId, PhysicalTableSchema>,
    named_tables: HashMap<TableId, TableSchema>,
}

impl Reducer {
    /// Open a reducer with `prefix` for surrogate table names, declaring each of
    /// `tables` as a named, user-facing table: its physical storage is
    /// created/reconciled in `db` up front, a SQL VIEW exposing it under the
    /// declared names is (re)created, and it is tracked so its changes surface as
    /// events.
    pub async fn new(
        prefix: &str,
        tables: &[TableSchema],
        db: &dyn Db,
    ) -> Result<Self, TablesError> {
        let mut named_tables = HashMap::new();
        let mut all_tables = HashMap::new();
        for table in tables {
            named_tables.insert(table.id, table.clone());
            let physical_table = PhysicalTableSchema::new_named(prefix, table, db).await?;
            all_tables.insert(table.id, physical_table);
            table.create_view(prefix, db).await?;
        }
        Ok(Self {
            prefix: prefix.into(),
            all_tables,
            named_tables,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl ubiquisync_sql::reducer::Reducer for Reducer {
    type Op = Op;
    type Error = TablesError;
    type ReadState = ();
    type ApplyState = ApplyState;
    type Event = Option<ChangeEvent>;

    async fn prepare(&mut self, db: &dyn Db, op: &Op) -> Result<(), Self::Error> {
        // Reject malformed ops before touching the schema or building any SQL.
        match op {
            Op::Upsert(upsert) => {
                validate::validate_upsert(upsert)?;
                self.sync_upsert_schema(db, upsert).await?
            }
            Op::Delete(delete) => {
                validate::validate_delete(delete)?;
                self.sync_delete_schema(db, delete).await?
            }
        };
        Ok(())
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: ubiquisync_core::hlc::Timestamp,
        op: &Op,
        _: (),
    ) -> Result<ApplyState, Self::Error> {
        match op {
            Op::Upsert(upsert) => self.apply_upsert(batch, timestamp, upsert),
            Op::Delete(delete) => self.apply_delete(batch, timestamp, delete),
        }
    }

    fn post_apply(
        &self,
        apply_state: Self::ApplyState,
        batch_result: &[DbStatementResult],
    ) -> Result<Option<ChangeEvent>, Self::Error> {
        if let Some(event) = apply_state.staged_event {
            match event {
                ChangeEvent::Upsert(event) => {
                    self.post_upsert(apply_state.stmt_id, event, batch_result)
                }
                ChangeEvent::Delete(event) => {
                    self.post_delete(apply_state.stmt_id, event, batch_result)
                }
            }
        } else {
            Ok(None)
        }
    }
}

/// Carried from [`apply`](ubiquisync_sql::reducer::Reducer::apply) to
/// [`post_apply`](ubiquisync_sql::reducer::Reducer::post_apply): where to find
/// this op's result in the committed batch, and the event to emit if it took
/// effect.
pub struct ApplyState {
    /// The op's statement in the batch; indexes its [`DbStatementResult`].
    pub stmt_id: StmtId,
    /// The event to emit, provisional until the batch result confirms the write
    /// changed something. `None` for ops on unnamed (surrogate) tables.
    pub staged_event: Option<ChangeEvent>,
}
