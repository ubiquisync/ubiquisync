use ubiquisync_sql::db::{Db, DbColumnDescription, DbTableDescriptor, DbType};
use ubiquisync_sql::util::quote_ident;

use crate::col_type::ColType;
use crate::id::{ColumnId, TableId};
use crate::reducer::ReducerError;
use crate::surrogate::{
    parse_surrogate_col_name, parse_surrogate_table_name, surrogate_col_name, surrogate_pk_name,
    surrogate_table_name,
};
use crate::util::{lww_col_name, parse_lww_col_name};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct TableSchema {
    id: TableId,
    name: String,
    pk_names: Vec<String>,
    value_cols: HashMap<ColumnId, ColumnSchema>,
}

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub id: ColumnId,
    pub lww_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SurrogateTableSchema {
    id: TableId,
    name: String,
    cols: BTreeSet<ColumnId>,
}

pub const DELETED_TS_COL: &'static str = "__deleted_ts";
pub const UPSERT_TS_COL: &'static str = "__upsert_ts";

fn schema_mismatch<T>(id: TableId, table: &str, detail: String) -> Result<T, ReducerError> {
    return Err(ReducerError::SchemaMismatch {
        id,
        table: table.into(),
        detail: detail.into(),
    });
}

fn validate_upsert_delete_ts_cols(
    col_name: &str,
    col_map: &mut BTreeMap<String, DbColumnDescription>,
) -> Result<(), ReducerError> {
    // Remove the col from the column map so it doesn't get picked up as a regular column.
    if let Some(col) = col_map.remove(col_name) {
        let db_type = col.db_type;
        if db_type != DbType::Integer {
            return schema_mismatch(id, table, format!("invalid {col_name} type {db_type}"));
        }
    } else {
        return schema_mismatch(id, table, format!("missing {col_name}"));
    }
    Ok(())
}

impl SurrogateTableSchema {
    fn new_from_id(prefix: &str, id: TableId) -> Self {
        let name = schema.id.name(prefix);
        Self {
            id,
            name,
            cols: Default::default(),
        }
    }

    pub(crate) async fn init_surrogate(prefix: &str, id: TableId, db: &dyn Db) -> Result<Self, ReducerError> {
        let res = Self::new_from_id(prefix, id);
        res.create_table(db).await?;
        Ok(res)
    }

    pub(crate) fn get_name(&self) -> &str {
        &self.name
    }

    fn new_from_schema(prefix: &str, schema: &TableSchema) -> Self {
        let name = schema.id.name(prefix);
        let cols = BTreeSet::new();
        for col in schema.value_cols {
            cols.insert(col.id);
        }
        Self { id, name, cols }
    }

    fn reconstruct_from_db(
        prefix: &str,
        db_table: DbTableDescriptor,
    ) -> Result<Self, ReducerError> {
        let name = &db_table.name;
        let id = if let Some(id) = TableId::parse(prefix, name) {
            id
        } else {
            return Err(ReducerError::Unknown(format!("can't parse table {name}"))))
        };

        // Validate primary key
        let pk_count = id.pk_count();
        let actual_pk_count = db_table.pk_cols.len();
        if pk_count != actual_pk_count {
            return schema_mismatch(
                id,
                table,
                format!("invalid primary key count, expected {pk_count} got {actual_pk_count}"),
            );
        }
        for i in 0..n {
            let db_col = db_table.pk_cols[i];
            let db_type = db_col.db_type;
            let pk_type = id.pk_col_type(i);
            // Check primary key type match
            if !pk_type.accepts(db_type) {
                return schema_mismatch(
                    id,
                    table,
                    format!("invalid primary key type at {i} expected {pk_type} got {db_type}"),
                );
            }

            // Check primary key name match
            let col_name = &db_col.name;
            let expected_col_name = surrogate_pk_name(i);
            if col_name != expected_col_name {
                return schema_mismatch(
                    id,
                    table,
                    format!(
                        "expected primary key column {i} to be named {expected_col_name} got {col_name}"
                    ),
                );
            }
        }

        // Put all other cols in a map
        let mut db_col_map = BTreeMap::new();
        for col in db_table.cols {
            db_col_map.insert(col.name.clone(), col.clone());
        }

        // Validate __upsert_ts and __deleted_ts cols
        validate_upsert_delete_ts_cols(UPSERT_TS_COL, &mut db_col_map)?;
        validate_upsert_delete_ts_cols(DELETED_TS_COL, &mut db_col_map)?;

