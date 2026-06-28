/// Column type for table columns, encoded in [`ColumnId`] type bits.
///
/// The type set is **closed**. Values 5–7 are invalid (not reserved).
/// A peer encountering an invalid type treats it as a protocol error —
/// this doubles as corruption detection, since a bit-flipped ID fails
/// loudly instead of silently misparsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColType {
    /// `BLOB`. Length-prefixed on wire. LWW merge.
    Bytes = 0,
    /// `TEXT`. Length-prefixed on wire. LWW merge. Must be valid UTF-8
    /// (validated at decode) with no embedded NUL bytes.
    Text = 1,
    /// `INTEGER`. Zigzag varint on wire. LWW merge.
    I64 = 2,
    /// `BLOB` (16-byte). Fixed 16 bytes on wire. LWW merge.
    Uuid = 3,
}

impl ColType {
    /// Returns the wire encoding used for this PK column type.
    pub const fn wire_encoding(&self) -> WireEncoding {
        match self {
            Self::Bytes | Self::Text => WireEncoding::LengthPrefixed,
            Self::Uuid => WireEncoding::Fixed16,
            Self::I64 => WireEncoding::ZigzagVarint,
        }
    }

    pub const fn from_bits(value: u8) -> Self {
        match value & 0b11 {
            0 => Self::Bytes,
            1 => Self::Uuid,
            2 => Self::Text,
            _ => Self::I64,
        }
    }
}

/// Wire encoding family for a column type. Determined by the type bits
/// in a [`ColumnId`] or the PK shape bits in a [`TableId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireEncoding {
    /// Length-prefixed variable-length bytes (Bytes, Text).
    LengthPrefixed,
    /// Fixed 16 bytes, no length prefix (UUID).
    Fixed16,
    /// Zigzag-encoded varint (I64, MaxI64).
    ZigzagVarint,
}
