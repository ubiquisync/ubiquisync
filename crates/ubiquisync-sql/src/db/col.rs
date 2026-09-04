use sea_query::{DynIden, Iden};
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
    fn to_db_val(value: Self) -> DbValue;
}

pub trait Cols {
    type Row<'a>;
    type Params;
    fn encode(row: Self::Params) -> Vec<DbValue>;
    fn decode<'a>(row: &'a DbRow) -> Result<Self::Row<'a>, DbError>;
    fn idens() -> Vec<DynIden>;
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

    fn to_db_val(value: u64) -> DbValue {
        DbValue::Integer(value as i64)
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

    fn to_db_val(value: i64) -> DbValue {
        DbValue::Integer(value)
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

    fn to_db_val(value: String) -> DbValue {
        DbValue::Text(value)
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

    fn to_db_val(value: Vec<u8>) -> DbValue {
        DbValue::Blob(value)
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

    fn to_db_val(value: Uuid) -> DbValue {
        DbValue::Uuid(value)
    }
}

pub trait ColRepr: Sized {
    type Repr: ColType;
    fn to_repr(self) -> Self::Repr;
    fn from_repr<'a>(value: <Self::Repr as ColType>::BorrowedType<'a>) -> Result<Self, DbError>;
}

impl<T: ColRepr> ColType for T {
    type BorrowedType<'a> = T;

    fn create_col_def(name: &str) -> CreateColDef {
        T::Repr::create_col_def(name)
    }

    fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
        let x = T::Repr::from_db_val(value)?;
        Self::from_repr(x)
    }

    fn to_db_val(value: Self) -> DbValue {
        T::Repr::to_db_val(value.to_repr())
    }
}

#[macro_export]
macro_rules! codeable_col_repr {
    ($typ:ty) => {
        impl ColRepr for $typ {
            type Repr = Vec<u8>;
            fn to_repr(self) -> Self::Repr {
                let mut w = ubiquisync_core::codec::Writer::new();
                self.encode(&mut w);
                w.finalize()
            }
            fn from_repr<'a>(
                value: <Self::Repr as $crate::db::ColType>::BorrowedType<'a>,
            ) -> Result<Self, $crate::db::DbError> {
                let mut r = ubiquisync_core::codec::Reader::new(value);
                if let Ok(res) = Self::decode(&mut r) {
                    if r.is_empty() {
                        return Ok(res);
                    }
                }
                Err($crate::db::DbError::TypeMismatch {
                    expected: std::any::type_name::<Self>(),
                    actual: Some($crate::db::DbType::Blob),
                })
            }
        }
    };
}

#[macro_export]
macro_rules! enum_col_repr {
    ($typ:ty) => {
        try_from_into_col_repr!($typ, i64);
    };
}

#[macro_export]
macro_rules! try_from_into_col_repr {
    ($typ:ty, $repr:ty) => {
        impl $crate::db::ColRepr for $typ {
            type Repr = $repr;
            fn to_repr(self) -> Self::Repr {
                self.into()
            }
            fn from_repr<'a>(
                value: <Self::Repr as $crate::db::ColType>::BorrowedType<'a>,
            ) -> Result<Self, $crate::db::DbError> {
                Self::try_from(value).map_err(|_| $crate::db::DbError::TypeMismatch {
                    expected: std::any::type_name::<Self>(),
                    actual: Some($crate::db::DbType::Integer),
                })
            }
        }
    };
}

try_from_into_col_repr!(bool, i64);

// impl<T: ColEnum> ColType for T {
//     type BorrowedType<'a> = T;

//     fn create_col_def(name: &str) -> CreateColDef {
//         col(name, DbType::Integer)
//     }

//     fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
//         let x = i64::from_db_val(value)?;
//         let e = Self::try_from(x).map_err(|_| DbError::TypeMismatch {
//             expected: type_name::<T>(),
//             actual: value.db_type(),
//         })?;
//         Ok(e)
//     }

//     fn to_db_val(value: Self) -> DbValue {
//         DbValue::Integer(value.into())
//     }
// }

// impl ColType for [u8; 32] {
//     type BorrowedType<'a> = [u8; 32];

//     fn create_col_def(name: &str) -> CreateColDef {
//         col(name, DbType::Blob)
//     }

//     fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
//         Ok(
//             Self::try_from(<Vec<u8>>::from_db_val(value)?).map_err(|_| DbError::TypeMismatch {
//                 expected: "[u8; 32]",
//                 actual: value.db_type(),
//             })?,
//         )
//     }

//     fn to_db_val(value: Self) -> DbValue {
//         DbValue::Blob(value.to_vec())
//     }
// }

// impl<T: HasCodec> ColType for T {
//     type BorrowedType<'a> = T;

//     fn create_col_def(name: &str) -> CreateColDef {
//         col(name, DbType::Blob)
//     }

//     fn from_db_val<'a>(value: &'a DbValue) -> Result<Self::BorrowedType<'a>, DbError> {
//         let buf = <Vec<u8>>::from_db_val(value)?;
//         let mut r = Reader::new(buf);
//         let res = T::read(&mut r);
//         if let Ok(res) = res {
//             if r.is_empty() {
//                 return Ok(res);
//             }
//         }
//         Err(DbError::TypeMismatch {
//             expected: type_name::<T>(),
//             actual: value.db_type(),
//         })
//     }

//     fn to_db_val(value: Self) -> DbValue {
//         let mut w = Writer::new();
//         value.write(&mut w);
//         DbValue::Blob(w.finalize())
//     }
// }

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

    fn to_db_val(value: Self) -> DbValue {
        match value {
            None => DbValue::Null,
            Some(v) => T::to_db_val(v),
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
    ($($param:ident $idx:tt),+) => {
        impl <$($param: Col,)+> Cols for ($($param,)+) {
            type Row<'a> = ($(<$param::Type as ColType>::BorrowedType<'a>,)+);
            type Params = ($($param::Type,)+);

            fn encode(row: Self::Params) -> Vec<DbValue> {
                vec![$(<$param::Type as ColType>::to_db_val(row.$idx),)+]
            }

            fn decode<'a>(row: &'a DbRow) -> Result<Self::Row<'a>, DbError> {
                Ok(($(row.get_at::<$param::Type>($idx)?,)+))
            }

            fn idens() -> Vec<sea_query::types::DynIden> {
                vec![$(<$param as sea_query::types::IntoIden>::into_iden($param::default()),)+]
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

impl Cols for () {
    type Row<'a> = ();
    type Params = ();

    fn encode(_: Self::Params) -> Vec<DbValue> {
        vec![]
    }

    fn decode<'a>(_: &'a DbRow) -> Result<Self::Row<'a>, DbError> {
        Ok(())
    }

    fn idens() -> Vec<DynIden> {
        vec![]
    }
}
