//! Generic pull-based sync: drain new entries from every known peer into a
//! local [`LogProcessor`].
//!
//! This is the storage- and domain-agnostic core of synchronization. It reads
//! decoded entries from a [`LogSource`] and drives them into a [`LogProcessor`]:
//! real [`LogEntry`](crate::log_entry::LogEntry) values are applied, while
//! expunged markers are recorded (not applied) so the cursor still advances past
//! the gap. Each peer's cursor is the count of entries already processed — the
//! index of the next entry to read — and the processor, typically backed by a
//! materialized store, owns how it is tracked.

use crate::codec::{DecodedEntry, Op};

use super::processor::LogProcessor;
use super::store::LogSource;

/// The outcome of a [`sync`](PullSynchronizer::sync) pass.
#[derive(Debug)]
pub struct SyncResult {
    /// Count of real entries applied to the processor (expunged markers
    /// excluded).
    pub entries_applied: usize,
}

/// Pulls new entries from known peers into a [`LogProcessor`].
///
/// The source returns `DecodedEntry<E>` which may include expunged markers.
/// PullSynchronizer filters those out, computes correct cursor positions that
/// account for expunged gaps, and passes only real `LogEntry<E>` entries
/// to the processor. This is also where hash/signature verification
/// would happen if needed.
pub struct PullSynchronizer<'a, E, S> {
    source: &'a S,
    /// When set, skip this peer during sync. Pass `None` at startup
    /// to include self (crash replay); pass `Some(node_id)` during
    /// normal sync to skip self (already applied by `tx()`).
    skip_id: Option<&'a [u8]>,
    phantom: std::marker::PhantomData<E>,
}

impl<'a, E: Op, S: LogSource<E>> PullSynchronizer<'a, E, S> {
    /// Create a synchronizer over `source`. `skip_id` optionally excludes one
    /// peer from the pass — `None` at startup to include self for crash replay,
    /// `Some(node_id)` during normal sync to skip self.
    pub fn new(source: &'a S, skip_id: Option<&'a [u8]>) -> Self {
        Self {
            source,
            skip_id,
            phantom: Default::default(),
        }
    }

