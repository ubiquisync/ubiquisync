//! Generic log source that reads entries from the filesystem.
//!
//! [`FsLogSource`] uses codec's [`Decoder`] to convert raw segment bytes
//! into [`DecodedEntry`] values. This allows the same filesystem logic to
//! serve any op vocabulary — table ops, doc ops, or anything else
//! implementing [`Op`].

use crate::batches::{BatchInfo, list_batches};
use crate::peers::{list_peers, peer_dir};
use crate::segments::*;
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use ubiquisync_core::codec::{DecodedEntry, Decoder, Op};
use ubiquisync_core::sync::{LogSource, SyncError};
use ubiquisync_core::uuid::Uuid;

/// Reads decoded entries from peer log directories on the filesystem.
///
/// Each peer's directory contains numbered segment files (see
/// [`segments`](crate::segments)).
/// On each [`read_entries`](LogSource::read_entries) call, the source reads
/// the raw file and decodes via codec's [`Decoder`].
pub struct FsLogSource<E> {
    root: PathBuf,
    /// App-supplied segment magic, checked against the head of every segment
    /// the decoder opens. Must match the magic the [`FsLogSink`](crate::FsLogSink)
    /// wrote with.
    magic: Vec<u8>,
    _phantom: std::marker::PhantomData<E>,
}

impl<E: Op> LogSource<E> for FsLogSource<E> {
    fn list_peers(&self) -> Vec<Uuid> {
        list_peers(&self.root)
    }

    fn read_entries<F, Err>(
        &self,
        peer: &Uuid,
        start_entry_idx: u64,
        mut consumer: F,
    ) -> Result<(), Err>
    where
        Err: From<SyncError>,
        F: FnMut(u64, DecodedEntry<E>) -> ControlFlow<Result<(), Err>>,
    {
        let dir = peer_dir(&self.root, peer);
        let batches = list_batches(&dir);
        iterate_entries(batches, start_entry_idx, &self.magic, &mut consumer)
    }
}

impl<E: Op> FsLogSource<E> {
    pub fn new(root: &Path, magic: &[u8]) -> Self {
        Self {
            root: root.to_path_buf(),
            magic: magic.to_vec(),
            _phantom: std::marker::PhantomData,
        }
    }
}

fn iterate_entries<E, F, Err>(
    batches: Vec<BatchInfo>,
    start_index: u64,
    magic: &[u8],
    consume: &mut F,
) -> Result<(), Err>
where
    E: Op,
    Err: From<SyncError>,
    F: FnMut(u64, DecodedEntry<E>) -> ControlFlow<Result<(), Err>>,
{
    for batch in batches {
        if let Some(sealed_info) = &batch.sealed_info {
            if sealed_info.end_index < start_index {
                continue;
            }
            if sealed_info.compressed {
                // Producer-side errors (`io::Error`, `CodecError`) only
                // satisfy `From<_> for SyncError`. We hop through
                // `SyncError` so the outer `?` can convert via the
                // `Err: From<SyncError>` bound.
                let file = File::open(&batch.path).map_err(SyncError::from)?;
                let reader = GzDecoder::new(file);
                if let Some(decoder) =
                    Decoder::<E, BufReader<GzDecoder<File>>>::new(BufReader::new(reader), magic)
                        .map_err(SyncError::from)?
                    && iterate_segment(batch.start_index, start_index, consume, decoder)?.is_break()
                {
                    return Ok(());
                }
                continue;
            }
        }
        let segments = list_segments(&batch.path);
        for segment in segments {
            let file = File::open(&segment.path).map_err(SyncError::from)?;
            let reader = BufReader::new(file);
            if let Some(decoder) =
                Decoder::<E, BufReader<File>>::new(reader, magic).map_err(SyncError::from)?
                && iterate_segment(segment.start_index, start_index, consume, decoder)?.is_break()
            {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn iterate_segment<E, R, F, Err>(
    segment_start_index: u64,
    consumer_start_index: u64,
    consume: &mut F,
    mut decoder: Decoder<E, R>,
) -> Result<ControlFlow<()>, Err>
where
    E: Op,
    R: BufRead,
    Err: From<SyncError>,
    F: FnMut(u64, DecodedEntry<E>) -> ControlFlow<Result<(), Err>>,
{
    let mut idx = segment_start_index;
    while let Some(e) = decoder.decode_entry().map_err(SyncError::from)? {
        // Skip entries preceding the consumer's requested start. The consumer
        // sees `idx` as the absolute log-entry index, so we advance `idx` for
        // every decoded entry — both skipped and consumed.
        if idx < consumer_start_index {
            idx += 1;
            continue;
        }
        match consume(idx, e) {
            ControlFlow::Continue(()) => {}
            ControlFlow::Break(Ok(())) => return Ok(ControlFlow::Break(())),
            ControlFlow::Break(Err(e)) => return Err(e),
        }
        idx += 1;
    }
    Ok(ControlFlow::Continue(()))
}
