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
            "
            CREATE TABLE peers (
                peer_id BYTES NOT NULL,
                genesis_bytes BYTES,
                status BYTES
            ) WITHOUT ROWID;

            CREATE TABLE streams (
                id BYTES NOT NULL PRIMARY KEY,
                peer_id BYTES NOT NULL,
                container_id UUID NOT NULL,
                parent_id INT NULL REFERENCES id,
                horizon_info BYTES NULL, -- horizon index + MMR peaks
                received_idx INT NOT NULL DEFAULT 0,
                received_mmr_peaks BYTES NULL, -- MMR peaks at received_idx
                active_cipher BYTES NULL, -- encryption key fingerprint + cipher suite at received idx
                processed_idx INT NOT NULL DEFAULT 0,
                committed_idx INT NOT NULL DEFAULT 0,
                status BYTES, -- awaiting cipher key, awaiting upgrade (entry type, cipher suite), stalled on hlc skew, other stall (should distinguish waiting vs unrecoverable)
            );

            CREATE TABLE segments (
                stream_id INT NOT NULL,
                start_idx INT NOT NULL,
                size INT NOT NULL
                encoding INT NOT NULL,
                body BYTES NOT NULL,
                PRIMARY KEY(stream_id, start_idx)
            ) WITHOUT ROWID;
            "
        );
        todo!()
    }
}
