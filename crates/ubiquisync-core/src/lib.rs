//! Core protocol types and sync engine for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate contains the storage-agnostic, domain-agnostic core of
//! Ubiquisync: the log entry envelope, opaque UUIDs, the HLC clock, and the
//! wire codec. It has no database driver dependencies and is generic over the
//! op vocabulary it carries — data domains such as tables live in companion
//! crates like `ubiquisync-tables`, and storage backends in crates such as
//! `ubiquisync-sqlite`.
//!
//! Most applications should depend on the [`ubiquisync`](https://crates.io/crates/ubiquisync)
//! facade crate rather than this crate directly.

extern crate alloc;

pub mod codec;
pub mod crypto;
pub mod event;
pub mod hlc;
pub mod ids;
pub mod log_entry;
pub mod store;
//pub mod sync;
pub mod bytes;
pub mod uuid;
