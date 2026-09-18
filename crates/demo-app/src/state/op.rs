use ubiquisync_core::uuid::Uuid;

use crate::{format::NumberFormatOp, formula::Value, state::AxisMetaOp};

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
        meta: Vec<CellMetaOp>,
    },
    SetAxisMeta {
        axis: Axis,
        ids: Vec<Uuid>,
        meta: Vec<AxisMetaOp>,
    },
}

pub enum CellMetaOp {
    NumberFormat(NumberFormatOp),
}

pub enum Axis {
    Row,
    Col,
}
