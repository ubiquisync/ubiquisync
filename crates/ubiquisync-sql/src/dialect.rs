//! SQL dialect abstraction for storage backends.
//!
//! The sync engine is storage-agnostic: it builds SQL as strings and runs
//! them through a backend connection. The few places where SQL dialects
//! genuinely diverge (currently type names; placeholder syntax and
//! scalar-max functions still to come) are abstracted behind
//! [`SqlDialect`], implemented by each backend crate.

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

    /// SQL column type for the HLC timestamp companion of an LWW column
    /// (the `__<col>_lww` column). Always a 64-bit integer.
    fn lww_col_type(&self) -> &'static str;

    /// Renders a positional bind placeholder for parameter `n` (1-based):
    /// `?n` on SQLite, `$n` on Postgres.
    fn placeholder(&self, n: usize) -> String;

    /// Scalar two-argument max function: `MAX` on SQLite, `GREATEST` on
    /// Postgres. Callers must keep the `COALESCE` wrapping around the
    /// arguments — SQLite's `MAX` returns NULL if *any* argument is NULL
    /// while Postgres's `GREATEST` ignores NULLs; the COALESCE is what makes
    /// both backends merge identically. Do not simplify it away.
    fn scalar_max(&self) -> &'static str;

    /// Collation suffix appended to text comparisons that must order
    /// bytewise (the LWW value-byte tiebreak, pull-sync cursor iteration).
    /// Empty on SQLite (TEXT already compares with BINARY collation);
    /// ` COLLATE "C"` on Postgres, whose default collation is locale-aware.
    fn text_collate(&self) -> &'static str;

    /// Leading verb for a PK-only "insert if absent" with no column updates.
    /// SQLite: `INSERT OR IGNORE`. Postgres: plain `INSERT` (the ignore is
    /// expressed by [`SqlDialect::conflict_ignore_clause`] instead).
    fn insert_ignore_verb(&self) -> &'static str;

    /// Trailing conflict clause paired with [`SqlDialect::insert_ignore_verb`]
    /// for a PK-only insert. Empty on SQLite (the verb carries the
    /// semantics); ` ON CONFLICT (<pk>) DO NOTHING` on Postgres. `pk_cols` is
    /// the already-quoted, comma-joined PK column list.
    fn conflict_ignore_clause(&self, pk_cols: &str) -> String;
}
