//! SQLite dialect: protocol type → SQLite column type mapping.

use ubiquisync_tables::dialect::SqlDialect;
use ubiquisync_tables::id::{ColType, PkColType};

/// The SQLite SQL dialect.
///
/// SQLite uses type affinity rather than strict types, so these names pick
/// the affinity that matches the protocol's value semantics: `BLOB` for raw
/// bytes and UUIDs, `TEXT` for UTF-8 text, `INTEGER` for i64 (SQLite
/// integers are 64-bit, so no width concern as with Postgres `INTEGER`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn col_type(&self, col_type: ColType) -> &'static str {
        match col_type {
            ColType::Bytes | ColType::Uuid => "BLOB",
            ColType::Text => "TEXT",
            ColType::I64 | ColType::MaxI64 => "INTEGER",
        }
    }

    fn pk_col_type(&self, col_type: PkColType) -> &'static str {
        match col_type {
            PkColType::Bytes | PkColType::Uuid => "BLOB",
            PkColType::Text => "TEXT",
            PkColType::I64 => "INTEGER",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn col_types_map_to_documented_sqlite_types() {
        // Goal: non-PK column types map exactly as documented in the
        // tables protocol reference (column types table).
        let d = SqliteDialect;
        assert_eq!(d.col_type(ColType::Bytes), "BLOB");
        assert_eq!(d.col_type(ColType::Text), "TEXT");
        assert_eq!(d.col_type(ColType::I64), "INTEGER");
        assert_eq!(d.col_type(ColType::Uuid), "BLOB");
        assert_eq!(d.col_type(ColType::MaxI64), "INTEGER");
    }

    #[test]
    fn pk_col_types_map_to_documented_sqlite_types() {
        // Goal: PK column types map exactly as documented in the
        // tables protocol reference (PK types table).
        let d = SqliteDialect;
        assert_eq!(d.pk_col_type(PkColType::Bytes), "BLOB");
        assert_eq!(d.pk_col_type(PkColType::Uuid), "BLOB");
        assert_eq!(d.pk_col_type(PkColType::Text), "TEXT");
        assert_eq!(d.pk_col_type(PkColType::I64), "INTEGER");
    }
}
