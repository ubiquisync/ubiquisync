use sea_query::{Iden, SelectStatement};
use ubiquisync_core::uuid::Uuid;

use crate::db::{CreateColDef, DbError, DbRow, DbType, DbValue};

pub trait Col: Iden + Copy + Default {
    type Type: ColType;

    fn create_col_def() -> CreateColDef;
}

pub trait ColType {
    type BorrowedType<'a>;

    fn create_col_def(name: &str) -> CreateColDef;
    fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError>;
}

pub trait Cols {
    type Row<'a>;
    fn add_to_select(stmt: &mut SelectStatement);
    fn decode<'a>(row: &'a DbRow) -> Result<Self::Row<'a>, DbError>;
}

impl ColType for u64 {
    type BorrowedType<'a> = u64;

    fn create_col_def(name: &str) -> CreateColDef {
        col(name, DbType::Integer)
    }

    fn from_db_val(value: &DbValue) -> Result<u64, DbError> {
        match value {
            DbValue::Integer(v) => {
                Ok(u64::try_from(*v).map_err(|_| DbError::IntegerOutOfRange(*v as i128))?)
            }
            _ => Err(DbError::TypeMismatch {
                expected: "u64",
                actual: value.db_type(),
            }),
        }
    }
}

impl ColType for i64 {
    type BorrowedType<'a> = i64;

    fn create_col_def(name: &str) -> CreateColDef {
        col(name, DbType::Integer)
    }

    fn from_db_val(value: &DbValue) -> Result<i64, DbError> {
        match value {
            DbValue::Integer(v) => Ok(*v),
            _ => Err(DbError::TypeMismatch {
                expected: "i64",
                actual: value.db_type(),
            }),
        }
    }
}

impl ColType for String {
    type BorrowedType<'a> = &'a str;

    fn create_col_def(name: &str) -> CreateColDef {
        col(name, DbType::Text)
    }

    fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
        match value {
            DbValue::Text(v) => Ok(v.as_ref()),
            _ => Err(DbError::TypeMismatch {
                expected: "String",
                actual: value.db_type(),
            }),
        }
    }
}

impl ColType for Vec<u8> {
    type BorrowedType<'a> = &'a [u8];

    fn create_col_def(name: &str) -> CreateColDef {
        col(name, DbType::Blob)
    }

    fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
        match value {
            DbValue::Blob(v) => Ok(v.as_ref()),
            _ => Err(DbError::TypeMismatch {
                expected: "Vec<u8>",
                actual: value.db_type(),
            }),
        }
    }
}

impl ColType for Uuid {
    type BorrowedType<'a> = Uuid;

    fn create_col_def(name: &str) -> CreateColDef {
        col(name, DbType::Uuid)
    }

    fn from_db_val(value: &DbValue) -> Result<Uuid, DbError> {
        match value {
            DbValue::Blob(v) => v.as_slice().try_into().map_err(|_| DbError::TypeMismatch {
                actual: value.db_type(),
                expected: "16-byte UUID blob",
            }),
            DbValue::Uuid(v) => Ok(*v),
            _ => Err(DbError::TypeMismatch {
                expected: "Uuid",
                actual: value.db_type(),
            }),
        }
    }
}

impl<T: ColType> ColType for Option<T> {
    type BorrowedType<'a> = Option<T::BorrowedType<'a>>;

    fn create_col_def(name: &str) -> CreateColDef {
        T::create_col_def(name).nullable()
    }

    fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
        match value {
            DbValue::Null => Ok(None),
            v => Ok(Some(<T as ColType>::from_db_val(v)?)),
        }
    }
}

fn col(name: &str, db_type: DbType) -> CreateColDef {
    CreateColDef {
        name: name.to_string(),
        db_type,
        nullable: false,
        default_zero: false,
    }
}

macro_rules! impl_col_tuples {
    ($($param:ident $idx:literal),+) => {
        impl <$($param: Col,)+> Cols for ($($param,)+) {
            type Row<'a> = ($(<$param::Type as ColType>::BorrowedType<'a>,)+);

            fn add_to_select(stmt: &mut SelectStatement) {
                $(stmt.column($param::default());)+
            }

            fn decode<'a>(row: &'a DbRow) -> Result<Self::Row<'a>, DbError> {
                Ok(($(row.get_at::<$param::Type>($idx)?,)+))
            }
        }
    }
}

impl_col_tuples!(A 0);
impl_col_tuples!(A 0, B 1);
impl_col_tuples!(A 0, B 1, C 2);
impl_col_tuples!(A 0, B 1, C 2, D 3);
impl_col_tuples!(A 0, B 1, C 2, D 3, E 4);
impl_col_tuples!(A 0, B 1, C 2, D 3, E 4, F 5);
impl_col_tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
impl_col_tuples!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
