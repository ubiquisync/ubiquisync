//! Live sync events for activity feeds and for knowing when to re-read [`Replica::status`].
//!
//! Events are best effort and not stored. A subscriber that falls too far behind gets
//! [`SyncEventKind::Missed`] with the number of events it lost, then carries on from the
//! newest ones; it should call `status()` again to catch up.

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};

use futures::{Stream, stream::BoxStream};
use tokio::sync::broadcast;
use ubiquisync_core::{
    ids::{ContainerId, PeerId},
    pack::PackFileId,
};

use crate::replica::{
    Replica, ReplicaInner,
    fs_sync_schema::BlockedReasons,
    status::{RemoteId, Stall, Task},
};

/// How many events a subscriber may fall behind before it gets `Missed`.
const EVENT_BUFFER: usize = 1024;

/// One thing the sync engine did.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncEvent {
    /// When the event happened, by this device's clock.
    pub at: SystemTime,
    pub kind: SyncEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncEventKind {
    /// This device wrote a file to a sync folder.
    FilePublished {
        remote: RemoteId,
        file: PackFileId,
        /// Entries in the file, over every log it carries.
        entries: u64,
        /// Size of the file body.
        bytes: u64,
    },
    /// A file from a sync folder was read in full.
    FileRead {
        remote: RemoteId,
        publisher: PeerId,
        file: PackFileId,
        /// Entries in the file, over every log it carries (some may have been held already).
        entries: u64,
    },
    /// A file could not be read in full and will be retried.
    FileBlocked {
        remote: RemoteId,
        publisher: PeerId,
        file: PackFileId,
        reasons: BlockedReasons,
    },
    /// Entries `from..to` of a log were applied to the tables. Also sent for this
    /// device's own writes.
    Applied {
        device: PeerId,
        container: ContainerId,
        from: u64,
        to: u64,
    },
    /// A log stopped applying entries.
    Stalled {
        device: PeerId,
        container: ContainerId,
        stall: Stall,
    },
    /// A stalled log applied an entry again.
    Recovered {
        device: PeerId,
        container: ContainerId,
    },
    /// A log forked: two different sets of entries after entry `at`.
    Forked {
        device: PeerId,
        container: ContainerId,
        at: u64,
    },
    /// A device was seen for the first time.
    DeviceSeen { device: PeerId },
    /// A sync folder round failed; it is retried with backoff.
    RemoteLost { remote: RemoteId, error: String },
    /// A sync folder that had failed worked again.
    RemoteBack { remote: RemoteId },
    /// A background task stopped (it panicked). It is restarted after a delay.
    TaskStopped { task: Task, error: String },
    /// This subscriber fell behind and lost this many events. Call `status()` again.
    Missed(u64),
}

/// A stream of [`SyncEvent`]s, from [`Replica::watch`]. Ends only when the replica is
/// dropped.
pub struct SyncEvents {
    inner: BoxStream<'static, SyncEvent>,
}

impl Stream for SyncEvents {
    type Item = SyncEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SyncEvent>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl SyncEvents {
    fn new(rx: broadcast::Receiver<SyncEvent>) -> Self {
        let inner = futures::stream::unfold(rx, |mut rx| async move {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(broadcast::error::RecvError::Lagged(n)) => SyncEvent {
                    at: SystemTime::now(),
                    kind: SyncEventKind::Missed(n),
                },
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            Some((ev, rx))
        });
        Self {
            inner: Box::pin(inner),
        }
    }
}

pub(crate) fn event_channel() -> broadcast::Sender<SyncEvent> {
    broadcast::channel(EVENT_BUFFER).0
}

impl<R> ReplicaInner<R> {
    /// Sends an event to every current subscriber. Never blocks; with no subscriber the
    /// event is dropped.
    pub(crate) fn emit(&self, kind: SyncEventKind) {
        let _ = self.sync.events.send(SyncEvent {
            at: SystemTime::now(),
            kind,
        });
    }
}

impl<R> Replica<R> {
    /// Subscribes to sync events from now on.
    pub fn watch(&self) -> SyncEvents {
        SyncEvents::new(self.inner.sync.events.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use futures::{FutureExt, StreamExt};

    use super::*;

    fn ev(n: u64) -> SyncEvent {
        SyncEvent {
            at: SystemTime::UNIX_EPOCH,
            kind: SyncEventKind::DeviceSeen {
                device: PeerId([n as u8; 32]),
            },
        }
    }

    #[test]
    fn lagging_subscriber_gets_missed_then_carries_on() {
        let (tx, rx) = broadcast::channel(4);
        let mut events = SyncEvents::new(rx);
        for i in 0..10 {
            tx.send(ev(i)).unwrap();
        }
        let first = events.next().now_or_never().flatten().unwrap();
        assert_eq!(first.kind, SyncEventKind::Missed(6));
        let next = events.next().now_or_never().flatten().unwrap();
        assert_eq!(next, ev(6));
    }

    #[test]
    fn ends_when_the_sender_is_dropped() {
        let (tx, rx) = broadcast::channel(4);
        let mut events = SyncEvents::new(rx);
        drop(tx);
        assert!(events.next().now_or_never().unwrap().is_none());
    }
}
