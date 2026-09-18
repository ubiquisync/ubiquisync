use ubiquisync_core::uuid::Uuid;

pub enum Value {
    Value(String),
    Formula(Expr),
}

pub enum Expr {
    Num(String),
    Str(String),
    Range {
        sheet: Option<Uuid>,
        range: RangeExpr,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
    Binary {},
}

pub enum RangeExpr {
    /// Single cell, ex. A1
    Cell(CellRef),
    /// Rectangle, ex. A1:C3
    Rect { start: CellRef, end: CellRef },
    /// Full column, ex. A:A
    FullCol(CoordRef),
    /// Full columns, ex. A:C
    FullCols { start: CoordRef, end: CoordRef },
    /// Full row, ex. 3:3
    FullRow(CoordRef),
    /// Full columns, ex. 3:5
    FullCols { start: CoordRef, end: CoordRef },
    /// Start at cell and go all the way to the end of the row, ex. C3:3
    OpenEndedRow(CellRef),
    /// Start and cell and go all the way to the end of specified row, ex. C3:5
    OpenEndedRowRect { start: CellRef, end_row: CoordRef },
    /// Start at cell and go all the way to the end of the col, ex. C3:C
    OpenEndedCol(CellRef),
    /// Start at cell and go all the way to the end of specified col, ex. C3:E
    OpenEndedColRect { start: CellRef, end_col: CoordRef },
}

pub struct CellRef {
    pub row: CoordRef,
    pub col: CoordRef,
}

pub struct CoordRef {
    pub coord: Uuid,
    pub locked: bool,
}

pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Eq,
    Lt,
    Lte,
    Gt,
    Gte,
    NotEq,
}

pub enum UnaryOp {}
