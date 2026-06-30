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
