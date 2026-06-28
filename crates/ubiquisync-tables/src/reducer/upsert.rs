use crate::col_type::ColType;
use crate::op::{Upsert, Value};
use crate::reducer::{ApplyState, Reducer, ReducerError};
use crate::schema::{ColumnSchema, UPSERT_TS_COL};
use crate::util::{quote_ident, value_to_db};
use crate::watch::{ChangeEvent, ColumnValue, UpsertEvent};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::{Db, DbBatch, DbValue, ValueBinder};
use ubiquisync_sql::dialect::SqlDialect;

impl Reducer {
    pub(crate) async fn sync_upsert_schema(
        &mut self,
        db: &dyn Db,
        upsert: &Upsert,
    ) -> Result<(), ReducerError> {
        let table = self.ensure_table(db, upsert.table_id)?;
        for col_update in upsert.sets.iter() {
            table.ensure_column(db, col_update.column_id)?;
        }

        for null_col_id in upsert.nulls.iter() {
            table.ensure_column(db, *null_col_id)?;
        }
        Ok(())
    }

    pub(crate) fn apply_upsert(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        upsert: &Upsert,
    ) -> Result<ApplyState, ReducerError> {
        let dialect = batch.dialect();

        let table = self.require_table(upsert.table_id)?;
        let quoted_table_name = quote_ident(table.get_name());

        let mut insert_into_cols = vec![]; // INSERT INTO (...)
        let mut insert_into_binds = vec![]; // VALUES (?1, ?2, ...)
        let mut value_binder = ValueBinder::new(dialect);

        let pk_count = table.get_id().pk_count();
        for i in 0..pk_count {
            // bind the quoted pk col name into the INSERT column list
            insert_into_cols.push(quote_ident(&table.pk_col_names()[i]));
            // create positional (?1) bind params for each pk val
            insert_into_binds.push(value_binder.bind_next(value_to_db(&upsert.primary_key[i])));
            // add the pk val to the list of bind values
            // TODO validate pkey value types
        }

        // quoted, comma-joined pk list for the ON CONFLICT statement
        let pk_name_list = table
            .pk_col_names()
            .iter()
            .map(|n| quote_ident(n))
            .collect::<Vec<_>>()
            .join(", ");

        // the SET clauses for each non-pk column
        let mut set_clauses = vec![];
        let mut where_clauses = vec![];

        let timestamp_value = DbValue::Integer(timestamp.raw() as i64);

        // UPSERT_TS_COL binding
        insert_into_cols.push(UPSERT_TS_COL.into());
        insert_into_binds.push(value_binder.bind_next(timestamp_value.clone()));
        set_clauses.push(set_lww_sql(UPSERT_TS_COL, &quoted_table_name, dialect));
        where_clauses.push(lww_winner_sql(&quoted_table_name, UPSERT_TS_COL));

        let mut all_updates: Vec<(&ColumnSchema, Option<Value>)> = Vec::new();
        for col_update in upsert.sets.iter() {
            let col_schema = table.require_column(col_update.column_id)?;
            all_updates.push((col_schema, Some(col_update.value)));
        }

        for null_col_id in upsert.nulls.iter() {
            let col_schema = table.require_column(*null_col_id)?;
            all_updates.push((col_schema, None));
        }

        let mut returning_clauses = vec![];
        let mut changed_col_events = vec![];
        for (col_schema, col_value) in all_updates {
            // TODO validate value types
            let value = if let Some(col_value) = col_value {
                value_to_db(&col_value)
            } else {
                DbValue::Null
            };

            // get the column schema
            let quoted_name = quote_ident(&col_schema.name);
            // bind column to the INSERT INTO clause
            insert_into_cols.push(quoted_name.clone());
            // create positional (?3) bind param
            insert_into_binds.push(value_binder.bind_next(value));
            // add the val to the list of bind values

            let quoted_lww = quote_ident(&col_schema.lww_name);
            // bind the lww timestamp column into the INSERT column list
            insert_into_cols.push(quoted_lww.clone());
            // create positional (?3) bind param
            insert_into_binds.push(value_binder.bind_next(timestamp_value.clone()));
            // add the pk val to the list of bind values

            let lww_clause = lww_winner_sql_with_tiebreak(
                &quoted_table_name,
                &quoted_name,
                &quoted_lww,
                col_schema.id.col_type(),
                dialect,
            );
            set_clauses.push(format!(
                "{quoted_name} = CASE WHEN {lww_clause} THEN EXCLUDED.{quoted_name} ELSE {quoted_table_name}.{quoted_name} END"));

            set_clauses.push(set_lww_sql(&quoted_lww, &quoted_table_name, dialect));

            where_clauses.push(lww_clause);

            returning_clauses.push(format!(
                "{quoted_lww} = {}",
                value_binder.bind_next(timestamp_value.clone())
            ));

            changed_col_events.push(ColumnValue {
                column_id: col_schema.id,
                name: col_schema.name.clone(),
                value: col_value.clone(),
            })
        }

        let mut sql = format!(
            "INSERT INTO {quoted_table_name} ({}) VALUES ({}) \
            ON CONFLICT ({}) DO UPDATE SET {} WHERE {}",
            insert_into_cols.join(", "),
            insert_into_binds.join(", "),
            pk_name_list,
            set_clauses.join(", "),
            where_clauses.join(" OR "),
        );
        if !returning_clauses.is_empty() {
            sql.push_str(&format!(" RETURNING {}", returning_clauses.join(", ")));
        }

        let stmt_id = batch.add_statement(&sql, &value_binder.values());
        Ok(ApplyState {
            stmt_id,
            staged_event: ChangeEvent::Upsert(UpsertEvent {
                table_id: upsert.table_id,
                table_name: table.get_name().into(),
                primary_key: upsert.primary_key,
                changed_columns: changed_col_events,
            }),
        })
    }
}

fn lww_winner_sql_with_tiebreak(
    quoted_table_name: &str,
    quoted_col: &str,
    quoted_lww: &str,
    col_type: ColType,
    dialect: SqlDialect,
) -> String {
    format!(
        "{} OR ({})",
        lww_winner_sql(quoted_table_name, quoted_lww),
        tiebreak_sql(quoted_table_name, quoted_col, quoted_lww, col_type, dialect)
    )
}

fn lww_winner_sql(table_name: &str, lww_col: &str) -> String {
    format!("EXCLUDED.{lww_col} > COALESCE({table_name}.{lww_col}, 0)")
}

fn tiebreak_sql(
    quoted_table_name: &str,
    quoted_col: &str,
    quoted_lww: &str,
    col_type: ColType,
    dialect: SqlDialect,
) -> String {
    let collate = if col_type == ColType::Text {
        dialect.text_collate()
    } else {
        ""
    };
    format!(
        "EXCLUDED.{quoted_lww} = {quoted_table_name}.{quoted_lww} \
        AND (EXCLUDED.{quoted_col}{collate} > {quoted_table_name}.{quoted_col}{collate} \
        OR (EXCLUDED.{quoted_col} IS NOT NULL AND {quoted_table_name}.{quoted_col} IS NULL))"
    )
}

fn set_lww_sql(lww_col: &str, table_name: &str, dialect: SqlDialect) -> String {
    let greatest = dialect.scalar_max();
    format!("{lww_col} = {greatest}(COALESCE({table_name}.{lww_col}, 0), EXCLUDED.{lww_col})")
}
