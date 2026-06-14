use crate::batches::{
    ActiveBatch, BATCH_ROLLOVER_FILE_COUNT, BATCH_ROLLOVER_SIZE, create_batch, list_batches,
    seal_batch,
};
use crate::peers::peer_dir;
use crate::segments::{ActiveSegment, create_segment, list_segments, seal_segment};
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ubiquisync_core::codec::{DecodedEntry, Decoder, Op};
use ubiquisync_core::hlc::{Timestamp, wall_ms};
use ubiquisync_core::sync::{LogEntrySink, SyncError};
use ubiquisync_core::uuid::Uuid;

pub struct FsLogSink<E> {
    /// Peer directory: `{root}/{base64(node_id)}/`
    dir: PathBuf,
    /// App-supplied segment magic, written at the head of every segment and
    /// checked on read. Held for the life of the sink so each new segment is
    /// framed identically.
    magic: Vec<u8>,
    next_entry_index: u64,
    active_batch: Option<ActiveBatch<E>>,
    /// Wall-time of the most recent successful `write` (or `new`). Drives
    /// `seal_if_idle` so the sync poller can flush idle segments without
    /// disrupting bursty writers.
    last_write: Instant,
}

impl<E: Op> FsLogSink<E> {
    pub fn new(root: &Path, node_id: &Uuid, magic: &[u8]) -> Result<Self, SyncError> {
        // create peer directory if needed
        let dir = peer_dir(root, node_id);
        fs::create_dir_all(&dir)?;

        // scan all batches in the directory
        let batches = list_batches(&dir);
        if let Some(last_batch) = batches.last() {
            if let Some(sealed_batch_info) = &last_batch.sealed_info {
                // new batch will be created when the first write occurs
                Ok(Self {
                    dir,
                    magic: magic.to_vec(),
                    next_entry_index: sealed_batch_info.end_index + 1,
                    active_batch: None,
                    last_write: Instant::now(),
                })
            } else {
                let segments = list_segments(&last_batch.path);
                if let Some(last_segment) = segments.last() {
                    let sealed_segment_info =
                        if let Some(sealed_segment_info) = &last_segment.sealed_info {
                            sealed_segment_info.clone()
                        } else {
                            // Figure out the end index and timestamp of the last entry in the segment by decoding.
                            let file = File::open(&last_segment.path)?;
                            let (decoded, _) = Decoder::<E, BufReader<File>>::decode_all(
                                BufReader::new(file),
                                magic,
                            );
                            let (count, end_ts) = if let Some(decoded) = decoded {
                                let count = decoded.entries.len() as u64;
                                // Filter out expunged entries.
                                let entries = decoded.entries.iter().filter_map(|e| match e {
                                    DecodedEntry::LogEntry(e) => Some(e),
                                    DecodedEntry::Expunged(_) => None,
                                });
                                if let Some(last_entry) = entries.last() {
                                    (count, Some(last_entry.timestamp))
                                } else {
                                    (count, None)
                                }
                            } else {
                                (0, None)
                            };
                            // Sealed segments store the inclusive last
                            // entry index. The reopen path at line 89/97
                            // adds 1 to recover next_entry_index, so we
                            // must subtract 1 here.
                            let end_idx = last_segment.start_index + count.saturating_sub(1);
                            // default to current wall time if no timestamp in last entry
                            let end_ts = end_ts.unwrap_or(Timestamp::from_parts(wall_ms(), 0));
                            let mut last_segment = last_segment.clone();
                            seal_segment(&mut last_segment, end_idx, end_ts)?
                        };

                    let batch_size = segments.iter().map(|s| s.size).sum();
                    let segment_count = segments.len() as u64;
                    // Check if batch needs to be sealed
                    if batch_size > BATCH_ROLLOVER_SIZE || segment_count > BATCH_ROLLOVER_FILE_COUNT {
                        // Seal batch
                        let mut last_batch = last_batch.clone();
                        seal_batch(&mut last_batch, &sealed_segment_info)?;
                        Ok(Self {
                            dir,
                            magic: magic.to_vec(),
                            next_entry_index: sealed_segment_info.end_index + 1,
                            // will create a new batch when the first write occurs
                            active_batch: None,
                            last_write: Instant::now(),
                        })
                    } else {
                        Ok(Self {
                            dir,
                            magic: magic.to_vec(),
                            next_entry_index: sealed_segment_info.end_index + 1,
                            active_batch: Some(ActiveBatch {
                                info: last_batch.clone(),
                                segment_count,
                                batch_size,
                                active_segment: None,
                            }),
                            last_write: Instant::now(),
                        })
                    }
                } else {
                    // No segments yet, so batch is still active
                    Ok(Self {
                        dir,
                        magic: magic.to_vec(),
                        next_entry_index: last_batch.start_index,
                        active_batch: Some(ActiveBatch {
                            info: last_batch.clone(),
                            segment_count: 0,
                            batch_size: 0,
                            active_segment: None,
                        }),
                        last_write: Instant::now(),
                    })
                }
            }
        } else {
            // first batch will be created when the first write occurs
            Ok(Self {
                dir,
                magic: magic.to_vec(),
                next_entry_index: 0,
                active_batch: None,
                last_write: Instant::now(),
            })
        }
    }

