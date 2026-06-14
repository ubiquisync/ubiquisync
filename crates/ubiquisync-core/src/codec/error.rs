#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("unknown tag: {0}")]
    UnknownTag(u8),
    #[error("unresolved UUID dictionary reference {0}")]
    UnresolvedUuid(u64),
    #[error("invalid column type bits in column ID {0:#04x}")]
    InvalidColumnType(u8),
    #[error("primary key has {got} value(s) but the table ID declares {expected}")]
    PkCountMismatch { expected: usize, got: usize },
    #[error("column value variant does not match the column ID's declared type")]
    ColumnValueMismatch,
    #[error("hash mismatch: expected {expected:#010x}, got {got:#010x}")]
    HashMismatch { expected: u32, got: u32 },
    #[error("varint overflows u64")]
    VarIntOverflow,
    #[error("non-monotonic delta")]
    NonMonotonicDelta,
    #[error("timestamp delta overflows u64")]
    TimestampOverflow,
    #[error("on-wire length/count {0} does not fit in usize on this target")]
    LengthTooLarge(u64),
    #[error("invalid utf-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("text value contains an embedded NUL byte")]
    TextContainsNul,
    #[error("bad segment magic bytes — not a ubiquisync segment")]
    BadSegmentMagic,
    #[error("unknown segment flags byte {0:#04x}")]
    UnknownSegmentFlags(u8),
    #[error("missing user id in server mode")]
    MissingUserId,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupted log file")]
    CorruptedLogFile,
}
