//! The demo's one synced table, and the names the log views use for it.

use ubiquisync_core::ids::{AppId, ContainerId};
use ubiquisync_tables::{codec::OpView, id::ColumnId, op::Value};

ubiquisync_tables::define_tables! {
    1 todos (id Uuid) => { (0 title Text), (1 done I64), (2 created_at I64) },
}

pub const APP_ID: AppId = AppId(*b"uqsync-explorer!");
pub const CONTAINER_ID: ContainerId = ContainerId(*b"explorer-todos!!");

/// The schema name of a table and column, when the op is for the todos table.
pub fn column_name(view: &OpView, column: ColumnId) -> String {
    if view.table == todos::TABLE_ID
        && let Ok(t) = todos::table()
        && let Some(c) = t.column(column)
    {
        return c.name.clone();
    }
    format!("col{}", column.index())
}

pub fn table_name(view: &OpView) -> String {
    if view.table == todos::TABLE_ID {
        "todos".into()
    } else {
        format!("{:?}", view.table)
    }
}

/// The todo id an op is about, if it is for the todos table.
pub fn todo_id(view: &OpView) -> Option<[u8; 16]> {
    if view.table != todos::TABLE_ID {
        return None;
    }
    match view.key.first() {
        Some(Value::Uuid(u)) => Some(*u),
        _ => None,
    }
}

/// A value as the views show it. `done` shows as yes/no.
pub fn show_value(name: &str, v: &Option<Value>) -> String {
    match (name, v) {
        (_, None) => "null".into(),
        ("done", Some(Value::I64(n))) => if *n != 0 { "yes" } else { "no" }.into(),
        (_, Some(Value::Text(s))) => format!("\"{s}\""),
        (_, Some(Value::I64(n))) => n.to_string(),
        (_, Some(Value::Uuid(u))) => hex::encode(&u[..4]),
        (_, Some(Value::Bytes(b))) => format!("{} bytes", b.len()),
    }
}
