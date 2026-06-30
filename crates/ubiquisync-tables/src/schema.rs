use std::collections::HashMap;

use crate::id::{ColumnId, TableId};

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub(crate) id: TableId,
    pub(crate) name: String,
    pub(crate) pk_names: Vec<String>,
    pub(crate) value_cols: HashMap<ColumnId, ColumnSchema>,
}

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub id: ColumnId,
    pub lww_name: String,
}
