use ubiquisync_sql::db::DbValue;

use crate::op::Value;

// pub fn lww_col_name(name: &str) -> String {
//     // TODO: is the best naming convention for lww columns?
//     format!("__{}_lww", name)
// }

// pub fn parse_lww_col_name(name: &str) -> Option<String> {
//     name.strip_prefix("__")?
//         .strip_suffix("_lww")
//         .map(|s| s.to_string())
// }

// pub fn value_to_db(pk: &Value) -> DbValue {
//     match pk {
//         Value::Bytes(b) => DbValue::Blob(b.clone()),
//         Value::Text(s) => DbValue::Text(s.clone()),
//         Value::Uuid(u) => DbValue::Uuid(*u),
//         Value::I64(i) => DbValue::Integer(*i),
//     }
// }
