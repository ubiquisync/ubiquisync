//! Declarative macros for building table [`schema`](crate::schema)s.
//!
//! [`define_tables!`] expands a compact table DSL into a `tables()` function
//! that returns the [`TableSchema`](crate::schema::TableSchema) values a caller
//! hands to [`Reducer::new`](crate::reducer::Reducer::new). Because the schema's
//! PK names and its table ID both come from the single `pk: (...)` list, the
//! count can never disagree — the one `TableSchema::new` error the macro cannot
//! hit by construction.
//!
//! Alongside the schema, [`define_table!`] emits per table:
//! - a typed **query** surface (a [`sea_query`](crate::macros::support::sea_query)
//!   `Table`/`Col` iden pair, a `Row`, and `get`/`get_all`/`query` readers) — see
//!   the sibling `query` helper-macro module;
//! - a typed **write** surface (`upsert` builder + `delete`, producing
//!   [`Op`](crate::op::Op)s) — see the sibling `write` module;
//! - a typed **watch** surface (an `Event` projected from
//!   [`ChangeEvent`](crate::watch::ChangeEvent) plus `watch`/`watch_row` streams)
//!   — see the sibling `watch` module.
//!
//! End-to-end coverage of all of the above lives in `ubiquisync-sqlite`'s
//! integration tests, which drive a real processor over real SQLite.

// Sibling helper-macro modules expanded by `define_table!`. `#[macro_export]`
// hoists their macros to the crate root, so these just need to be compiled.
mod query;
mod watch;
mod write;

/// Runtime glue and re-exports the generated code expands against. `pub` (but
/// hidden) so `$crate::macros::support::…` resolves in downstream crates.
#[doc(hidden)]
pub mod support;

/// Internal: map a column-type keyword (`Bytes`/`Text`/`I64`/`Uuid`) to a
/// [`ColType`](crate::col_type::ColType). Shared by PK and value columns.
#[doc(hidden)]
#[macro_export]
macro_rules! __col_type {
    (Bytes) => {
        $crate::col_type::ColType::Bytes
    };
    (Text) => {
        $crate::col_type::ColType::Text
    };
    (I64) => {
        $crate::col_type::ColType::I64
    };
    (Uuid) => {
        $crate::col_type::ColType::Uuid
    };
}

/// Define one table's schema constructor: `index name (key type, ...) => { (idx col type) ... }`.
///
/// Generates `pub mod name { pub fn table() -> Result<TableSchema, TablesError> }`.
/// The table `name` is the user-facing VIEW name; the key and column idents are
/// the VIEW's column names. Types are keywords: `Bytes`, `Text`, `I64`, `Uuid`.
///
/// Usually invoked through [`define_tables!`](crate::define_tables) rather than
/// directly.
#[macro_export]
macro_rules! define_table {
    (
        $index:literal $mod:ident
        ( $($pk_name:ident $pk_type:ident),+ $(,)? )
        => { $( ($col_idx:literal $col_name:ident $col_type:ident) ),* $(,)? }
    ) => {
        #[doc = concat!("Schema for the `", stringify!($mod), "` table.")]
        pub mod $mod {
            /// This table's type-encoded [`TableId`](crate::id::TableId) (PK
            /// shape + index), shared by the schema, the op builders, and the
            /// query readers. `const` so `TableId::new`'s shape/index asserts run
            /// at compile time: an out-of-range index or >4 PK columns is a build
            /// error, not a runtime panic.
            #[allow(dead_code)]
            pub const TABLE_ID: $crate::id::TableId = $crate::id::TableId::new(
                &[ $( $crate::__col_type!($pk_type) ),+ ],
                $index,
            );

            /// Build this table's `TableSchema`. The returned `Result` surfaces
            /// only `TableSchema::new` validation errors (e.g. a duplicate
            /// column name); the PK-count check can't fire, since the ID and PK
            /// names share one declaration. A table `index` too large for its PK
            /// count, more than four PK columns, or a column index `>= 64` are
            /// rejected at compile time (const evaluation) — never at runtime.
            pub fn table() -> ::core::result::Result<
                $crate::schema::TableSchema,
                $crate::error::TablesError,
            > {
                $crate::schema::TableSchema::new(
                    TABLE_ID,
                    stringify!($mod).into(),
                    ::std::vec![ $( stringify!($pk_name).into() ),+ ],
                    ::std::vec![ $(
                        $crate::schema::ColumnSchema {
                            name: stringify!($col_name).into(),
                            id: {
                                // `ColumnId`'s index is a 6-bit field, and
                                // `ColumnId::new` truncates rather than checking;
                                // reject an out-of-range index at compile time so
                                // it can't silently collide with another column.
                                const _: () = ::core::assert!(
                                    $col_idx < 64,
                                    "column index must be < 64 (ColumnId uses a 6-bit index field)",
                                );
                                $crate::id::ColumnId::new(
                                    $col_idx,
                                    $crate::__col_type!($col_type),
                                )
                            },
                        }
                    ),* ],
                )
            }

            // Typed query surface (Table/Col idens, Row, get/get_all/query) over
            // this table's VIEW. See the `macros::query` module.
            $crate::__define_table_query!(
                $mod,
                ( $($pk_name $pk_type),+ ),
                { $( $col_name $col_type ),* }
            );

            // Typed write surface (upsert builder + delete). See `macros::write`.
            $crate::__define_table_write!(
                $mod,
                ( $($pk_name $pk_type),+ ),
                { $( ($col_idx $col_name $col_type) ),* }
            );

            // Typed watch surface (Event + watch/watch_row). See `macros::watch`.
            $crate::__define_table_watch!(
                $mod,
                ( $($pk_name $pk_type),+ ),
                { $( ($col_idx $col_name $col_type) ),* }
            );
        }
    };
}

/// Define a set of table schemas and a `tables()` collector to hand to
/// `Reducer::new`.
///
/// Each entry is `index name (key type, ...) => { (idx col type) ... }`: the row's
/// primary key maps to its columns, mirroring the LWW model. `name` becomes the
/// VIEW name; `index` is the table's slot within its PK shape (see
/// [`TableId`](crate::id::TableId)). Column and key types are the keywords
/// `Bytes`, `Text`, `I64`, `Uuid`.
///
/// ```ignore
/// ubiquisync_tables::define_tables! {
///     1 notes  (id Uuid)         => { (0 body Text), (1 n I64) },
///     2 events (k Text, seq I64) => { (0 payload Bytes) },
/// }
/// let reducer = Reducer::new("app", &tables()?, &db).await?;
/// ```
#[macro_export]
macro_rules! define_tables {
    (
        $( $index:literal $mod:ident
            ( $($pk_name:ident $pk_type:ident),+ $(,)? )
            => { $( ($col_idx:literal $col_name:ident $col_type:ident) ),* $(,)? }
        ),+ $(,)?
    ) => {
        $(
            $crate::define_table!(
                $index $mod
                ( $($pk_name $pk_type),+ )
                => { $( ($col_idx $col_name $col_type) ),* }
            );
        )+

        /// All declared table schemas, in declaration order — pass to
        /// `Reducer::new`.
        pub fn tables() -> ::core::result::Result<
            ::std::vec::Vec<$crate::schema::TableSchema>,
            $crate::error::TablesError,
        > {
            ::core::result::Result::Ok(::std::vec![ $( $mod::table()? ),+ ])
        }
    };
}
