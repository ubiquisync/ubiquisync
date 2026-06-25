use crate::col_type::ColType;
use crate::db::{Db, DbValue};
use crate::id::ColumnId;
use crate::op::{Upsert, Value};
use crate::reducer::{Reducer, ReducerError};
use crate::util::value_to_db;
use crate::watch::UpsertEvent;
use ubiquisync_core::hlc::Timestamp;

impl Reducer {
    pub(crate) fn apply_upsert(
        &mut self,
        db: &dyn Db,
        timestamp: Timestamp,
        upsert: &Upsert,
    ) -> Result<Option<UpsertEvent>, ReducerError> {
        let table = self.ensure_table(db, upsert.table_id)?;

        let pk_vals: Vec<DbValue> = upsert
            .primary_key
            .iter()
            .map(|pk| value_to_db(pk))
            .collect();

        let mut update_cols: Vec<(ColumnId, String, ColType, Option<DbValue>)> = Vec::new();
        for col_update in upsert.sets.iter() {
            let col_name = table.ensure_column(db, col_update.column_id)?;
            let col_type = col_update.column_id.col_type();
            let db_val = value_to_db(&col_update.value);
            update_cols.push((col_update.column_id, col_name, col_type, Some(db_val)));
        }

        for null_col_id in upsert.nulls.iter() {
            let col_name = table.ensure_column(db, *null_col_id)?;
            let col_type = null_col_id.col_type();
            update_cols.push((*null_col_id, col_name, col_type, None));
        }

        todo!()
        //     // Resolve table: known (compiled SysTable) or unknown (surrogate).
        //     let (sql_name, table_name, pk_col_names, resolve_col) = match self.find_sys_table(upsert.table_id) {
        //         Some(table) => {
        //             let pk_names: Vec<String> = table.pk_names.iter().map(|n| quote_ident(n)).collect();
        //             let name = table.name.to_string();
        //             let sql = table.sql_table_name(&self.prefix);
        //             (sql, name, pk_names, ResolveCol::Known(table))
        //         }
        //         None => {
        //             // Unknown table — create/ensure surrogate
        //             let all_col_ids: Vec<ColumnId> = upsert.sets.iter()
        //                 .map(|u| u.column_id)
        //                 .chain(upsert.nulls.iter().copied())
        //                 .collect();
        //             let sql = ensure_surrogate_table(db, &self.prefix, upsert.table_id, &all_col_ids)?;
        //             let pk_count = (upsert.table_id.pk_count() + 1) as usize;
        //             let pk_names: Vec<String> = (0..pk_count).map(|i| quote_ident(&surrogate_pk_name(i))).collect();
        //             let raw: u16 = upsert.table_id.into();
        //             let name = format!("sys_0x{raw:04X}");
        //             (sql, name, pk_names, ResolveCol::Surrogate)
        //         }
        //     };
        //
        //     // Resolve each SysColumnUpdate into (col_id, col_name, col_type, db_value).
        //     let mut update_cols: Vec<(ColumnId, String, ColType, DbValue)> = Vec::new();
        //     for col_update in &upsert.updates {
        //         let col_name = resolve_col.name(col_update.column_id)?;
        //         let col_type = col_update.column_id.col_type();
        //         let db_val = col_value_to_db(&col_update.value);
        //         update_cols.push((col_update.column_id, col_name, col_type, db_val));
        //     }
        //
        //     // Null columns: treated as LWW with NULL value.
        //     let mut null_cols: Vec<(SysColumnId, String, SysColType)> = Vec::new();
        //     for &null_col_id in &upsert.nulls {
        //         let col_name = resolve_col.name(null_col_id)?;
        //         let col_type = null_col_id.col_type();
        //         null_cols.push((null_col_id, col_name, col_type));
        //     }
        //
        //     // Build INSERT column list and parameter values.
        //     let mut insert_cols: Vec<String> = pk_col_names.clone();
        //     let mut params: Vec<DbValue> = upsert.primary_key.iter().map(|pk| pk_to_db(pk)).collect();
        //
        //     for (_, col_name, col_type, db_val) in &update_cols {
        //         insert_cols.push(quote_ident(col_name));
        //         params.push(db_val.clone());
        //         if needs_lww_ts(*col_type) {
        //             insert_cols.push(quote_ident(&ts_col_name(col_name)));
        //             params.push(DbValue::Integer(timestamp.raw() as i64));
        //         }
        //     }
        //
        //     for (_, col_name, col_type) in &null_cols {
        //         insert_cols.push(quote_ident(col_name));
        //         params.push(DbValue::Null);
        //         if needs_lww_ts(*col_type) {
        //             insert_cols.push(quote_ident(&ts_col_name(col_name)));
        //             params.push(DbValue::Integer(timestamp.raw() as i64));
        //         }
        //     }
        //
        //     let placeholders: String = (1..=params.len())
        //         .map(|i| format!("?{i}"))
        //         .collect::<Vec<_>>()
        //         .join(", ");
        //
        //     // Track columns by merge strategy for RETURNING parsing.
        //     let mut lww_cols: Vec<(SysColumnId, String, Option<SysColValue>)> = Vec::new(); // (id, name, value or None for null)
        //     let mut max_cols: Vec<(SysColumnId, String)> = Vec::new();
        //     let mut has_max = false;
        //
        //     // Build ON CONFLICT DO UPDATE SET clauses.
        //     let mut set_clauses: Vec<String> = Vec::new();
        //     let mut where_conditions: Vec<String> = Vec::new();
        //
        //     for (col_id, col_name, col_type, _) in &update_cols {
        //         let qcol = quote_ident(col_name);
        //         match col_type {
        //             SysColType::MaxI64 => {
        //                 has_max = true;
        //                 max_cols.push((*col_id, col_name.clone()));
        //                 set_clauses.push(format!(
        //                     "{qcol} = MAX(COALESCE({sql_name}.{qcol}, excluded.{qcol}), excluded.{qcol})",
        //                 ));
        //             }
        //             _ => {
        //                 // LWW: newer timestamp wins
        //                 let ts = quote_ident(&ts_col_name(col_name));
        //                 lww_cols.push((*col_id, col_name.clone(), Some(upsert.updates.iter()
        //                     .find(|u| u.column_id == *col_id)
        //                     .unwrap()
        //                     .value.clone())));
        //                 set_clauses.push(format!(
        //                     "{qcol} = CASE WHEN excluded.{ts} > COALESCE({sql_name}.{ts}, 0) THEN excluded.{qcol} ELSE {sql_name}.{qcol} END",
        //                 ));
        //                 set_clauses.push(format!(
        //                     "{ts} = MAX(COALESCE({sql_name}.{ts}, 0), excluded.{ts})",
        //                 ));
        //                 where_conditions.push(format!("excluded.{ts} > COALESCE({sql_name}.{ts}, 0)"));
        //             }
        //         }
        //     }
        //
        //     // Null columns are always LWW.
        //     for (col_id, col_name, _) in &null_cols {
        //         let qcol = quote_ident(col_name);
        //         let ts = quote_ident(&ts_col_name(col_name));
        //         lww_cols.push((*col_id, col_name.clone(), None));
        //         set_clauses.push(format!(
        //             "{qcol} = CASE WHEN excluded.{ts} > COALESCE({sql_name}.{ts}, 0) THEN excluded.{qcol} ELSE {sql_name}.{qcol} END",
        //         ));
        //         set_clauses.push(format!(
        //             "{ts} = MAX(COALESCE({sql_name}.{ts}, 0), excluded.{ts})",
        //         ));
        //         where_conditions.push(format!("excluded.{ts} > COALESCE({sql_name}.{ts}, 0)"));
        //     }
        //
        //     if set_clauses.is_empty() {
        //         // PK-only insert, no columns to update.
        //         let sql = format!(
        //             "INSERT OR IGNORE INTO {sql_name} ({cols}) VALUES ({vals})",
        //             cols = insert_cols.join(", "),
        //             vals = placeholders,
        //         );
        //         let rows_changed = db.execute(&sql, &params)?;
        //         return if rows_changed > 0 {
        //             Ok(Some(UpsertResult {
        //                 table_id: upsert.table_id,
        //                 table_name: table_name.clone(),
        //                 primary_key: upsert.primary_key.clone(),
        //                 changed_columns: vec![],
        //             }))
        //         } else {
        //             Ok(None)
        //         };
        //     }
        //
        //     // Build RETURNING clause.
        //     let mut returning_exprs: Vec<String> = Vec::new();
        //     let needs_ts_param = !lww_cols.is_empty();
        //     if needs_ts_param {
        //         let ts_param_idx = params.len() + 1;
        //         for (_, col_name, _) in &lww_cols {
        //             let ts = quote_ident(&ts_col_name(col_name));
        //             returning_exprs.push(format!("({ts} = ?{ts_param_idx}) AS \"{col_name}_won\""));
        //         }
        //     }
        //     for (_, col_name) in &max_cols {
        //         returning_exprs.push(quote_ident(col_name));
        //     }
        //
        //     // Build WHERE clause — only if no max cols (those always apply).
        //     let where_clause = if !has_max && !where_conditions.is_empty() {
        //         format!(" WHERE {}", where_conditions.join(" OR "))
        //     } else {
        //         String::new()
        //     };
        //
        //     let returning_clause = if returning_exprs.is_empty() {
        //         " RETURNING 1".to_string()
        //     } else {
        //         format!(" RETURNING {}", returning_exprs.join(", "))
        //     };
        //
        //     let sql = format!(
        //         "INSERT INTO {sql_name} ({cols}) VALUES ({vals}) ON CONFLICT ({pk}) DO UPDATE SET {set}{where}{returning}",
        //         cols = insert_cols.join(", "),
        //         vals = placeholders,
        //         pk = pk_col_names.join(", "),
        //         set = set_clauses.join(", "),
        //         where = where_clause,
        //         returning = returning_clause,
        //     );
        //
        //     if needs_ts_param {
        //         params.push(DbValue::Integer(timestamp.raw() as i64));
        //     }
        //
        //     let result = db.query_row(&sql, &params)?;
        //
        //     match result {
        //         None => Ok(None),
        //         Some(row) => {
        //             let mut changed_columns: Vec<SysColumnValue> = Vec::new();
        //             // LWW columns: if won, use the value we already have.
        //             for (i, (col_id, col_name, raw_val)) in lww_cols.iter().enumerate() {
        //                 if row.get_bool(i)? {
        //                     changed_columns.push(SysColumnValue {
        //                         column_id: *col_id,
        //                         name: col_name.clone(),
        //                         value: raw_val.clone(),
        //                     });
        //                 }
        //             }
        //             // MaxI64 columns: read actual value from RETURNING.
        //             let max_offset = lww_cols.len();
        //             for (i, (col_id, col_name)) in max_cols.iter().enumerate() {
        //                 let value = Some(SysColValue::I64(row.get_i64(max_offset + i)?));
        //                 changed_columns.push(SysColumnValue {
        //                     column_id: *col_id,
        //                     name: col_name.clone(),
        //                     value,
        //                 });
        //             }
        //             Ok(Some(UpsertResult {
        //                 table_id: upsert.table_id,
        //                 table_name: table_name.clone(),
        //                 primary_key: upsert.primary_key.clone(),
        //                 changed_columns,
        //             }))
        //         }
        //     }
    }
}

// /// How to resolve column IDs to SQL column names.
// enum ResolveCol<'a> {
//     /// Known table — look up column name from compiled SysTable.
//     Known(&'a crate::statelogs::def::SysTable),
//     /// Unknown table — derive surrogate name from column ID.
//     Surrogate,
// }
//
// impl ResolveCol<'_> {
//     fn name(&self, col_id: SysColumnId) -> Result<String, ReducerError> {
//         match self {
//             ResolveCol::Known(table) => {
//                 let col = table
//                     .find_col(col_id)
//                     .ok_or(ReducerError::SysColumnNotFound(col_id))?;
//                 Ok(col.raw_name().to_string())
//             }
//             ResolveCol::Surrogate => Ok(surrogate_col_name(col_id)),
//         }
//     }
// }
