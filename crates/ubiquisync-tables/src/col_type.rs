use crate::id::{ColumnId, TableId};

/// Primary key column type, encoded as 2 bits in the [`TableId`] PK shape.
///
/// All four 2-bit values are valid — the field is total, so PK shapes can
/// never fail to parse. PK values are row identity: they are compared, never
/// merged, so every type here is deterministic by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PkColType {
    /// Variable-length byte string (length-prefixed on wire).
    Bytes = 0,
    /// Fixed 16-byte UUID (no length prefix on wire).
    Uuid = 1,
    /// UTF-8 text (length-prefixed on wire). Must be valid UTF-8 with no
    /// embedded NUL bytes. Compared as raw bytes: no Unicode normalization,
    /// no case folding — "café" in NFC and NFD are different keys.
    Text = 2,
    /// Signed 64-bit integer (zigzag varint on wire).
    I64 = 3,
}

impl PkColType {
    /// Returns the wire encoding used for this PK column type.
    pub const fn wire_encoding(&self) -> WireEncoding {
        match self {
            Self::Bytes | Self::Text => WireEncoding::LengthPrefixed,
            Self::Uuid => WireEncoding::Fixed16,
            Self::I64 => WireEncoding::ZigzagVarint,
        }
    }

    const fn from_bits(value: u8) -> Self {
        match value & 0b11 {
            0 => Self::Bytes,
            1 => Self::Uuid,
            2 => Self::Text,
            _ => Self::I64,
        }
    }
}

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
    /// `INTEGER`. Zigzag varint on wire. Max-wins merge (value only increases).
    /// No timestamp companion needed. Use for monotonic values like `revoked_at`.
    /// For min semantics, negate at the application layer.
    /// Also the building block for counter patterns: a table keyed by
    /// `(counter_id, peer_id)` where each peer raises only its own row's
    /// MaxI64 value is a deterministic G-counter (sum rows at read time).
    MaxI64 = 4,
    // 5, 6, 7 = invalid (protocol error).
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

impl ColType {
    pub const fn is_lww(&self) -> bool {
        match self {
            Self::Bytes | Self::Text | Self::I64 | Self::Uuid => true,
            Self::MaxI64 => false,
        }
    }

    /// Returns the wire encoding used for this column type.
    pub const fn wire_encoding(&self) -> WireEncoding {
        match self {
            Self::Bytes | Self::Text => WireEncoding::LengthPrefixed,
            Self::Uuid => WireEncoding::Fixed16,
            Self::I64 | Self::MaxI64 => WireEncoding::ZigzagVarint,
        }
    }

    const fn into_bits(self) -> u8 {
        self as _
    }

    /// Returns `None` for invalid type values (5, 6, 7).
    pub const fn try_from_bits(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Bytes),
            1 => Some(Self::Text),
            2 => Some(Self::I64),
            3 => Some(Self::Uuid),
            4 => Some(Self::MaxI64),
            _ => None,
        }
    }

    const fn from_bits(value: u8) -> Self {
        match value {
            0 => Self::Bytes,
            1 => Self::Text,
            2 => Self::I64,
            3 => Self::Uuid,
            4 => Self::MaxI64,
            // bitfield-struct requires a total function; callers should
            // validate with try_from_bits before constructing.
            _ => panic!("invalid ColType"),
        }
    }
}

