use crate::error::SchemaSyncError;
use crate::id::ColumnId;
use crate::op::Delete;
use crate::physical_schema::{DELETED_TS_COL, PhysicalTableSchema, row_visible_sql};
use crate::reducer::upsert::{bind_pkey, lww_winner_sql, set_lww_sql};
use crate::reducer::{ApplyState, Reducer};
use crate::watch::{ChangeEvent, ColumnValue, DeleteEvent, UpsertEvent};
use tokio::sync::OwnedRwLockReadGuard;
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::{Db, DbBatch, DbError, DbStatementResult, StmtId, ValueBinder};

impl Reducer {
    pub(crate) async fn sync_delete_schema(
        &self,
        db: &dyn Db,
        delete: &Delete,
    ) -> Result<OwnedRwLockReadGuard<PhysicalTableSchema>, SchemaSyncError> {
        let table = self.ensure_table(db, delete.table_id).await?;
        Ok(table.read_owned().await)
    }

    pub(crate) fn apply_delete(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        delete: &Delete,
        table: OwnedRwLockReadGuard<PhysicalTableSchema>,
    ) -> ApplyState {
        let dialect = batch.dialect();
        let table_id = delete.table_id;
        let quoted_table_name = table.get_quoted_name();

        // Because deletes are soft deletes, counter-intuitively we're actually building a INSERT ON CONFLICT SET statement
        let mut insert_into_cols = vec![]; // INSERT INTO (...)
        let mut insert_into_value_binds = vec![]; // VALUES (?1, ?2, ...)
        let mut value_binder = ValueBinder::new(dialect);
        let mut set_clauses = vec![];

        bind_pkey(
            table_id,
            &delete.primary_key,
            &mut insert_into_cols,
            &mut insert_into_value_binds,
            &mut value_binder,
        );

        let pk_name_list = table_id.pk_col_name_list();

        // DELETED_TS_COL binding
        insert_into_cols.push(DELETED_TS_COL.into());
        let ts_placeholder = value_binder.bind_next(timestamp.into());
        insert_into_value_binds.push(ts_placeholder.clone());
        set_clauses.push(set_lww_sql(DELETED_TS_COL, quoted_table_name, dialect));

        // What the row looks like after the write: whether it is still visible,
        // then per physical value column whether it is unset (NULL with the
        // never-written/cleared lww of 0). Like the rest of the statement this
        // is built from the physical table alone — the same schema snapshot,
        // held by the read guard, that the SET clauses come from. User-facing
        // names are attached later, in `post_delete`.
        let mut returning = vec![row_visible_sql()];
        let returning_cols: Vec<ColumnId> = table.cols.iter().copied().collect();

        for col_id in returning_cols.iter() {
            // Surrogate column names are auto-generated and don't need to be quoted
            let col_name = col_id.col_name();
            let lww_col_name = col_id.lww_col_name();
            set_clauses.push(format!(
                "{col_name} = CASE WHEN {lww_col_name} < {ts_placeholder} THEN NULL ELSE {col_name} END",
            ));
            // Clear the lww back to 0, the never-written sentinel
            set_clauses.push(format!(
                "{lww_col_name} = CASE WHEN {lww_col_name} < {ts_placeholder} THEN 0 ELSE {lww_col_name} END",
            ));
            returning.push(format!("({col_name} IS NULL AND {lww_col_name} = 0)"));
        }

        let sql = format!(
            "INSERT INTO {quoted_table_name} ({}) VALUES ({}) \
            ON CONFLICT ({}) DO UPDATE SET {} WHERE {} RETURNING {}",
            insert_into_cols.join(", "),
            insert_into_value_binds.join(", "),
            pk_name_list,
            set_clauses.join(", "),
            lww_winner_sql(quoted_table_name, DELETED_TS_COL),
            returning.join(", "),
        );

        let stmt_id = batch.add_statement(&sql, &value_binder.values());
        let staged_event = self.logical_tables.get(&table_id).map(|named_table| {
            ChangeEvent::Delete(DeleteEvent {
                table_id,
                primary_key: delete.primary_key.clone(),
                table_name: named_table.name.clone(),
            })
        });
        ApplyState {
            stmt_id,
            staged_event,
            returning_cols,
            table_rguard: table,
        }
    }

    /// Build the event from the delete statement's `RETURNING` row, which
    /// describes the physical row *after* the write (`returning_cols` are the
    /// columns it flags, in order). Names are attached here: a flagged column
    /// the named table doesn't declare is skipped.
    /// - no row: the delete lost to a newer stored delete and changed nothing,
    ///   so no event;
    /// - the row is hidden: a `DeleteEvent`. This may be redundant — it also
    ///   fires for a row that was already deleted or never seen — but it is
    ///   never wrong: it is never emitted for a row that is still visible;
    /// - the row is still visible (the delete is not newer than its latest
    ///   upsert): an `UpsertEvent` reporting as NULL the columns the delete may
    ///   have cleared, or no event if there are none. This too can be
    ///   redundant: it lists every unset column, including ones never written.
    pub(crate) fn post_delete(
        &self,
        stmt_id: StmtId,
        delete_event: DeleteEvent,
        returning_cols: &[ColumnId],
        batch_result: &[DbStatementResult],
    ) -> Result<Option<ChangeEvent>, DbError> {
        let Some(row) = batch_result[stmt_id.0].rows.first() else {
            return Ok(None);
        };
        if row.get_i64(0)? == 0 {
            return Ok(Some(ChangeEvent::Delete(delete_event)));
        }

        let named_table = self.logical_tables.get(&delete_event.table_id);
        let mut cleared_columns = vec![];
        for (idx, col_id) in returning_cols.iter().enumerate() {
            let named_col = named_table.and_then(|t| t.value_cols.get(col_id));
            if let Some(named_col) = named_col
                && row.get_i64(idx + 1)? != 0
            {
                cleared_columns.push(ColumnValue {
                    column_id: *col_id,
                    name: named_col.name.clone(),
                    value: None,
                });
            }
        }
        if cleared_columns.is_empty() {
            return Ok(None);
        }
        Ok(Some(ChangeEvent::Upsert(UpsertEvent {
            table_id: delete_event.table_id,
            table_name: delete_event.table_name,
            primary_key: delete_event.primary_key,
            changed_columns: cleared_columns,
        })))
    }
}
