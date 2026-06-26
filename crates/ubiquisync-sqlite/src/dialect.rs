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

    fn lww_col_type(&self) -> &'static str {
        // HLC timestamps are i64; SQLite INTEGER is already 64-bit.
        "INTEGER"
    }

    fn placeholder(&self, n: usize) -> String {
        format!("?{n}")
    }

    fn scalar_max(&self) -> &'static str {
        "MAX"
    }

    fn text_collate(&self) -> &'static str {
        // SQLite TEXT/BLOB already compare bytewise (BINARY collation), so
        // no collation override is needed.
        ""
    }

    fn insert_ignore_verb(&self) -> &'static str {
        "INSERT OR IGNORE"
    }

    fn conflict_ignore_clause(&self, _pk_cols: &str) -> String {
        // The verb (INSERT OR IGNORE) already carries the ignore semantics,
        // so no trailing ON CONFLICT clause is needed.
        String::new()
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

    #[test]
    fn dialect_tokens_use_sqlite_syntax() {
        // Goal: the SQL builder tokens that diverge between backends render
        // in SQLite's syntax (placeholders, scalar max, conflict-ignore).
        let d = SqliteDialect;
        assert_eq!(d.lww_col_type(), "INTEGER");
        assert_eq!(d.placeholder(3), "?3");
        assert_eq!(d.scalar_max(), "MAX");
        // SQLite already compares bytewise, so no collation override.
        assert_eq!(d.text_collate(), "");
        // PK-only insert: the verb carries the ignore, no trailing clause.
        assert_eq!(d.insert_ignore_verb(), "INSERT OR IGNORE");
        assert_eq!(d.conflict_ignore_clause("\"id\""), "");
    }
}
