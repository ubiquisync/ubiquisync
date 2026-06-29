use ubiquisync_sql::db::{ColumnDescription, Db, DbType};

use crate::col_type::ColType;
use crate::id::{ColumnId, TableId};
use crate::reducer::ReducerError;
use crate::surrogate::{parse_surrogate_col_name, surrogate_col_name, surrogate_table_name};
use crate::util::{lww_col_name, parse_lww_col_name, quote_ident};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct TableSchema {
    id: TableId,
    name: String,
    pk_names: Vec<String>,
    value_cols: BTreeMap<ColumnId, ColumnSchema>,
}

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub id: ColumnId,
    pub lww_name: String,
}

pub const DELETED_TS_COL: &'static str = "__deleted_ts";
pub const UPSERT_TS_COL: &'static str = "__upsert_ts";

impl TableSchema {
    pub async fn init_default(
        db: &dyn Db,
        prefix: &str,
        id: TableId,
        name: &str,
        pk_names: Vec<String>,
        cols: Vec<ColumnSchema>,
    ) -> Result<Self, ReducerError> {
        // Check db for existing table info
        if id.pk_count() != pk_names.len() {
            todo!("error")
        }
        let name = format!("{prefix}{name}");
        let mut ts = Self {
            id,
            name,
            pk_names,
            value_cols: cols.into_iter().map(|c| (c.id, c)).collect(),
        };
        let existing_cols: Vec<ColumnDescription> =
            if let Some(existing) = db.describe_table(&ts.name).await? {
                // Check pk columns
                let n = existing.pk_cols.len();
                if n != id.pk_count() {
                    todo!("error")
                }
                for i in 0..n {
                    if &existing.pk_cols[i].name != &ts.pk_names[i] {
                        todo!("error")
                    }
                }

                existing.cols
            } else {
                let surrogate_name = surrogate_table_name(prefix, id);
                if let Some(surrogate) = db.describe_table(&surrogate_name).await? {
                    let n = surrogate.pk_cols.len();
                    if n != id.pk_count() {
                        todo!("error")
                    }

                    // Check pk columns and rename
                    for i in 0..n {
                        let surrogate_name = crate::surrogate::surrogate_pk_name(i);
                        if &surrogate.pk_cols[i].name != &surrogate_name {
                            todo!("error")
                        }
                        // Rename pk column
                        let real_name = ts.pk_names[i];
                        db.exec(
                            &format!(
                                "ALTER TABLE {} RENAME COLUMN {} TO {}",
                                quote_ident(&ts.name),
                                quote_ident(&surrogate_name),
                                quote_ident(&real_name),
                            ),
                            &[],
                        )
                        .await?;
                    }

                    // Rename
                    db.exec(
                        &format!(
                            "ALTER TABLE {} RENAME TO {}",
                            quote_ident(&surrogate_name),
                            quote_ident(&ts.name),
                        ),
                        &[],
                    )
                    .await?;

                    surrogate.cols
                } else {
                    // Create table
                    ts.create_table(db).await?;

                    // No existing schema to check so we're done.
                    return Ok(ts);
                }
            };

        // Collect map of existing cols
        let mut existing_col_map = BTreeMap::default();
        let mut have_deleted_ts_col = false;
        let mut have_upsert_ts_col = false;
        for col in existing_cols {
            if col.name == DELETED_TS_COL {
                have_deleted_ts_col = true;
            } else if col.name == UPSERT_TS_COL {
                have_upsert_ts_col = true;
            } else if let Some(name) = parse_lww_col_name(&col.name) {
                if let Some(existing) = existing_col_map.get_mut(&name) {
                    existing.lww_col_type = Some(col.db_type);
                } else {
                    existing_col_map.insert(
                        name,
                        ExistingColInfo {
                            name,
                            col_type: Some(col.db_type),
                            lww_col_type: None,
                        },
                    );
                }
            } else {
                if let Some(existing) = existing_col_map.get_mut(&col.name) {
                    existing.col_type = Some(col.db_type);
                } else {
                    existing_col_map.insert(
                        col.name,
                        ExistingColInfo {
                            name: col.name.clone(),
                            col_type: Some(col.db_type),
                            lww_col_type: None,
                        },
                    );
                }
            }
        }
        if !have_deleted_ts_col {
            todo!("error")
        }

        // Check existing columns
        let mut cols_to_define = BTreeMap::default();
        for col in ts.value_cols.values() {
            cols_to_define.insert(col.name.clone(), col);
        }
        for existing in existing_col_map.values() {
            if let Some(col) = cols_to_define.remove(&existing.name) {
                // If we have a named column that is already defined, check that its schema is valid
                // for the column type.
                existing.validate(col.id.col_type())?;
            } else if let Some(surrogate) = parse_surrogate_col_name(existing.name.as_str()) {
                existing.validate(surrogate.col_type())?;
                if let Some(to_define) = ts.value_cols.get(&surrogate) {
                    // Rename surrogate to real column name
                    db.exec(
                        &format!(
                            "ALTER TABLE {} RENAME COLUMN {} TO {}",
                            quote_ident(&ts.name),
                            quote_ident(&existing.name),
                            quote_ident(&to_define.name),
                        ),
                        &[],
                    )
                    .await?;
                    // TODO move this to outer
                    let lww_name = lww_col_name(&to_define.name);
                    db.exec(
                        &format!(
                            "ALTER TABLE {} RENAME COLUMN {} TO {}",
                            quote_ident(&ts.name),
                            quote_ident(&lww_col_name(&existing.name)),
                            quote_ident(&lww_name),
                        ),
                        &[],
                    )
                    .await?;

                    // Remove the renamed surrogate column from the list of columns to define.
                    cols_to_define.remove(&to_define.name);
                }
                ts.value_cols.insert(
                    surrogate,
                    ColumnSchema {
                        name: existing.name.clone(),
                        id: surrogate,
                        lww_name,
                    },
                );
            }
        }

        // Create any missing columns
        for col in cols_to_define.values() {
            ts.alter_table_add_col(db, &col.name, col.id);
        }

        Ok(ts)
    }

