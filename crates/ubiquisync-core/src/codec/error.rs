#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("unknown tag: {0}")]
    UnknownTag(u8),
    #[error("invalid UUID length: expected 16, got {0}")]
    UnresolvedUuid(u64),
    #[error("invalid column type bits in column ID {0:#04x}")]
    InvalidColumnType(u8),
    #[error("hash mismatch: expected {expected:#010x}, got {got:#010x}")]
    HashMismatch { expected: u32, got: u32 },
    #[error("varint overflows u64")]
    VarIntOverflow,
    #[error("non-monotonic delta")]
    NonMonotonicDelta,
    #[error("invalid utf-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("text value contains an embedded NUL byte")]
    TextContainsNul,
    #[error("bad segment magic bytes — not a ubiquisync segment")]
    BadSegmentMagic,
    #[error("missing user id in server mode")]
    MissingUserId,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupted log file")]
    CorruptedLogFile,
}
