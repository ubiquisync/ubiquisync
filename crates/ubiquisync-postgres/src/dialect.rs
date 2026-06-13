//! Postgres dialect: protocol type → Postgres column type mapping.

use ubiquisync_tables::dialect::SqlDialect;
use ubiquisync_tables::id::{ColType, PkColType};

/// The Postgres SQL dialect.
///
/// Integer columns map to `BIGINT`, not `INTEGER` — Postgres `INTEGER` is
/// 32-bit and would overflow protocol i64 values. Raw bytes and UUIDs map
/// to `BYTEA` (the protocol stores UUIDs as 16 raw bytes, not the Postgres
/// `UUID` type, so the column type matches the wire representation).
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDialect;

impl SqlDialect for PostgresDialect {
    fn col_type(&self, col_type: ColType) -> &'static str {
        match col_type {
            ColType::Bytes | ColType::Uuid => "BYTEA",
            ColType::Text => "TEXT",
            ColType::I64 | ColType::MaxI64 => "BIGINT",
        }
    }

    fn pk_col_type(&self, col_type: PkColType) -> &'static str {
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
    fn col_types_map_to_documented_postgres_types() {
        // Goal: non-PK column types map exactly as documented in the
        // tables protocol reference (column types table).
        let d = PostgresDialect;
        assert_eq!(d.col_type(ColType::Bytes), "BYTEA");
        assert_eq!(d.col_type(ColType::Text), "TEXT");
        assert_eq!(d.col_type(ColType::I64), "BIGINT");
        assert_eq!(d.col_type(ColType::Uuid), "BYTEA");
        assert_eq!(d.col_type(ColType::MaxI64), "BIGINT");
    }

    #[test]
    fn pk_col_types_map_to_documented_postgres_types() {
        // Goal: PK column types map exactly as documented in the
        // tables protocol reference (PK types table).
        let d = PostgresDialect;
        assert_eq!(d.pk_col_type(PkColType::Bytes), "BYTEA");
        assert_eq!(d.pk_col_type(PkColType::Uuid), "BYTEA");
        assert_eq!(d.pk_col_type(PkColType::Text), "TEXT");
        assert_eq!(d.pk_col_type(PkColType::I64), "BIGINT");
    }
}
