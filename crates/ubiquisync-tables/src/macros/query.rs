//! Query-side codegen for [`define_tables!`](crate::define_tables): a typed
//! [`sea_query`](crate::macros::support::sea_query) `Table`/`Col` identifier pair
//! plus a typed `Row` and `get`/`get_all` readers over the table's user-facing
//! SQL VIEW.
//!
//! sea-query is a pure SQL *builder* — it turns typed columns into
//! dialect-correct `(sql, params)`, but does not map result rows back. So the
//! macro emits both halves: the `Table`/`Col` idens (what sea-query needs) and
//! the `Row`/`from_row` extraction (what it doesn't). Reads run through any
//! [`SqlStore`](crate::macros::support::SqlStore) — the readers are generic over
//! its `Op`/`Event`, so they work equally against this crate's own vocabulary or
//! an app that has bundled it into a wider one; only `query`/`dialect` are used.
//!
//! Invoked by [`define_table!`](crate::define_table); not called directly.

/// Generate the query surface for one table into its schema module.
#[doc(hidden)]
#[macro_export]
macro_rules! __define_table_query {
    (
        $mod:ident,
        ( $($pk_name:ident $pk_type:ident),+ ),
        { $( $col_name:ident $col_type:ident ),* }
    ) => {
        /// Zero-sized table identifier — renders this table's VIEW name for
        /// sea-query's `FROM`.
        #[allow(dead_code, missing_docs)]
        #[derive(Clone, Copy)]
        pub struct Table;

        impl $crate::macros::support::sea_query::Iden for Table {
            fn unquoted(&self) -> &str {
                stringify!($mod)
            }
        }

        // Typed column identifiers for sea-query. Variants are the exact PK and
        // column idents (hence the lowercase allow); `unquoted` returns each name
        // straight from the DSL — no case conversion, so a `#[derive(Iden)]`
        // (which would snake_case) isn't usable here.
        #[allow(non_camel_case_types, dead_code, missing_docs)]
        #[derive(Clone, Copy)]
        pub enum Col { $($pk_name,)+ $($col_name,)* }

        impl $crate::macros::support::sea_query::Iden for Col {
            fn unquoted(&self) -> &str {
                match self {
                    $(Col::$pk_name => stringify!($pk_name),)+
                    $(Col::$col_name => stringify!($col_name),)*
                }
            }
        }

        /// A typed row of this table: primary-key columns (always present)
        /// followed by the value columns (each nullable).
        #[allow(dead_code, missing_docs)]
        #[derive(Debug, Clone)]
        pub struct Row {
            $(pub $pk_name: $crate::__q_pk_ty!($pk_type),)+
            $(pub $col_name: $crate::__q_col_ty!($col_type),)*
        }

        impl Row {
            /// Extract a row from a [`DbRow`](crate::macros::support::DbRow),
            /// positionally: PK columns first, then value columns in declaration
            /// order — the same order `Col` is selected in.
            #[allow(dead_code, unused_assignments)]
            fn from_row(
                row: &$crate::macros::support::DbRow,
            ) -> ::core::result::Result<Self, $crate::macros::support::DbError> {
                #[allow(unused_mut, unused_variables)]
                let mut idx: usize = 0;
                $(let $pk_name = { let v = $crate::__q_pk_get!(row, idx, $pk_type); idx += 1; v };)+
                $(let $col_name = { let v = $crate::__q_col_get!(row, idx, $col_type); idx += 1; v };)*
                ::core::result::Result::Ok(Row { $($pk_name,)+ $($col_name,)* })
            }
        }

        /// Fetch the row with this primary key, or `None` if it doesn't exist.
        #[allow(dead_code)]
        pub async fn get<O, E, S>(
            store: &S,
            $($pk_name: $crate::__q_pk_arg!($pk_type),)+
        ) -> ::core::result::Result<
            ::core::option::Option<Row>,
            $crate::macros::support::DbError,
        >
        where
            S: $crate::macros::support::SqlStore<O, E> + ?Sized,
            E: $crate::macros::support::RoutableEvent,
        {
            // Brings the SQL `.eq` (and friends) into scope; without it method
            // resolution finds `PartialEq::eq` and yields a `bool`.
            use $crate::macros::support::sea_query::ExprTrait as _;
            let mut stmt = $crate::macros::support::sea_query::Query::select();
            stmt.columns([$(Col::$pk_name,)+ $(Col::$col_name,)*]).from(Table);
            $(stmt.and_where(
                $crate::macros::support::sea_query::Expr::col(Col::$pk_name)
                    .eq($crate::__q_pk_bind!($pk_name, $pk_type)),
            );)+
            let (sql, params) = $crate::macros::support::build_select(&stmt, store.dialect())?;
            let rows = store.query(&sql, &params).await?;
            match rows.first() {
                ::core::option::Option::Some(row) =>
                    ::core::result::Result::Ok(::core::option::Option::Some(Row::from_row(row)?)),
                ::core::option::Option::None =>
                    ::core::result::Result::Ok(::core::option::Option::None),
            }
        }

        /// Fetch every live (non-tombstoned) row of the table. The VIEW already
        /// hides tombstones, so no delete filter is needed here.
        #[allow(dead_code)]
        pub async fn get_all<O, E, S>(
            store: &S,
        ) -> ::core::result::Result<::std::vec::Vec<Row>, $crate::macros::support::DbError>
        where
            S: $crate::macros::support::SqlStore<O, E> + ?Sized,
            E: $crate::macros::support::RoutableEvent,
        {
            let mut stmt = $crate::macros::support::sea_query::Query::select();
            stmt.columns([$(Col::$pk_name,)+ $(Col::$col_name,)*]).from(Table);
            let (sql, params) = $crate::macros::support::build_select(&stmt, store.dialect())?;
            let rows = store.query(&sql, &params).await?;
            rows.iter().map(Row::from_row).collect()
        }

        /// Run a filtered/ordered query and map each result into a typed [`Row`].
        ///
        /// `compose` shapes the `WHERE`/`ORDER BY`/`LIMIT`/`OFFSET` on the
        /// statement using [`sea_query`](crate::sea_query); the projection is
        /// fixed to this table's columns first, so don't call `.columns()` in
        /// `compose` (it would break the positional [`Row`] mapping).
        ///
        /// ```ignore
        /// use ubiquisync_tables::sea_query::{Expr, ExprTrait, Order};
        /// let rows = notes::query(&store, |q| {
        ///     q.and_where(Expr::col(notes::Col::n).gte(10))
        ///      .order_by(notes::Col::n, Order::Desc)
        ///      .limit(20);
        /// }).await?;
        /// ```
        #[allow(dead_code)]
        pub async fn query<O, E, S, F>(
            store: &S,
            compose: F,
        ) -> ::core::result::Result<::std::vec::Vec<Row>, $crate::macros::support::DbError>
        where
            S: $crate::macros::support::SqlStore<O, E> + ?Sized,
            E: $crate::macros::support::RoutableEvent,
            F: ::core::ops::FnOnce(&mut $crate::macros::support::sea_query::SelectStatement),
        {
            let mut stmt = $crate::macros::support::sea_query::Query::select();
            stmt.from(Table).columns([$(Col::$pk_name,)+ $(Col::$col_name,)*]);
            compose(&mut stmt);
            let (sql, params) = $crate::macros::support::build_select(&stmt, store.dialect())?;
            let rows = store.query(&sql, &params).await?;
            rows.iter().map(Row::from_row).collect()
        }
    };
}

