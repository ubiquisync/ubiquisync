use crate::db::DbValue;
use crate::op::Value;

/// Double-quote a SQL identifier to safely handle reserved keywords.
/// `quote_ident("name")` → `"name"`, `quote_ident("select")` → `"select"`
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn lww_col_name(name: &str) -> String {
    // TODO: is the best naming convention for lww columns
    format!("__{}_lww", name)
}

pub fn parse_lww_col_name(name: &str) -> Option<String> {
    name.strip_prefix("__").strip_suffix("_lww").map(|s| s.to_string())
}

pub fn value_to_db(pk: &Value) -> DbValue {
    match pk {
        Value::Bytes(b) => DbValue::Blob(b.clone()),
        Value::Text(s) => DbValue::Text(s.clone()),
        Value::Uuid(u) => DbValue::Uuid(u),
        Value::I64(i) => DbValue::Integer(*i),
    }
}
