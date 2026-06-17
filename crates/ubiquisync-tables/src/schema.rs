use crate::id::{ColumnId, TableId};

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub id: TableId,
    pub name: &'static str,
    pub pk_names: &'static [&'static str],
    pub cols: &'static [ColumnSchema],
}

#[derive(Debug, Clone, Copy)]
pub struct ColumnSchema {
    pub name: &'static str,
    pub id: ColumnId,
}
