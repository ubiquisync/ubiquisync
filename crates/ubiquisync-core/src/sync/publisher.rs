//! [`FileLogPublisher`]: publish a node's own log to shared storage.
//!
//! The oplog is the write side of a [`Replica`](super::Replica); this node's own
//! origin is the authoritative record of its writes. The file log is a
//! projection of that one origin onto shared storage for peers to pull. A
//! publisher copies the gap between the two — every own-origin entry the oplog
//! has committed but the file log has not yet received — from the oplog's
//! [`LogSource`] into the [`FileLogSink`], resuming from the file log's own
//! extent. A crash between the oplog commit and the file write leaves the file
//! behind; the next [`sync`](FileLogPublisher::sync) reconciles it.
//!
//! It is the outbound counterpart to [`FileLogPuller`](super::FileLogPuller):
//! the puller reads foreign origins in, the publisher writes this origin out.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::uuid::Uuid;

use super::cursors::HasCursors;
use super::error::SyncError;
use super::file_log::FileLogSink;
use super::source::LogSource;

/// Publishes this node's own-origin entries from an oplog [`LogSource`] into a
/// [`FileLogSink`]. One per node: it only ever copies `sink.self_id()`'s origin,
/// upholding the single-writer file-log invariant.
pub struct FileLogPublisher<E, S, K> {
    source: Rc<S>,
    sink: K,
    self_id: Uuid,
    /// Next own-origin index not yet written to the file log.
    written: u64,
    _marker: PhantomData<E>,
}

impl<E, S, K> FileLogPublisher<E, S, K>
where
    S: LogSource<E>,
    K: FileLogSink<E> + HasCursors,
{
    /// Open a publisher for `sink.self_id()`, seeding the resume point from the
    /// file log's own extent (its cursor for this origin).
    pub async fn open(source: Rc<S>, sink: K) -> Result<Self, SyncError> {
        let self_id = sink.self_id();
        let written = sink.get_cursor(self_id).await?;
        Ok(Self {
            source,
            sink,
            self_id,
            written,
            _marker: PhantomData,
        })
    }

    /// Copy every own-origin entry the oplog holds past the file log's extent
    /// into the file log. Returns how many entries were written.
    pub async fn sync(&mut self) -> Result<u64, SyncError> {
        let target = self.source.get_cursor(self.self_id).await?;
        let mut copied = 0;
        while self.written < target {
            let batch = self.source.read_since(self.self_id, self.written).await?;
            let Some(&(first, _)) = batch.first() else {
                // The cursor claims more, but the source handed back nothing.
                // Stop rather than spin; the next sync retries from here.
                break;
            };
            if first != self.written {
                // A hole between the file log and the oplog. Own-origin writes
                // are gapless, so the source can't backfill it — surface it
                // rather than write a discontiguous segment.
                return Err(SyncError::CursorMismatch {
                    expected_idx: self.written,
                    actual_idx: first,
                });
            }
            self.sink.write(&batch)?;
            copied += batch.len() as u64;
            self.written = batch.last().expect("batch is non-empty").0 + 1;
        }
        Ok(copied)
    }

    /// The next own-origin index not yet published — the file log's extent.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// The file log this publisher writes into.
    pub fn sink(&self) -> &K {
        &self.sink
    }
}
