use crate::col_type::ColType;
use crate::error::TablesError;
use crate::id::{ColumnId, TableId};
use crate::op::{Upsert, Value};
use crate::physical_schema::{DELETED_TS_COL, UPSERT_TS_COL};
use crate::reducer::{ApplyState, Reducer};
use crate::watch::{ChangeEvent, ColumnValue, UpsertEvent};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::{Db, DbBatch, DbStatementResult, DbValue, StmtId, ValueBinder};
use ubiquisync_sql::dialect::SqlDialect;
use ubiquisync_sql::util::quote_ident;

impl Reducer {
    pub(crate) async fn sync_upsert_schema(
        &mut self,
        db: &dyn Db,
        upsert: &Upsert,
    ) -> Result<(), TablesError> {
        let table = self.ensure_table(db, upsert.table_id).await?;
        for col_update in upsert.sets.iter() {
            table.ensure_column(db, col_update.column_id).await?;
        }

        for null_col_id in upsert.nulls.iter() {
            table.ensure_column(db, *null_col_id).await?;
        }
        Ok(())
    }

    pub(crate) fn apply_upsert(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        upsert: &Upsert,
    ) -> Result<ApplyState, TablesError> {
        let dialect = batch.dialect();

        let table_id = upsert.table_id;
        let table = self.require_table(table_id)?;
        let named_table = self.named_tables.get(&table_id);
        let quoted_table_name = table.get_quoted_name();

        let mut insert_into_cols = vec![]; // INSERT INTO (...)
        let mut insert_into_value_binds = vec![]; // VALUES (?1, ?2, ...)
        let mut value_binder = ValueBinder::new(dialect);

        bind_pkey(
            table_id,
            &upsert.primary_key,
            &mut insert_into_cols,
            &mut insert_into_value_binds,
            &mut value_binder,
        );

        let pk_name_list = table_id.pk_col_name_list();

        // the SET clauses for each non-pk column
        let mut set_clauses = vec![];
        let mut where_clauses = vec![];

        let timestamp_value = DbValue::from_u64(timestamp.raw())?;
        let timestamp_placeholder = value_binder.bind_next(timestamp_value.clone());

        // UPSERT_TS_COL binding
        insert_into_cols.push(UPSERT_TS_COL.into());
        insert_into_value_binds.push(timestamp_placeholder.clone());
        set_clauses.push(set_lww_sql(UPSERT_TS_COL, &quoted_table_name, dialect));
        where_clauses.push(lww_winner_sql(&quoted_table_name, UPSERT_TS_COL));

        let mut all_updates: Vec<(ColumnId, Option<Value>)> = Vec::new();
        for col_update in upsert.sets.iter() {
            all_updates.push((col_update.column_id, Some(col_update.value.clone())));
        }

        for null_col_id in upsert.nulls.iter() {
            all_updates.push((*null_col_id, None));
        }

        let mut returning_clauses = vec![];
        let mut changed_col_events = vec![];
        for (col_id, col_value) in all_updates.iter() {
            // TODO validate value types
            let value = if let Some(col_value) = col_value {
                col_value.to_db()
            } else {
                DbValue::Null
            };

            // Surrogate column names are auto-generated and don't need to be quoted
            let col_name = col_id.col_name();
            let lww_col_name = col_id.lww_col_name();
            insert_into_cols.push(col_name.clone());
            insert_into_value_binds.push(value_binder.bind_next(value));

            insert_into_cols.push(lww_col_name.clone());
            insert_into_value_binds.push(timestamp_placeholder.clone());

            let lww_clause = lww_winner_sql_with_tiebreak(
                &quoted_table_name,
                &col_name,
                &lww_col_name,
                col_id.col_type(),
                dialect,
            );
            set_clauses.push(format!(
                "{col_name} = CASE WHEN {lww_clause} THEN EXCLUDED.{col_name} ELSE {quoted_table_name}.{col_name} END"));

            set_clauses.push(set_lww_sql(&lww_col_name, &quoted_table_name, dialect));

            where_clauses.push(lww_clause);

            if let Some(named_table) = named_table {
                if let Some(named_col) = named_table.value_cols.get(col_id) {
                    // Only add returning clauses and events when we have a named column in a named table.
                    returning_clauses.push(format!("{lww_col_name} = {timestamp_placeholder}"));

                    changed_col_events.push(ColumnValue {
                        column_id: *col_id,
                        name: named_col.name.clone(),
                        value: col_value.clone(),
                    })
                }
            }
        }

        let mut sql = format!(
            "INSERT INTO {quoted_table_name} ({}) VALUES ({}) \
            ON CONFLICT ({}) DO UPDATE SET {} WHERE ({}) \
            AND {timestamp_placeholder} >= COALESCE({DELETED_TS_COL},0)",
            insert_into_cols.join(", "),
            insert_into_value_binds.join(", "),
            pk_name_list,
            set_clauses.join(", "),
            where_clauses.join(" OR "),
        );
        if !returning_clauses.is_empty() {
            sql.push_str(&format!(" RETURNING {}", returning_clauses.join(", ")));
        }

        let stmt_id = batch.add_statement(&sql, &value_binder.values());
        let staged_event = if let Some(named_table) = named_table {
            Some(ChangeEvent::Upsert(UpsertEvent {
                table_id: upsert.table_id,
                table_name: named_table.name.clone(),
                primary_key: upsert.primary_key.clone(),
                changed_columns: changed_col_events,
            }))
        } else {
            None
        };
        Ok(ApplyState {
            stmt_id,
            staged_event,
        })
    }