    pub async fn init_surrogate(
        prefix: &str,
        id: TableId,
        db: &dyn Db,
    ) -> Result<Self, ReducerError> {
        let name = surrogate_table_name(prefix, id);
        let mut pk_names = vec![];
        let pk_count = id.pk_count();
        for i in 0..pk_count {
            pk_names.push(crate::surrogate::surrogate_pk_name(i));
        }
        let mut ts = Self {
            id,
            name,
            pk_names,
            value_cols: BTreeMap::default(),
        };

        // TODO: check db for table info
        Ok(ts)
    }

    pub async fn ensure_column(
        &mut self,
        db: &dyn Db,
        col_id: ColumnId,
    ) -> Result<(), ReducerError> {
        if let Some(col) = self.value_cols.get(&col_id) {
            return Ok(());
        }

        // Create surrogate column.
        let col_name = surrogate_col_name(col_id);
        let lww_name = lww_col_name(&col_name);
        self.alter_table_add_col(db, &col_name, &lww_name, col_id)
            .await;
        self.value_cols.insert(
            col_id,
            ColumnSchema {
                name: col_name,
                id: col_id,
                lww_name,
            },
        );
        Ok(())
    }

    pub fn require_column(&self, col_id: ColumnId) -> Result<&ColumnSchema, ReducerError> {
        self.value_cols
            .get(&col_id)
            .ok_or(ReducerError::ColumnNotFound(col_id))
    }

    async fn create_table(&self, db: &dyn Db) -> Result<(), ReducerError> {
        let pk_count = self.id.pk_count();
        if pk_count != self.pk_names.len() {
            todo!("error")
        }
        let mut col_defs = vec![];
        for i in 0..pk_count {
            col_defs.push(format!(
                "{} {}",
                quote_ident(&self.pk_names[i]),
                db.pk_col_type(self.id.pk_col_type(i))
            ));
        }

        col_defs.push(format!("{UPSERT_TS_COL} {}", db.lww_col_type()));
        col_defs.push(format!("{DELETED_TS_COL} {}", db.lww_col_type()));

        for (_, col) in &self.value_cols {
            col_defs.push(format!(
                "{} {}",
                quote_ident(&col.name),
                db.col_type(col.id.col_type())
            ));
            if col.id.col_type().is_lww() {
                col_defs.push(format! {
                    "{} {}",
                    quote_ident(&lww_col_name(&col.name)),
                    db.lww_col_type(),
                })
            }
        }
        db.exec(
            &format!(
                "CREATE TABLE {} ({}) PRIMARY KEY ({})",
                self.name,
                col_defs.join(", "),
                self.pk_names.join(", ")
            ),
            &[],
        )
        .await?;
        Ok(())
    }

    async fn alter_table_add_col(
        &self,
        db: &dyn Db,
        col_name: &str,
        lww_col_name: &str,
        col_id: ColumnId,
    ) {
        // TODO maybe batch these statements
        db.exec(
            &format!(
                // TODO ensure that we don't need to specify NULL
                "ALTER TABLE {} ADD COLUMN {} {}",
                quote_ident(&self.name),
                quote_ident(&col_name),
                db.col_type(col_id.col_type()),
            ),
            &[],
        )
        .await?;
        // Add LWW column
        db.exec(
            &format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                quote_ident(&self.name),
                quote_ident(&lww_col_name),
                db.lww_col_type(),
            ),
            &[],
        )
        .await?;
    }

    pub fn pk_col_names(&self) -> &[String] {
        &self.pk_names
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_id(&self) -> TableId {
        self.id
    }

    pub fn non_pkey_cols(&self) -> impl Iterator<Item = ColumnSchema> {
        self.value_cols.values()
    }
}

struct ExistingColInfo {
    name: String,
    col_type: Option<DbType>,
    lww_col_type: Option<DbType>,
}

impl ExistingColInfo {
    fn validate(&self, col_type: ColType) -> Result<(), ReducerError> {
        if let Some(db_col_type) = self.col_type {
            if !db_col_type.is_valid_for(col_type) {
                todo!("error")
            }
            if col_type.is_lww() {
                if let Some(lww_col_type) = self.lww_col_type {
                    if lww_col_type != DbType::Integer {
                        todo!("error")
                    }
                } else {
                    todo!("missing lww col")
                }
            }
            Ok(())
        } else {
            todo!("missing col type")
        }
    }
}
