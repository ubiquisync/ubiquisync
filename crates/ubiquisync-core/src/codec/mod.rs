//! Byte-level wire codec for log segments.
//!
//! A segment is an app-supplied magic header plus a sequence of entries. Each
//! entry is one op (encoded through the [`Op`] trait), a delta-encoded
//! timestamp, an optional user id (server mode), and a truncated blake3
//! integrity check. The magic identifies the application and is passed to
//! [`Encoder::new`] / [`Decoder::new`] by the caller, never defined here.
//!
//! This module is generic over the op vocabulary `E: Op`: the framing here
//! (header, timestamp deltas, UUID dictionary compression, blake3 trailer,
//! expungement markers) knows nothing about any particular op. Each data
//! domain implements [`Op`] for its own op type — for example the table op
//! vocabulary in `ubiquisync-tables`.

mod consts;
mod decoder;
mod encoder;
mod error;
mod op;
mod reader;
mod writer;

pub use consts::{FLAG_DEVICE, FLAG_SERVER, TAG_EXPUNGED};
pub use decoder::{DecodedEntry, DecodedLogs, Decoder};
pub use encoder::Encoder;
pub use error::CodecError;
pub use op::Op;
pub use reader::EntryBufferReader;
pub use writer::EntryBufferWriter;