    /// Drain each peer's new entries into `store`: apply real entries, record
    /// expunged markers, and let each write advance that peer's cursor. Returns
    /// how many real entries were applied.
    ///
    /// Each peer's stream is pulled one source-sized batch at a time, looping
    /// with an advancing cursor until [`read_entries`](LogSource::read_entries)
    /// returns empty — so a cold pull doesn't materialize a whole long stream at
    /// once. Each `apply_entry` / `save_expunged_entry` advances the cursor
    /// atomically, so a mid-stream failure leaves everything committed up to
    /// that point and re-reads from the failing index on the next pass — no
    /// separate cursor save to lose.
    pub async fn sync<P: LogProcessor<E>>(&self, store: &mut P) -> Result<SyncResult, P::Error> {
        let peers = self.source.list_peers();
        let mut total_entries = 0;

        for peer_id in &peers {
            if let Some(skip) = self.skip_id
                && peer_id.as_slice() == skip
            {
                continue;
            }

            let mut next_idx = store.get_peer_cursor(peer_id).await?;
            loop {
                let batch = self.source.read_entries(peer_id, next_idx)?;
                // An empty batch means the peer's stream is drained.
                if batch.is_empty() {
                    break;
                }
                for (idx, e) in batch {
                    match e {
                        DecodedEntry::LogEntry(e) => {
                            store.apply_entry(peer_id, idx, &e).await?;
                            total_entries += 1;
                        }
                        DecodedEntry::Expunged(hash) => {
                            store.save_expunged_entry(peer_id, idx, &hash).await?;
                        }
                    }
                    next_idx = idx + 1;
                }
            }
        }

        Ok(SyncResult {
            entries_applied: total_entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::BufRead;

    use async_trait::async_trait;
    use pollster::block_on;

    use super::*;
    use crate::codec::{CodecError, EntryBufferReader, EntryBufferWriter};
    use crate::hlc::Timestamp;
    use crate::log_entry::LogEntry;
    use crate::uuid::Uuid;

    use super::super::error::SyncError;

    // ── A trivial op vocabulary for exercising the engine ────────────────
    // PullSynchronizer only shuttles already-decoded entries, but `E: Op` is part of
    // the contract, so the test op implements the trait with a one-byte body.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestOp(u8);

    impl Op for TestOp {
        fn decode<R: BufRead>(
            _tag: u8,
            r: &mut EntryBufferReader<R>,
        ) -> Result<Self, CodecError> {
            let bytes = r.read_bytes(1)?;
            Ok(TestOp(bytes[0]))
        }

        fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError> {
            w.write_byte(0); // tag
            w.write_byte(self.0); // body
            Ok(())
        }
    }

    fn entry(payload: u8) -> LogEntry<TestOp> {
        LogEntry {
            server_user_id: None,
            timestamp: Timestamp::from_raw(payload as u64),
            op: TestOp(payload),
        }
    }

    // ── Mock source: a fixed stream of decoded entries per peer ──────────
    struct MockSource {
        peers: Vec<Uuid>,
        streams: HashMap<Uuid, Vec<DecodedEntry<TestOp>>>,
    }

    impl LogSource<TestOp> for MockSource {
        fn list_peers(&self) -> Vec<Uuid> {
            self.peers.clone()
        }

        fn read_entries(
            &self,
            peer: &Uuid,
            start_entry_idx: u64,
        ) -> Result<Vec<(u64, DecodedEntry<TestOp>)>, SyncError> {
            let stream = self.streams.get(peer).cloned().unwrap_or_default();
            Ok(stream
                .into_iter()
                .enumerate()
                .skip(usize::try_from(start_entry_idx).expect("cursor exceeds usize"))
                .map(|(i, e)| (i as u64, e))
                .collect())
        }
    }

    // ── Chunking source: returns at most `chunk` entries per call, so the sync
    // loop must make several calls to drain one peer ─────────────────────────
    struct ChunkedSource {
        peer: Uuid,
        stream: Vec<DecodedEntry<TestOp>>,
        chunk: usize,
    }

    impl LogSource<TestOp> for ChunkedSource {
        fn list_peers(&self) -> Vec<Uuid> {
            vec![self.peer]
        }

        fn read_entries(
            &self,
            peer: &Uuid,
            start_entry_idx: u64,
        ) -> Result<Vec<(u64, DecodedEntry<TestOp>)>, SyncError> {
            if peer != &self.peer {
                return Ok(vec![]);
            }
            Ok(self
                .stream
                .iter()
                .cloned()
                .enumerate()
                .skip(usize::try_from(start_entry_idx).expect("cursor exceeds usize"))
                .take(self.chunk)
                .map(|(i, e)| (i as u64, e))
                .collect())
        }
    }

    // ── Mock processor: records applied entries, expunged markers, and the
    // per-peer cursor each write advances ────────────────────────────────
    #[derive(Default)]
    struct MockProcessor {
        applied: Vec<(u64, LogEntry<TestOp>)>,
        expunged: Vec<(u64, blake3::Hash)>,
        cursors: HashMap<Uuid, u64>,
        fail_at: Option<u64>,
    }

    #[async_trait(?Send)]
    impl LogProcessor<TestOp> for MockProcessor {
        type Error = SyncError;

        async fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, Self::Error> {
            Ok(self.cursors.get(peer_id).copied().unwrap_or(0))
        }

        async fn apply_entry(
            &mut self,
            peer_id: &Uuid,
            index: u64,
            entry: &LogEntry<TestOp>,
        ) -> Result<(), Self::Error> {
            if self.fail_at == Some(index) {
                return Err(SyncError::EncodingError("boom".into()));
            }
            self.applied.push((index, entry.clone()));
            self.cursors.insert(*peer_id, index + 1);
            Ok(())
        }

        async fn save_expunged_entry(
            &mut self,
            peer_id: &Uuid,
            index: u64,
            hash: &blake3::Hash,
        ) -> Result<(), Self::Error> {
            self.expunged.push((index, *hash));
            self.cursors.insert(*peer_id, index + 1);
            Ok(())
        }
    }

    fn peer(byte: u8) -> Uuid {
        [byte; 16]
    }

    #[test]
    fn sync_applies_entries_and_advances_cursor() {
        // Goal: a fresh peer's whole stream is applied and the cursor lands
        // just past the last entry.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(
                p,
                vec![
                    DecodedEntry::LogEntry(entry(10)),
                    DecodedEntry::LogEntry(entry(20)),
                ],
            )]),
        };
        let mut store = MockProcessor::default();

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 2);
        assert_eq!(store.applied.len(), 2);
        assert_eq!(store.cursors[&p], 2);
    }

    #[test]
    fn sync_resumes_from_saved_cursor() {
        // Given a cursor already past the first entry, only the tail is read.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(
                p,
                vec![
                    DecodedEntry::LogEntry(entry(10)),
                    DecodedEntry::LogEntry(entry(20)),
                    DecodedEntry::LogEntry(entry(30)),
                ],
            )]),
        };
        let mut store = MockProcessor::default();
        store.cursors.insert(p, 1);

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 2);
        assert_eq!(store.applied[0].0, 1);
        assert_eq!(store.cursors[&p], 3);
    }

    #[test]
    fn sync_skips_expunged_but_counts_them_in_cursor() {
        // Expunged markers are not applied, yet the cursor must advance past
        // them so they aren't re-read on the next sync.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(
                p,
                vec![
                    DecodedEntry::LogEntry(entry(10)),
                    DecodedEntry::Expunged(blake3::hash(b"gone")),
                    DecodedEntry::LogEntry(entry(30)),
                ],
            )]),
        };
        let mut store = MockProcessor::default();

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 2);
        assert_eq!(store.applied.len(), 2);
        assert_eq!(store.expunged.len(), 1, "the marker was recorded, not applied");
        // Cursor sits past the third entry (index 2 + 1), gap included.
        assert_eq!(store.cursors[&p], 3);
    }

    #[test]
    fn sync_skips_configured_peer() {
        // skip_id mirrors "don't replay my own log during normal sync".
        let me = peer(1);
        let other = peer(2);
        let source = MockSource {
            peers: vec![me, other],
            streams: HashMap::from([
                (me, vec![DecodedEntry::LogEntry(entry(10))]),
                (other, vec![DecodedEntry::LogEntry(entry(20))]),
            ]),
        };
        let mut store = MockProcessor::default();

        let skip = me.to_vec();
        let result = block_on(PullSynchronizer::new(&source, Some(&skip)).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 1);
        assert_eq!(store.applied[0].1.op, TestOp(20));
        assert!(!store.cursors.contains_key(&me));
        assert_eq!(store.cursors[&other], 1);
    }

    #[test]
    fn sync_stops_and_leaves_cursor_unadvanced_on_apply_error() {
        // A processor error on the first entry aborts the sync; because the
        // failing entry was never applied, its atomic cursor advance never
        // happened either, so the peer has no recorded cursor.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(
                p,
                vec![
                    DecodedEntry::LogEntry(entry(10)),
                    DecodedEntry::LogEntry(entry(20)),
                ],
            )]),
        };
        let mut store = MockProcessor {
            fail_at: Some(0),
            ..Default::default()
        };

        let err = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap_err();

        assert!(matches!(err, SyncError::EncodingError(_)));
        assert!(store.applied.is_empty());
        assert!(!store.cursors.contains_key(&p));
    }

    #[test]
    fn sync_advances_cursor_past_all_expunged_stream() {
        // A stream of nothing but expunged markers applies zero entries but
        // must still move the cursor past the gap so it isn't re-read.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(
                p,
                vec![
                    DecodedEntry::Expunged(blake3::hash(b"a")),
                    DecodedEntry::Expunged(blake3::hash(b"b")),
                ],
            )]),
        };
        let mut store = MockProcessor::default();

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 0);
        assert!(store.applied.is_empty());
        assert_eq!(store.cursors[&p], 2);
    }

    #[test]
    fn sync_does_no_writes_when_no_new_entries() {
        // A peer already fully drained (cursor at end, nothing new) does no
        // work: nothing applied, and its cursor is untouched.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(p, vec![DecodedEntry::LogEntry(entry(10))])]),
        };
        let mut store = MockProcessor::default();
        store.cursors.insert(p, 1); // already past the only entry

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 0);
        assert!(store.applied.is_empty());
        assert!(store.expunged.is_empty());
        assert_eq!(store.cursors[&p], 1, "cursor unchanged for a drained peer");
    }

    #[test]
    fn sync_drains_multiple_active_peers_independently() {
        // Two active (non-skipped) peers both drain, and each gets its own
        // cursor saved at its own stream length.
        let p1 = peer(1);
        let p2 = peer(2);
        let source = MockSource {
            peers: vec![p1, p2],
            streams: HashMap::from([
                (p1, vec![DecodedEntry::LogEntry(entry(10))]),
                (
                    p2,
                    vec![
                        DecodedEntry::LogEntry(entry(20)),
                        DecodedEntry::LogEntry(entry(30)),
                    ],
                ),
            ]),
        };
        let mut store = MockProcessor::default();

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, 3);
        assert_eq!(store.cursors[&p1], 1);
        assert_eq!(store.cursors[&p2], 2);
    }

    #[test]
    fn sync_commits_entries_before_a_mid_stream_failure() {
        // Each apply advances the cursor atomically, so a failure partway
        // through leaves everything before it committed — cursor included — and
        // the next pass resumes at the failing index. Here entry 0 applies (and
        // advances the cursor to 1), then entry 1 fails.
        let p = peer(1);
        let source = MockSource {
            peers: vec![p],
            streams: HashMap::from([(
                p,
                vec![
                    DecodedEntry::LogEntry(entry(10)),
                    DecodedEntry::LogEntry(entry(20)),
                    DecodedEntry::LogEntry(entry(30)),
                ],
            )]),
        };
        let mut store = MockProcessor {
            fail_at: Some(1),
            ..Default::default()
        };

        let err = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap_err();

        assert!(matches!(err, SyncError::EncodingError(_)));
        assert_eq!(store.applied.len(), 1, "only entry 0 applied");
        assert_eq!(store.applied[0].0, 0);
        assert_eq!(store.cursors[&p], 1, "cursor advanced past the committed entry");
    }

    #[test]
    fn sync_drains_stream_across_multiple_source_batches() {
        // A source that hands back only `chunk` entries per call forces the
        // loop to make several calls; every entry must still apply, in order,
        // and the cursor land at the full length.
        let p = peer(1);
        let len = 13usize;
        let stream: Vec<_> = (0..len)
            .map(|i| DecodedEntry::LogEntry(entry(i as u8)))
            .collect();
        let source = ChunkedSource {
            peer: p,
            stream,
            chunk: 5, // 13 entries drain as 5 + 5 + 3, then an empty call
        };
        let mut store = MockProcessor::default();

        let result = block_on(PullSynchronizer::new(&source, None).sync(&mut store)).unwrap();

        assert_eq!(result.entries_applied, len);
        assert_eq!(store.applied.len(), len);
        // Applied strictly in ascending stream order across the batch boundaries.
        assert!(store.applied.iter().map(|(i, _)| *i).eq(0..len as u64));
        assert_eq!(store.cursors[&p], len as u64);
    }
}
