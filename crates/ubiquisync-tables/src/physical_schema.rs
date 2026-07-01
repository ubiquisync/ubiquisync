use std::collections::{BTreeMap, BTreeSet};

use ubiquisync_sql::db::{Db, DbColumnDescription, DbTableDescriptor, DbType};

use crate::{
    error::TablesError,
    id::{ColumnId, TableId},
    schema::TableSchema,
};

/// PhysicalTableSchema represents a the physical storage for a table in the database
/// using names derived from the table and column IDs.
/// It may or may or may not have a user facing VIEW derived from a TableSchema.
/// If the user specifically declared this table then we will have a TableSchema.
/// If the table or some column was only referred to in updates from other peers
/// then we will only have the table or column with surrogate names.
#[derive(Debug, Clone)]
pub(crate) struct PhysicalTableSchema {
    id: TableId,
    name: String,
    cols: BTreeSet<ColumnId>,
}

/// The timestamp of the latest upsert operation on the table.
/// A nullable i64 column: the reducer reads it as `COALESCE(ts, 0)`.
pub const UPSERT_TS_COL: &str = "__upsert_ts";

/// The timestamp of the latest delete operation on the table.
/// A nullable i64 column: the reducer reads it as `COALESCE(ts, 0)`.
pub const DELETED_TS_COL: &str = "__deleted_ts";

impl PhysicalTableSchema {
    /// Initialize a physical table based on a table ID when we have no user-defined
    /// TableSchema. If the table exists, reconstruct its schema from the database.
    /// If the table does not exist, create it.
    pub(crate) async fn new_surrogate(
        prefix: &str,
        id: TableId,
        db: &dyn Db,
    ) -> Result<Self, TablesError> {
        let name = id.table_name(prefix);
        if let Some(descriptor) = db.describe_table(&name).await? {
            Ok(Self::new_from_db_descriptor(prefix, descriptor)?)
        } else {
            let res = Self::new_from_id(prefix, id);
            res.create_table(db).await?;
            Ok(res)
        }
    }

    /// Initialize a physical table with a user-defined TableSchema.
    /// If the table exists, reconstruct its shema from the database and
    /// then add any columns declared in the schema that aren't already present.
    /// If the table doesn't exist, create it.
    pub(crate) async fn new_named(
        prefix: &str,
        schema: &TableSchema,
        db: &dyn Db,
    ) -> Result<Self, TablesError> {
        let name = schema.id.table_name(prefix);
        if let Some(descriptor) = db.describe_table(&name).await? {
            let mut res = Self::new_from_db_descriptor(prefix, descriptor)?;
            // Make sure all named columns are defined
            for col_id in schema.value_cols.keys() {
                res.ensure_column(db, *col_id).await?;
            }
            Ok(res)
        } else {
            let res = Self::new_from_schema(prefix, schema);
            res.create_table(db).await?;
            Ok(res)
        }
    }

    pub(crate) fn get_name(&self) -> &str {
        &self.name
    }

    /// The set of value-column IDs this table currently tracks.
    pub(crate) fn col_ids(&self) -> &BTreeSet<ColumnId> {
        &self.cols
    }

