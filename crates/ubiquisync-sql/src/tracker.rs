//! Recording the log entries a processor ingests.
//!
//! [`LogTracker`] is the hook the processor calls for every ingested entry,
//! enqueued into the same batch that applies the entry. Implementations decide
//! what to record. [`LogIndexTracker`] is the built-in one — an op-log table
//! keyed by `(peer_id, entry_idx)` that future readers can dedup, revision
//! history, and attribution from without replaying the reducer.
//!
//! Today the only reader is [`peer_cursor`](LogTracker::peer_cursor), which just
//! takes `MAX(entry_idx)`. Readers added later must skip the expunged-marker
//! rows [`track_expunged`](LogTracker::track_expunged) writes (`tag` =
//! [`TAG_EXPUNGED`], no attribution, no timestamp): they occupy a stream index
//! but are not real entries, so counting or attributing them would be wrong.

use std::marker::PhantomData;

use ubiquisync_core::{
    codec::{CodecError, IndexableOp, TAG_EXPUNGED},
    log_entry::LogEntry,
    uuid::Uuid,
};

use crate::{
    db::{Db, DbBatch, DbError, DbType, DbValue, ValueBinder},
    util::quote_ident,
};

/// Records each log entry as the processor ingests it.
///
/// The processor calls [`init`](LogTracker::init) once at startup, then
/// [`track_one`](LogTracker::track_one) for every ingested entry (or
/// [`track_expunged`](LogTracker::track_expunged) for an expunged marker),
/// inside the batch that applies that entry. What gets recorded, and how, is up
/// to the implementation — see [`LogIndexTracker`] for the built-in op-log.
///
/// # Duplicate rejection
///
/// An implementation may or may not reject a repeated `(peer_id, entry_idx)`.
/// If it does — e.g. by making `(peer_id, entry_idx)` a unique key, so a
/// repeat fails the batch and rolls the whole apply back — then the reducer it
/// is paired with need not be idempotent. If it does not, the reducer must be.
/// Nothing yet enforces that pairing at the type level (a future marker on the
/// reducer could); today it is a contract the wiring must honor.
#[async_trait::async_trait(?Send)]
pub trait LogTracker<Op>: Sized {
    /// Initialize this tracker's backing state, namespaced by `prefix`, and
    /// return an instance bound to it.
    async fn init(db: &dyn Db, prefix: &str) -> Result<Self, DbError>;

    /// Record `entry`, identified by `(peer_id, entry_idx)`, by enqueuing its
    /// writes into `batch` so they commit together with the rest of the entry's
    /// application. Must not commit on its own.
    fn track_one(
        &self,
        peer_id: &Uuid,
        entry_idx: u64,
        entry: &LogEntry<Op>,
        batch: &mut dyn DbBatch,
    ) -> Result<(), LogTrackerError>;

    /// Record the expunged marker at `(peer_id, entry_idx)`, naming the `hash`
    /// of the entry that was expunged, by enqueuing into `batch`. This occupies
    /// the stream index so the peer's cursor advances past the gap without a
    /// real entry ever being applied there. Must not commit on its own.
    fn track_expunged(
        &self,
        peer_id: &Uuid,
        entry_idx: u64,
        hash: &blake3::Hash,
        batch: &mut dyn DbBatch,
    ) -> Result<(), LogTrackerError>;

    /// The next-entry index for `peer_id`: one past the highest `entry_idx` this
    /// tracker has recorded for that peer (real entries and expunged markers
    /// alike), or `0` if it has recorded none. This is the sync cursor, derived
    /// from what has actually been tracked rather than stored separately.
    async fn peer_cursor(&self, db: &dyn Db, peer_id: &Uuid) -> Result<u64, DbError>;
}

/// Failure from [`LogTracker::track_one`]. Splitting the op into its index
/// triple is a [`CodecError`]; binding a value that won't fit the backend's
/// signed-integer column (a `u64` past `i64::MAX`, via [`DbValue::from_u64`])
/// is a [`DbError`]. Both are surfaced so the tracker can use the same checked
/// conversion the rest of the crate relies on instead of a lossy `as` cast.
#[derive(Debug, thiserror::Error)]
pub enum LogTrackerError {
    /// Splitting the op into its index triple failed.
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),
    /// A backend operation failed — including a `u64` that won't fit the
    /// signed-integer column.
    #[error("db error: {0}")]
    Db(#[from] DbError),
}

/// The default [`LogTracker`]: writes each entry into a single
/// `<prefix>__oplog` table keyed by `(peer_id, entry_idx)`, storing the HLC
/// timestamp and the op's decoded index key/value.
///
/// The `(peer_id, entry_idx)` primary key rejects a repeated index with a
/// unique violation, rolling the whole apply back — so a reducer driven through
/// this tracker is not required to be idempotent (see the
/// [duplicate-rejection contract](LogTracker#duplicate-rejection)).
pub struct LogIndexTracker<Op> {
    quoted_table_name: String,
    _phantom: PhantomData<Op>,
}

