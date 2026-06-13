//! Type-encoded IDs for tables and columns.
//!
//! Table and column IDs embed type information directly in their bits,
//! making every valid ID fully self-describing. Any peer can parse entries for
//! unknown tables/columns without a schema lookup — the wire encoding is
//! determined entirely by the ID.
//!
//! All non-PK columns are implicitly nullable at the protocol level.
//! Non-null guarantees on non-PK columns cannot be enforced
//! in a distributed system without full schema coordination.
//!
//! ## Table IDs (`TableId`, u16)
//!
//! The layout is variable: the top 2 bits encode the PK column count, each PK
//! column type takes 2 bits below that, and the remaining low bits are the
//! table index. Parsing is still fully self-describing — the count is always
//! in the top 2 bits, which determines the rest of the layout.
//!
//! ```text
//! 1 PK:  ┌ count:2 ┬ t1:2 ┬─────── index:12 ───────┐   4096 indices
//! 2 PKs: ┌ count:2 ┬ t1:2 ┬ t2:2 ┬── index:10 ─────┐   1024 indices
//! 3 PKs: ┌ count:2 ┬ t1:2 ┬ t2:2 ┬ t3:2 ┬ index:8 ─┐    256 indices
//! 4 PKs: ┌ count:2 ┬ t1:2 ┬ t2:2 ┬ t3:2 ┬ t4:2 ┬ index:6 ┐  64 indices
//! ```
//!
//! Every `u16` bit pattern is a valid table ID — both the count field and the
//! 2-bit type fields are total, so there is no protocol-error path for table IDs.
//!
//! ## Column IDs (`ColumnId`, u8)
//!
//! ```text
//! ┌─────────────┬───────────────┐
//! │    type     │ column index  │
//! │   (3 bits)  │   (5 bits)    │
//! └─────────────┴───────────────┘
//! ```
//!
//! Type bits (high 3): column wire type. Values 5–7 are invalid (protocol error).
//!
//! ## Frozen type vocabulary
//!
//! The type sets below are frozen at protocol v1. A new type value cannot be
//! added compatibly: a peer that does not know a type cannot even skip its
//! wire bytes, so any addition is a hard protocol fork. Future semantic types
//! must instead be encoded over `Bytes` or `Text` — old peers parse, store,
//! and re-sync them as opaque LWW values, which is exactly the correct
//! behavior for data they do not understand.

use bitfield_struct::bitfield;

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

/// Type-encoded table ID.
///
/// The top 2 bits encode the PK column count (count − 1, so 1–4 columns),
/// followed by 2 type bits per PK column, with the remaining low bits as an
/// arbitrary table index. The layout width varies with the count, so this is
/// a manual pack/unpack rather than a fixed bitfield — see the module docs
/// for the per-count layouts.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TableId(u16);

impl TableId {
    /// Build a table ID from its PK column types and table index.
    ///
    /// `const`-evaluable so table IDs can be compile-time constants; invalid
    /// shapes (0 or >4 PK columns, index out of range for the count) panic,
    /// which surfaces as a compile error in const context.
    pub const fn new(pk_types: &[PkColType], index: u16) -> Self {
        let count = pk_types.len();
        assert!(count >= 1 && count <= 4, "PK column count must be 1-4");
        let index_bits = Self::index_bits_for(count);
        assert!(
            (index as u32) < (1u32 << index_bits),
            "table index exceeds available bits for this PK count"
        );

        let mut raw = (((count - 1) as u16) << 14) | index;
        let mut i = 0;
        while i < count {
            // PK type i sits directly below the count field, 2 bits each,
            // descending: t1 at bits 13-12, t2 at 11-10, ...
            raw |= (pk_types[i] as u16) << (12 - 2 * i);
            i += 1;
        }
        Self(raw)
    }

    /// Reconstruct from a raw u16. Total — every bit pattern is a valid ID.
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// The raw u16 representation.
    pub const fn raw(&self) -> u16 {
        self.0
    }

    /// Number of PK columns (1–4), from the top 2 bits.
    pub const fn pk_count(&self) -> usize {
        ((self.0 >> 14) as usize) + 1
    }

    /// Type of PK column `i` (0-based). Panics if `i >= pk_count()`.
    pub const fn pk_col_type(&self, i: usize) -> PkColType {
        assert!(i < self.pk_count(), "PK column index out of range");
        PkColType::from_bits(((self.0 >> (12 - 2 * i)) & 0b11) as u8)
    }

    /// Width of the table index field for this ID's PK count
    /// (12, 10, 8, or 6 bits).
    pub const fn index_bits(&self) -> u32 {
        Self::index_bits_for(self.pk_count())
    }

