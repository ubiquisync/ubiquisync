use crate::segments::{ActiveSegment, MAX_SEGMENT_SIZE, SealedSegmentInfo, seal_segment};
use crate::timestamp::format_timestamp;
use std::fs;
use std::path::{Path, PathBuf};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_core::sync::SyncError;

#[derive(Debug, Clone)]
pub struct BatchInfo {
    pub start_index: u64,
    pub start_ts: String,
    pub sealed_info: Option<SealedBatchInfo>,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SealedBatchInfo {
    pub end_index: u64,
    pub end_ts: String,
    /// True if this batch is a .gz pack file; false if it's still a
    /// (post-seal, pre-compaction) directory of segment files.
    pub compressed: bool,
}

// Lists all batches in the given directory.
// Batches are sorted by start_index.
pub fn list_batches(dir: &Path) -> Vec<BatchInfo> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    // Convert fs entries to (name, is_dir) pairs so parse_batches stays
    // fs-independent and unit-testable. Entries with non-utf8 names or
    // unknowable file types are silently dropped — same failure mode as
    // a parser rejection.
    let named = entries.filter_map(|e| e.ok()).filter_map(|e| {
        let name = e.file_name().to_str()?.to_string();
        let is_dir = e.file_type().ok()?.is_dir();
        Some((name, is_dir))
    });
    parse_batches(dir, named)
}

/// Parse an iterator of (name, is_dir) pairs into sorted, deduped batches.
///
/// Split out from `list_batches` so tests can exercise the parse/sort/dedup
/// logic without touching the filesystem. `dir` is only used to build the
/// `path` field on each `BatchInfo`; no fs calls happen here.
pub(crate) fn parse_batches(
    dir: &Path,
    entries: impl Iterator<Item = (String, bool)>,
) -> Vec<BatchInfo> {
    let mut batches: Vec<BatchInfo> = entries
        .filter_map(|(name, is_dir)| {
            // Compressed batches are a single .gz pack file; active and sealed
            // batches are directories. Strip the suffix to parse the shared
            // name body uniformly.
            let (base, compressed) = match name.strip_suffix(".gz") {
                Some(b) => (b, true),
                None => (name.as_str(), false),
            };

            // Name shape and entry kind must agree: .gz implies file, plain
            // implies directory. Mismatches are almost certainly stale junk
            // (e.g. a leftover from a half-finished compaction) — drop them
            // rather than hand a caller a path they'll misuse.
            if is_dir == compressed {
                return None;
            }

            let parts: Vec<&str> = base.split('-').collect();
            match parts.len() {
                // Active batches can't be compressed — no end info yet.
                2 if !compressed => {
                    let start_idx = parts[0].parse::<u64>().ok()?;
                    Some(BatchInfo {
                        start_index: start_idx,
                        start_ts: parts[1].to_string(),
                        sealed_info: None,
                        path: dir.join(&name),
                    })
                }
                4 => {
                    let start_idx = parts[0].parse::<u64>().ok()?;
                    let end_idx = parts[1].parse::<u64>().ok()?;
                    Some(BatchInfo {
                        start_index: start_idx,
                        start_ts: parts[2].to_string(),
                        sealed_info: Some(SealedBatchInfo {
                            end_index: end_idx,
                            end_ts: parts[3].to_string(),
                            compressed,
                        }),
                        path: dir.join(&name),
                    })
                }
                _ => None,
            }
        })
        .collect();

    // Sort by start_index, breaking ties by state priority so the winner
    // (Compressed > Sealed > Active) sits first in each group. dedup_by then
    // drops the loser(s). Compressed+Sealed at the same start_index is the
    // normal grace-window state; Sealed+Active shouldn't happen because seal
    // is a rename not a copy, but handle it defensively so a stray leftover
    // can't confuse the reader.
    batches.sort_unstable_by(|a, b| {
        a.start_index
            .cmp(&b.start_index)
            .then_with(|| state_priority(a).cmp(&state_priority(b)))
    });
    batches.dedup_by(|later, kept| later.start_index == kept.start_index);

    batches
}

fn state_priority(batch: &BatchInfo) -> u8 {
    match &batch.sealed_info {
        Some(info) if info.compressed => 0, // Compressed — grace-window winner
        Some(_) => 1,                       // Sealed, not yet compressed
        None => 2,                          // Active
    }
}

/// Name of an active batch directory: `{start_idx}-{start_date}`.
/// 2-part shape — no end info yet because the batch is still accepting segments.
pub fn active_batch_name(start_idx: u64, start_date: &str) -> String {
    format!("{}-{}", start_idx, start_date)
}

