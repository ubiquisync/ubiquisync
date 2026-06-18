//! SQL dialect abstraction for storage backends.
//!
//! The sync engine is storage-agnostic: it builds SQL as strings and runs
//! them through a backend connection. The few places where SQL dialects
//! genuinely diverge (currently type names; placeholder syntax and
//! scalar-max functions still to come) are abstracted behind
//! [`SqlDialect`], implemented by each backend crate.

use crate::db::DbType;
use crate::id::{ColType, PkColType};

/// Maps protocol types to a backend's SQL type names.
///
/// Implemented by storage backend crates (e.g. `SqliteDialect` in
/// `ubiquisync-sqlite`). The engine never hardcodes a type name — it always
/// goes through the active dialect, so a table created on one backend has
/// the documented column types for that backend.
pub trait SqlDialect {
    /// SQL column type for a non-PK column.
    fn col_type(&self, col_type: ColType) -> &'static str;

    /// SQL column type for a table primary key column.
    fn pk_col_type(&self, col_type: PkColType) -> &'static str;

    fn lww_col_type(&self) -> &'static str;
}
