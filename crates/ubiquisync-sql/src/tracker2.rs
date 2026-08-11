// use ubiquisync_core::log_entry::OpOrExpunge;

// #[async_trait::async_trait]
// pub trait LogTracker<Op>: Sized + Send + Sync {
//     /// Initialize this tracker's backing state, namespaced by `prefix`, and
//     /// return an instance bound to it.
//     async fn init(db: &dyn Db, prefix: &str) -> Result<Self, DbError>;

//     /// Record `entry`, identified by `(peer_id, entry_idx)`, by enqueuing its
//     /// writes into `batch` so they commit together with the rest of the entry's
//     /// application. Must not commit on its own.
//     fn track_batch(
//         &self,
//         peer_id: &Uuid,
//         entry_idx: u64,
//         timestamp: Timestamp,
//         server_user_id: Option<Uuid>,
//         ops: &[OpOrExpunge<Op>],
//         db_batch: &mut dyn DbBatch,
//     ) -> Result<(), LogTrackerError>;
// }

pub struct PeerTracker {}

impl PeerTracker {
    /// Initialize this tracker's backing state, namespaced by `prefix`, and
    /// return an instance bound to it.
    async fn init(db: &dyn Db, prefix: &str) -> Result<Self, DbError> {
        let sql = format!(
            "CREATE TABLE peer_cursors (
            log_id BIGSERIAL PRIMARY KEY, // TODO type
            peer_id UUID,
            container_id UUID,
            received_idx INT,
            mmr_peaks BYTES, // MMR peaks at received_idx
            active_cipher BYTES NULL // encryption key fingerprint at received idx
            signed_idx INT,
            committed_idx INT,
            );

            CREATE TABLE processing_queue (
                log_id INT,
                entry_idx INT,
                payload BYTES,
                sign_bytes BYTES
            )
            "
        );
        todo!()
    }
}
