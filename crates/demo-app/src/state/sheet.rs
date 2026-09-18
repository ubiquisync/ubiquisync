use ubiquisync_core::hlc::Timestamp;

use crate::formula::Value;

pub struct SheetData {}

pub struct CellData {
    value: Value,
    ts: Timestamp,
}

pub struct RowMeta {
    sort_order: String,
    delete_ts: Timestamp,
    upsert_ts: Timestamp,
}

pub struct ColMeta {
    sort_order: String,
    delete_ts: Timestamp,
    upsert_ts: Timestamp,
}

pub struct AxisMeta {
    pub sort: Lww<Vec<u8>>,
    pub deleted: Lww<bool>,
}

pub struct Lww<T> {
    pub value: T,
    pub timestamp: Timestamp,
}