        let cols = BTreeSet::new();
        // Extract other columns into value column/lww column pairs
        for col in db_table.cols {
            // If we can find an lww col match for this column then we track both it and
            // its lww column as a column pair and remove them from the map.
            if let Some(lww_col) = db_col_map.remove(&lww_col_name(&col.name)) {
                let col = db_col_map.remove(&col.name);
                let col_name = &col.name;
                if let Some(col_id) = parse_surrogate_col_name(col_name) {
                    let db_type = col.db_type;
                    let col_type = col_id.col_type();
                    if !col_type.accepts(db_type) {
                        return schema_mismatch(
                            id,
                            table,
                            format!(
                                "column {id} db type {db_type} doesn't match column type {col_type}"
                            ),
                        );
                    }

                    cols.insert(col_id);
                } else {
                    return schema_mismatch(
                        id,
                        table,
                        format!("can't parse surrogate column {col_name}"),
                    );
                }
            }
        }

        Ok(Self { id, name: name.into(), cols })
    }

    async fn create_table(&self, db: &dyn Db) -> Result<(), ReducerError> {
        let mut col_defs = vec![];

        let pk_count = self.id.pk_count();
        // TODO do we need to quote identifers since they're auto-generated??
        for i in 0..pk_count {
            col_defs.push(format("{} {}", self.id.pk_name(i), self.id.pk_col_type(i).db_type()));
        }

        col_defs.push(format!("{UPSERT_TS_COL} {}", db.lww_col_type()));
        col_defs.push(format!("{DELETED_TS_COL} {}", db.lww_col_type()));

        for col in &self.cols{
            col_defs.push(format("{} {}", col.name(), col.col_type().db_type()));
            col_defs.push(format! ("{} {}", col.lww_name(), DbType::Integer));
        }
        let without_rowid = db.dialect().without_rowid();
        db.exec(
            &format!(
                "CREATE TABLE {} ({}, PRIMARY KEY ({})){without_rowid};",
                self.name,
                col_defs.join(", "),
                self.pk_names.join(", ")
            ),
            &[],
        ).await?;
        Ok(())
    }

    pub(crate) async fn ensure_column(
        &mut self,
        db: &dyn Db,
        col_id: ColumnId,
    ) -> Result<(), ReducerError> {
        if self.cols.contains(&col_id) {
            return Ok(());
        }

        let mut batch = db.new_batch();
        // Add column
        // TODO do we need to quote the names now that they're all surrogates
        batch.add_statement(
            &format!(
                // TODO ensure that we don't need to specify NULL
                "ALTER TABLE {} ADD COLUMN {} {};",
                self.name,
                col_id.name(),
                db.col_type(col_id.col_type()),
            ),
            &[],
        );
        // Add LWW column
        batch.add_statement(
            &format!(
                "ALTER TABLE {} ADD COLUMN {} {};",
                self.name,
                col_id.lww_name(),
                DbType::Integer,
            ),
            &[],
        );
        batch.commit().await?;

        self.cols.insert(col_id);

        Ok(())
    }
}

impl TableSchema {
    async fn create_view(&self, surrogate_prefix: &str, db: &dyn Db) -> Result<(), ReducerError> {
        let id = self.id;
        let surrogate_name = id.name(surrogate_prefix);
        let name = &self.name;
        let pk_count = self.id.pk_count();
        if pk_count != self.pk_names.len() {
            // TODO better error
            return schema_mismatch(id, table, format!("invalid TableSchema, pk count doesn't match"));
        }

        let mut select_clauses = vec![];
        for i in 0..pk_count {
            let surrogate_name = id.pk_name(i);
            let real_name = quote_ident(&self.pk_names[i]);
            select_clauses.push(format!("{surrogate_name} AS {real_name}"));
        }

        for (id, col) in self.value_cols.iter() {
            let surrogate_name = id.name();
            let real_name = quote_ident(&col.name);
            select_clauses.push(format!("{surrogate_name} AS {real_name}"));
        }

        // TODO should we have separate ways of prefixing both the surrogate tables and the view names?
        let quoted_name = quote_ident(&name);

        let mut batch = db.new_batch();
        batch.add_statement(&format!("DROP VIEW IF EXISTS {quoted_name}"), &[]);
        batch.add_statement(&format!("CREATE VIEW {quoted_name} AS SELECT {} FROM {surrogate_name} \
            WHERE COALESCE({UPSERT_TS_COL}, 0) >= COALESCE({DELETED_TS_COL}, 0)",
            select_clauses.join(", ")),
            &[]);
        batch.commit().await?;
        Ok(())
    }
}