    /// Arbitrary table index within this PK shape.
    pub const fn index(&self) -> u16 {
        self.0 & ((1u16 << self.index_bits()) - 1)
    }

    const fn index_bits_for(pk_count: usize) -> u32 {
        // 16 bits total - 2 count bits - 2 per PK type.
        14 - 2 * pk_count as u32
    }
}

impl From<u16> for TableId {
    fn from(raw: u16) -> Self {
        Self(raw)
    }
}

impl From<TableId> for u16 {
    fn from(id: TableId) -> u16 {
        id.0
    }
}

impl core::fmt::Debug for TableId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Render the decoded shape, not just the raw bits — table IDs show up
        // in sync diagnostics where the PK shape is what matters.
        write!(f, "TableId(0x{:04X}, pk=[", self.0)?;
        let mut i = 0;
        while i < self.pk_count() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:?}", self.pk_col_type(i))?;
            i += 1;
        }
        write!(f, "], index={})", self.index())
    }
}

// bitfield-struct packs fields from LSB upward, so fields listed first
// occupy the lowest bits. We want index in low bits and type in high bits.

/// Type-encoded column ID.
///
/// High 3 bits encode the column's wire type, making the ID self-describing
/// for wire parsing. The lower 5 bits are an arbitrary column index within
/// the table. All non-PK columns are implicitly nullable.
#[bitfield(u8)]
#[derive(PartialEq, Eq, Hash)]
pub struct ColumnId {
    // -- Column index (low 5 bits) --
    /// Arbitrary column index within the table.
    #[bits(5)]
    pub index: u8,

    // -- Type bits (high 3 bits) --
    /// Column wire type (bits 5–7).
    #[bits(3)]
    pub col_type: ColType,
}

