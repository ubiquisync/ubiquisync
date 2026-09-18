use std::collections::HashMap;

use ubiquisync_core::uuid::Uuid;

use crate::{def_state, format::NumberFormat, formula::Value, state::lww::Lww};

pub struct SheetData {
    pub row_data: Vec<RowData>,
    pub col_meta: Vec<ColMeta>,

    pub rows_by_uuid: HashMap<Uuid, usize>,
    pub cols_by_uuid: HashMap<Uuid, usize>,

    // visible order -> index order
    pub row_order_to_idx: Vec<usize>,
    pub col_order_to_idx: Vec<usize>,
}

#[derive(Default)]
pub struct RowData {
    cells: HashMap<usize, CellData>,
    meta: AxisMeta,
    // cached visible order (or hidden/deleted)
    visual_order: Option<usize>,
}

#[derive(Default)]
pub struct ColMeta {
    meta: AxisMeta,
    // cached visible order (or hidden/deleted)
    visual_order: Option<usize>,
}

#[derive(Default)]
pub struct CellData {
    pub value: Lww<Value>,
    pub meta: CellMeta,
}

def_state!(CellMeta {
    number_format: NumberFormat,
});

def_state!(AxisMeta {
    sort: Lww<Vec<u8>>,
    deleted: Lww<bool>,
    size: Lww<u16>,
    default_number_format: NumberFormat,
});
