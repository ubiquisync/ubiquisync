//! SQLite dialect: protocol type → SQLite column type mapping.

use ubiquisync_core::dialect::SqlDialect;
use ubiquisync_core::sys_id::{PkColType, SysColType};

/// The SQLite SQL dialect.
///
/// SQLite uses type affinity rather than strict types, so these names pick
/// the affinity that matches the protocol's value semantics: `BLOB` for raw
/// bytes and UUIDs, `TEXT` for UTF-8 text, `INTEGER` for i64 (SQLite
/// integers are 64-bit, so no width concern as with Postgres `INTEGER`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn sys_col_type(&self, col_type: SysColType) -> &'static str {
        match col_type {
            SysColType::Bytes | SysColType::Uuid => "BLOB",
            SysColType::Text => "TEXT",
            SysColType::I64 | SysColType::MaxI64 => "INTEGER",
        }
    }

    fn sys_pk_col_type(&self, col_type: PkColType) -> &'static str {
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
    fn sys_col_types_map_to_documented_sqlite_types() {
        // Goal: non-PK column types map exactly as documented in the
        // sys-tables protocol reference (column types table).
        let d = SqliteDialect;
        assert_eq!(d.sys_col_type(SysColType::Bytes), "BLOB");
        assert_eq!(d.sys_col_type(SysColType::Text), "TEXT");
        assert_eq!(d.sys_col_type(SysColType::I64), "INTEGER");
        assert_eq!(d.sys_col_type(SysColType::Uuid), "BLOB");
        assert_eq!(d.sys_col_type(SysColType::MaxI64), "INTEGER");
    }

    #[test]
    fn pk_col_types_map_to_documented_sqlite_types() {
        // Goal: PK column types map exactly as documented in the
        // sys-tables protocol reference (PK types table).
        let d = SqliteDialect;
        assert_eq!(d.sys_pk_col_type(PkColType::Bytes), "BLOB");
        assert_eq!(d.sys_pk_col_type(PkColType::Uuid), "BLOB");
        assert_eq!(d.sys_pk_col_type(PkColType::Text), "TEXT");
        assert_eq!(d.sys_pk_col_type(PkColType::I64), "INTEGER");
    }
}
