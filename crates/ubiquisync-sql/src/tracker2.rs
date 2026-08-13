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
            CREATE TABLE streams (
                stream_id UUID NOT NULL PRIMARY KEY, -- the peer id or synthentic (local, non-consensus) fork id
                peer_id UUID NOT NULL, -- the real peer id
                genesis_bytes BYTES,
                genesis_signature BYTES,
                fork_proof BYTES,
                parent_stream_id UUID,
                status INT
            ) WITHOUT ROWID;

            CREATE TABLE logs (
                log_id BIGSERIAL  NOT NULLPRIMARY KEY, // TODO type
                container_id UUID NOT NULL,
                stream_id UUID NOT NULL,
                horizon_info BYTES NULL, -- horizon index + MMR peaks
                received_idx INT NOT NULL DEFAULT 0,
                received_mmr_peaks BYTES NULL, -- MMR peaks at received_idx
                active_cipher BYTES NULL, -- encryption key fingerprint + cipher suite at received idx
                processed_idx INT NOT NULL DEFAULT 0,
                committed_idx INT NOT NULL DEFAULT 0,
                UNIQUE(container_id, stream_id)
            );

            CREATE TABLE log_entries (
                log_id INT NOT NULL,
                entry_idx INT NOT NULL,
                type INT NOT NULL,
                meta BYTES NULL, -- includes expungement info or encrypted header bytes or op count if decrypted (1 byte prefix) or key for UseKey entries
                leaf_hash BYTES NULL,
                hlc INT NULL,
                server_user_id BYTES NULL,
                PRIMARY KEY(log_id, entry_idx)
            ) WITHOUT ROWID;

            CREATE TABLE log_ops (
                log_id INT NOT NULL,
                entry_idx INT NOT NULL,
                op_idx INT NOT NULL,
                part_idx INT NOT NULL,
                key BYTES NULL,
                value BYTES NULL,
                meta BYTES NULL, -- includes expungement info or encrypted bytes (1 byte prefix)
                PRIMARY KEY(log_id, entry_idx, op_idx, part_idx)
            ) WITHOUT ROWID;

            CREATE TABLE signatures (
                log_id INT NOT NULL,
                size INT NOT NULL,
                signature BYTES NOT NULL,
                PRIMARY_KEY(log_id, size)
            ) WITHOUT ROWID;

            CREATE mmr_peak_cache (
                log_id INT,
                size INT, -- we'll usually want to retain at some power of 2 multiple, say every 1024 peaks
                peaks BYTES,
                PRIMARY KEY(log_id, size)
            );
            "
        );
        todo!()
    }
}
