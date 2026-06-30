//! Table protocol for Ubiquisync — structured rows synced over a Ubiquisync log.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate defines the table data domain: tables with a compile-time
//! schema, addressed by self-describing [type-encoded IDs](crate::id), mutated
//! by a small [op vocabulary](crate::op), and encoded to the log wire format by
//! the [codec](crate::codec). Each [`ColType`](crate::col_type::ColType) maps to
//! a generic SQL storage class ([`DbType`](ubiquisync_sql::db::DbType)) that
//! storage backends turn into concrete column types. The storage-agnostic log
//! engine, clock, and codec framing live in
//! [`ubiquisync-core`](ubiquisync_core); SQL backends live in companion crates
//! such as `ubiquisync-sqlite`.

pub mod codec;
mod col_type;
pub mod id;
mod index_codec;
mod naming;
pub mod op;
mod physical_schema;
mod reducer;
mod schema;
mod util;
mod watch;
