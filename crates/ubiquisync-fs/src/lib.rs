//! Filesystem segment-store backend for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate implements the storage seam from
//! [`ubiquisync-core`](ubiquisync_core)'s `sync` module against the local
//! filesystem: [`FsLogSink`] is the write side ([`LogEntrySink`]) and
//! [`FsLogSource`] is the read side ([`LogSource`]). Both are generic over the
//! op vocabulary `E: Op`, so one storage layer carries any data domain (table
//! ops, doc ops, …).
//!
//! # On-disk layout
//!
//! Each peer's log lives under `{root}/{base64url(peer_id)}/`, organized as a
//! tree of *batches* and *segments*:
//!
//! - A **segment** is one append-only codec stream (an [`Encoder`] output). It
//!   seals — gets renamed to carry its end index — once it grows past
//!   [`MAX_SEGMENT_SIZE`](segments::MAX_SEGMENT_SIZE).
//! - A **batch** is a directory of segments. It rolls over once it accumulates
//!   too many segments or bytes, after which a fresh batch starts. Sealed
//!   batches may later be compacted into a single gzipped `.gz` pack file; the
//!   source reads both forms transparently.
//!
//! Entry indices are global per peer: the source presents every entry across
//! all of a peer's segments as one contiguous, monotonically-indexed stream,
//! which is exactly what the sync engine's cursor walks.
//!
//! [`LogEntrySink`]: ubiquisync_core::sync::LogEntrySink
//! [`LogSource`]: ubiquisync_core::sync::LogSource
//! [`Op`]: ubiquisync_core::codec::Op
//! [`Encoder`]: ubiquisync_core::codec::Encoder

pub mod segments;
pub mod sink;
pub mod source;
#[cfg(test)]
mod tests;
mod batches;
mod peers;
mod timestamp;

pub use sink::{FsLogSink, SharedFsLogSink};
pub use source::FsLogSource;
