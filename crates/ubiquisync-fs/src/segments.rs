use crate::batches::ActiveBatch;
use crate::timestamp::format_timestamp;
use std::fs;
use std::path::{Path, PathBuf};
use ubiquisync_core::codec::{Encoder, Op};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_core::sync::SyncError;

#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub start_index: u64,
    pub start_ts: String,
    pub sealed_info: Option<SealedSegmentInfo>,
    pub path: PathBuf,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct SealedSegmentInfo {
    pub end_index: u64,
    pub end_ts: String,
}

pub fn list_segments(dir: &Path) -> Vec<SegmentInfo> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    // Convert fs entries to (name, is_dir, size) triples so parse_segments
    // stays fs-independent and unit-testable. Non-utf8 names or entries with
    // unreadable metadata are silently dropped — same failure mode as a
    // parser rejection. Directories carry whatever length the fs reports for
    // them (often non-zero), but parse_segments drops directories anyway, so
    // their size is never read.
    let named = entries.filter_map(|e| e.ok()).filter_map(|e| {
        let name = e.file_name().to_str()?.to_string();
        let md = e.metadata().ok()?;
        let is_dir = md.is_dir();
        // SegmentInfo.size is `usize` to match `Encoder::size()`. Cast at
        // the fs boundary; segment files won't realistically exceed usize
        // range on 32-bit targets and overflow would be a separate bug.
        let size = md.len() as usize;
        Some((name, is_dir, size))
    });
    parse_segments(dir, named)
}

/// Parse an iterator of `(name, is_dir, size)` triples into sorted, deduped
/// segments.
///
/// Split out from `list_segments` so tests can exercise the parse/sort/dedup
/// logic without touching the filesystem. `dir` is only used to build the
/// `path` field on each `SegmentInfo`; no fs calls happen here.
pub(crate) fn parse_segments(
    dir: &Path,
    entries: impl Iterator<Item = (String, bool, usize)>,
) -> Vec<SegmentInfo> {
    let mut segments: Vec<SegmentInfo> = entries
        .filter_map(|(name, is_dir, size)| {
            // Segments are always files. A directory in this scan is either
            // an unrelated entry or leftover junk — drop it so nobody tries
            // to fs::read a directory as a segment.
            if is_dir {
                return None;
            }

            let parts: Vec<&str> = name.split('-').collect();
            match parts.len() {
                // Active segment: {start_idx}-{start_date}. 2-part shape —
                // still being appended to, no end info yet.
                2 => {
                    let start_idx = parts[0].parse::<u64>().ok()?;
                    Some(SegmentInfo {
                        start_index: start_idx,
                        start_ts: parts[1].to_string(),
                        sealed_info: None,
                        path: dir.join(&name),
                        size,
                    })
                }
                // Sealed segment: {start_idx}-{end_idx}-{start_date}-{end_date}.
                // 4-part shape — immutable, end info recorded.
                4 => {
                    let start_idx = parts[0].parse::<u64>().ok()?;
                    let end_idx = parts[1].parse::<u64>().ok()?;
                    Some(SegmentInfo {
                        start_index: start_idx,
                        start_ts: parts[2].to_string(),
                        sealed_info: Some(SealedSegmentInfo {
                            end_index: end_idx,
                            end_ts: parts[3].to_string(),
                        }),
                        path: dir.join(&name),
                        size,
                    })
                }
                _ => None,
            }
        })
        .collect();

    // Sort ascending by start_index, breaking ties with sealed first so
    // that if an impossible state produced both active+sealed at the same
    // start_index, the immutable one wins. seal is a rename (not a copy),
    // so this shouldn't happen — mirrors the batch dedup, same rationale.
    segments.sort_unstable_by(|a, b| {
        a.start_index
            .cmp(&b.start_index)
            .then_with(|| sealed_priority(a).cmp(&sealed_priority(b)))
    });
    segments.dedup_by(|later, kept| later.start_index == kept.start_index);

    segments
}

fn sealed_priority(seg: &SegmentInfo) -> u8 {
    if seg.sealed_info.is_some() { 0 } else { 1 }
}

pub fn active_segment_name(start_idx: u64, start_date: &str) -> String {
    format!("{}-{}", start_idx, start_date)
}

pub fn sealed_segment_name(start_idx: u64, end_idx: u64, start_date: &str, end_date: &str) -> String {
    format!("{}-{}-{}-{}", start_idx, end_idx, start_date, end_date)
}

pub fn seal_segment(
    info: &mut SegmentInfo,
    end_idx: u64,
    end_ts: Timestamp,
) -> Result<SealedSegmentInfo, SyncError> {
    let ts_str = format_timestamp(end_ts)?;
    let sealed_name =
        sealed_segment_name(info.start_index, end_idx, &info.start_ts, ts_str.as_str());
    let new_path = info.path.with_file_name(sealed_name);
    fs::rename(&info.path, &new_path)?;
    let sealed_info = SealedSegmentInfo {
        end_index: end_idx,
        end_ts: ts_str,
    };
    info.sealed_info = Some(sealed_info.clone());
    info.path = new_path;
    Ok(sealed_info)
}

