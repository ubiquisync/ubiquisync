use std::collections::BTreeMap;

use crate::id::{ColumnId, TableId};

/// TableSchema represents a named table in our schema. It will be exposed for
/// user queries as an SQL VIEW with the provided names. Under the hood, data
/// will be stored in a physical table with surrogate names derived from the table
/// and colummn IDs.
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub(crate) id: TableId,
    pub(crate) name: String,
    pub(crate) pk_names: Vec<String>,
    pub(crate) value_cols: BTreeMap<ColumnId, ColumnSchema>,
}

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub id: ColumnId,
}

impl TableSchema {
    pub fn new(
        id: TableId,
        name: String,
        pk_names: Vec<String>,
        non_pk_cols: Vec<ColumnSchema>,
    ) -> Self {
        // One VIEW column name per PK slot.
        assert_eq!(pk_names.len(), id.pk_count(), "pk_names must match PK count");
        let value_cols = non_pk_cols.into_iter().map(|col| (col.id, col)).collect();
        Self {
            id,
            name,
            pk_names,
            value_cols,
        }
    }
}
