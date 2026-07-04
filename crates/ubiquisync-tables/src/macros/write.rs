//! Write-side codegen for [`define_tables!`](crate::define_tables): typed
//! `upsert`/`delete` builders that produce [`Op`](crate::op::Op) values.
//!
//! The write vocabulary is deliberately just upsert + delete (the conflict-free
//! merge primitives) — these builders only *construct* an `Op`; callers apply it
//! with `Store::exec`. Column setters take `Option<T>`: `Some(v)` writes the
//! value, `None` writes SQL NULL; columns left unset merge last-writer-wins.
//!
//! Invoked by [`define_table!`](crate::define_table); not called directly.

/// Generate the `upsert`/`delete` op builders for one table.
#[doc(hidden)]
#[macro_export]
macro_rules! __define_table_write {
    (
        $mod:ident,
        ( $($pk_name:ident $pk_type:ident),+ ),
        { $( ($col_idx:literal $col_name:ident $col_type:ident) ),* }
    ) => {
        /// Begin an upsert of the row with this primary key. Chain typed column
        /// setters, then [`build`](UpsertBuilder::build) into an
        /// [`Op`](crate::op::Op).
        #[allow(dead_code)]
        pub fn upsert($($pk_name: $crate::__q_pk_arg!($pk_type),)+) -> UpsertBuilder {
            UpsertBuilder {
                primary_key: ::std::vec![ $( $crate::__w_pk_val!($pk_name, $pk_type) ),+ ],
                sets: ::std::vec::Vec::new(),
                nulls: ::std::vec::Vec::new(),
            }
        }

        /// Accumulates column writes for an [`upsert`]. Each setter takes an
        /// `Option`: `Some` writes the value, `None` writes SQL NULL. Columns
        /// never set are left to merge last-writer-wins.
        #[allow(dead_code, missing_docs)]
        pub struct UpsertBuilder {
            primary_key: ::std::vec::Vec<$crate::op::Value>,
            sets: ::std::vec::Vec<$crate::op::ColumnSet>,
            nulls: ::std::vec::Vec<$crate::id::ColumnId>,
        }

        #[allow(dead_code)]
        impl UpsertBuilder {
            $( $crate::__w_setter!($col_idx, $col_name, $col_type); )*

            /// Finish into an [`Op`](crate::op::Op) ready for `Store::exec`.
            pub fn build(self) -> $crate::op::Op {
                $crate::op::Op::Upsert($crate::op::Upsert {
                    table_id: TABLE_ID,
                    primary_key: self.primary_key,
                    sets: self.sets,
                    nulls: self.nulls,
                })
            }
        }

        /// Build a delete (soft-tombstone) op for the row with this primary key.
        #[allow(dead_code)]
        pub fn delete($($pk_name: $crate::__q_pk_arg!($pk_type),)+) -> $crate::op::Op {
            $crate::op::Op::Delete($crate::op::Delete {
                table_id: TABLE_ID,
                primary_key: ::std::vec![ $( $crate::__w_pk_val!($pk_name, $pk_type) ),+ ],
            })
        }
    };
}

/// Convert a PK argument to an [`op::Value`](crate::op::Value).
#[doc(hidden)]
#[macro_export]
macro_rules! __w_pk_val {
    ($v:expr, Uuid) => { $crate::op::Value::Uuid(*$v) };
    ($v:expr, Text) => { $crate::op::Value::Text(::std::string::ToString::to_string($v)) };
    ($v:expr, I64) => { $crate::op::Value::I64($v) };
    ($v:expr, Bytes) => { $crate::op::Value::Bytes($v.to_vec()) };
}

/// Generate a column's two setters on the upsert builder: `<col>(value)` sets it,
/// `<col>_null()` sets it to SQL NULL. A column left unset merges last-writer-wins.
#[doc(hidden)]
#[macro_export]
macro_rules! __w_setter {
    ($idx:literal, $name:ident, $variant:ident) => {
        #[doc = concat!("Set `", stringify!($name), "` to a value.")]
        pub fn $name(mut self, value: $crate::__w_set_arg!($variant)) -> Self {
            $crate::macros::support::push_col(
                &mut self.sets, &mut self.nulls, $idx, $crate::__col_type!($variant),
                ::core::option::Option::Some($crate::__w_set_val!(value, $variant)),
            );
            self
        }

        $crate::macros::support::pastey::paste! {
            #[doc = concat!("Set `", stringify!($name), "` to SQL NULL.")]
            pub fn [< $name _null >](mut self) -> Self {
                $crate::macros::support::push_col(
                    &mut self.sets, &mut self.nulls, $idx, $crate::__col_type!($variant),
                    ::core::option::Option::None,
                );
                self
            }
        }
    };
}

/// Argument type for a value-setting column setter.
#[doc(hidden)]
#[macro_export]
macro_rules! __w_set_arg {
    (Text) => { &str };
    (I64) => { i64 };
    (Uuid) => { &$crate::macros::support::CoreUuid };
    (Bytes) => { &[u8] };
}

/// Convert a setter argument into the `op::Value` written for that column.
#[doc(hidden)]
#[macro_export]
macro_rules! __w_set_val {
    ($v:expr, Text) => { $crate::op::Value::Text(::std::string::ToString::to_string($v)) };
    ($v:expr, I64) => { $crate::op::Value::I64($v) };
    ($v:expr, Uuid) => { $crate::op::Value::Uuid(*$v) };
    ($v:expr, Bytes) => { $crate::op::Value::Bytes($v.to_vec()) };
}
