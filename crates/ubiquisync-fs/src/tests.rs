//! Integration tests for the fs_log layer (sink + source + segments + batches).
//!
//! These tests exercise the durability boundary: how `FsLogSink` lays
//! entries out across batches and segments on disk, and how `FsLogSource`
//! reads them back. They use a real temp directory and the real codec —
//! no mocks — because the bugs that matter here are filesystem-shape bugs.

use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::ops::ControlFlow;

use tempfile::TempDir;
use ubiquisync_core::codec::{
    CodecError, DecodedEntry, EntryBufferReader, EntryBufferWriter, Op,
};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_core::log_entry::LogEntry;
use ubiquisync_core::sync::{LogEntrySink, LogProcessor, LogSource, PullSynchronizer, SyncError};
use ubiquisync_core::uuid::Uuid;

use crate::batches::list_batches;
use crate::peers::peer_dir;
use crate::segments::list_segments;
use crate::sink::FsLogSink;
use crate::source::FsLogSource;

// ── Fixed peer ids ───────────────────────────────────────────────────
// Two distinct Uuids so multi-peer tests can distinguish streams
// without needing randomness.
const NODE_A: Uuid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
const NODE_B: Uuid = [20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35];

// App-supplied segment magic. The fs layer is domain-agnostic; any non-empty
// magic works as long as sink and source agree on it.
const MAGIC: &[u8] = b"FSLOGTEST";

// ── Domain-agnostic test op ──────────────────────────────────────────
// The fs layer is generic over `E: Op` and only moves bytes — it never
// inspects op contents. So tests carry a trivial op that wraps an opaque
// payload, rather than dragging in a real data domain like ubiquisync-tables.
// The payload doubles as a content marker so assertions can tell ops apart.
#[derive(Debug, Clone, PartialEq)]
struct TestOp(Vec<u8>);

impl Op for TestOp {
    fn decode<R: BufRead>(_tag: u8, r: &mut EntryBufferReader<R>) -> Result<Self, CodecError> {
        // Single op kind, so the tag is ignored — the body is just the blob
        // `encode` wrote (length-prefixed by `write_blob`/`read_blob`).
        Ok(TestOp(r.read_blob()?))
    }

    fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError> {
        // Tag 0 — any value but the reserved 0xFF expunged marker.
        w.write_byte(0);
        w.write_blob(&self.0);
        Ok(())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Per-test scratch directory, auto-removed when the returned `TempDir`
/// drops. Tests bind it for their whole body so the root survives across
/// sink drop/reopen boundaries.
fn temp_root() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// Build a Timestamp from raw millis with counter 0. Tests don't care
/// about HLC ordering details — they just need any valid Timestamp.
fn ts(ms: u64) -> Timestamp {
    Timestamp::from_parts(ms, 0)
}

/// A `TestOp` tagged with `key`. Tests use the key bytes as a content
/// marker so assertions can identify which op is which.
fn upsert(key: &[u8]) -> TestOp {
    TestOp(key.to_vec())
}

/// Collect every entry the source yields for `peer` starting at
/// `start_idx` into a Vec. Wraps the callback-based `read_entries` so
/// tests can do straightforward Vec assertions.
fn collect(
    src: &FsLogSource<TestOp>,
    peer: &Uuid,
    start_idx: u64,
) -> Vec<(u64, DecodedEntry<TestOp>)> {
    let mut out = Vec::new();
    src.read_entries::<_, SyncError>(peer, start_idx, |idx, e| {
        out.push((idx, e));
        ControlFlow::Continue(())
    })
    .unwrap();
    out
}

/// Extract the op payload from a decoded entry. Panics on any shape that
/// doesn't match what the tests in this file produce — the goal is loud
/// failure if a test accidentally writes the wrong shape, not graceful
/// handling.
fn op_payload(e: &DecodedEntry<TestOp>) -> Vec<u8> {
    match e {
        DecodedEntry::LogEntry(le) => le.op.0.clone(),
        DecodedEntry::Expunged(_) => panic!("unexpected Expunged entry"),
    }
}

// ── 1. Boot recovery — empty peer directory ─────────────────────────

/// Goal: constructing a sink against a fresh root creates the peer
/// directory but defers creating any batch/segment until the first
/// write. Without this, every short-lived sink would litter the disk
/// with empty batch dirs.
#[test]
fn boot_on_empty_dir_creates_peer_dir_with_no_batches() {
    let root = temp_root();
    let _sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let dir = peer_dir(root.path(), &NODE_A);
    assert!(dir.is_dir(), "peer dir should be created on construction");
    assert!(
        list_batches(&dir).is_empty(),
        "no batch directory should exist before the first write"
    );
}

// ── 2. Write → source roundtrip ─────────────────────────────────────

/// Goal: every op written via `LogEntrySink::write` is readable via
/// `LogSource::read_entries` in insertion order, with the correct
/// global per-peer entry index.
#[test]
fn writes_are_readable_via_source_in_order() {
    let root = temp_root();
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();

    // Three ops in a single write call — sink batches them into one
    // segment, one fsync.
    sink.write(ts(100), None, &[upsert(b"a"), upsert(b"b"), upsert(b"c")])
        .unwrap();

    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);
    let got = collect(&src, &NODE_A, 0);

    // Three entries come back, indexed 0..3 globally per peer.
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].0, 0);
    assert_eq!(got[1].0, 1);
    assert_eq!(got[2].0, 2);
    // Content matches the keys we wrote, in order.
    assert_eq!(op_payload(&got[0].1), b"a");
    assert_eq!(op_payload(&got[1].1), b"b");
    assert_eq!(op_payload(&got[2].1), b"c");
}

