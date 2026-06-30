use crate::dialect::SqlDialect;

pub struct DbTableDescriptor {
    pub name: String,
    pub pk_cols: Vec<DbColumnDescription>,
    pub cols: Vec<DbColumnDescription>,
}

pub struct DbColumnDescription {
    pub name: String,
    pub db_type: DbType,
    pub nullable: bool,
}

/// A generic SQL storage class, independent of any data protocol.
///
/// This is the vocabulary the dialect names: a data domain (e.g. the tables
/// protocol) maps its own column types down to a `DbType`, and the dialect
/// turns that into a concrete backend type name via [`DbType::sql_type`]. The
/// `Uuid` variant is kept distinct from `Blob` so a backend may later map it
/// to a native UUID type rather than raw bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DbType {
    Integer,
    Text,
    Blob,
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
    /// `dialect`. SQLite uses type affinity (`INTEGER`/`TEXT`/`BLOB`);
    /// Postgres needs `BIGINT` (its `INTEGER` is 32-bit and would overflow an
    /// i64) and `BYTEA` for raw bytes. UUIDs are stored as 16 raw bytes, so
    /// they take the same type as `Blob` on both backends today.
    pub fn sql_type(self, dialect: SqlDialect) -> &'static str {
        match (dialect, self) {
            (SqlDialect::Sqlite, DbType::Integer) => "INTEGER",
            (SqlDialect::Sqlite, DbType::Text) => "TEXT",
            (SqlDialect::Sqlite, DbType::Blob | DbType::Uuid) => "BLOB",
            (SqlDialect::Postgres, DbType::Integer) => "BIGINT",
            (SqlDialect::Postgres, DbType::Text) => "TEXT",
            (SqlDialect::Postgres, DbType::Blob | DbType::Uuid) => "BYTEA",
            // `Other` is introspection-only — it names a column type the engine
            // doesn't model, so it has no DDL spelling and is never emitted.
            (_, DbType::Other) => {
                unreachable!("DbType::Other is introspection-only and never rendered as DDL")
            }
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
        // i64 needs BIGINT (Postgres INTEGER is 32-bit); bytes/uuid are BYTEA.
        assert_eq!(DbType::Integer.sql_type(SqlDialect::Postgres), "BIGINT");
        assert_eq!(DbType::Text.sql_type(SqlDialect::Postgres), "TEXT");
        assert_eq!(DbType::Blob.sql_type(SqlDialect::Postgres), "BYTEA");
        assert_eq!(DbType::Uuid.sql_type(SqlDialect::Postgres), "BYTEA");
    }
}
