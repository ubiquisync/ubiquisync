use std::marker::PhantomData;

use ubiquisync_core::{
    codec::{CodecError, IndexableOp},
    log_entry::LogEntry,
    uuid::Uuid,
};

use crate::{
    db::{Db, DbBatch, DbError, DbType, DbValue, ValueBinder},
    util::quote_ident,
};

#[async_trait::async_trait(?Send)]
pub trait LogTracker<Op>: Sized {
    async fn init(db: &dyn Db, prefix: &str) -> Result<Self, DbError>;

    fn track_one(
        &self,
        client_id: &Uuid,
        client_idx: u64,
        entry: &LogEntry<Op>,
        batch: &mut dyn DbBatch,
    ) -> Result<(), CodecError>;
}

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
            client_id {uuid_type} NOT NULL,\
            client_idx {int_type} NOT NULL,\
            user_id {uuid_type} NULL,\
            ts {int_type} NOT NULL,\
            tag {int_type} NOT NULL,\
            index_key {blob_type} NULL,\
            index_value {blob_type} NULL,\
            PRIMARY KEY(client_id, client_idx))\
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
        client_id: &Uuid,
        client_idx: u64,
        entry: LogEntry<Op>,
        batch: &mut dyn DbBatch,
    ) -> Result<(), CodecError> {
        let mut value_binder = ValueBinder::new(batch.dialect());

        let client_id_bind = value_binder.bind_next(DbValue::Uuid(*client_id));
        // TODO any problem with stuffing the u64 into i64 ??
        let client_idx_bind = value_binder.bind_next(DbValue::Integer(client_idx as i64));
        let user_id_bind = if let Some(user_id) = entry.user_id {
            value_binder.bind_next(DbValue::Uuid(user_id))
        } else {
            value_binder.bind_next(DbValue::Null)
        };
        // TODO any problem with stuffing the u64 into i64 ??
        let ts_bind = value_binder.bind_next(DbValue::Integer(entry.timestamp.raw() as i64));

        let index_entry = entry.op.to_index_entry()?;
        let tag_bind = value_binder.bind_next(DbValue::Integer(index_entry.tag as i64));
        let index_key_bind = value_binder.bind_next(DbValue::Blob(index_entry.key));
        let value_bind = value_binder.bind_next(DbValue::Blob(index_entry.value));

        let sql = format!(
            "INSERT INTO {} (\"client_id\", \"client_idx\", \"user_id\", \"ts\", \"tag\", \"index_key\", \"value\") \
             VALUES({client_id_bind}, {client_idx_bind}, {user_id_bind}, {ts_bind}, {tag_bind}, {index_key_bind}, {value_bind})",
            self.quoted_table_name
        );

        batch.add_statement(&sql, &value_binder.values());
        Ok(())
    }
}
