use std::{collections::HashMap, time::Duration};

use ubiquisync_core::pack::FileRemoteProvider;

#[derive(Default)]
pub struct ReplicaConfig {
    pub pack_remote_providers: HashMap<String, Box<dyn FileRemoteProvider>>,
    pub pack_sync: PackSyncConfig,
    pub commit_retry: CommitRetryConfig,
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

/// Timing for retrying commits that stalled or fell behind their stream's head.
#[derive(Debug, Clone)]
pub struct CommitRetryConfig {
    /// How often streams are checked for commits to retry.
    pub poll_interval: Duration,
    /// First retry delay after an internal error. Doubles on each further failure up to
    /// `retry_max`. A restart retries once right away, whatever the delay. (A stall on
    /// incompatible software is only retried at a restart, never on this backoff.)
    pub retry_min: Duration,
    pub retry_max: Duration,
}

impl Default for CommitRetryConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            retry_min: Duration::from_secs(10),
            retry_max: Duration::from_hours(1),
        }
    }
}

/// Exponential backoff: `min` doubled `doublings` times, capped at `max`.
pub(crate) fn backoff_delay(min: Duration, max: Duration, doublings: u64) -> Duration {
    let factor = 1u32
        .checked_shl(doublings.min(31) as u32)
        .unwrap_or(u32::MAX);
    min.saturating_mul(factor).min(max)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use test_strategy::proptest;

    use super::*;

    #[proptest]
    fn backoff_stays_in_bounds_and_never_shrinks(
        #[strategy(1u64..100_000)] min_ms: u64,
        #[strategy(0u64..10_000_000)] range_ms: u64,
        doublings: u64,
    ) {
        let min = Duration::from_millis(min_ms);
        let max = Duration::from_millis(min_ms + range_ms);
        let delay = backoff_delay(min, max, doublings);
        prop_assert!(min <= delay && delay <= max);
        prop_assert!(delay <= backoff_delay(min, max, doublings.saturating_add(1)));
    }
}