    fn ensure_active_segment(&mut self, ts: Timestamp) -> Result<&mut ActiveSegment<E>, SyncError> {
        // Three-state walk written as sequential checks so the borrow checker
        // doesn't trip on a long-lived `&mut self.active_batch` straddling
        // both arms of an `if let` whose return type carries '1.
        if self.active_batch.is_none() {
            let batch = create_batch(&self.dir, self.next_entry_index, ts)?;
            self.active_batch = Some(batch);
        }
        let next_entry_index = self.next_entry_index;
        let magic = &self.magic;
        let batch = self.active_batch.as_mut().unwrap();
        if batch.active_segment.is_none() {
            return create_segment(batch, next_entry_index, ts, magic);
        }
        Ok(batch.active_segment.as_mut().unwrap())
    }

    /// Seal the current active segment (rename to include end timestamp)
    /// and set `active` to `None`. The next write will create a fresh one.
    /// Public so the sync poller can force-seal for cloud sync visibility.
    pub fn seal(&mut self) -> Result<(), SyncError> {
        if let Some(ref mut batch) = self.active_batch {
            batch.force_seal_segment()?;
        }
        Ok(())
    }

    /// Seal the active segment only if it has been idle for at least
    /// `threshold`. Called periodically by the sync poller so idle data
    /// becomes available for cloud sync without disturbing active writers.
    /// No-op when there's no active segment to seal.
    pub fn seal_if_idle(&mut self, threshold: Duration) -> Result<(), SyncError> {
        let has_active_segment = self
            .active_batch
            .as_ref()
            .and_then(|b| b.active_segment.as_ref())
            .is_some();
        if has_active_segment && self.last_write.elapsed() > threshold {
            self.seal()?;
        }
        Ok(())
    }
}

impl<E: Op> LogEntrySink<E> for FsLogSink<E> {
    fn write(
        &mut self,
        timestamp: Timestamp,
        user_id: Option<Uuid>,
        entries: &[E],
    ) -> Result<u64, SyncError> {
        if entries.is_empty() {
            return Ok(self.next_entry_index);
        }
        let seg = self.ensure_active_segment(timestamp)?;
        for op in entries {
            seg.encoder.encode_entry(op, timestamp, user_id)?;
        }
        seg.info.size = seg.encoder.size();
        // Advance the segment's end markers. `seal_segment` bakes these into
        // the sealed filename (its inclusive last-entry index and timestamp);
        // `create_segment` only seeds them to the segment start, so without
        // this a multi-entry segment that seals — by size here, or later —
        // would record its *start* index as its end, corrupting the
        // next-entry index recovered on reopen.
        seg.end_ts = timestamp;
        seg.end_index = seg.info.start_index + seg.encoder.entry_index() as u64 - 1;
        // Fsync the segment
        seg.encoder.sink_mut().sync_all()?;
        // Update segment size and next entry index
        self.next_entry_index += entries.len() as u64;
        // Seal the segment and batch if needed
        if let Some(batch) = &mut self.active_batch
            && batch.seal_if_needed()?.is_some()
        {
            self.active_batch = None;
        }
        // Drives `seal_if_idle` — only update on successful writes so a long
        // idle period after a failed write still trips the threshold.
        self.last_write = Instant::now();
        Ok(self.next_entry_index)
    }
}

/// Newtype around `Arc<Mutex<FsLogSink<E>>>` so we can implement
/// `LogEntrySink` (foreign trait) on a local type — the orphan rule
/// rejects implementing on the bare `Arc<Mutex<_>>` since the outermost
/// type is foreign. Lets the same sink be shared between the Store
/// (which holds `Box<dyn LogEntrySink>`) and the SyncPoller (which
/// keeps the inner Arc to call inherent sealing methods).
pub struct SharedFsLogSink<E>(pub Arc<Mutex<FsLogSink<E>>>);

impl<E: Op> LogEntrySink<E> for SharedFsLogSink<E> {
    fn write(
        &mut self,
        ts: Timestamp,
        user_id: Option<Uuid>,
        ops: &[E],
    ) -> Result<u64, SyncError> {
        self.0.lock().unwrap().write(ts, user_id, ops)
    }
}
