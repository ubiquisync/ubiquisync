use ubiquisync_sql::db::DbType;

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

    /// The generic SQL storage class this column type materializes to.
    /// Drives `CREATE TABLE`/`ALTER TABLE` column type names via
    /// [`DbType::sql_type`].
    pub const fn db_type(self) -> DbType {
        match self {
            Self::Bytes => DbType::Blob,
            Self::Text => DbType::Text,
            Self::I64 => DbType::Integer,
            Self::Uuid => DbType::Uuid,
        }
    }

    /// Whether an existing database column of type `db` can hold values of
    /// this column type. Used during schema reconciliation to detect a table
    /// whose on-disk types disagree with the declared schema. UUIDs accept a
    /// raw `Blob` since that is how they are stored on backends without a
    /// native UUID type.
    pub const fn accepts(self, db: DbType) -> bool {
        match self {
            Self::Bytes => matches!(db, DbType::Blob),
            Self::Text => matches!(db, DbType::Text),
            Self::I64 => matches!(db, DbType::Integer),
            Self::Uuid => matches!(db, DbType::Uuid | DbType::Blob),
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
