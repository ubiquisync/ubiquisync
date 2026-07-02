use crate::id::{ColumnId, TableId};

impl TableId {
    /// Surrogate SQL table name for this ID: `{prefix}__t0x{raw:04x}`.
    pub fn table_name(&self, prefix: &str) -> String {
        let raw: u16 = self.raw();
        format!("{prefix}__t0x{raw:04x}")
    }

    /// Inverse of [`table_name`](Self::table_name): recovers the ID from a
    /// table name, or `None` if it doesn't match the `{prefix}__t0x…` shape.
    pub fn parse_table_name(prefix: &str, name: &str) -> Option<Self> {
        name.strip_prefix(prefix)
            .and_then(|s| s.strip_prefix("__t0x"))
            .and_then(|s| u16::from_str_radix(s, 16).ok())
            .map(TableId::from_raw)
    }

    /// SQL column name for PK column `i` (0-based): `k0`, `k1`, … Panics if
    /// `i >= pk_count()`.
    pub fn pk_col_name(&self, i: usize) -> String {
        assert!(i < self.pk_count(), "PK column index out of range");
        format!("k{i}")
    }

    /// Comma-separated list of this table's PK column names, e.g. `k0, k1` —
    /// for splicing into SQL key clauses.
    pub fn pk_col_name_list(&self) -> String {
        (0..self.pk_count())
            .map(|i| self.pk_col_name(i))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl ColumnId {
    /// Surrogate SQL column name for this ID: `c0x{raw:02x}` (the byte encodes
    /// both type and index).
    pub fn col_name(&self) -> String {
        let raw: u8 = self.into_bits();
        format!("c0x{raw:02x}")
    }

    /// Name of this column's companion LWW-timestamp column: `{col_name}_lww`.
    pub fn lww_col_name(&self) -> String {
        format!("{}_lww", self.col_name())
    }

    /// Inverse of [`col_name`](Self::col_name), or `None` if `name` isn't a
    /// `c0x…` value-column name (e.g. a PK, ts, or `_lww` column).
    pub fn parse_col_name(name: &str) -> Option<Self> {
        name.strip_prefix("c0x")
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .map(ColumnId::from_bits)
    }

    /// Recovers the value-column ID from its LWW column name, or `None` if
    /// `lww_name` isn't a `c0x…_lww` name.
    pub fn parse_lww_col_name(lww_name: &str) -> Option<Self> {
        lww_name.strip_suffix("_lww").and_then(Self::parse_col_name)
    }
}

#[cfg(test)]
mod tests {
    use crate::col_type::ColType;

    use super::*;

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
