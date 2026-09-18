use ubiquisync_core::hlc::Timestamp;

use crate::{def_state, format::NumberFormat, formula::Value, state::lww::Lww};

pub struct SheetData {}

pub struct CellData {
    pub value: Lww<Value>,
    pub number_format: NumberFormat,
}

def_state!(AxisMeta {
    sort: Lww<Vec<u8>>,
    deleted: Lww<bool>,
    size: Lww<u16>,
    default_number_format: NumberFormat,
});