/// Name of a sealed batch directory: `{start_idx}-{end_idx}-{start_date}-{end_date}`.
/// 4-part shape — directory is closed to new segments but not yet compressed.
/// Lives alongside its compressed sibling during the grace window after
/// compaction before the originals are deleted.
pub fn sealed_batch_name(start_idx: u64, end_idx: u64, start_date: &str, end_date: &str) -> String {
    format!("{}-{}-{}-{}", start_idx, end_idx, start_date, end_date)
}

pub fn seal_batch(
    info: &mut BatchInfo,
    last_sealed_segment: &SealedSegmentInfo,
) -> Result<SealedBatchInfo, SyncError> {
    let sealed_name = sealed_batch_name(
        info.start_index,
        last_sealed_segment.end_index,
        &info.start_ts,
        &last_sealed_segment.end_ts,
    );
    let new_path = info.path.with_file_name(sealed_name);
    fs::rename(&info.path, &new_path)?;
    let sealed_info = SealedBatchInfo {
        end_index: last_sealed_segment.end_index,
        end_ts: last_sealed_segment.end_ts.clone(),
        compressed: false,
    };
    info.sealed_info = Some(sealed_info.clone());
    info.path = new_path;
    Ok(sealed_info)
}

pub struct ActiveBatch<E> {
    pub info: BatchInfo,
    pub batch_size: usize,
    pub segment_count: u64,
    pub active_segment: Option<ActiveSegment<E>>,
}

pub fn create_batch<E>(
    peer_dir: &Path,
    start_idx: u64,
    start_ts: Timestamp,
) -> Result<ActiveBatch<E>, SyncError> {
    let start_ts_str = format_timestamp(start_ts)?;
    let dir_name = active_batch_name(start_idx, &start_ts_str);
    let path = peer_dir.join(dir_name);
    fs::create_dir_all(&path)?;
    let batch = BatchInfo {
        start_index: start_idx,
        start_ts: start_ts_str,
        sealed_info: None,
        path: path.to_path_buf(),
    };
    Ok(ActiveBatch {
        info: batch,
        batch_size: 0,
        segment_count: 0,
        active_segment: None,
    })
}

pub const BATCH_ROLLOVER_SIZE: usize = 32 * 1024 * 1024; // 32 MB
pub const BATCH_ROLLOVER_FILE_COUNT: u64 = 2048;

impl<E> ActiveBatch<E> {
    pub fn seal_if_needed(&mut self) -> Result<Option<SealedBatchInfo>, SyncError> {
        self.seal_segment(false)
    }

    pub fn force_seal_segment(&mut self) -> Result<Option<SealedBatchInfo>, SyncError> {
        self.seal_segment(true)
    }

