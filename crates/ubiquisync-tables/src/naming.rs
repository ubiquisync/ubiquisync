use crate::id::{ColumnId, TableId};

impl TableId {
    pub fn table_name(&self, prefix: &str) -> String {
        let raw: u16 = self.into();
        format!("{prefix}__t0x{raw:04X}")
    }

    pub fn parse_table_name(prefix: &str, name: &str) -> Option<Self> {
        name.strip_prefix(prefix)
            .and_thn(|s| s.strip_prefix("__t0x"))
            .and_then(TableId::try_from_raw)
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
}

impl ColumnId {
    pub fn col_name(&self) -> String {
        let raw: u8 = self.into();
        format!("c0x{raw:02X}")
    }

    pub fn lww_col_name(&self) -> String {
        format!("{}_lww", self.col_name())
    }

    pub fn parse_col_name(name: &str) -> Option<Self> {
        name.strip_prefix("c0x")
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .and_then(ColumnId::try_from_raw)
    }

    pub fn parse_lww_col_name(lww_name: &str) -> Option<Self> {
        lww_name
            .strip_suffix("_lww")
            .and_then(|s| Self::parse_col_name(s))
    }
}

#[cfg(test)]
mod tests {
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
