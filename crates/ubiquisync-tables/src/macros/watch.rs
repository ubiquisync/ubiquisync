//! Watch-side codegen for [`define_tables!`](crate::define_tables): per-table
//! typed change events projected from the engine's
//! [`ChangeEvent`](crate::watch::ChangeEvent), plus `watch`/`watch_row` streams.
//!
//! The engine emits one untyped `ChangeEvent` stream; the macro generates a typed
//! `Event` per table and a `TryFrom<ChangeEvent>` that filters to this table and
//! decodes its PK/columns. `watch`/`watch_row` subscribe through any store whose
//! event type is `ChangeEvent` and project each event into that typed `Event`.
//!
//! Invoked by [`define_table!`](crate::define_table); not called directly.

/// Generate the typed event types + `watch`/`watch_row` for one table.
#[doc(hidden)]
#[macro_export]
macro_rules! __define_table_watch {
    (
        $mod:ident,
        ( $($pk_name:ident $pk_type:ident),+ ),
        { $( ($col_idx:literal $col_name:ident $col_type:ident) ),* }
    ) => {
        /// A change to this table, projected from the engine's `ChangeEvent`.
        #[allow(dead_code, missing_docs)]
        #[derive(Debug, Clone)]
        pub enum Event {
            Upsert(UpsertEvent),
            Delete(DeleteEvent),
        }

        /// An insert or column update. Each value column is a
        /// [`ColumnChange`](crate::watch::ColumnChange): `Unchanged` when this
        /// event didn't touch it, `SetNull` when it was set to SQL NULL, or
        /// `Set(v)` when it was set to `v`.
        #[allow(dead_code, missing_docs)]
        #[derive(Debug, Clone)]
        pub struct UpsertEvent {
            $(pub $pk_name: $crate::__q_pk_ty!($pk_type),)+
            $(pub $col_name: $crate::__ev_col_ty!($col_type),)*
        }

        /// A soft-delete (tombstone): the primary key of the removed row.
        #[allow(dead_code, missing_docs)]
        #[derive(Debug, Clone)]
        pub struct DeleteEvent {
            $(pub $pk_name: $crate::__q_pk_ty!($pk_type),)+
        }

        impl ::core::convert::TryFrom<$crate::watch::ChangeEvent> for Event {
            // Non-matching events are returned untouched, so a caller can try the
            // next table's `Event` in turn.
            type Error = $crate::watch::ChangeEvent;

            // `unused_*`: a key-only table (no value columns) never mutates
            // `upsert` and never binds `cv`.
            #[allow(unused_mut, unused_variables)]
            fn try_from(
                event: $crate::watch::ChangeEvent,
            ) -> ::core::result::Result<Self, $crate::watch::ChangeEvent> {
                match event {
                    $crate::watch::ChangeEvent::Upsert(e) if e.table_id == TABLE_ID => {
                        // PK decode is infallible: `table_id` matching guarantees
                        // the value shape lines up with this table's schema.
                        let mut pk = e.primary_key.into_iter();
                        let mut upsert = UpsertEvent {
                            $($pk_name: $crate::__ev_pk_val!(pk.next(), $pk_type),)+
                            $($col_name: $crate::watch::ColumnChange::Unchanged,)*
                        };
                        for cv in e.changed_columns {
                            $( if cv.column_id
                                == $crate::id::ColumnId::new($col_idx, $crate::__col_type!($col_type))
                            {
                                upsert.$col_name = $crate::__ev_col_val!(cv.value, $col_type);
                            } else )* {}
                        }
                        ::core::result::Result::Ok(Event::Upsert(upsert))
                    }
                    $crate::watch::ChangeEvent::Delete(e) if e.table_id == TABLE_ID => {
                        let mut pk = e.primary_key.into_iter();
                        ::core::result::Result::Ok(Event::Delete(DeleteEvent {
                            $($pk_name: $crate::__ev_pk_val!(pk.next(), $pk_type),)+
                        }))
                    }
                    other => ::core::result::Result::Err(other),
                }
            }
        }

        /// Watch every change to this table as a stream of typed [`Event`]s.
        /// Dropping the stream unsubscribes.
        #[allow(dead_code)]
        pub fn watch<O, S>(store: &S) -> impl $crate::macros::support::Stream<Item = Event>
        where
            S: $crate::macros::support::SqlStore<O, $crate::watch::ChangeEvent> + ?Sized,
        {
            $crate::macros::support::project_events::<Event>(
                store.watch($crate::watch::WatchTarget::Table(TABLE_ID)),
            )
        }

        /// Watch changes to the single row with this primary key.
        #[allow(dead_code)]
        pub fn watch_row<O, S>(
            store: &S,
            $($pk_name: $crate::__q_pk_arg!($pk_type),)+
        ) -> impl $crate::macros::support::Stream<Item = Event>
        where
            S: $crate::macros::support::SqlStore<O, $crate::watch::ChangeEvent> + ?Sized,
        {
            $crate::macros::support::project_events::<Event>(store.watch(
                $crate::watch::WatchTarget::TableRow(
                    TABLE_ID,
                    ::std::vec![ $( $crate::__w_pk_val!($pk_name, $pk_type) ),+ ],
                ),
            ))
        }
    };
}

