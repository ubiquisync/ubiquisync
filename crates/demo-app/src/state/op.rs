use ubiquisync_core::uuid::Uuid;

use crate::formula::Value;

pub enum Op {
    Workbook { workbook: Uuid, op: WorkbookOp },
}

pub enum WorkbookOp {
    SetName(String),
    Sheet { sheet: Uuid, op: SheetOp },
}

pub enum SheetOp {
    SetName(String),
    SetCellData {
        rows: Vec<Uuid>,
        cols: Vec<Uuid>,
        /// Cell values with a flat layout with each row laid out continguously.
        values: Vec<Value>,
    },
    SetCellMeta {
        rows: Vec<Uuid>,
        cols: Vec<Uuid>,
        meta: Vec<CellMeta>,
    },
    SetAxisMeta {
        axis: Axis,
        ids: Vec<Uuid>,
        meta: Vec<AxisMeta>,
    },
}

pub enum CellMeta {}

pub enum AxisMeta {
    Live(bool),
    Sort(Vec<u8>),
}

pub enum Axis {
    Row,
    Col,
}
