//! Generic pull-based sync: drain new entries from every known peer into a
//! local [`LogProcessor`].
//!
//! This is the storage- and domain-agnostic core of synchronization. It reads
//! decoded entries from a [`LogSource`], skips expunged markers, tracks each
//! peer's cursor (the count of entries already processed, which is the index of
//! the next entry to read), and hands real [`LogEntry`](crate::log_entry::LogEntry)
//! values to a processor that applies them however it likes. The processor —
//! typically backed by a materialized store — owns cursor persistence.

use std::ops::ControlFlow;

use crate::codec::{DecodedEntry, Op};

use super::processor::LogProcessor;
use super::store::LogSource;

#[derive(Debug)]
pub struct SyncResult {
    pub entries_applied: usize,
}

/// Pulls new entries from known peers into a [`LogProcessor`].
///
/// The source returns `DecodedEntry<E>` which may include expunged markers.
/// PullSync filters those out, computes correct cursor positions that
/// account for expunged gaps, and passes only real `LogEntry<E>` entries
/// to the processor. This is also where hash/signature verification
/// would happen if needed.
pub struct PullSync<'a, E, S> {
    source: &'a S,
    /// When set, skip this peer during sync. Pass `None` at startup
    /// to include self (crash replay); pass `Some(node_id)` during
    /// normal sync to skip self (already applied by `tx()`).
    skip_id: Option<&'a [u8]>,
    phantom: std::marker::PhantomData<E>,
}

impl<'a, E: Op, S: LogSource<E>> PullSync<'a, E, S> {
    pub fn new(source: &'a S, skip_id: Option<&'a [u8]>) -> Self {
        Self {
            source,
            skip_id,
            phantom: Default::default(),
        }
    }

    pub fn sync<P: LogProcessor<E>>(&self, store: &mut P) -> Result<SyncResult, P::Error> {
        let peers = self.source.list_peers();
        let mut total_entries = 0;

        for peer_id in &peers {
            if let Some(skip) = self.skip_id
                && peer_id.as_slice() == skip
            {
                continue;
            }

            let start_idx = store.get_peer_cursor(peer_id)?;
            let mut next_entry_index = start_idx;
            self.source.read_entries(peer_id, start_idx, |idx, e| {
                match e {
                    DecodedEntry::LogEntry(e) => match store.apply_remote_entry(idx, &e) {
                        Ok(()) => {
                            total_entries += 1;
                        }
                        Err(err) => {
                            return ControlFlow::Break(Err(err));
                        }
                    },
                    DecodedEntry::Expunged(_) => {}
                };
                next_entry_index = idx + 1;
                ControlFlow::Continue(())
            })?;
            if next_entry_index > start_idx {
                store.save_peer_cursor(peer_id, next_entry_index)?;
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
    use std::ops::ControlFlow;

    use super::*;
    use crate::codec::{CodecError, EntryBufferReader, EntryBufferWriter};
    use crate::hlc::Timestamp;
    use crate::log_entry::LogEntry;
    use crate::uuid::Uuid;

    use super::super::error::SyncError;

    // ── A trivial op vocabulary for exercising the engine ────────────────
    // PullSync only shuttles already-decoded entries, but `E: Op` is part of
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
            user_id: None,
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

        fn read_entries<F, Err>(
            &self,
            peer: &Uuid,
            start_entry_idx: u64,
            mut consumer: F,
        ) -> Result<(), Err>
        where
            Err: From<SyncError>,
            F: FnMut(u64, DecodedEntry<TestOp>) -> ControlFlow<Result<(), Err>>,
        {
            let stream = self.streams.get(peer).cloned().unwrap_or_default();
            for (i, e) in stream.into_iter().enumerate().skip(start_entry_idx as usize) {
                if let ControlFlow::Break(res) = consumer(i as u64, e) {
                    return res;
                }
            }
            Ok(())
        }
    }

    // ── Mock processor: records applied entries and peer cursors ─────────
    #[derive(Default)]
    struct MockProcessor {
        applied: Vec<(u64, LogEntry<TestOp>)>,
        cursors: HashMap<Uuid, u64>,
        fail_at: Option<u64>,
    }

    impl LogProcessor<TestOp> for MockProcessor {
        type Error = SyncError;

        fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, Self::Error> {
            Ok(self.cursors.get(peer_id).copied().unwrap_or(0))
        }

        fn save_peer_cursor(
            &mut self,
            peer_id: &Uuid,
            next_entry_idx: u64,
        ) -> Result<(), Self::Error> {
            self.cursors.insert(*peer_id, next_entry_idx);
            Ok(())
        }

        fn apply_remote_entry(
            &mut self,
            index: u64,
            entry: &LogEntry<TestOp>,
        ) -> Result<(), Self::Error> {
            if self.fail_at == Some(index) {
                return Err(SyncError::EncodingError("boom".into()));
            }
            self.applied.push((index, entry.clone()));
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

        let result = PullSync::new(&source, None).sync(&mut store).unwrap();

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

        let result = PullSync::new(&source, None).sync(&mut store).unwrap();

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

        let result = PullSync::new(&source, None).sync(&mut store).unwrap();

        assert_eq!(result.entries_applied, 2);
        assert_eq!(store.applied.len(), 2);
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
        let result = PullSync::new(&source, Some(&skip)).sync(&mut store).unwrap();

        assert_eq!(result.entries_applied, 1);
        assert_eq!(store.applied[0].1.op, TestOp(20));
        assert!(!store.cursors.contains_key(&me));
        assert_eq!(store.cursors[&other], 1);
    }

    #[test]
    fn sync_stops_and_skips_cursor_save_on_apply_error() {
        // A processor error mid-stream aborts the whole sync; because the
        // failing entry was never applied, the cursor must not be saved.
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

        let err = PullSync::new(&source, None).sync(&mut store).unwrap_err();

        assert!(matches!(err, SyncError::EncodingError(_)));
        assert!(store.applied.is_empty());
        assert!(!store.cursors.contains_key(&p));
    }
}
