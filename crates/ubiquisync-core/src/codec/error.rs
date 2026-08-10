/// An error encoding or decoding the log wire format.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The input ended mid-value (truncated entry or segment).
    #[error("unexpected end of input")]
    UnexpectedEof,
    /// Bytes remained after a value that should have consumed its whole input.
    #[error("trailing bytes remain after decoding")]
    TrailingBytes,
    /// An entry tag the op vocabulary does not recognize.
    #[error("unknown tag: {0}")]
    UnknownTag(u8),
    /// A UUID dictionary reference pointed at an index that was never defined.
    #[error("unresolved UUID dictionary reference {0}")]
    UnresolvedUuid(u64),
    /// The number of primary-key values disagreed with the table ID's PK count.
    #[error("primary key has {got} value(s) but the table ID declares {expected}")]
    PkCountMismatch {
        /// PK column count the table ID declares.
        expected: usize,
        /// Number of PK values actually supplied.
        got: usize,
    },
    /// A column value's variant did not match the column ID's declared type.
    #[error("column value variant does not match the column ID's declared type")]
    ColumnValueMismatch,
    /// A primary-key value's variant did not match the table ID's declared PK
    /// type.
    #[error("primary key value variant does not match the table ID's declared PK type")]
    PkValueMismatch,
    /// The recomputed integrity hash did not match the stored one — corruption
    /// or tampering.
    #[error("hash mismatch: expected {expected:#010x}, got {got:#010x}")]
    HashMismatch {
        /// Truncated hash read from the entry.
        expected: u32,
        /// Truncated hash recomputed over the decoded bytes.
        got: u32,
    },
    /// A varint encoded a value larger than `u64`.
    #[error("varint overflows u64")]
    VarIntOverflow,
    /// A timestamp delta went backwards, violating the monotonic ordering.
    #[error("non-monotonic delta")]
    NonMonotonicDelta,
    /// Adding a timestamp delta to the running base overflowed `u64`.
    #[error("timestamp delta overflows u64")]
    TimestampOverflow,
    /// An on-wire length/count did not fit in `usize` on this target.
    #[error("on-wire length/count {0} does not fit in usize on this target")]
    LengthTooLarge(u64),
    /// A text value was not valid UTF-8.
    #[error("invalid utf-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    /// A text value contained an embedded NUL byte, which is disallowed.
    #[error("text value contains an embedded NUL byte")]
    TextContainsNul,
    /// The segment's leading magic bytes did not match the expected app magic.
    #[error("bad magic bytes")]
    BadMagic,
    /// The segment flags byte was neither the device nor the server mode value.
    #[error("unknown segment flags byte {0:#04x}")]
    UnknownSegmentFlags(u8),
    /// A server-mode entry was missing its required user id.
    #[error("missing user id in server mode")]
    MissingUserId,
    /// An underlying I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The log file's framing was structurally invalid.
    #[error("corrupted log file")]
    CorruptedLogFile,
    #[error("varint not in minimal form")]
    NonMinimalVarint,
}