impl ColumnId {
    /// Validate that the column type bits are valid (not 5, 6, or 7).
    /// Returns `None` if the raw byte encodes an invalid column type.
    pub fn try_from_raw(raw: u8) -> Option<Self> {
        let type_bits = (raw >> 5) & 0x07;
        ColType::try_from_bits(type_bits)?;
        Some(Self::from(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_id_round_trip_all_pk_counts() {
        // Goal: a table ID survives a raw u16 round trip for every PK shape.
        // Given: one table ID per PK count, with mixed PK column types.
        let shapes: [&[PkColType]; 4] = [
            &[PkColType::Uuid],
            &[PkColType::Bytes, PkColType::Uuid],
            &[PkColType::Text, PkColType::I64, PkColType::Uuid],
            &[
                PkColType::Bytes,
                PkColType::Text,
                PkColType::I64,
                PkColType::Uuid,
            ],
        ];
        for (n, pk_types) in shapes.iter().enumerate() {
            // When: constructing the ID and round-tripping through u16.
            let id = TableId::new(pk_types, 5);
            let recovered = TableId::from_raw(id.raw());

            // Then: count, every PK type, and index are all recovered.
            assert_eq!(recovered, id);
            assert_eq!(id.pk_count(), n + 1);
            for (i, expected) in pk_types.iter().enumerate() {
                assert_eq!(id.pk_col_type(i), *expected);
            }
            assert_eq!(id.index(), 5);
        }
    }

    #[test]
    fn table_id_index_in_low_bits() {
        // Goal: the table index occupies the low bits unshifted.
        // Given: a 1-PK Bytes table (count=0, type=0 → all shape bits zero).
        let id = TableId::new(&[PkColType::Bytes], 1);
        // Then: the raw value is exactly the index.
        assert_eq!(id.raw(), 1);
    }

    #[test]
    fn table_id_pk_shape_in_high_bits() {
        // Goal: PK shape packs into the high bits in declaration order.
        // Given: 1-col UUID PK: count=0, t1=Uuid(01).
        let id = TableId::new(&[PkColType::Uuid], 0);
        // Then: count in bits 15-14, t1 in bits 13-12.
        assert_eq!(id.raw() >> 12, 0b00_01);

        // Given: 2-col (Text, I64) PK: count=1, t1=Text(10), t2=I64(11).
        let id = TableId::new(&[PkColType::Text, PkColType::I64], 0);
        // Then: high 6 bits are count(01) | t1(10) | t2(11).
        assert_eq!(id.raw() >> 10, 0b01_10_11);
    }

    #[test]
    fn table_id_index_headroom_per_pk_count() {
        // Goal: the index width shrinks by 2 bits per extra PK column.
        // Given/When: the max index for each PK count.
        // Then: 12, 10, 8, 6 bits respectively, all constructible.
        let one = TableId::new(&[PkColType::Uuid], 4095);
        assert_eq!((one.index_bits(), one.index()), (12, 4095));
        let two = TableId::new(&[PkColType::Uuid; 2], 1023);
        assert_eq!((two.index_bits(), two.index()), (10, 1023));
        let three = TableId::new(&[PkColType::Uuid; 3], 255);
        assert_eq!((three.index_bits(), three.index()), (8, 255));
        let four = TableId::new(&[PkColType::Uuid; 4], 63);
        assert_eq!((four.index_bits(), four.index()), (6, 63));
    }

    #[test]
    #[should_panic(expected = "table index exceeds available bits")]
    fn table_id_rejects_index_overflow() {
        // Goal: an index too wide for the PK count is a construction error,
        // not a silent truncation that would collide with another table.
        // Given: a 1-PK table whose index needs 13 bits.
        // When: constructing it. Then: panic.
        let _ = TableId::new(&[PkColType::Uuid], 4096);
    }

    #[test]
    fn table_id_every_bit_pattern_is_valid() {
        // Goal: table IDs are total — any u16 off the wire decodes without
        // panicking and re-encodes to itself.
        for raw in 0..=u16::MAX {
            // When: decoding an arbitrary bit pattern.
            let id = TableId::from_raw(raw);
            // Then: all accessors are callable and the identity holds.
            let count = id.pk_count();
            assert!((1..=4).contains(&count));
            for i in 0..count {
                let _ = id.pk_col_type(i);
            }
            assert!(id.index() < (1 << id.index_bits()));
            assert_eq!(id.raw(), raw);
        }
    }

    #[test]
    fn pk_type_wire_encodings() {
        // Goal: each PK type maps to its documented wire encoding.
        assert_eq!(
            PkColType::Bytes.wire_encoding(),
            WireEncoding::LengthPrefixed
        );
        assert_eq!(
            PkColType::Text.wire_encoding(),
            WireEncoding::LengthPrefixed
        );
        assert_eq!(PkColType::Uuid.wire_encoding(), WireEncoding::Fixed16);
        assert_eq!(PkColType::I64.wire_encoding(), WireEncoding::ZigzagVarint);
    }

    #[test]
    fn column_id_round_trip() {
        // Goal: a column ID survives a raw u8 round trip.
        // Given: a Text column at index 3.
        let id = ColumnId::new()
            .with_index(3)
            .with_col_type(ColType::Text);

        assert_eq!(id.index(), 3);
        assert_eq!(id.col_type(), ColType::Text);

        // When/Then: round trip through the raw byte.
        let raw: u8 = id.into();
        let recovered = ColumnId::from(raw);
        assert_eq!(recovered, id);
    }

    #[test]
    fn column_id_index_in_low_bits() {
        // Goal: the column index occupies the low bits unshifted.
        let id = ColumnId::new()
            .with_index(1)
            .with_col_type(ColType::Bytes);
        let raw: u8 = id.into();
        assert_eq!(raw, 1);
    }

    #[test]
    fn column_id_type_in_high_bits() {
        // I64: bits 5-7=col_type(2)=010, index=0
        // high 3 bits = 010
        let id = ColumnId::new()
            .with_index(0)
            .with_col_type(ColType::I64);
        let raw: u8 = id.into();
        assert_eq!(raw >> 5, 0b010);

        // Text: bits 5-7=col_type(1)=001, index=0
        // high 3 bits = 001
        let id = ColumnId::new()
            .with_index(0)
            .with_col_type(ColType::Text);
        let raw: u8 = id.into();
        assert_eq!(raw >> 5, 0b001);
    }

    #[test]
    fn invalid_col_type_detected() {
        // Goal: type values 5-7 are protocol errors, 0-4 are valid.
        assert!(ColType::try_from_bits(4).is_some()); // MaxI64
        assert!(ColType::try_from_bits(5).is_none());
        assert!(ColType::try_from_bits(6).is_none());
        assert!(ColType::try_from_bits(7).is_none());

        // Then: try_from_raw rejects a byte whose type bits are invalid.
        assert!(ColumnId::try_from_raw(0b101_00000).is_none());
        assert!(ColumnId::try_from_raw(0b100_00011).is_some());
    }

    #[test]
    fn table_id_const_constructible() {
        // Goal: table IDs work as compile-time constants for def macros.
        const SETTINGS: TableId = TableId::new(&[PkColType::Text], 7);
        assert_eq!(SETTINGS.pk_count(), 1);
        assert_eq!(SETTINGS.pk_col_type(0), PkColType::Text);
        assert_eq!(SETTINGS.index(), 7);
    }
}
