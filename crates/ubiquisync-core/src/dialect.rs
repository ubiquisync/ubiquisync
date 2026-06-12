//! SQL dialect abstraction for storage backends.
//!
//! The sync engine is storage-agnostic: it builds SQL as strings and runs
//! them through a backend connection. The few places where SQL dialects
//! genuinely diverge (type names, and as the engine port progresses,
//! placeholder syntax and scalar-max functions) are abstracted behind
//! [`SqlDialect`], implemented by each backend crate.

use crate::sys_id::{PkColType, SysColType};

/// Maps protocol types to a backend's SQL type names.
///
/// Implemented by storage backend crates (e.g. `SqliteDialect` in
/// `ubiquisync-sqlite`). The engine never hardcodes a type name — it always
/// goes through the active dialect, so a table created on one backend has
/// the documented column types for that backend.
pub trait SqlDialect {
    /// SQL column type for a non-PK system column.
    fn sys_col_type(&self, col_type: SysColType) -> &'static str;

    /// SQL column type for a system table primary key column.
    fn sys_pk_col_type(&self, col_type: PkColType) -> &'static str;
}