    fn new_from_id(prefix: &str, id: TableId) -> Self {
        let name = id.table_name(prefix);
        Self {
            id,
            name,
            cols: Default::default(),
        }
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

    fn new_from_db_descriptor(
        prefix: &str,
        db_table: DbTableDescriptor,
    ) -> Result<Self, TablesError> {
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
        for i in 0..pk_count {
            let db_col = &db_table.pk_cols[i];
            let db_type = db_col.db_type;
            let pk_type = id.pk_col_type(i);
            // Check primary key type match
            if !pk_type.accepts(db_type) {
                return schema_mismatch(
                    id,
                    format!("invalid primary key type at {i} expected {pk_type:?} got {db_type:?}"),
                );
            }

            if db_col.nullable {
                return schema_mismatch(id, format!("primary key {i} shouldn't be nullable"));
            }

            // Check primary key name match
            let col_name = &db_col.name;
            let expected_col_name = id.pk_col_name(i);
            if col_name != &expected_col_name {
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
        for col in db_table.cols.iter() {
            db_col_map.insert(col.name.clone(), col.clone());
        }

        // Validate __upsert_ts and __deleted_ts cols
        validate_upsert_delete_ts_cols(UPSERT_TS_COL, &mut db_col_map)?;
        validate_upsert_delete_ts_cols(DELETED_TS_COL, &mut db_col_map)?;

        let mut cols = BTreeSet::new();
        // Extract other columns into value column/lww column pairs
        for col in db_table.cols {
            if let Some(col_id) = ColumnId::parse_col_name(&col.name) {
                if let Some(lww_col) = db_col_map.remove(&col_id.lww_col_name()) {
                    if !col_id.col_type().accepts(col.db_type) {
                        return schema_mismatch(
                            id,
                            format!(
                                "column {id:?} db type {:?} doesn't match column type {:?}",
                                col.db_type,
                                col_id.col_type(),
                            ),
                        );
                    }
                    if !col.nullable {
                        return schema_mismatch(
                            id,
                            format!("column {} is not nullable", &col.name),
                        );
                    }

                    if lww_col.db_type != DbType::Integer {
                        return schema_mismatch(
                            id,
                            format!(
                                "lww column {} db type {:?} doesn't match {:?}",
                                lww_col.name,
                                lww_col.db_type,
                                DbType::Integer,
                            ),
                        );
                    }
                    if !lww_col.nullable {
                        return schema_mismatch(
                            id,
                            format!("lww column {} is not nullable", &lww_col.name),
                        );
                    }

                    // TODO check nullable
                    cols.insert(col_id);
                } else {
                    return schema_mismatch(id, format!("missing lww column for {}", col.name));
                }
            }
        }

        Ok(Self {
            id,
            name: name.into(),
            cols,
        })
    }

    async fn create_table(&self, db: &dyn Db) -> Result<(), TablesError> {
        let mut col_defs = vec![];

        let pk_count = self.id.pk_count();
        // TODO do we need to quote identifers since they're auto-generated??
        let dialect = db.dialect();
        for i in 0..pk_count {
            col_defs.push(format!(
                "{} {} NOT NULL",
                self.id.pk_col_name(i),
                self.id.pk_col_type(i).db_type().sql_type(dialect),
            ));
        }

        let int_type = DbType::Integer.sql_type(dialect);
        col_defs.push(format!("{UPSERT_TS_COL} {int_type}"));
        col_defs.push(format!("{DELETED_TS_COL} {int_type}"));

        for col in &self.cols {
            col_defs.push(format!(
                "{} {}",
                col.col_name(),
                col.col_type().db_type().sql_type(dialect),
            ));
            col_defs.push(format!("{} {int_type}", col.lww_col_name(),));
        }
        let without_rowid = db.dialect().without_rowid();
        db.exec(
            &format!(
                "CREATE TABLE {} ({}, PRIMARY KEY ({})){without_rowid};",
                self.name,
                col_defs.join(", "),
                self.id.pk_col_name_list(),
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
    ) -> Result<(), TablesError> {
        if self.cols.contains(&col_id) {
            return Ok(());
        }

        let dialect = db.dialect();
        let mut batch = db.new_batch();
        // Add column
        // TODO do we need to quote the names now that they're all surrogates
        batch.add_statement(
            &format!(
                // TODO ensure that we don't need to specify NULL
                "ALTER TABLE {} ADD COLUMN {} {};",
                self.name,
                col_id.col_name(),
                col_id.col_type().db_type().sql_type(dialect),
            ),
            &[],
        );
        // Add LWW column
        batch.add_statement(
            &format!(
                "ALTER TABLE {} ADD COLUMN {} {};",
                self.name,
                col_id.lww_col_name(),
                DbType::Integer.sql_type(dialect),
            ),
            &[],
        );
        batch.commit().await?;

        self.cols.insert(col_id);

        Ok(())
    }
}

fn schema_mismatch<T>(id: TableId, detail: String) -> Result<T, TablesError> {
    Err(TablesError::SchemaError(format!("table {id:?}: {detail}")))
}

fn validate_upsert_delete_ts_cols(
    col_name: &str,
    col_map: &mut BTreeMap<String, DbColumnDescription>,
) -> Result<(), TablesError> {
    // Remove the col from the column map so it doesn't get picked up as a regular column.
    if let Some(col) = col_map.remove(col_name) {
        let db_type = col.db_type;
        if db_type != DbType::Integer {
            return Err(TablesError::SchemaError(format!(
                "invalid {col_name} type {db_type:?}"
            )));
        }
    } else {
        return Err(TablesError::SchemaError(format!("missing {col_name}")));
    }
    Ok(())
}
