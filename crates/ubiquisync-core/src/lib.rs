//! Core protocol types and sync engine for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate contains the storage-agnostic parts of Ubiquisync: the wire
//! protocol, type-encoded IDs, and (as the port progresses) the HLC clock,
//! codec, and merge reducer. It has no database driver dependencies —
//! storage backends live in companion crates such as `ubiquisync-sqlite`.
//!
//! Most applications should depend on the [`ubiquisync`](https://crates.io/crates/ubiquisync)
//! facade crate rather than this crate directly.

pub mod dialect;
pub mod sys_id;
