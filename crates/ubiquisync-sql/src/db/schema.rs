use crate::{dialect::SqlDialect, util::quote_ident};

/// An existing table's shape as reported by backend introspection
/// ([`Db::describe_table`](super::Db::describe_table)). Used by schema
/// reconciliation to compare the live table against the declared schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbTableDescriptor {
    /// The table's name.
    pub name: String,
    pub pk: DbPrimaryKey,
    /// The remaining (non-primary-key) columns.
    pub cols: Vec<DbColumnDescription>,
}

/// One column of an introspected table (see [`DbTableDescriptor`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbColumnDescription {
    /// The column's name.
    pub name: String,
    /// The column's storage class, mapped from the backend's native type
    /// (or [`DbType::Other`] if it falls outside the engine's vocabulary).
    pub db_type: DbType,
    /// Whether the column permits SQL NULL.
    pub nullable: bool,
    pub default: Option<DbColumnDefault>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbPrimaryKey {
    AutoId(String),
    Columns(Vec<DbColumnDescription>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbColumnDefault {
    I64Zero,
}

/// A generic SQL storage class, independent of any data protocol.
///
/// This is the vocabulary the dialect names: a data domain (e.g. the tables
/// protocol) maps its own column types down to a `DbType`, and the dialect
/// turns that into a concrete backend type name via [`sql_type`](Self::sql_type). The
/// `Uuid` variant is kept distinct from `Blob` so a backend may later map it
/// to a native UUID type rather than raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbType {
    /// 64-bit signed integer (`INTEGER` / `BIGINT`).
    Integer,
    /// UTF-8 text (`TEXT`).
    Text,
    /// Raw byte string (`BLOB` / `BYTEA`).
    Blob,
    /// 16-byte UUID — stored as raw bytes today, but kept distinct from
    /// [`Blob`](Self::Blob) so a backend may later use a native UUID type.
    Uuid,
    /// A column type the engine does not model (e.g. SQLite `REAL`/`NUMERIC`, a
    /// Postgres enum). Produced only by backend introspection
    /// ([`Db::describe_table`](super::Db::describe_table)) when a real table has
    /// a column outside the engine's vocabulary; the engine never *emits* it as
    /// DDL. Schema reconciliation treats it as a mismatch rather than silently
    /// coercing it to a class it isn't.
    Other,
}

impl DbType {
    /// The concrete SQL column type name for this storage class under
    /// `dialect`. SQLite uses type affinity (`INTEGER`/`TEXT`/`BLOB`) and has no
    /// native UUID type, so a `Uuid` is stored as a raw `BLOB`. Postgres needs
    /// `BIGINT` (its `INTEGER` is 32-bit and would overflow an i64) and `BYTEA`
    /// for raw bytes, and maps `Uuid` to its native `UUID` type.
    pub fn sql_type(self, dialect: SqlDialect) -> &'static str {
        match (dialect, self) {
            (SqlDialect::Sqlite, DbType::Integer) => "INTEGER",
            (SqlDialect::Sqlite, DbType::Text) => "TEXT",
            (SqlDialect::Sqlite, DbType::Blob | DbType::Uuid) => "BLOB",
            (SqlDialect::Postgres, DbType::Integer) => "BIGINT",
            (SqlDialect::Postgres, DbType::Text) => "TEXT",
            (SqlDialect::Postgres, DbType::Blob) => "BYTEA",
            (SqlDialect::Postgres, DbType::Uuid) => "UUID",
            // `Other` is introspection-only — it names a column type the engine
            // doesn't model, so it has no DDL spelling and is never emitted.
            (_, DbType::Other) => {
                unreachable!("DbType::Other is introspection-only and never rendered as DDL")
            }
        }
    }
}

impl DbTableDescriptor {
    pub fn create_table_sql(&self, dialect: SqlDialect) -> String {
        let quoted_table_name = quote_ident(&self.name);
        let mut col_defs = self.pk.create_cols_clauses(dialect);
        col_defs.append(&mut DbColumnDescription::create_cols_sql(
            &self.cols, dialect,
        ));
        let col_sql = col_defs.join(", ");
        let pk_clause = self.pk.pk_clause();
        let rowid_clause = self.pk.rowid_clause(dialect);
        format!("CREATE TABLE {quoted_table_name} ({col_sql}{pk_clause}){rowid_clause};")
    }
}

impl DbPrimaryKey {
    fn create_cols_clauses(&self, dialect: SqlDialect) -> Vec<String> {
        match self {
            DbPrimaryKey::AutoId(name) => {
                let name_quoted = quote_ident(name);
                let sql = match dialect {
                    SqlDialect::Sqlite => {
                        format!("{name_quoted} INTEGER PRIMARY KEY AUTOINCREMENT")
                    }
                    SqlDialect::Postgres => {
                        format!("{name_quoted} BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY")
                    }
                };
                vec![sql]
            }
            DbPrimaryKey::Columns(cols) => DbColumnDescription::create_cols_sql(cols, dialect),
        }
    }

    fn pk_clause(&self) -> String {
        match self {
            DbPrimaryKey::AutoId(_) => "".into(),
            DbPrimaryKey::Columns(cols) => {
                let cols_str = cols
                    .iter()
                    .map(|c| quote_ident(&c.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(", PRIMARY KEY({cols_str})")
            }
        }
    }

    fn rowid_clause(&self, dialect: SqlDialect) -> &str {
        match self {
            DbPrimaryKey::AutoId(_) => "",
            DbPrimaryKey::Columns(_) => dialect.without_rowid(),
        }
    }
}

impl DbColumnDescription {
    fn create_col_sql(&self, dialect: SqlDialect) -> String {
        let quoted_name = quote_ident(&self.name);
        let sql_type = self.db_type.sql_type(dialect);
        let mut sql = format!("{quoted_name} {sql_type}");
        if self.nullable {
            sql += " NULL";
        } else {
            sql += " NOT NULL";
        }

        if let Some(ref default) = self.default {
            sql += &format!(" DEFAULT {0}", default.to_str());
        }

        sql
    }

    fn create_cols_sql(cols: &[Self], dialect: SqlDialect) -> Vec<String> {
        cols.iter()
            .map(|c| c.create_col_sql(dialect))
            .collect::<Vec<_>>()
    }
}

impl DbColumnDefault {
    fn to_str(&self) -> String {
        match self {
            DbColumnDefault::I64Zero => "0".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_type_maps_to_sqlite_names() {
        assert_eq!(DbType::Integer.sql_type(SqlDialect::Sqlite), "INTEGER");
        assert_eq!(DbType::Text.sql_type(SqlDialect::Sqlite), "TEXT");
        assert_eq!(DbType::Blob.sql_type(SqlDialect::Sqlite), "BLOB");
        assert_eq!(DbType::Uuid.sql_type(SqlDialect::Sqlite), "BLOB");
    }

    #[test]
    fn sql_type_maps_to_postgres_names() {
        // i64 needs BIGINT (Postgres INTEGER is 32-bit); raw bytes are BYTEA;
        // UUIDs use the native UUID type.
        assert_eq!(DbType::Integer.sql_type(SqlDialect::Postgres), "BIGINT");
        assert_eq!(DbType::Text.sql_type(SqlDialect::Postgres), "TEXT");
        assert_eq!(DbType::Blob.sql_type(SqlDialect::Postgres), "BYTEA");
        assert_eq!(DbType::Uuid.sql_type(SqlDialect::Postgres), "UUID");
    }
}
