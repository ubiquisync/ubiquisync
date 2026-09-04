// //! Table protocol for Ubiquisync — structured rows synced over a Ubiquisync log.
// //!
// //! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
// //! >
// //! > This crate is in active, early development. APIs are incomplete, unproven,
// //! > and **will change without notice**. Do not use it in production. Breaking
// //! > changes may land on any commit.
// //!
// //! This crate defines the table data domain: tables with a compile-time
// //! schema, addressed by self-describing [type-encoded IDs](crate::id), mutated
// //! by a small [op vocabulary](crate::op), and encoded to the log wire format by
// //! the [codec]. Each [`ColType`](crate::col_type::ColType) maps to
// //! a generic SQL storage class ([`DbType`](ubiquisync_sql::db::DbType)) that
// //! storage backends turn into concrete column types. The storage-agnostic log
// //! engine, clock, and codec framing live in
// //! [`ubiquisync-core`](ubiquisync_core); SQL backends live in companion crates
// //! such as `ubiquisync-sqlite`.

pub mod codec;
pub mod col_type;
pub mod error;
pub mod id;
// /// Declarative macros for building [`schema`] values from a compact table DSL.
// pub mod macros;
mod naming;

// /// The `sea-query` version this crate builds its typed table columns against.
// ///
// /// Compose filters/ordering for the generated `query` readers through this
// /// re-export (e.g. `ubiquisync_tables::sea_query::Expr`) so the `Iden` impls on
// /// each table's `Col`/`Table` line up — a separately-versioned `sea-query`
// /// dependency would not.
// pub use sea_query;
pub mod op;
// // Physical storage layer (surrogate tables, schema reconciliation). Wired into
// // the shipping build by the table reducer; `allow(dead_code)` until then. The
// // `test_support` suite exercises it in the meantime.
#[allow(dead_code)]
mod physical_schema;
// /// User-declared table schemas ([`TableSchema`](schema::TableSchema)): the
// /// user-facing name and columns exposed as a SQL VIEW over surrogate storage.
pub mod schema;

pub mod reducer;
// /// Backend-agnostic physical-schema suite the driver crates run against their
// /// real `Db`. Compiled for this crate's own tests and for any crate that enables
// /// the `test-support` feature (a SQL driver, in its dev-dependencies).
// #[cfg(any(test, feature = "test-support"))]
// pub mod test_support;
pub mod watch;
