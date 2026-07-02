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
        set_clauses.push(set_lww_sql(UPSERT_TS_COL, quoted_table_name, dialect));
        where_clauses.push(lww_winner_sql(quoted_table_name, UPSERT_TS_COL));

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
            let value = if let Some(col_value) = col_value {
                col_value.to_db()
            } else {
                DbValue::Null
            };

            // Surrogate column names are auto-generated and don't need to be quoted
            let col_name = col_id.col_name();
            let lww_col_name = col_id.lww_col_name();
            let value_placeholder = value_binder.bind_next(value);
            insert_into_cols.push(col_name.clone());
            insert_into_value_binds.push(value_placeholder.clone());

            insert_into_cols.push(lww_col_name.clone());
            insert_into_value_binds.push(timestamp_placeholder.clone());

            let lww_clause = lww_winner_sql_with_tiebreak(
                quoted_table_name,
                &col_name,
                &lww_col_name,
                col_id.col_type(),
                dialect,
            );
            set_clauses.push(format!(
                "{col_name} = CASE WHEN {lww_clause} THEN EXCLUDED.{col_name} ELSE {quoted_table_name}.{col_name} END"));

            set_clauses.push(set_lww_sql(&lww_col_name, quoted_table_name, dialect));

            where_clauses.push(lww_clause);

            // Only add returning clauses and events for a named column of a named table.
            if let Some(named_col) = named_table.and_then(|t| t.value_cols.get(col_id)) {
                // Report a column as changed only if it actually holds our value
                // at our timestamp. The lww check alone would wrongly report a
                // value-tiebreak LOSER (its lww still equals our ts), so we also
                // confirm the stored value is the one we wrote (NULL-safe).
                //
                // This still over-reports one benign case: any winning write of a
                // column's *existing* value — re-setting the same value at an
                // equal or newer ts still wins lww and passes the check — is
                // reported as a change, because RETURNING sees only the
                // post-update value, not whether it differed. A spurious
                // notification, never wrong data; and we can't skip the lww
                // advance that LWW correctness depends on.
                let null_safe_eq = dialect.null_safe_eq();
                returning_clauses.push(format!(
                    "({lww_col_name} = {timestamp_placeholder} AND {col_name} {null_safe_eq} {value_placeholder})"
                ));

                changed_col_events.push(ColumnValue {
                    column_id: *col_id,
                    name: named_col.name.clone(),
                    value: col_value.clone(),
                })
            }
        }

        // The trailing `ts >= __deleted_ts` guard blocks the whole upsert — data
        // and `__upsert_ts` alike — when a newer delete has tombstoned the row.
        // Consequence: `__upsert_ts` can differ between replicas that saw the
        // same ops in a different order (an upsert shadowed by a later delete
        // advances it on one peer, is skipped on another). This is unobservable —
        // `__upsert_ts` never gates column data and the live/deleted state still
        // converges — and re-heals on the next winning upsert, so we accept it
        // rather than complicate the statement to make `__upsert_ts` converge.
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
        let staged_event = named_table.map(|named_table| {
            ChangeEvent::Upsert(UpsertEvent {
                table_id: upsert.table_id,
                table_name: named_table.name.clone(),
                primary_key: upsert.primary_key.clone(),
                changed_columns: changed_col_events,
            })
        });
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
    // PK arity and value types are already validated
    for (i, value) in primary_key.iter().enumerate() {
        // bind the quoted pk col name into the INSERT column list, and a
        // positional (?1) bind param for its value
        insert_into_cols.push(quote_ident(&table_id.pk_col_name(i)));
        insert_into_value_binds.push(value_binder.bind_next(value.to_db()));
    }
}
