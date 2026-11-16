//! Catches up streams whose commit is behind their head.
//!
//! Ingest only commits a segment when the stream's commit had already reached its head, so once
//! a commit stalls (or the process stops between storing a segment and committing it) nothing
//! else moves it forward. This loop does: on every tick it finds those streams and commits
//! their stored segments, if their stall allows a retry yet.

use std::collections::HashMap;

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    hlc::WallTime,
    ids::{ContainerId, LogId, PeerId},
    log::LogHashContext,
};

use crate::{
    db::{
        DbError,
        sea_query::{select_cols, update_cols},
    },
    reducer::Reducer,
    replica::{
        Replica, ReplicaInner,
        commit::CommitError,
        config::{CommitRetryConfig, backoff_delay},
        hlc::check_skew,
        poll_interval,
        schema::{CommitErr, peers, streams},
        segment::GetSegmentError,
        stream_lock::KeyedLockGuard,
        streams::{StreamInfo, StreamLog},
        tick_or_cancelled,
    },
};

#[derive(Debug, Error)]
enum CommitRetryError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("commit error: {0}")]
    Commit(#[from] CommitError),
    #[error("get segment error: {0}")]
    GetSegment(#[from] GetSegmentError),
}

impl<R: Reducer> Replica<R> {
    pub(crate) fn start_commit_retry(&mut self) {
        let inner = self.inner.clone();
        let cancel_token = self.cancel.clone();
        self.tasks.spawn(async move {
            let mut interval = poll_interval(inner.config.commit_retry.poll_interval);
            let mut startup = true;
            while tick_or_cancelled(&mut interval, &cancel_token).await {
                inner.retry_commits(startup).await;
                startup = false;
            }
        });
    }
}

impl<R: Reducer> ReplicaInner<R> {
    /// One round: every log with a stream behind its head gets a chance to catch up. A log
    /// that fails doesn't stop the others.
    #[tracing::instrument(skip_all)]
    async fn retry_commits(&self, startup: bool) {
        let logs = match self.logs_behind().await {
            Ok(logs) => logs,
            Err(error) => {
                tracing::warn!(%error, "commit retry round failed");
                return;
            }
        };
        for (peer, log) in logs {
            if let Err(error) = self.retry_log_commits(peer, log, startup).await {
                tracing::warn!(%peer, container = ?log.container_id, %error, "commit retry failed");
            }
        }
    }

    /// The logs that have a stream whose commit is behind its head.
    async fn logs_behind(&self) -> Result<Vec<(PeerId, StreamLog)>, DbError> {
        select_cols::<(peers::PeerId, streams::PeerId, streams::ContainerId)>(
            self.db.as_ref(),
            Query::select()
                .distinct()
                .from(streams::Table)
                .inner_join(
                    peers::Table,
                    Expr::col((streams::Table, streams::PeerId))
                        .eq(Expr::col((peers::Table, peers::Id))),
                )
                .and_where(
                    Expr::col((streams::Table, streams::CommitSize))
                        .lt(Expr::col((streams::Table, streams::HeadSize))),
                ),
        )
        .await?
        .iter()
        .map(|row| {
            let (peer, peer_db_id, container) = row?;
            Ok((
                PeerId(peer),
                StreamLog::new(peer_db_id, ContainerId(container)),
            ))
        })
        .collect()
    }

    async fn retry_log_commits(
        &self,
        peer: PeerId,
        log: StreamLog,
        startup: bool,
    ) -> Result<(), CommitRetryError> {
        let guard = self.stream_locks.lock(&log).await;
        let hash_ctx = LogHashContext::new(&LogId {
            peer_id: peer,
            container_id: log.container_id,
        });
        let mut streams = self.resolve_streams(&guard).await?;
        // a fork is created after its parent, so this order commits parents first
        streams.sort_by_key(|s| s.id);
        let mut commit_sizes = HashMap::new();
        for mut stream in streams {
            let parent_commit_size = stream
                .parent_id
                .and_then(|id| commit_sizes.get(&id).copied());
            if should_retry(
                stream.commit_err.as_ref(),
                WallTime::now(),
                &self.config.commit_retry,
                startup,
            ) {
                self.catch_up(&guard, &hash_ctx, &mut stream, parent_commit_size)
                    .await?;
            }
            commit_sizes.insert(stream.id, stream.commit_size);
        }
        Ok(())
    }

    /// Commits a stream's stored segments until it reaches its head, stalls, or stops making
    /// progress.
    async fn catch_up(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        hash_ctx: &LogHashContext,
        stream: &mut StreamInfo,
        parent_commit_size: Option<u64>,
    ) -> Result<(), CommitRetryError> {
        // entries below a fork point are stored and committed on the parent, the fork takes
        // over from there once the parent has committed them
        if let Some(fork_size) = stream.fork_size
            && stream.commit_size < fork_size
        {
            if parent_commit_size.is_none_or(|size| size < fork_size) {
                return Ok(());
            }
            update_cols::<(streams::CommitSize,)>(
                self.db.as_ref(),
                (fork_size,),
                Query::update()
                    .table(streams::Table)
                    .and_where(Expr::column(streams::Id).eq(stream.id)),
            )
            .await?;
            stream.commit_size = fork_size;
        }

        while stream.commit_size < stream.head_chain.size {
            let Some(segment) = self
                .get_segment_by_index(guard, hash_ctx, stream.id, stream.commit_size)
                .await?
            else {
                tracing::warn!(
                    stream_id = stream.id,
                    commit_size = stream.commit_size,
                    "no stored segment holds the next entry to commit"
                );
                break;
            };
            let commit_size = stream.commit_size;
            self.try_commit(guard, stream, segment).await?;
            if stream.commit_err.is_some() || stream.commit_size == commit_size {
                break;
            }
        }
        Ok(())
    }
}

/// Whether a stream whose commit is behind its head should be tried now.
fn should_retry(
    commit_err: Option<&CommitErr>,
    now: WallTime,
    cfg: &CommitRetryConfig,
    startup: bool,
) -> bool {
    match commit_err {
        // nothing failed, the commit just fell behind (e.g. we stopped mid-commit)
        None => true,
        // a restart may have fixed the cause (a transient db problem, a software upgrade), so
        // it gets one attempt that ignores the backoff; if that fails the count carries on
        Some(CommitErr::Internal {
            retry_time_span,
            retry_count,
        }) => {
            let delay = backoff_delay(cfg.retry_min, cfg.retry_max, *retry_count);
            startup
                || now.as_millis() >= retry_time_span.end.saturating_add(delay.as_millis() as u64)
        }
        Some(CommitErr::HLCForwardSkew(remote)) => check_skew(now, *remote).is_ok(),
        // only new software can read it, and new software means a restart
        Some(CommitErr::IncompatibleSoftware(_)) => startup,
        // TODO wake NeedKey when the key arrives and AwaitingDeps when the deps are committed
        Some(CommitErr::NeedKey(_) | CommitErr::AwaitingDeps | CommitErr::Frozen) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use test_case::test_case;
    use ubiquisync_core::hlc::Timestamp;

    use super::*;
    use crate::replica::{hlc::MAX_SKEW_MS, schema::UnknownSoftwareVersion};

    const NOW: u64 = 10_000_000;

    /// An internal error that last failed `secs_ago`, after `retry_count` earlier retries.
    fn internal(secs_ago: u64, retry_count: u64) -> CommitErr {
        let last_failed = NOW - secs_ago * 1000;
        CommitErr::Internal {
            retry_time_span: last_failed..last_failed,
            retry_count,
        }
    }

    /// A stall on an op stamped `ms_ahead` of our clock.
    fn skew(ms_ahead: u64) -> CommitErr {
        let remote = WallTime::from_millis(NOW + ms_ahead);
        CommitErr::HLCForwardSkew(Timestamp::from_parts(remote, 0).unwrap())
    }

    fn incompatible() -> CommitErr {
        CommitErr::IncompatibleSoftware(UnknownSoftwareVersion::OpType(200))
    }

    #[test_case(None, false => true ; "behind without an error")]
    #[test_case(Some(internal(9, 0)), false => false ; "internal before the first delay")]
    #[test_case(Some(internal(10, 0)), false => true ; "internal after the first delay")]
    #[test_case(Some(internal(39, 2)), false => false ; "internal before a doubled delay")]
    #[test_case(Some(internal(40, 2)), false => true ; "internal after a doubled delay")]
    #[test_case(Some(internal(60, 50)), false => true ; "internal delay is capped")]
    #[test_case(Some(internal(0, 50)), true => true ; "restart ignores the internal delay")]
    #[test_case(Some(skew(MAX_SKEW_MS + 1)), true => false ; "clock still too far behind")]
    #[test_case(Some(skew(MAX_SKEW_MS)), false => true ; "clock caught up")]
    #[test_case(Some(incompatible()), false => false ; "incompatible software waits for a restart")]
    #[test_case(Some(incompatible()), true => true ; "incompatible software retries at startup")]
    #[test_case(Some(CommitErr::AwaitingDeps), true => false ; "awaiting deps has no wake yet")]
    #[test_case(Some(CommitErr::Frozen), true => false ; "frozen never retries")]
    fn retry(commit_err: Option<CommitErr>, startup: bool) -> bool {
        let cfg = CommitRetryConfig {
            retry_min: Duration::from_secs(10),
            retry_max: Duration::from_secs(60),
            ..Default::default()
        };
        should_retry(
            commit_err.as_ref(),
            WallTime::from_millis(NOW),
            &cfg,
            startup,
        )
    }
}