pub struct ActiveSegment<E> {
    pub info: SegmentInfo,
    /// Encoder that owns the file handle and tracks codec state
    /// (timestamps, uuid dict, entry count).
    pub encoder: Encoder<E, fs::File>,
    pub end_index: u64,
    pub end_ts: Timestamp,
}

pub fn create_segment<'a, E: Op>(
    batch: &'a mut ActiveBatch<E>,
    start_idx: u64,
    start_ts: Timestamp,
    magic: &[u8],
) -> Result<&'a mut ActiveSegment<E>, SyncError> {
    let ts_str = format_timestamp(start_ts)?;
    let path = batch
        .info
        .path
        .join(active_segment_name(start_idx, ts_str.as_str()));
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    // Propagate codec failures as the structured `SyncError::CodecError`
    // (via `From`), matching how `encode_entry` errors surface — don't
    // flatten them into a stringly-typed `EncodingError`.
    let encoder = Encoder::new(file, magic, false)?;
    let info = SegmentInfo {
        start_index: start_idx,
        start_ts: ts_str,
        sealed_info: None,
        path,
        size: encoder.size(),
    };
    let seg = ActiveSegment {
        info,
        end_index: start_idx,
        end_ts: start_ts,
        encoder,
    };
    batch.segment_count += 1;
    batch.active_segment = Some(seg);
    Ok(batch.active_segment.as_mut().unwrap())
}

/// Segments are sealed when they exceed this size. 1MB keeps sealed
/// segments small for faster cloud sync and more frequent gzip. The seal
/// decision and the rename live in the batch sealing logic so the segment's
/// file handle can be closed before the rename (required on Windows).
pub const MAX_SEGMENT_SIZE: usize = 1024 * 1024; // 1 MB

#[cfg(test)]
mod tests {
    use super::*;

    // Placeholder timestamp values used to build test names. The parser
    // splits on '-' and doesn't validate timestamp format — any hyphen-free
    // string works as a stand-in.
    const START_DATE: &str = "20260421120000";
    const END_DATE: &str = "20260421130000";

    // Convenience constructors returning (name, is_dir) pairs in the shape
    // parse_segments consumes. They use the real name helpers so a format
    // change flags in one place, not across every test string literal.
    fn active(start: u64) -> (String, bool, usize) {
        // Segments live as files → is_dir = false. Size is 0 for test
        // purposes; parse_segments treats it as opaque.
        (active_segment_name(start, START_DATE), false, 0)
    }
    fn sealed(start: u64, end: u64) -> (String, bool, usize) {
        // Sealed segments are also files, just with the 4-part name.
        (sealed_segment_name(start, end, START_DATE, END_DATE), false, 0)
    }

    #[test]
    fn parses_active_and_sealed_segments() {
        // Goal: parser recognizes both shapes and populates `sealed_info`
        // only for the sealed variant.

        // Given: one active and one sealed segment at distinct indices.
        let entries = vec![active(1), sealed(2, 10)];

        // When: parse runs with a dummy dir (no fs access happens).
        let segments = parse_segments(Path::new("/fake"), entries.into_iter());

        // Then: both come back, sorted ascending by start_index.
        assert_eq!(segments.len(), 2);

        // Active at index 1: sealed_info is None — that's what distinguishes
        // the active shape.
        assert_eq!(segments[0].start_index, 1);
        assert!(segments[0].sealed_info.is_none());

        // Sealed at index 2: sealed_info populated with end_index + end_date.
        assert_eq!(segments[1].start_index, 2);
        let sealed_info = segments[1].sealed_info.as_ref().expect("sealed_info");
        assert_eq!(sealed_info.end_index, 10);
        assert_eq!(sealed_info.end_ts, END_DATE);
    }

    #[test]
    fn rejects_directories_and_garbage_names() {
        // Goal: entries that can't be a segment file — directories, or
        // names that don't match either shape — are silently dropped.

        let entries = vec![
            // Directory with a segment-shaped name. Shouldn't occur
            // (segments are files) but if it does we drop it so callers
            // don't try to fs::read a directory as a segment.
            (active_segment_name(1, START_DATE), true, 0),
            // 3-part name: matches neither 2-part (Active) nor 4-part
            // (Sealed). Falls through the `_` arm of the match.
            ("3-aaa-bbb".to_string(), false, 0),
            // Non-numeric start_index: parts[0].parse::<u64>() returns
            // None, `?` converts to filter_map drop.
            ("notanumber-20260421".to_string(), false, 0),
            // One valid entry so we confirm the filter isn't nuking
            // everything, just the bad ones.
            sealed(99, 150),
        ];

        // When: parse.
        let segments = parse_segments(Path::new("/fake"), entries.into_iter());

        // Then: only the valid sealed segment survives.
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_index, 99);
        let sealed_info = segments[0].sealed_info.as_ref().expect("sealed_info");
        assert_eq!(sealed_info.end_index, 150);
    }
}
