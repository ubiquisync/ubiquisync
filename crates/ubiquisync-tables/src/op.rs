//! Table operations — the core mutation types applied by the reducer.
//!
//! An [`Op`] is a single atomic state change. It is the payload inside a
//! [`LogEntry`](ubiquisync_core::log::LogEntry) in the table log: the
//! application layer constructs `Op` values, the log layer wraps them with
//! timestamp and attribution metadata, and the merge reducer applies them to
//! local storage.
//!
//! Tables have a compile-time schema with type-encoded IDs — see
//! [`crate::id`].

use crate::col_type::ColType;
use crate::id::{ColumnId, TableId};
use ubiquisync_core::uuid::Uuid;
use ubiquisync_sql::db::DbValue;

/// A single state mutation against a table (compile-time schema).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum Op {
    /// Insert or merge a table row.
    Upsert(Upsert),
    /// Soft-delete a table row.
    Delete(Delete),
}

// ── Table operations ─────────────────────────────────────────────────────────

/// Inserts or merges a row in a table. Every column merges last-writer-wins,
/// keyed by the timestamp from the enclosing
/// [`LogEntry`](ubiquisync_core::log::LogEntry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upsert {
    /// The table this row belongs to.
    pub table_id: TableId,
    /// PK values identifying the row. Count and per-column wire encoding are
    /// determined by the table ID's PK shape bits — each value's variant must
    /// match the corresponding [`pk_col_type`](crate::id::TableId::pk_col_type)
    /// positionally.
    pub primary_key: Vec<Value>,
    /// Columns to set with new values. The column ID's type bits determine
    /// the wire encoding.
    pub sets: Vec<ColumnSet>,
    /// Columns to set to SQL NULL. All non-PK columns are implicitly nullable.
    pub nulls: Vec<ColumnId>,
}

/// A typed value — used both as a primary-key component and as a column value.
/// The variant is fixed positionally by the table's PK shape (for PK values)
/// or by the column ID's type bits (for column values). PK values are row
/// identity (compared, never merged); column values merge last-writer-wins.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    /// Raw byte data (length-prefixed on wire).
    Bytes(Vec<u8>),
    /// 16-byte UUID (fixed-width on wire).
    Uuid(Uuid),
    /// UTF-8 text (length-prefixed on wire). Strict UTF-8, no embedded NUL,
    /// compared as raw bytes — see the table protocol's text rules.
    Text(String),
    /// Signed 64-bit integer (zigzag varint on wire).
    I64(i64),
}

impl Value {
    /// Convert to the backend [`DbValue`] bound into SQL, mapping each variant
    /// to its storage class (`Bytes`→`Blob`, `Uuid`→`Uuid`, `Text`→`Text`,
    /// `I64`→`Integer`).
    pub fn to_db(&self) -> DbValue {
        match self {
            Value::Bytes(bytes) => DbValue::Blob(bytes.clone()),
            Value::Uuid(uuid) => DbValue::Uuid(*uuid),
            Value::Text(text) => DbValue::Text(text.clone()),
            Value::I64(i) => DbValue::Integer(*i),
        }
    }

    /// The [`ColType`] this value's variant represents, for checking it against
    /// a column's or PK slot's declared type during op validation.
    pub fn col_type(&self) -> ColType {
        match self {
            Value::Bytes(_) => ColType::Bytes,
            Value::Uuid(_) => ColType::Uuid,
            Value::Text(_) => ColType::Text,
            Value::I64(_) => ColType::I64,
        }
    }
}

/// Soft-deletes a table row by advancing `__deleted_ts`. LWW — a later
/// timestamp always wins; an earlier timestamp is silently ignored.
/// Timestamp comes from the enclosing
/// [`LogEntry`](ubiquisync_core::log::LogEntry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delete {
    /// The table the row belongs to.
    pub table_id: TableId,
    /// PK values identifying the row (see [`Upsert::primary_key`]).
    pub primary_key: Vec<Value>,
}

/// A column ID paired with the value to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSet {
    /// The column being written; its type bits fix the value's wire encoding.
    pub column_id: ColumnId,
    /// The value to write — its variant must match `column_id`'s declared type.
    pub value: Value,
}

#[cfg(test)]
mod test {
    use proptest::{collection::btree_set, prelude::*};
    use ubiquisync_core::uuid::Uuid;

    use crate::{
        col_type::ColType,
        id::{ColumnId, TableId},
        op::{ColumnSet, Delete, Upsert, Value},
    };

    impl Arbitrary for Upsert {
        type Parameters = ();

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            (table_pk_data(), upsert_col_data())
                .prop_map(|((id, pk), col_data)| mk_upsert(id, pk, col_data))
                .boxed()
        }

        type Strategy = BoxedStrategy<Self>;
    }

    impl Arbitrary for Delete {
        type Parameters = ();

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            table_pk_data()
                .prop_map(|(id, pk)| Delete {
                    table_id: id,
                    primary_key: pk,
                })
                .boxed()
        }

        type Strategy = BoxedStrategy<Self>;
    }

    fn table_pk_data() -> impl Strategy<Value = (TableId, Vec<Value>)> {
        any::<TableId>().prop_flat_map(|id| (Just(id), pk_for(id)))
    }

    fn upsert_col_data() -> impl Strategy<Value = Vec<(ColumnId, Option<Value>)>> {
        btree_set(any::<ColumnId>(), 0..=16).prop_flat_map(|cols| {
            cols.iter()
                .map(|col| (Just(*col), proptest::option::of(value_for(col.col_type()))))
                .collect::<Vec<_>>()
        })
    }

    fn mk_upsert(
        table_id: TableId,
        primary_key: Vec<Value>,
        col_data: Vec<(ColumnId, Option<Value>)>,
    ) -> Upsert {
        let nulls = col_data
            .iter()
            .filter_map(|(c, v)| if v.is_none() { Some(*c) } else { None })
            .collect::<Vec<_>>();
        let sets = col_data
            .into_iter()
            .filter_map(|(column_id, v)| v.map(|value| ColumnSet { column_id, value }))
            .collect::<Vec<_>>();
        Upsert {
            table_id,
            primary_key,
            sets,
            nulls,
        }
    }

    fn value_for(ct: ColType) -> impl Strategy<Value = Value> {
        match ct {
            ColType::Bytes => any::<Vec<u8>>().prop_map(Value::Bytes).boxed(),
            ColType::Text => any::<String>().prop_map(Value::Text).boxed(),
            ColType::I64 => any::<i64>().prop_map(Value::I64).boxed(),
            ColType::Uuid => any::<Uuid>().prop_map(Value::Uuid).boxed(),
        }
    }

    fn pk_for(id: TableId) -> impl Strategy<Value = Vec<Value>> {
        let mut strategies = vec![];
        for i in 0..id.pk_count() {
            strategies.push(value_for(id.pk_col_type(i)));
        }
        strategies
    }
}
