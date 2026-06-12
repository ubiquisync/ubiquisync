//! The protocol's UUID representation: 16 raw bytes.
//!
//! Ubiquisync treats UUIDs as opaque 16-byte identifiers — it never inspects
//! version or variant bits, and stores them as raw bytes (`BLOB`/`BYTEA`),
//! not as strings or native UUID column types. Applications generate them
//! however they like (v4, v7, ...); the protocol only requires uniqueness.

/// A 16-byte UUID, stored and compared as raw bytes.
pub type Uuid = [u8; 16];

/// Validate a byte slice as a 16-byte UUID.
pub fn try_uuid(bytes: &[u8]) -> Result<Uuid, UuidLengthError> {
    bytes.try_into().map_err(|_| UuidLengthError(bytes.len()))
}

/// Error returned by [`try_uuid`] when the slice is not exactly 16 bytes.
/// Carries the actual length seen.
#[derive(Debug)]
pub struct UuidLengthError(pub usize);

impl std::fmt::Display for UuidLengthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "expected 16-byte UUID, got {} bytes", self.0)
    }
}

impl std::error::Error for UuidLengthError {}
