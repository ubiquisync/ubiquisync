use crate::id::{ColumnId, TableId};

/// Surrogate SQL table name for an unknown table ID.
pub fn surrogate_table_name(prefix: &str, table_id: TableId) -> String {
    let raw: u16 = table_id.into();
    format!("{prefix}t0x{raw:04X}")
}

/// Surrogate SQL column name for an unknown column ID.
pub fn surrogate_col_name(col_id: ColumnId) -> String {
    let raw: u8 = col_id.into();
    format!("c0x{raw:02X}")
}

/// Surrogate PK column name by position.
pub fn surrogate_pk_name(pos: usize) -> String {
    format!("k{pos}")
}

pub fn parse_surrogate_col_name(name: &str) -> Option<ColumnId> {
    name.strip_prefix("__c0x")
        .and_then(|s| u8::from_str_radix(s, 16).ok())
        .and_then(ColumnId::try_from_raw)
}
