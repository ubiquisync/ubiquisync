use std::collections::HashMap;
use std::sync::Mutex;
use crate::id::TableId;
use crate::schema::TableSchema;

impl Reducer {
    pub fn new(db: &dyn Db, prefix: impl Into<String>, tables: &[&'static TableSchema]) -> Result<Self, ReducerError> {
        // TODO prefix isn't used properly here - we have no use case for prefix, maybe remove and add support later if needed
        let prefix = prefix.into();
        let mut table_schemas = HashMap::new();
        for &table in tables {
            table_schemas.insert(table.id, table);
        }
        let reducer = Reducer {
            prefix,
            table_schemas
        };
        reducer.sync_schema(db).map_err(|e| match e {
            SchemaSyncError::Reducer(r) => r,
            SchemaSyncError::Db(d) => ReducerError::Db(d),
            other => ReducerError::NotImplemented(other.to_string()),
        })?;
        Ok(reducer)
    }

    /// Create or validate SQLite tables for all registered system tables.
    /// Handles migration from surrogate tables: if a surrogate table already
    /// exists for a now-registered table ID, renames it and its columns.
    pub fn sync_schema(
        &self,
        db: &dyn Db,
    ) -> Result<(), SchemaSyncError> {
        for table in self.sys_tables.values() {
            self.sync_sys_table(db, table)?;
        }
        Ok(())
    }

    fn sync_sys_table(
        &self,
        db: &dyn Db,
        table: &SysTable,
    ) -> Result<(), SchemaSyncError> {
        let named = table.sql_table_name(&self.prefix);
        let surrogate = surrogate_table_name(&self.prefix, table.id);
        let named_exists = sql_table_exists(db, &named)?;
        let surrogate_exists = sql_table_exists(db, &surrogate)?;

        if surrogate_exists && !named_exists {
            // Migrate: rename surrogate table → proper name
            db.exec_batch(&format!(
                "ALTER TABLE {} RENAME TO {};",
                quote_ident(&surrogate),
                quote_ident(&named),
            ))?;

            // Rename surrogate PK columns → proper names
            let pk_count = table.pk_count();
            for i in 0..pk_count {
                let old_pk = surrogate_pk_name(i);
                let new_pk = table.pk_names[i];
                if old_pk != new_pk {
                    db.exec_batch(&format!(
                        "ALTER TABLE {} RENAME COLUMN {} TO {};",
                        quote_ident(&named),
                        quote_ident(&old_pk),
                        quote_ident(new_pk),
                    ))?;
                }
            }

            // Rename surrogate columns → proper names (and their __ts companions)
            for col in table.cols {
                let old_col = surrogate_col_name(col.id);
                let new_col = col.raw_name();
                let existing_cols = sql_table_columns(db, &named)?;
                if existing_cols.iter().any(|c| c == &old_col) && old_col != new_col {
                    db.exec_batch(&format!(
                        "ALTER TABLE {} RENAME COLUMN {} TO {};",
                        quote_ident(&named),
                        quote_ident(&old_col),
                        quote_ident(new_col),
                    ))?;
                    // Rename ts companion if it exists
                    let old_ts = ts_col_name(&old_col);
                    let new_ts = ts_col_name(new_col);
                    if existing_cols.iter().any(|c| c == &old_ts) {
                        db.exec_batch(&format!(
                            "ALTER TABLE {} RENAME COLUMN {} TO {};",
                            quote_ident(&named),
                            quote_ident(&old_ts),
                            quote_ident(&new_ts),
                        ))?;
                    }
                }
            }
            // Fall through to add any missing columns
        }

        if !named_exists && !surrogate_exists {
            // Fresh creation
            let sql = build_create_sys_table_sql(&named, table);
            db.exec_batch(&sql)?;
        } else {
            // Table exists (either already named, or just renamed from surrogate).
            // Add any missing columns.
            let existing_cols = sql_table_columns(db, &named)?;
            for col in table.cols {
                ensure_column(db, &named, &existing_cols, col.raw_name(), col.id.col_type())?;
            }
        }

        Ok(())
    }
}