// ── 3. Cursor advances correctly ────────────────────────────────────

/// Goal: each `write` returns the next-entry index — the cumulative count
/// of ops written so far. Empty writes are a no-op and report the existing
/// position.
#[test]
fn write_returns_cursor_advanced_by_op_count() {
    let root = temp_root();
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();

    // Single op → cursor 1 (next idx to be written is 1, since this
    // entry occupies idx 0).
    let c1 = sink.write(ts(100), None, &[upsert(b"a")]).unwrap();
    assert_eq!(c1, 1);

    // Two ops → cursor advances by 2 → 3.
    let c2 = sink.write(ts(101), None, &[upsert(b"b"), upsert(b"c")]).unwrap();
    assert_eq!(c2, 3);

    // Empty write must not advance the cursor.
    let c3 = sink.write(ts(102), None, &[]).unwrap();
    assert_eq!(c3, 3);
}

// ── 4. Segment rotation ─────────────────────────────────────────────

/// Goal: writing data that pushes a segment past `MAX_SEGMENT_SIZE`
/// causes that segment to seal after the write, and the next write
/// lands in a freshly-created active segment in the same batch.
///
/// We trigger rotation with a single ~1.1 MB op rather than thousands
/// of small ones — keeps the test fast while still exercising the
/// real seal-on-size code path.
#[test]
fn write_past_segment_size_triggers_rotation() {
    let root = temp_root();
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();

    // 1.1 MB payload — one op exceeds the 1 MB segment threshold.
    let big_key = vec![0u8; 1_100_000];
    sink.write(ts(100), None, &[upsert(&big_key)]).unwrap();

    // The next write should open a new segment because the previous
    // one was sealed during the prior write's tail.
    sink.write(ts(101), None, &[upsert(b"after-rotation")]).unwrap();

    // Inspect the open batch directory.
    let peer = peer_dir(root.path(), &NODE_A);
    let batches = list_batches(&peer);
    let batch = batches.last().expect("expected at least one batch");
    let segments = list_segments(&batch.path);

    assert!(
        segments.len() >= 2,
        "expected ≥2 segments after rotation, got {}",
        segments.len()
    );
    // The first segment must be sealed (end_index recorded in name).
    // The last segment may still be active — we don't assert on it.
    assert!(
        segments[0].sealed_info.is_some(),
        "first segment should be sealed after exceeding size threshold"
    );
}

