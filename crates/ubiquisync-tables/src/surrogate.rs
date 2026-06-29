use crate::id::{ColumnId, TableId};

impl TableId {
    pub fn name(&self, prefix: &str) -> String {
        let raw: u16 = self.into();
        format!("{prefix}t0x{raw:04X}")
    }

    pub fn parse(prefix: &str, name: &str) -> Option<Self> {
        name.strip_prefix(prefix)
            .and_then(|s| s.strip_prefix("t0x"))
            .and_then(TableId::try_from_raw)
    }
}

impl ColumnId {
    pub fn name(&self) -> String {
        let raw: u8 = self.into();
        format!("c0x{raw:02X}")
    }

    pub fn lww_name(&self) -> String {
        format!("{}_lww", self.name())
    }

    pub fn parse(name: &str) -> Option<Self> {
        name.strip_prefix("c0x")
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .and_then(ColumnId::try_from_raw)
    }

    pub fn parse_lww(lww_name: &str) -> Option<Self> {
        lww_name.strip_suffix("_lww").and_then(|s| Self::parse(s))
    }
}

// /// Surrogate SQL table name for an unknown table ID.
// pub fn surrogate_table_name(prefix: &str, table_id: TableId) -> String {
//     let raw: u16 = table_id.into();
//     format!("{prefix}t0x{raw:04X}")
// }

// pub fn parse_surrogate_table_name(prefix: &str, name: &str) -> Option<TableId> {
//     name.strip_prefix(prefix)
//         .and_then(|s| s.strip_prefix("t0x"))
//         .and_then(TableId::try_from_raw)
// }

// /// Surrogate SQL column name for an unknown column ID.
// pub fn surrogate_col_name(col_id: ColumnId) -> String {
//     let raw: u8 = col_id.into();
//     format!("c0x{raw:02X}")
// }

// /// Surrogate PK column name by position.
// pub fn surrogate_pk_name(pos: usize) -> String {
//     format!("k{pos}")
// }

// pub fn parse_surrogate_col_name(name: &str) -> Option<ColumnId> {}
