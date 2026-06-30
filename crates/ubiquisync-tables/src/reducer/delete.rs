use std::os::unix::fs::MetadataExt;
use std::panic::take_hook;

use crate::op::Delete;
use crate::reducer::upsert::{bind_pkey, lww_winner_sql, mk_pkey_name_list, set_lww_sql};
use crate::reducer::{Reducer, ReducerError};
use crate::schema::DELETED_TS_COL;
use crate::util::quote_ident;
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::{Db, DbBatch, DbValue, StmtId, ValueBinder};

impl Reducer {
    pub(crate) async fn sync_delete_schema(
        &mut self,
        db: &dyn Db,
        delete: &Delete,
    ) -> Result<(), ReducerError> {
        self.ensure_table(db, delete.table_id).await?;
        Ok(())
    }

    pub(crate) fn apply_delete(
        &mut self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        delete: &Delete,
    ) -> Result<StmtId, ReducerError> {
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
            table,
            &delete.primary_key,
            &mut insert_into_cols,
            &mut insert_into_value_binds,
            &mut value_binder,
        );

        let pk_name_list = mk_pkey_name_list(table);

        let timestamp_value = DbValue::Integer(timestamp.raw() as i64);

        // DELETED_TS_COL binding
        insert_into_cols.push(DELETED_TS_COL.into());
        let ts_placeholder = value_binder.bind_next(timestamp_value.clone());
        insert_into_value_binds.push(ts_placeholder.clone());
        set_clauses.push(set_lww_sql(DELETED_TS_COL, &quoted_table_name, dialect));

        for col in table.non_pkey_cols() {
            let quoted_col = quote_ident(&col.name);
            let quoted_lww = quote_ident(&col.lww_name);
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

        Ok(batch.add_statement(&sql, &value_binder.values()))
    }
}