// ── 5. Multi-peer isolation ─────────────────────────────────────────

/// Goal: two sinks at the same root but with different peer ids
/// produce isolated streams. `list_peers` returns both; reading one
/// peer never returns the other peer's entries.
#[test]
fn two_peers_have_independent_streams() {
    let root = temp_root();
    let mut sink_a: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let mut sink_b: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_B, MAGIC).unwrap();

    sink_a.write(ts(100), None, &[upsert(b"a-only")]).unwrap();
    sink_b
        .write(ts(101), None, &[upsert(b"b-only-1"), upsert(b"b-only-2")])
        .unwrap();

    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);

    // list_peers returns both ids, regardless of order.
    let mut peers = src.list_peers();
    peers.sort();
    let mut expected = vec![NODE_A, NODE_B];
    expected.sort();
    assert_eq!(peers, expected);

    // Peer A sees only its one entry, indexed 0.
    let a_entries = collect(&src, &NODE_A, 0);
    assert_eq!(a_entries.len(), 1);
    assert_eq!(a_entries[0].0, 0);
    assert_eq!(op_payload(&a_entries[0].1), b"a-only");

    // Peer B sees both of its entries — its index space is independent
    // of peer A's, so they start from 0 too.
    let b_entries = collect(&src, &NODE_B, 0);
    assert_eq!(b_entries.len(), 2);
    assert_eq!(b_entries[0].0, 0);
    assert_eq!(b_entries[1].0, 1);
    assert_eq!(op_payload(&b_entries[0].1), b"b-only-1");
    assert_eq!(op_payload(&b_entries[1].1), b"b-only-2");
}

// ── 6. Resume across drop / process boundary ────────────────────────

/// Goal: dropping a sink while it still has an unsealed active
/// segment and creating a new sink against the same root must
/// continue numbering from the right entry index. The source must
/// read both pre-drop and post-reopen entries in a single contiguous
/// stream.
///
/// This exercises the boot-time recovery path that decodes the
/// previously-active segment to count entries (since its name doesn't
/// carry an `end_index`), seals it, and continues.
#[test]
fn reopening_sink_resumes_at_correct_entry_index() {
    let root = temp_root();

    // First sink: write 2 ops, then drop without sealing. The active
    // segment is left in 2-part-name form on disk.
    {
        let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
        let c = sink.write(ts(100), None, &[upsert(b"x"), upsert(b"y")]).unwrap();
        assert_eq!(c, 2, "first sink should report 2 entries");
    }

    // Second sink on the same root must seal the prior active segment
    // during boot recovery and pick up at entry index 2.
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let c = sink.write(ts(101), None, &[upsert(b"z")]).unwrap();
    assert_eq!(c, 3, "post-reopen cursor must reflect prior 2 + new 1");

    // Source reads the full 3-entry stream regardless of the
    // sealed/active segment boundary that now lives in the middle.
    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);
    let got = collect(&src, &NODE_A, 0);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].0, 0);
    assert_eq!(got[1].0, 1);
    assert_eq!(got[2].0, 2);
    assert_eq!(op_payload(&got[0].1), b"x");
    assert_eq!(op_payload(&got[1].1), b"y");
    assert_eq!(op_payload(&got[2].1), b"z");
}

// ── 7. Multi-entry size-seal records the correct end index ──────────

