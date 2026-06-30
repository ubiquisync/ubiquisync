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
//! ┌───────────┬─────────────────┐
//! │   type    │  column index   │
//! │  (2 bits) │    (6 bits)     │
//! └───────────┴─────────────────┘
//! ```
//!
//! Type bits (high 2): column wire type. All four values are valid, so every
//! byte decodes to a column ID — there is no protocol-error path.
//!
//! ## Frozen type vocabulary
//!
//! The type sets below are frozen at protocol v1. A new type value cannot be
//! added compatibly: a peer that does not know a type cannot even skip its
//! wire bytes, so any addition is a hard protocol fork. Future semantic types
//! must instead be encoded over `Bytes` or `Text` — old peers parse, store,
//! and re-sync them as opaque LWW values, which is exactly the correct
//! behavior for data they do not understand.

use crate::col_type::ColType;
use bitfield_struct::bitfield;

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
    pub const fn new(pk_types: &[ColType], index: u16) -> Self {
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

    pub fn pk_col_name(&self, i: usize) -> String {
        assert!(i < self.pk_count(), "PK column index out of range");
        format!("k{i}")
    }

    pub fn pk_col_name_list(&self) -> String {
        (0..self.pk_count())
            .map(|i| self.pk_col_name(i))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Type of PK column `i` (0-based). Panics if `i >= pk_count()`.
    pub const fn pk_col_type(&self, i: usize) -> ColType {
        assert!(i < self.pk_count(), "PK column index out of range");
        ColType::from_bits(((self.0 >> (12 - 2 * i)) & 0b11) as u8)
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
/// High 2 bits encode the column's wire type, making the ID self-describing
/// for wire parsing. The lower 6 bits are an arbitrary column index within
/// the table. All non-PK columns are implicitly nullable.
#[bitfield(u8)]
#[derive(PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ColumnId {
    // -- Column index (low 6 bits) --
    /// Arbitrary column index within the table.
    #[bits(6)]
    pub index: u8,

    // -- Type bits (high 2 bits) --
    /// Column wire type (bits 6–7).
    #[bits(2)]
    pub col_type: ColType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_id_round_trip_all_pk_counts() {
        // Goal: a table ID survives a raw u16 round trip for every PK shape.
        // Given: one table ID per PK count, with mixed PK column types.
        let shapes: [&[ColType]; 4] = [
            &[ColType::Uuid],
            &[ColType::Bytes, ColType::Uuid],
            &[ColType::Text, ColType::I64, ColType::Uuid],
            &[ColType::Bytes, ColType::Text, ColType::I64, ColType::Uuid],
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
        let id = TableId::new(&[ColType::Bytes], 1);
        // Then: the raw value is exactly the index.
        assert_eq!(id.raw(), 1);
    }

    #[test]
    fn table_id_pk_shape_in_high_bits() {
        // Goal: PK shape packs into the high bits in declaration order.
        // Given: 1-col UUID PK: count=0, t1=Uuid(11).
        let id = TableId::new(&[ColType::Uuid], 0);
        // Then: count in bits 15-14, t1 in bits 13-12.
        assert_eq!(id.raw() >> 12, 0b00_11);

        // Given: 2-col (Text, I64) PK: count=1, t1=Text(01), t2=I64(10).
        let id = TableId::new(&[ColType::Text, ColType::I64], 0);
        // Then: high 6 bits are count(01) | t1(01) | t2(10).
        assert_eq!(id.raw() >> 10, 0b01_01_10);
    }

    #[test]
    fn table_id_index_headroom_per_pk_count() {
        // Goal: the index width shrinks by 2 bits per extra PK column.
        // Given/When: the max index for each PK count.
        // Then: 12, 10, 8, 6 bits respectively, all constructible.
        let one = TableId::new(&[ColType::Uuid], 4095);
        assert_eq!((one.index_bits(), one.index()), (12, 4095));
        let two = TableId::new(&[ColType::Uuid; 2], 1023);
        assert_eq!((two.index_bits(), two.index()), (10, 1023));
        let three = TableId::new(&[ColType::Uuid; 3], 255);
        assert_eq!((three.index_bits(), three.index()), (8, 255));
        let four = TableId::new(&[ColType::Uuid; 4], 63);
        assert_eq!((four.index_bits(), four.index()), (6, 63));
    }

    #[test]
    #[should_panic(expected = "table index exceeds available bits")]
    fn table_id_rejects_index_overflow() {
        // Goal: an index too wide for the PK count is a construction error,
        // not a silent truncation that would collide with another table.
        // Given: a 1-PK table whose index needs 13 bits.
        // When: constructing it. Then: panic.
        let _ = TableId::new(&[ColType::Uuid], 4096);
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
    fn column_id_round_trip() {
        // Goal: a column ID survives a raw u8 round trip.
        // Given: a Text column at index 3.
        let id = ColumnId::new().with_index(3).with_col_type(ColType::Text);

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
        let id = ColumnId::new().with_index(1).with_col_type(ColType::Bytes);
        let raw: u8 = id.into();
        assert_eq!(raw, 1);
    }

    #[test]
    fn column_id_type_in_high_bits() {
        // I64: bits 6-7=col_type(2)=10, index=0
        let id = ColumnId::new().with_index(0).with_col_type(ColType::I64);
        let raw: u8 = id.into();
        assert_eq!(raw >> 6, 0b10);

        // Text: bits 6-7=col_type(1)=01, index=0
        let id = ColumnId::new().with_index(0).with_col_type(ColType::Text);
        let raw: u8 = id.into();
        assert_eq!(raw >> 6, 0b01);
    }

    #[test]
    fn every_byte_is_a_valid_column_id() {
        // Goal: with a 2-bit type field every value is assigned, so there is no
        // invalid column type — every byte decodes and re-encodes to itself.
        for raw in 0..=u8::MAX {
            let id = ColumnId::from(raw);
            assert_eq!(u8::from(id), raw);
        }
    }

    #[test]
    fn table_id_const_constructible() {
        // Goal: table IDs work as compile-time constants for def macros.
        const SETTINGS: TableId = TableId::new(&[ColType::Text], 7);
        assert_eq!(SETTINGS.pk_count(), 1);
        assert_eq!(SETTINGS.pk_col_type(0), ColType::Text);
        assert_eq!(SETTINGS.index(), 7);
    }

    #[test]
    fn pk_names_are_positional() {
        // Goal: PK column names are `k0..k{n-1}` in declaration order, and the
        // comma-joined list matches.
        let single = TableId::new(&[ColType::Uuid], 1);
        assert_eq!(single.pk_col_name(0), "k0");
        assert_eq!(single.pk_col_name_list(), "k0");

        let composite = TableId::new(&[ColType::Text, ColType::I64, ColType::Uuid], 1);
        assert_eq!(composite.pk_col_name(0), "k0");
        assert_eq!(composite.pk_col_name(2), "k2");
        assert_eq!(composite.pk_col_name_list(), "k0, k1, k2");
    }

    #[test]
    #[should_panic(expected = "PK column index out of range")]
    fn pk_name_rejects_out_of_range() {
        // Goal: asking for a PK column past the table's PK count panics rather
        // than fabricating a name.
        let id = TableId::new(&[ColType::Uuid], 1);
        let _ = id.pk_col_name(1);
    }
}