#[async_trait::async_trait(?Send)]
impl<Op: IndexableOp> LogTracker<Op> for LogIndexTracker<Op> {
    async fn init(db: &dyn Db, prefix: &str) -> Result<Self, DbError> {
        let quoted_table_name = quote_ident(&format!("{prefix}__oplog"));
        let dialect = db.dialect();
        let int_type = DbType::Integer.sql_type(dialect);
        let blob_type = DbType::Blob.sql_type(dialect);
        let uuid_type = DbType::Uuid.sql_type(dialect);
        let without_rowid = dialect.without_rowid();
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {quoted_table_name} (\
            peer_id {uuid_type} NOT NULL,\
            entry_idx {int_type} NOT NULL,\
            server_user_id {uuid_type} NULL,\
            ts {int_type} NOT NULL,\
            tag {int_type} NOT NULL,\
            index_key {blob_type} NULL,\
            index_value {blob_type} NULL,\
            PRIMARY KEY(peer_id, entry_idx))\
            {without_rowid};"
        );
        db.exec(&sql, &[]).await?;
        Ok(Self {
            quoted_table_name,
            _phantom: Default::default(),
        })
    }

    fn track_one(
        &self,
        peer_id: &Uuid,
        entry_idx: u64,
        entry: &LogEntry<Op>,
        batch: &mut dyn DbBatch,
    ) -> Result<(), LogTrackerError> {
        let mut value_binder = ValueBinder::new(batch.dialect());

        let peer_id_bind = value_binder.bind_next(DbValue::Uuid(*peer_id));
        // `entry_idx` is a u64 stream counter going into a signed-integer
        // column. `from_u64` rejects a value past `i64::MAX` rather than wrap it
        // negative — the same checked store the rest of the crate uses.
        let entry_idx_bind = value_binder.bind_next(DbValue::from_u64(entry_idx)?);
        let server_user_id_bind = if let Some(server_user_id) = entry.server_user_id {
            value_binder.bind_next(DbValue::Uuid(server_user_id))
        } else {
            value_binder.bind_next(DbValue::Null)
        };
        // The packed HLC timestamp is a full-width u64; store it through the same
        // `from_u64` guard `SqlHlcStorage` uses, so a value past `i64::MAX` can't
        // wrap negative and misorder a `MAX`/`GREATEST` merge on `ts`.
        let ts_bind = value_binder.bind_next(DbValue::from_u64(entry.timestamp.raw())?);

        let index_entry = entry.op.to_index_entry()?;
        let tag_bind = value_binder.bind_next(DbValue::Integer(index_entry.tag as i64));
        let index_key_bind = value_binder.bind_next(DbValue::Blob(index_entry.key));
        let value_bind = value_binder.bind_next(DbValue::Blob(index_entry.value));

        let sql = format!(
            "INSERT INTO {} (\"peer_id\", \"entry_idx\", \"server_user_id\", \"ts\", \"tag\", \"index_key\", \"index_value\") \
             VALUES({peer_id_bind}, {entry_idx_bind}, {server_user_id_bind}, {ts_bind}, {tag_bind}, {index_key_bind}, {value_bind})",
            self.quoted_table_name
        );

        batch.add_statement(&sql, &value_binder.values());
        Ok(())
    }

    async fn peer_cursor(&self, db: &dyn Db, peer_id: &Uuid) -> Result<u64, DbError> {
        let mut value_binder = ValueBinder::new(db.dialect());
        let peer_id_bind = value_binder.bind_next(DbValue::Uuid(*peer_id));
        // This is the *aggregate* `MAX(column)`, which is standard SQL and reads
        // identically on both dialects — deliberately NOT
        // [`SqlDialect::scalar_max`](crate::dialect::SqlDialect::scalar_max),
        // whose `GREATEST` is the two-argument row max used by the merge
        // upserts. There is no dialect divergence here, so no branch: the
        // placeholder (via `ValueBinder`) and double-quoted identifiers are the
        // only flavor-sensitive pieces and both are already dialect-correct.
        let sql = format!(
            "SELECT MAX(\"entry_idx\") FROM {} WHERE \"peer_id\" = {peer_id_bind}",
            self.quoted_table_name
        );
        // `MAX` over no rows is NULL — an unseen peer, cursor 0. Otherwise the
        // next index is one past the highest recorded. Stored via `from_u64`, so
        // the value is a non-negative `i64` that reads back cleanly as `u64`.
        let rows = db.query(&sql, &value_binder.values()).await?;
        Ok(rows[0].get_optional_u64(0)?.map_or(0, |max| max + 1))
    }

    fn track_expunged(
        &self,
        peer_id: &Uuid,
        entry_idx: u64,
        hash: &blake3::Hash,
        batch: &mut dyn DbBatch,
    ) -> Result<(), LogTrackerError> {
        let mut value_binder = ValueBinder::new(batch.dialect());

        let peer_id_bind = value_binder.bind_next(DbValue::Uuid(*peer_id));
        let entry_idx_bind = value_binder.bind_next(DbValue::from_u64(entry_idx)?);
        // An expunged marker carries no attribution and no timestamp (see
        // `TAG_EXPUNGED`); the row exists only to occupy the stream index. Store
        // NULL attribution and ts 0, and stash the expunged entry's hash in
        // `index_value` under the reserved tag so `index_key` stays NULL.
        let tag_bind = value_binder.bind_next(DbValue::Integer(TAG_EXPUNGED as i64));
        let hash_bind = value_binder.bind_next(DbValue::Blob(hash.as_bytes().to_vec()));

        let sql = format!(
            "INSERT INTO {} (\"peer_id\", \"entry_idx\", \"server_user_id\", \"ts\", \"tag\", \"index_key\", \"index_value\") \
             VALUES({peer_id_bind}, {entry_idx_bind}, NULL, 0, {tag_bind}, NULL, {hash_bind})",
            self.quoted_table_name
        );

        batch.add_statement(&sql, &value_binder.values());
        Ok(())
    }
}