/// Goal (regression): when a segment holding *multiple* entries seals
/// because it crossed the size threshold, its sealed filename must record
/// the inclusive index of its last entry — not the segment's start index. A
/// reopened sink resumes at `end_index + 1`, so a wrong end index silently
/// corrupts per-peer numbering (duplicate indices across the sealed/new
/// boundary).
#[test]
fn multi_entry_size_seal_records_correct_end_index() {
    let root = temp_root();
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();

    // Two ~600 KB ops in one write → 2 entries (idx 0, 1), ~1.2 MB total,
    // which trips the 1 MB seal-on-size once the write completes.
    let half = vec![7u8; 600_000];
    let cursor = sink
        .write(ts(100), None, &[upsert(&half), upsert(&half)])
        .unwrap();
    assert_eq!(cursor, 2);

    // The batch's first segment is sealed and must name its end index as 1
    // (start 0 + 2 entries − 1), not 0.
    let peer = peer_dir(root.path(), &NODE_A);
    let batch = list_batches(&peer).pop().expect("a batch");
    let segments = list_segments(&batch.path);
    let sealed = segments[0]
        .sealed_info
        .as_ref()
        .expect("first segment sealed");
    assert_eq!(
        sealed.end_index, 1,
        "sealed segment must record its last entry index, not its start"
    );

    // Reopen must resume at index 2 (end_index + 1), so the next op gets a
    // fresh index rather than colliding with the sealed segment's entries.
    drop(sink);
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let cursor = sink.write(ts(101), None, &[upsert(b"z")]).unwrap();
    assert_eq!(cursor, 3, "reopen must continue past the size-sealed segment");

    // The full stream reads back as three contiguously-indexed entries.
    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);
    let got = collect(&src, &NODE_A, 0);
    assert_eq!(got.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(op_payload(&got[2].1), b"z");
}

// ── C1. Torn trailing entry stays readable, not bricked ─────────────

/// Goal (regression, C1): a crash can leave the final entry of a segment
/// partially written (writes fsync per batch, not per entry). After reopen,
/// boot recovery seals that segment around the good prefix but leaves the
/// torn bytes on disk, and a fresh write appends a new segment *after* it —
/// making the torn segment interior. The reader must tolerate the torn tail
/// (present the good prefix and continue to the next segment) rather than
/// propagating a decode error, which would stall the peer's sync forever.
#[test]
fn torn_trailing_entry_is_tolerated_by_reader() {
    use std::fs::OpenOptions;

    let root = temp_root();

    // Write three entries into a single unsealed segment, then drop.
    let seg_path = {
        let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
        sink.write(ts(100), None, &[upsert(b"aaaa"), upsert(b"bbbb"), upsert(b"cccccccc")])
            .unwrap();
        let peer = peer_dir(root.path(), &NODE_A);
        let batch = list_batches(&peer).pop().expect("a batch");
        list_segments(&batch.path).pop().expect("a segment").path
    };

    // Corrupt the trailing entry by lopping a few bytes off the end of the
    // segment file — enough to truncate the last entry's integrity trailer
    // so it fails to decode, while leaving the first two entries intact.
    let len = fs::metadata(&seg_path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&seg_path)
        .unwrap()
        .set_len(len - 3)
        .unwrap();

    // Reopen: recovery decodes the good prefix (2 entries), seals the
    // segment at end_index 1, and resumes at index 2. The torn bytes remain
    // in the now-sealed segment.
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let cursor = sink.write(ts(101), None, &[upsert(b"z")]).unwrap();
    assert_eq!(cursor, 3, "recovery should resume past the good prefix");

    // The source reads the good prefix from the (now interior) torn segment
    // and continues into the fresh segment — three contiguous entries, no
    // error.
    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);
    let got = collect(&src, &NODE_A, 0);
    assert_eq!(got.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(op_payload(&got[0].1), b"aaaa");
    assert_eq!(op_payload(&got[1].1), b"bbbb");
    assert_eq!(op_payload(&got[2].1), b"z");
}

// ── M1. Empty/torn tail segment doesn't skip an index ───────────────