/// Event field type for a value column: a
/// [`ColumnChange`](crate::watch::ColumnChange) over the column's Rust type.
#[doc(hidden)]
#[macro_export]
macro_rules! __ev_col_ty {
    (Text) => { $crate::watch::ColumnChange<::std::string::String> };
    (I64) => { $crate::watch::ColumnChange<i64> };
    (Uuid) => { $crate::watch::ColumnChange<$crate::macros::support::CoreUuid> };
    (Bytes) => { $crate::watch::ColumnChange<::std::vec::Vec<u8>> };
}

/// Decode the next PK component (an `Option<op::Value>`) into its typed value.
/// `unreachable` arms can't fire: a matched `table_id` fixes the PK shape.
#[doc(hidden)]
#[macro_export]
macro_rules! __ev_pk_val {
    ($v:expr, Uuid) => {
        match $v { ::core::option::Option::Some($crate::op::Value::Uuid(u)) => u, _ => ::core::unreachable!() }
    };
    ($v:expr, Text) => {
        match $v { ::core::option::Option::Some($crate::op::Value::Text(s)) => s, _ => ::core::unreachable!() }
    };
    ($v:expr, I64) => {
        match $v { ::core::option::Option::Some($crate::op::Value::I64(i)) => i, _ => ::core::unreachable!() }
    };
    ($v:expr, Bytes) => {
        match $v { ::core::option::Option::Some($crate::op::Value::Bytes(b)) => b, _ => ::core::unreachable!() }
    };
}

/// Decode a changed column's `Option<op::Value>` into a
/// [`ColumnChange`](crate::watch::ColumnChange): `None` = `SetNull`, `Some(v)` =
/// `Set(v)`.
#[doc(hidden)]
#[macro_export]
macro_rules! __ev_col_val {
    ($v:expr, Text) => {
        match $v {
            ::core::option::Option::Some($crate::op::Value::Text(s)) => $crate::watch::ColumnChange::Set(s),
            ::core::option::Option::None => $crate::watch::ColumnChange::SetNull,
            _ => ::core::unreachable!(),
        }
    };
    ($v:expr, I64) => {
        match $v {
            ::core::option::Option::Some($crate::op::Value::I64(i)) => $crate::watch::ColumnChange::Set(i),
            ::core::option::Option::None => $crate::watch::ColumnChange::SetNull,
            _ => ::core::unreachable!(),
        }
    };
    ($v:expr, Uuid) => {
        match $v {
            ::core::option::Option::Some($crate::op::Value::Uuid(u)) => $crate::watch::ColumnChange::Set(u),
            ::core::option::Option::None => $crate::watch::ColumnChange::SetNull,
            _ => ::core::unreachable!(),
        }
    };
    ($v:expr, Bytes) => {
        match $v {
            ::core::option::Option::Some($crate::op::Value::Bytes(b)) => $crate::watch::ColumnChange::Set(b),
            ::core::option::Option::None => $crate::watch::ColumnChange::SetNull,
            _ => ::core::unreachable!(),
        }
    };
}
