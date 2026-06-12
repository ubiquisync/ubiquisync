//! Postgres dialect: protocol type → Postgres column type mapping.

use ubiquisync_core::dialect::SqlDialect;
use ubiquisync_core::sys_id::{PkColType, SysColType};

/// The Postgres SQL dialect.
///
/// Integer columns map to `BIGINT`, not `INTEGER` — Postgres `INTEGER` is
/// 32-bit and would overflow protocol i64 values. Raw bytes and UUIDs map
/// to `BYTEA` (the protocol stores UUIDs as 16 raw bytes, not the Postgres
/// `UUID` type, so the column type matches the wire representation).
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDialect;

impl SqlDialect for PostgresDialect {
    fn sys_col_type(&self, col_type: SysColType) -> &'static str {
        match col_type {
            SysColType::Bytes | SysColType::Uuid => "BYTEA",
            SysColType::Text => "TEXT",
            SysColType::I64 | SysColType::MaxI64 => "BIGINT",
        }
    }

    fn sys_pk_col_type(&self, col_type: PkColType) -> &'static str {
        match col_type {
            PkColType::Bytes | PkColType::Uuid => "BYTEA",
            PkColType::Text => "TEXT",
            PkColType::I64 => "BIGINT",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sys_col_types_map_to_documented_postgres_types() {
        // Goal: non-PK column types map exactly as documented in the
        // sys-tables protocol reference (column types table).
        let d = PostgresDialect;
        assert_eq!(d.sys_col_type(SysColType::Bytes), "BYTEA");
        assert_eq!(d.sys_col_type(SysColType::Text), "TEXT");
        assert_eq!(d.sys_col_type(SysColType::I64), "BIGINT");
        assert_eq!(d.sys_col_type(SysColType::Uuid), "BYTEA");
        assert_eq!(d.sys_col_type(SysColType::MaxI64), "BIGINT");
    }

    #[test]
    fn pk_col_types_map_to_documented_postgres_types() {
        // Goal: PK column types map exactly as documented in the
        // sys-tables protocol reference (PK types table).
        let d = PostgresDialect;
        assert_eq!(d.sys_pk_col_type(PkColType::Bytes), "BYTEA");
        assert_eq!(d.sys_pk_col_type(PkColType::Uuid), "BYTEA");
        assert_eq!(d.sys_pk_col_type(PkColType::Text), "TEXT");
        assert_eq!(d.sys_pk_col_type(PkColType::I64), "BIGINT");
    }
}
