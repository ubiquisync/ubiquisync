//! Declarative macros for building table [`schema`](crate::schema)s.
//!
//! [`define_tables!`] expands a compact table DSL into a `tables()` function
//! that returns the [`TableSchema`](crate::schema::TableSchema) values a caller
//! hands to [`Reducer::new`](crate::reducer::Reducer::new). Because the schema's
//! PK names and its table ID both come from the single `pk: (...)` list, the
//! count can never disagree — the one `TableSchema::new` error the macro cannot
//! hit by construction.
//!
//! This is intentionally schema-only for now. The source `def` module also
//! generated a typed update/query/event/store API per table; those layers
//! depend on store plumbing that isn't ported yet and will grow here (likely as
//! sibling helper-macro modules) once it lands.

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

/// Define one table's schema constructor inside a module named `$mod`.
///
/// Generates `pub mod $mod { pub fn table() -> Result<TableSchema, TablesError> }`.
/// The module name is the user-facing VIEW name; the PK and column idents are
/// the VIEW's column names. Types are keywords: `Bytes`, `Text`, `I64`, `Uuid`.
///
/// Usually invoked through [`define_tables!`](crate::define_tables) rather than
/// directly.
#[macro_export]
macro_rules! define_table {
    (
        $mod:ident, $index:expr,
        pk: ( $($pk_name:ident $pk_type:ident),+ $(,)? ),
        { $( ($col_idx:expr, $col_name:ident, $col_type:ident) ),* $(,)? }
    ) => {
        #[doc = concat!("Schema for the `", stringify!($mod), "` table.")]
        pub mod $mod {
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
                // `const` so `TableId::new`'s shape/index asserts run at compile
                // time: an out-of-range index or >4 PK columns is a build error,
                // not a runtime panic.
                const ID: $crate::id::TableId = $crate::id::TableId::new(
                    &[ $( $crate::__col_type!($pk_type) ),+ ],
                    $index,
                );
                $crate::schema::TableSchema::new(
                    ID,
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
        }
    };
}

/// Define a set of table schemas and a `tables()` collector to hand to
/// `Reducer::new`.
///
/// Each entry is `name: index ( pk: (col type, ...), { (idx, col, type), ... } )`.
/// The module `name` becomes the VIEW name; `index` is the table's slot within
/// its PK shape (see [`TableId`](crate::id::TableId)). Column and PK types are
/// the keywords `Bytes`, `Text`, `I64`, `Uuid`.
///
/// ```ignore
/// ubiquisync_tables::define_tables! {
///     notes:  1 ( pk: (id Uuid),         { (0, body, Text), (1, n, I64) } ),
///     events: 2 ( pk: (k Text, seq I64), { (0, payload, Bytes) } ),
/// }
/// let reducer = Reducer::new("app", &tables()?, &db).await?;
/// ```
#[macro_export]
macro_rules! define_tables {
    (
        $( $mod:ident : $index:literal (
            pk: ( $($pk_name:ident $pk_type:ident),+ $(,)? ),
            { $( ($col_idx:literal, $col_name:ident, $col_type:ident) ),* $(,)? }
        ) ),+ $(,)?
    ) => {
        $(
            $crate::define_table!(
                $mod, $index,
                pk: ( $($pk_name $pk_type),+ ),
                { $( ($col_idx, $col_name, $col_type) ),* }
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

#[cfg(test)]
mod tests {
    use crate::col_type::ColType;

    // A representative schema: a single-PK table and a composite-PK table,
    // exercising every column-type keyword and an empty value-column list.
    define_tables! {
        notes: 1 (
            pk: (id Uuid),
            {
                (0, body, Text),
                (1, n, I64),
                (2, blob, Bytes),
                (3, author, Uuid),
            }
        ),
        events: 2 (
            pk: (k Text, seq I64),
            {
                (0, payload, Bytes),
            }
        ),
        counters: 3 (
            pk: (id I64),
            {}
        ),
    }

    #[test]
    fn builds_all_declared_schemas() {
        // Goal: the collector yields one schema per declaration, in order.
        let all = tables().expect("schemas build");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "notes");
        assert_eq!(all[1].name, "events");
        assert_eq!(all[2].name, "counters");
    }

    #[test]
    fn single_pk_table_shape() {
        // Goal: PK name/type and value columns come straight from the DSL.
        let s = notes::table().unwrap();
        assert_eq!(s.pk_names, ["id"]);
        assert_eq!(s.id.pk_count(), 1);
        assert_eq!(s.id.pk_col_type(0), ColType::Uuid);
        assert_eq!(s.id.index(), 1);
        // Four value columns, keyed by their (type, index)-encoded IDs.
        assert_eq!(s.value_cols.len(), 4);
    }

    #[test]
    fn composite_pk_table_shape() {
        // Goal: multi-column PKs keep declaration order for both name and type.
        let s = events::table().unwrap();
        assert_eq!(s.pk_names, ["k", "seq"]);
        assert_eq!(s.id.pk_count(), 2);
        assert_eq!(s.id.pk_col_type(0), ColType::Text);
        assert_eq!(s.id.pk_col_type(1), ColType::I64);
    }

    #[test]
    fn table_with_no_value_columns() {
        // Goal: an empty `{}` value-column list is valid — a key-only table.
        let s = counters::table().unwrap();
        assert_eq!(s.pk_names, ["id"]);
        assert!(s.value_cols.is_empty());
    }
}
