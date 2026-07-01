use crate::error::TablesError;
use crate::op::Delete;
use crate::physical_schema::DELETED_TS_COL;
use crate::reducer::upsert::{bind_pkey, lww_winner_sql, set_lww_sql};
use crate::reducer::{ApplyState, Reducer};
use crate::watch::{ChangeEvent, DeleteEvent};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::{Db, DbBatch, DbValue, StmtId, ValueBinder};

impl Reducer {
    pub(crate) async fn sync_delete_schema(
        &mut self,
        db: &dyn Db,
        delete: &Delete,
    ) -> Result<(), TablesError> {
        self.ensure_table(db, delete.table_id).await?;
        Ok(())
    }

    pub(crate) fn apply_delete(
        &mut self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        delete: &Delete,
    ) -> Result<ApplyState, TablesError> {
        let dialect = batch.dialect();
        let table_id = delete.table_id;
        let table = self.require_table(table_id)?;
        let quoted_table_name = table.get_name();

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

        let timestamp_value = DbValue::Integer(timestamp.raw() as i64);

        // DELETED_TS_COL binding
        insert_into_cols.push(DELETED_TS_COL.into());
        let ts_placeholder = value_binder.bind_next(timestamp_value.clone());
        insert_into_value_binds.push(ts_placeholder.clone());
        set_clauses.push(set_lww_sql(DELETED_TS_COL, &quoted_table_name, dialect));

        for col_id in table.cols.iter() {
            let quoted_col = col_id.col_name();
            let quoted_lww = col_id.lww_col_name();
            set_clauses.push(format!(
                "{quoted_col} = CASE WHEN {quoted_lww} < {ts_placeholder} THEN NULL ELSE {quoted_col} END",
            ));
            set_clauses.push(format!(
                "{quoted_lww} = CASE WHEN {quoted_lww} < {ts_placeholder} THEN NULL ELSE {quoted_lww} END",
            ))
        }

        let sql = format!(
            "INSERT INTO {quoted_table_name} ({}) VALUES ({}) \
            ON CONFLICT ({}) DO UPDATE SET {} WHERE {}",
            insert_into_cols.join(", "),
            insert_into_value_binds.join(", "),
            pk_name_list,
            set_clauses.join(", "),
            lww_winner_sql(&quoted_table_name, DELETED_TS_COL)
        );

        let stmt_id = batch.add_statement(&sql, &value_binder.values());
        let staged_event = if let Some(named_table) = self.named_tables.get(&table_id) {
            Some(ChangeEvent::Delete(DeleteEvent {
                table_id,
                primary_key: delete.primary_key.clone(),
                table_name: named_table.name.clone(),
            }))
        } else {
            None
        };
        Ok(ApplyState {
            stmt_id,
            staged_event,
        })
    }
}