/// Goal (regression, M1): a crash between segment creation and the first
/// durable entry leaves a header-only (zero-entry) segment. Recovery must
/// not seal it as if it held one entry — doing so would advance the cursor
/// past the segment's start index, leaving a permanent gap (or a duplicate
/// index once a fresh segment is created at the bumped index). It must
/// resume *at* the start index instead.
#[test]
fn empty_tail_segment_resumes_without_skipping_index() {
    use std::fs::OpenOptions;

    let root = temp_root();

    // Write one entry so a segment file exists, then drop.
    let seg_path = {
        let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
        sink.write(ts(100), None, &[upsert(b"a")]).unwrap();
        let peer = peer_dir(root.path(), &NODE_A);
        let batch = list_batches(&peer).pop().expect("a batch");
        list_segments(&batch.path).pop().expect("a segment").path
    };

    // Truncate to the segment header (magic + 1 flag byte) — a zero-entry
    // segment, as if the process died right after creating the file.
    OpenOptions::new()
        .write(true)
        .open(&seg_path)
        .unwrap()
        .set_len(MAGIC.len() as u64 + 1)
        .unwrap();

    // Reopen: recovery deletes the empty segment and resumes at index 0.
    let mut sink: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let cursor = sink.write(ts(101), None, &[upsert(b"z")]).unwrap();
    assert_eq!(cursor, 1, "must resume at the empty segment's start index, not skip it");

    // Exactly one entry, at index 0 — no gap, no duplicate.
    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);
    let got = collect(&src, &NODE_A, 0);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, 0);
    assert_eq!(op_payload(&got[0].1), b"z");
}

// ── 8. End-to-end through the sync engine ───────────────────────────

/// A minimal [`LogProcessor`] that records what it applied and tracks a
/// per-peer cursor in memory — the apply side of the sync seam, standing
/// in for a real materialized store. The engine drives all peers in one
/// `sync` call, attributing cursors per peer via `save_peer_cursor`.
struct MockProcessor {
    applied: Vec<(u64, Vec<u8>)>,
    cursors: HashMap<Uuid, u64>,
}

impl LogProcessor<TestOp> for MockProcessor {
    type Error = SyncError;

    fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, SyncError> {
        Ok(self.cursors.get(peer_id).copied().unwrap_or(0))
    }

    fn save_peer_cursor(&mut self, peer_id: &Uuid, next_entry_idx: u64) -> Result<(), SyncError> {
        self.cursors.insert(*peer_id, next_entry_idx);
        Ok(())
    }

    fn apply_remote_entry(&mut self, index: u64, entry: &LogEntry<TestOp>) -> Result<(), SyncError> {
        self.applied.push((index, entry.op.0.clone()));
        Ok(())
    }
}

/// Goal: the real [`PullSynchronizer`] driving a real [`FsLogSource`] into
/// a processor applies every peer's on-disk entries exactly once and leaves
/// each per-peer cursor at the stream end. This is the whole core pipeline —
/// fs storage seam + engine + apply seam — running against actual disk I/O,
/// with no mock source.
#[test]
fn pull_synchronizer_drains_fs_source_into_processor() {
    let root = temp_root();

    // Two peers each write a couple of entries to disk.
    let mut sink_a: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_A, MAGIC).unwrap();
    let mut sink_b: FsLogSink<TestOp> = FsLogSink::new(root.path(), &NODE_B, MAGIC).unwrap();
    sink_a.write(ts(100), None, &[upsert(b"a0"), upsert(b"a1")]).unwrap();
    sink_b.write(ts(101), None, &[upsert(b"b0")]).unwrap();

    let src: FsLogSource<TestOp> = FsLogSource::new(root.path(), MAGIC);
    let mut proc = MockProcessor {
        applied: Vec::new(),
        cursors: HashMap::new(),
    };

    // First sync drains every peer the source lists. skip_id None — we want
    // every peer drained, including "self".
    let result = PullSynchronizer::new(&src, None).sync(&mut proc).unwrap();
    assert_eq!(
        result.entries_applied, 3,
        "all three entries across both peers should apply on first sync"
    );

    // Cursors sit at each peer's stream end (next index past the last entry).
    assert_eq!(proc.cursors.get(&NODE_A).copied(), Some(2));
    assert_eq!(proc.cursors.get(&NODE_B).copied(), Some(1));

    // A second sync with no new writes is a clean no-op — idempotent at the
    // stream level because the cursors already point past the end.
    let again = PullSynchronizer::new(&src, None).sync(&mut proc).unwrap();
    assert_eq!(
        again.entries_applied, 0,
        "re-syncing an unchanged source applies nothing"
    );
}
