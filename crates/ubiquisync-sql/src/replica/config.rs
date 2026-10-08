use std::{collections::HashMap, time::Duration};

use ubiquisync_core::pack::FileRemoteProvider;

#[derive(Default)]
pub struct ReplicaConfig {
    pub pack_remote_providers: HashMap<String, Box<dyn FileRemoteProvider>>,
    pub pack_sync: PackSyncConfig,
}

/// Timing for pack remote syncing. The defaults suit real use; tests and simulations
/// shrink them to milliseconds.
#[derive(Debug, Clone)]
pub struct PackSyncConfig {
    /// How often each remote is read and written.
    pub poll_interval: Duration,
    /// First retry delay for a pack that couldn't be fully processed. Doubles on each
    /// further attempt up to `pack_retry_max`.
    pub pack_retry_min: Duration,
    pub pack_retry_max: Duration,
    /// First retry delay for a remote whose whole round failed. Doubles (with jitter)
    /// on each further failure up to `remote_retry_max`.
    pub remote_retry_min: Duration,
    pub remote_retry_max: Duration,
    /// How long another peer's segments may go unpublished on a remote before we
    /// publish them ourselves.
    pub peer_publish_grace: Duration,
}

impl Default for PackSyncConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
            pack_retry_min: Duration::from_secs(10),
            pack_retry_max: Duration::from_hours(6),
            remote_retry_min: Duration::from_secs(2),
            remote_retry_max: Duration::from_mins(5),
            peer_publish_grace: Duration::from_mins(5),
        }
    }
}
