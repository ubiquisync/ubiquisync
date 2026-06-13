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
//! by a small [op vocabulary](crate::op), encoded to the log wire format by the
//! [codec](crate::codec), and materialized to SQL through a backend
//! [`SqlDialect`](crate::dialect::SqlDialect). The storage-agnostic log engine,
//! clock, and codec framing live in [`ubiquisync-core`](ubiquisync_core); SQL
//! backends live in companion crates such as `ubiquisync-sqlite`.

pub mod codec;
pub mod dialect;
pub mod id;
pub mod op;
