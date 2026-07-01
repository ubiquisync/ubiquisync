use crate::id::{ColumnId, TableId};
use crate::op::Value;

#[derive(Debug, Clone)]
pub enum ChangeEvent {
    Upsert(UpsertEvent),
    Delete(DeleteEvent),
}

#[derive(Clone, Eq, PartialEq, Hash)]
pub enum WatchTarget {
    Table(TableId),
    TableRow(TableId, Vec<Value>),
}

#[derive(Debug, Clone)]
pub struct UpsertEvent {
    pub table_id: TableId,
    pub table_name: String,
    pub primary_key: Vec<Value>,
    pub changed_columns: Vec<ColumnValue>,
}

#[derive(Debug, Clone)]
pub struct ColumnValue {
    pub column_id: ColumnId,
    pub name: String,
    pub value: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct DeleteEvent {
    pub table_id: TableId,
    pub table_name: String,
    pub primary_key: Vec<Value>,
}
