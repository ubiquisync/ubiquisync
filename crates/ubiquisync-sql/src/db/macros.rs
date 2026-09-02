#[macro_export]
macro_rules! def_table {
    ($name:ident ( $($pk_name:ident $pk_typ:ty),+ ) => { $($col_name:ident $col_typ:ty),* }) => {
        pastey::paste! {
            pub mod $name {
                #[derive(sea_query::Iden, Clone, Copy)]
                pub struct TableName;

                pub fn create_table_def() -> $crate::db::CreateTableDef {
                    $crate::db::table(
                        stringify!($name),
                        &[ $(< [<$pk_name:camel>] as $crate::db::Col>::create_col_def(),)+ ],
                        &[ $(< [<$col_name:camel>] as $crate::db::Col>::create_col_def(),)* ],
                    )
                }

                $( $crate::def_col!($pk_name $pk_typ); )+
                $( $crate::def_col!($col_name $col_typ); )*

            }
        }
    };
}

#[macro_export]
macro_rules! def_table_with_auto_id {
    ($name:ident ($id_col:ident) => { $($col_name:ident $col_typ:ty),* }) => {
        pastey::paste! {
            pub mod $name {
                #[derive(sea_query::Iden, Clone, Copy)]
                pub struct TableName;

                pub fn create_table_def() -> $crate::db::CreateTableDef {
                    $crate::db::table_with_auto_id(
                        stringify!($name),
                        stringify!($id_col),
                        &[ $(< [< $col_name:camel >] as $crate::db::Col>::create_col_def(),)* ],
                    )
                }

                $crate::def_col!($id_col i64);
                $( $crate::def_col!($col_name $col_typ); )*
            }
        }
    };
}

#[macro_export]
macro_rules! def_col {
    ($name:ident $typ:ty $(: $modifier:ident $args:tt )? ) => {
        pastey::paste! {
            // TODO we could re-export sea_query::Iden
            #[derive(sea_query::Iden, Clone, Copy, Default)]
            pub struct [< $name:camel >];
            impl $crate::db::Col for [< $name:camel >] {
                type Type = $typ;
                fn create_col_def() -> $crate::db::CreateColDef {
                    <$typ as $crate::db::ColType>::create_col_def(stringify!($name)) $(.$modifier $args)?
                }
            }
        }
    };
}
