//! Core protocol types and sync engine for Ubiquisync.
//!
//! This crate contains the storage-agnostic parts of Ubiquisync: the wire
//! protocol, type-encoded IDs, and (as the port progresses) the HLC clock,
//! codec, and merge reducer. It has no database driver dependencies —
//! storage backends live in companion crates such as `ubiquisync-sqlite`.
//!
//! Most applications should depend on the [`ubiquisync`](https://crates.io/crates/ubiquisync)
//! facade crate rather than this crate directly.

pub mod sys_id;
