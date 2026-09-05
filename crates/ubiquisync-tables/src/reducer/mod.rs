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

use crate::id::TableId;
use crate::op::Op;
use crate::physical_schema::PhysicalTableSchema;
use crate::schema::TableSchema;
use crate::watch::ChangeEvent;
use crate::{codec::Codec, error::TablesError};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{OwnedRwLockReadGuard, RwLock};
use ubiquisync_core::ids::ContainerId;
use ubiquisync_sql::{
    db::{Db, DbBatch, DbStatementResult, StmtId},
    op::OpCodec,
};

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
    codec: Codec,
    prefix: String,
    physical_tables: RwLock<HashMap<TableId, Arc<RwLock<PhysicalTableSchema>>>>,
    logical_tables: HashMap<TableId, TableSchema>,
}

impl Reducer {
    /// Open a reducer with `prefix` for surrogate table names, declaring each of
    /// `tables` as a named, user-facing table: its physical storage is
    /// created/reconciled in `db` up front, a SQL VIEW exposing it under the
    /// declared names is (re)created, and it is tracked so its changes surface as
    /// events.
    ///
    /// Returns [`TablesError::InvalidSchema`] if two declarations share a table
    /// `id` or a view `name`; the check runs before any table or view is
    /// created, so a rejected call has no side effects.
    pub async fn new(
        container_id: ContainerId,
        prefix: &str,
        tables: &[TableSchema],
        db: &dyn Db,
    ) -> Result<Self, TablesError> {
        let mut seen_ids = HashSet::new();
        let mut seen_names = HashSet::new();
        for table in tables {
            if !seen_ids.insert(table.id) {
                return Err(TablesError::InvalidSchema(format!(
                    "duplicate table id {:?}",
                    table.id
                )));
            }
            if !seen_names.insert(table.name.as_str()) {
                return Err(TablesError::InvalidSchema(format!(
                    "duplicate table name {:?}",
                    table.name
                )));
            }
        }

        let mut named_tables = HashMap::new();
        let mut all_tables = HashMap::new();
        for table in tables {
            named_tables.insert(table.id, table.clone());
            let physical_table = PhysicalTableSchema::new_named(prefix, table, db).await?;
            all_tables.insert(table.id, Arc::new(RwLock::new(physical_table)));
            table.create_view(prefix, db).await?;
        }
        Ok(Self {
            codec: Codec::new(container_id),
            prefix: prefix.into(),
            physical_tables: RwLock::new(all_tables),
            logical_tables: named_tables,
        })
    }
}

#[async_trait::async_trait]
impl ubiquisync_sql::reducer::Reducer for Reducer {
    type Op = Op;
    type Error = TablesError;
    type ReadState = ReadState;
    type ApplyState = ApplyState;

    fn codec(&self) -> &dyn OpCodec<Op> {
        &self.codec
    }

    async fn prepare(&self, db: &dyn Db, op: &Op) -> Result<Self::ReadState, Self::Error> {
        // Reject malformed ops before touching the schema or building any SQL.
        let table_rguard = match op {
            Op::Upsert(upsert) => {
                validate::validate_upsert(upsert)?;
                self.sync_upsert_schema(db, upsert).await?
            }
            Op::Delete(delete) => {
                validate::validate_delete(delete)?;
                self.sync_delete_schema(db, delete).await?
            }
        };
        Ok(ReadState { table_rguard })
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: ubiquisync_core::hlc::Timestamp,
        op: &Op,
        read_state: Self::ReadState,
    ) -> Result<ApplyState, Self::Error> {
        match op {
            Op::Upsert(upsert) => {
                self.apply_upsert(batch, timestamp, upsert, read_state.table_rguard)
            }
            Op::Delete(delete) => {
                self.apply_delete(batch, timestamp, delete, read_state.table_rguard)
            }
        }
    }

    fn post_apply(
        &self,
        apply_state: Self::ApplyState,
        batch_result: &[DbStatementResult],
    ) -> Result<(), Self::Error> {
        // A single table op maps to at most one change event; `post_upsert`/
        // `post_delete` return `None` when the write lost LWW or hit an unnamed
        // table. Collect that 0-or-1 into the reducer's 0-or-many contract.
        let event = match apply_state.staged_event {
            Some(ChangeEvent::Upsert(event)) => {
                self.post_upsert(apply_state.stmt_id, event, batch_result)?
            }
            Some(ChangeEvent::Delete(event)) => {
                self.post_delete(apply_state.stmt_id, event, batch_result)?
            }
            None => None,
        };
        // TODO manage event dispatch ourselves
        // let _events = event.into_iter().collect();
        Ok(())
    }
}

pub struct ReadState {
    /// We thread a read guard through from prepare -> apply -> post_apply
    /// to ensure that there are no schema changes to the table before the
    /// SQL written in apply has been committed.
    /// If this does not happen we could end up with a weird edge case where
    /// some other transaction adds a column that is not visible to the SQL
    /// generation in apply but exists at the time when the SQL is committed.
    /// This edge case in particular appears in the case of NULLing out column
    /// data on delete and can result in inconsistent table state, especially
    /// if the deleted column is ever resurrected.
    /// While this edge case is likely quite rare, we want to have strong eventual
    /// consistency guarantees in our system and in general allowing DDL to
    /// race SQL generation that depends on a table schema being consistent can result in such
    /// scenarios, so to avoid this we simply ensure the table we saw when prepare
    /// was called is the table schema that will be active when that SQL is committed.
    /// An alternative solution to this particular race condition would be to
    /// resolve column liveness based on delete timestamp vs the col lww timestamp
    /// in the view and all query layers.
    /// While this solution may result in equivalent observable state and also
    /// reduces the complexity of delete SQL (it wouldn't need to NULL all known columns),
    /// it does push more complexity into the query layer and if some query didn't
    /// use prescribed query paths and read the db directly, it would observe the inconsistent state.
    /// More generally, the issue is that this alternative is a specific patch for this data
    /// race rather than a general solution to such data races which is to simply ensure
    /// (as we are doing here with the read guard) that the schema doesn't change before
    /// we commit SQL built on that schema.
    /// (Note that adding new columns AFTER such SQL executes is always safe because those
    /// columns will get added with the expected default values, not some stale values we
    /// should have NULLED at SQL generation time.)
    pub(crate) table_rguard: OwnedRwLockReadGuard<PhysicalTableSchema>,
}

/// Carried from [`apply`](ubiquisync_sql::reducer::Reducer::apply) to
/// [`post_apply`](ubiquisync_sql::reducer::Reducer::post_apply): where to find
/// this op's result in the committed batch, and the event to emit if it took
/// effect.
pub struct ApplyState {
    /// The op's statement in the batch; indexes its [`DbStatementResult`].
    pub(crate) stmt_id: StmtId,
    /// The event to emit, provisional until the batch result confirms the write
    /// changed something. `None` for ops on unnamed (surrogate) tables.
    pub(crate) staged_event: Option<ChangeEvent>,
    /// Passing the guard back in ApplyState ensures that it is not released before
    /// the batch has been committed, then it can be released in post_apply.
    pub(crate) table_rguard: OwnedRwLockReadGuard<PhysicalTableSchema>,
}
