use std::collections::{BTreeMap, BTreeSet};

use ubiquisync_sql::db::{Db, DbTableDescriptor};

use crate::{
    error::TablesError,
    id::{ColumnId, TableId},
    schema::TableSchema,
};

#[derive(Debug, Clone)]
pub(crate) struct PhysicalTableSchema {
    id: TableId,
    name: String,
    cols: BTreeSet<ColumnId>,
}

pub const DELETED_TS_COL: &'static str = "__deleted_ts";
pub const UPSERT_TS_COL: &'static str = "__upsert_ts";

impl PhysicalTableSchema {
    fn new_from_id(prefix: &str, id: TableId) -> Self {
        let name = id.table_name(prefix);
        Self {
            id,
            name,
            cols: Default::default(),
        }
    }

    pub(crate) async fn init_surrogate(
        prefix: &str,
        id: TableId,
        db: &dyn Db,
    ) -> Result<Self, TablesError> {
        let res = Self::new_from_id(prefix, id);
        res.create_table(db).await?;
        Ok(res)
    }

    pub(crate) fn get_name(&self) -> &str {
        &self.name
    }

    fn new_from_schema(prefix: &str, schema: &TableSchema) -> Self {
        let id = schema.id;
        let name = id.table_name(prefix);
        let mut cols = BTreeSet::new();
        for (id, _) in schema.value_cols.iter() {
            cols.insert(*id);
        }
        Self { id, name, cols }
    }

    fn reconstruct_from_db(prefix: &str, db_table: DbTableDescriptor) -> Result<Self, TablesError> {
        let name = &db_table.name;
        let id = if let Some(id) = TableId::parse_table_name(prefix, name) {
            id
        } else {
            return Err(TablesError::SchemaError(format!(
                "can't parse table {name}"
            )));
        };

        // Validate primary key
        let pk_count = id.pk_count();
        let actual_pk_count = db_table.pk_cols.len();
        if pk_count != actual_pk_count {
            return schema_mismatch(
                id,
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
                    format!("invalid primary key type at {i} expected {pk_type} got {db_type}"),
                );
            }

            if db_col.nullable {
                return schema_mismatch(id, format!("primary key {i} shouldn't be nullable"));
            }

            // Check primary key name match
            let col_name = &db_col.name;
            let expected_col_name = surrogate_pk_name(i);
            if col_name != expected_col_name {
                return schema_mismatch(
                    id,
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
                // TODO check lww_col type and nullable
                if let Some(col) = db_col_map.remove(&col.name) {
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

                        // TODO check nullable
                        cols.insert(col_id);
                    } else {
                        return schema_mismatch(
                            id,
                            table,
                            format!("can't parse surrogate column {col_name}"),
                        );
                    }
                } else {
                    todo!("missing column match");
                }
            }
        }

        Ok(Self {
            id,
            name: name.into(),
            cols,
        })
    }

    async fn create_table(&self, db: &dyn Db) -> Result<(), ReducerError> {
        let mut col_defs = vec![];

        let pk_count = self.id.pk_count();
        // TODO do we need to quote identifers since they're auto-generated??
        for i in 0..pk_count {
            col_defs.push(format(
                "{} {} NOT NULL",
                self.id.pk_name(i),
                self.id.pk_col_type(i).db_type(),
            ));
        }

        col_defs.push(format!("{UPSERT_TS_COL} {}", db.lww_col_type()));
        col_defs.push(format!("{DELETED_TS_COL} {}", db.lww_col_type()));

        for col in &self.cols {
            col_defs.push(format("{} {}", col.name(), col.col_type().db_type()));
            col_defs.push(format!("{} {}", col.lww_name(), DbType::Integer));
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
        )
        .await?;
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

fn schema_mismatch<T>(id: TableId, detail: String) -> Result<T, ReducerError> {
    return Err(ReducerError::Schema(format!("table {id}: {detail}")));
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