/// Rust field type for a non-null PK column.
#[doc(hidden)]
#[macro_export]
macro_rules! __q_pk_ty {
    (Uuid) => { $crate::macros::support::CoreUuid };
    (Text) => { ::std::string::String };
    (I64) => { i64 };
    (Bytes) => { ::std::vec::Vec<u8> };
}

/// Rust field type for a nullable value column (all value columns are nullable).
#[doc(hidden)]
#[macro_export]
macro_rules! __q_col_ty {
    (Uuid) => { ::core::option::Option<$crate::macros::support::CoreUuid> };
    (Text) => { ::core::option::Option<::std::string::String> };
    (I64) => { ::core::option::Option<i64> };
    (Bytes) => { ::core::option::Option<::std::vec::Vec<u8>> };
}

/// Extract a non-null PK column from a `DbRow` at `idx`.
#[doc(hidden)]
#[macro_export]
macro_rules! __q_pk_get {
    ($row:expr, $idx:expr, Uuid) => { $row.get_uuid($idx)? };
    ($row:expr, $idx:expr, Text) => { $row.get_text($idx)?.to_string() };
    ($row:expr, $idx:expr, I64) => { $row.get_i64($idx)? };
    ($row:expr, $idx:expr, Bytes) => { $row.get_blob($idx)?.to_vec() };
}

/// Extract a nullable value column from a `DbRow` at `idx`.
#[doc(hidden)]
#[macro_export]
macro_rules! __q_col_get {
    ($row:expr, $idx:expr, Uuid) => { $row.get_optional_uuid($idx)? };
    ($row:expr, $idx:expr, Text) => { $row.get_optional_text($idx)?.map(|s| s.to_string()) };
    ($row:expr, $idx:expr, I64) => { $row.get_optional_i64($idx)? };
    ($row:expr, $idx:expr, Bytes) => { $row.get_optional_blob($idx)?.map(|b| b.to_vec()) };
}

/// Rust argument type accepting a PK value in a reader.
#[doc(hidden)]
#[macro_export]
macro_rules! __q_pk_arg {
    (Uuid) => { &$crate::macros::support::CoreUuid };
    (Text) => { &str };
    (I64) => { i64 };
    (Bytes) => { &[u8] };
}

/// Convert a PK argument into the sea-query [`Value`] bound into the WHERE
/// clause. UUIDs bridge our raw-bytes `[u8; 16]` to `uuid::Uuid` so sea-query
/// carries them as `Value::Uuid` — which maps back to `DbValue::Uuid`, binding as
/// a BLOB on SQLite and a native `UUID` on Postgres, matching the reducer's
/// writes.
#[doc(hidden)]
#[macro_export]
macro_rules! __q_pk_bind {
    ($v:expr, Uuid) => {
        $crate::macros::support::sea_query::Value::Uuid(::core::option::Option::Some(
            $crate::macros::support::uuid::Uuid::from_bytes(*$v),
        ))
    };
    ($v:expr, Text) => {
        $crate::macros::support::sea_query::Value::String(::core::option::Option::Some($v.to_owned()))
    };
    ($v:expr, I64) => {
        $crate::macros::support::sea_query::Value::BigInt(::core::option::Option::Some($v))
    };
    ($v:expr, Bytes) => {
        $crate::macros::support::sea_query::Value::Bytes(::core::option::Option::Some($v.to_vec()))
    };
}
