//! Postgres storage backend for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate will implement the [`Db`](ubiquisync_sql::db::Db) backend
//! abstraction over a real Postgres driver, reporting
//! [`SqlDialect::Postgres`](ubiquisync_sql::dialect::SqlDialect::Postgres). The
//! SQL flavor itself is not implemented here — it lives in `ubiquisync-sql`;
//! this crate only drives the connection. The driver integration is not yet
//! written.