    fn seal_segment(&mut self, force: bool) -> Result<Option<SealedBatchInfo>, SyncError> {
        // Decide whether the active segment should seal before disturbing it.
        let should_seal = self
            .active_segment
            .as_ref()
            .is_some_and(|seg| force || seg.info.size > MAX_SEGMENT_SIZE);
        if !should_seal {
            return Ok(None);
        }
        // Take the segment out and drop its Encoder — closing the open file
        // handle — *before* the rename in `seal_segment`. Renaming a file
        // that still has a live handle fails on Windows (the handle isn't
        // opened with FILE_SHARE_DELETE). Per-write fsync already flushed the
        // bytes durably, so closing here loses nothing.
        let ActiveSegment {
            mut info,
            encoder,
            end_index,
            end_ts,
        } = self.active_segment.take().unwrap();
        drop(encoder);
        let sealed_info = seal_segment(&mut info, end_index, end_ts)?;
        self.batch_size += info.size;
        if self.segment_count > BATCH_ROLLOVER_FILE_COUNT || self.batch_size > BATCH_ROLLOVER_SIZE {
            return Ok(Some(seal_batch(&mut self.info, &sealed_info)?));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Placeholder timestamp values used to build test names. The parser
    // splits on '-' and doesn't validate timestamp format, so any value
    // without a hyphen works.
    const START_DATE: &str = "20260421120000";
    const END_DATE: &str = "20260421130000";

    // Convenience constructors that return (name, is_dir) pairs in the
    // same shape parse_batches consumes. Using the real name helpers here
    // means the tests also exercise the name constructors — if someone
    // changes the format, tests flag it in one place.
    fn active(start: u64) -> (String, bool) {
        // Active batches live as directories → is_dir = true.
        (active_batch_name(start, START_DATE), true)
    }
    fn sealed(start: u64, end: u64) -> (String, bool) {
        // Sealed batches live as directories → is_dir = true.
        (sealed_batch_name(start, end, START_DATE, END_DATE), true)
    }
    /// Name of a compressed batch pack file: the sealed 4-part name plus
    /// `.gz`. A single file (not a directory) holding the re-encoded +
    /// gzipped batch contents. Production only ever *reads* these (the
    /// compaction writer that emits them isn't built yet), so the
    /// constructor lives here with the tests that exercise the read path.
    fn compressed_batch_name(start: u64, end: u64, start_date: &str, end_date: &str) -> String {
        format!("{}-{}-{}-{}.gz", start, end, start_date, end_date)
    }
    fn compressed(start: u64, end: u64) -> (String, bool) {
        // Compressed packs are single files → is_dir = false.
        (compressed_batch_name(start, end, START_DATE, END_DATE), false)
    }

    #[test]
    fn parses_the_three_batch_states() {
        // Goal: parser recognizes Active, Sealed, Compressed and extracts
        // start/end fields correctly from each shape.

        // Given: one entry of each state, with distinct start indices so
        // dedup doesn't fire and all three reach the output.
        let entries = vec![active(1), sealed(2, 10), compressed(11, 20)];

        // When: parse runs with a dummy dir (no fs access happens).
        let batches = parse_batches(Path::new("/fake"), entries.into_iter());

        // Then: three batches come back, sorted ascending by start_index.
        assert_eq!(batches.len(), 3);

        // Active has no sealed_info — that's what distinguishes its shape.
        assert_eq!(batches[0].start_index, 1);
        assert!(batches[0].sealed_info.is_none());

        // Sealed has sealed_info populated with compressed=false.
        assert_eq!(batches[1].start_index, 2);
        let sealed_info = batches[1].sealed_info.as_ref().expect("sealed_info");
        assert_eq!(sealed_info.end_index, 10);
        assert_eq!(sealed_info.end_ts, END_DATE);
        assert!(!sealed_info.compressed);

        // Compressed shares Sealed's shape but came from a .gz file.
        assert_eq!(batches[2].start_index, 11);
        let compressed_info = batches[2].sealed_info.as_ref().expect("sealed_info");
        assert_eq!(compressed_info.end_index, 20);
        assert!(compressed_info.compressed);
    }

    #[test]
    fn dedup_prefers_compressed_over_sealed_at_same_start_index() {
        // Goal: during the grace window, both the compressed pack and the
        // sealed source dir coexist with the same start_index. The reader
        // must pick the pack — it's the authoritative form that survives
        // once the grace window ends and the sealed dir is deleted.

        // Given: a sealed+compressed pair colliding at start=5, plus an
        // unrelated sealed at start=20 to confirm dedup only collapses
        // within a start_index group, not across.
        let entries = vec![sealed(5, 50), compressed(5, 50), sealed(20, 30)];

        // When: parse.
        let batches = parse_batches(Path::new("/fake"), entries.into_iter());

        // Then: two batches survive — the compressed winner at start=5 and
        // the untouched sealed at start=20.
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].start_index, 5);
        assert!(batches[0].sealed_info.as_ref().is_some_and(|s| s.compressed));
        assert_eq!(batches[1].start_index, 20);
        assert!(batches[1].sealed_info.as_ref().is_some_and(|s| !s.compressed));
    }

    #[test]
    fn rejects_kind_mismatches_and_garbage_names() {
        // Goal: entries whose name shape disagrees with their filesystem
        // kind (file vs dir) are dropped, as are names that don't match
        // any known shape. These are treated as stale junk.

        let entries = vec![
            // .gz name but it's a directory — shouldn't happen; drop so
            // nobody tries to read a directory as a pack file.
            (compressed_batch_name(1, 10, START_DATE, END_DATE), true),
            // Sealed dir name but it's a file — also shouldn't happen.
            (sealed_batch_name(2, 10, START_DATE, END_DATE), false),
            // 3-part name — doesn't match 2-part (Active) or 4-part
            // (Sealed/Compressed), so the `_` arm of the match drops it.
            ("3-aaa-bbb".to_string(), true),
            // Non-numeric start_index — parts[0].parse::<u64>() fails and
            // the `?` operator converts the None into a filter_map drop.
            ("notanumber-20260421".to_string(), true),
            // One valid entry so we can confirm the filter isn't eating
            // everything, just the bad ones.
            active(99),
        ];

        // When: parse.
        let batches = parse_batches(Path::new("/fake"), entries.into_iter());

        // Then: exactly one batch — the legitimate active — makes it through.
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].start_index, 99);
        assert!(batches[0].sealed_info.is_none());
    }
}
