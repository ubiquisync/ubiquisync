use ubiquisync_core::uuid::Uuid;

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
pub enum Value {
    Value(String),
    Formula(Expr),
}

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
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

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
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
    /// Full rows, ex. 3:5
    FullRows { start: CoordRef, end: CoordRef },
}

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct CellRef {
    pub row: CoordRef,
    pub col: CoordRef,
}

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct CoordRef {
    pub coord: Uuid,
    pub locked: bool,
}

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
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

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
pub enum UnaryOp {}

impl Default for Value {
    fn default() -> Self {
        Self::Value("".into())
    }
}