    pub(crate) fn post_upsert(
        &self,
        stmt_id: StmtId,
        mut upsert_event: UpsertEvent,
        batch_result: &[DbStatementResult],
    ) -> Result<Option<ChangeEvent>, TablesError> {
        let res = &batch_result[stmt_id.0];
        if res.rows_affected == 0 {
            return Ok(None);
        }

        if let Some(row) = res.rows.first() {
            let changed_columns = upsert_event.changed_columns;

            // Filter out the column which "won" based on a newer lww timestamp
            let mut winning_columns = Vec::with_capacity(changed_columns.len());
            for (idx, column) in changed_columns.into_iter().enumerate() {
                if row.get_bool(idx)? {
                    winning_columns.push(column);
                }
            }

            upsert_event.changed_columns = winning_columns;
        };

        Ok(Some(ChangeEvent::Upsert(upsert_event)))
    }
}

fn lww_winner_sql_with_tiebreak(
    quoted_table_name: &str,
    col: &str,
    lww_col: &str,
    col_type: ColType,
    dialect: SqlDialect,
) -> String {
    format!(
        "{} OR ({})",
        lww_winner_sql(quoted_table_name, lww_col),
        tiebreak_sql(quoted_table_name, col, lww_col, col_type, dialect)
    )
}

pub(crate) fn lww_winner_sql(table_name: &str, lww_col: &str) -> String {
    format!("EXCLUDED.{lww_col} > COALESCE({table_name}.{lww_col}, 0)")
}

fn tiebreak_sql(
    quoted_table_name: &str,
    col: &str,
    lww_col: &str,
    col_type: ColType,
    dialect: SqlDialect,
) -> String {
    let collate = if col_type == ColType::Text {
        dialect.text_collate()
    } else {
        ""
    };
    format!(
        "EXCLUDED.{lww_col} = {quoted_table_name}.{lww_col} \
        AND (EXCLUDED.{col}{collate} > {quoted_table_name}.{col}{collate} \
        OR (EXCLUDED.{col} IS NOT NULL AND {quoted_table_name}.{col} IS NULL))"
    )
}

pub(crate) fn set_lww_sql(lww_col: &str, table_name: &str, dialect: SqlDialect) -> String {
    let greatest = dialect.scalar_max();
    format!("{lww_col} = {greatest}(COALESCE({table_name}.{lww_col}, 0), EXCLUDED.{lww_col})")
}

pub(crate) fn bind_pkey(
    table_id: TableId,
    primary_key: &[Value],
    insert_into_cols: &mut Vec<String>,
    insert_into_value_binds: &mut Vec<String>,
    value_binder: &mut ValueBinder,
) {
    let pk_count = table_id.pk_count();
    for i in 0..pk_count {
        // bind the quoted pk col name into the INSERT column list
        insert_into_cols.push(quote_ident(&table_id.pk_col_name(i)));
        // create positional (?1) bind params for each pk val
        insert_into_value_binds.push(value_binder.bind_next(primary_key[i].to_db()));
        // add the pk val to the list of bind values
        // TODO validate pkey value types
    }
}
