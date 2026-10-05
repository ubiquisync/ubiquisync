use std::{
    borrow::Borrow,
    cmp::min,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
    time::Duration,
};

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::{CipherKeyResolver, NullCipherKeyResolver},
    hlc::{HlcError, WallTime, wall_ms},
    ids::LogId,
    log::{
        LogEntry, LogHashContext, LogValidationError, SegmentCipherError,
        segment::{
            DecodedEntries, PlaintextSegmentEncoding, SegmentDecodeError, SegmentEncoding,
            SegmentReader, SegmentVerifyError, VerifiedSegment,
        },
    },
    pack::{
        PackFileDescriptor, PackFileId, PackHeader, PackRef, PackStore, PackStoreError,
        SegmentDescriptor, dedupe_pack_files,
    },
};

use crate::{
    BoxError,
    db::{
        DbError,
        sea_query::{insert_cols_batch, select_cols, update_cols_batch},
    },
    op::OpDecodeError,
    reducer::Reducer,
    replica::{
        Replica, ReplicaInner,
        fs::PackRemoteProcessError,
        fs_sync_schema::{BlockedPackInfo, PackReadState},
        peers::{PeerInfo, PeerResolveError},
        schema::{CommitErr, segments, streams},
        stream_lock::KeyedLockGuard,
        streams::{StreamInfo, StreamLog},
    },
};

#[derive(Debug, Clone)]
pub(crate) struct PackReadPlan {
    pub new_state: PackReadState,

    /// The files we need to inspect. We only retain this
    /// transiently between directory listings and add files
    /// to the consumed state as we inspect them.
    pub to_read: Vec<PackReadTodo>,
}

#[derive(Debug, Clone)]
pub(crate) struct PackReadTodo {
    file: PackFileId,
    attempts: u64,
    first_last_timestamps: Range<u64>,
}

impl PackReadTodo {
    fn new(file: &PackFileId) -> Self {
        PackReadTodo {
            file: file.clone(),
            attempts: 0,
            first_last_timestamps: 0..0,
        }
    }
}

impl BlockedPackInfo {
    fn todo(&self, file: &PackFileId, ts: u64) -> PackReadTodo {
        PackReadTodo {
            file: file.clone(),
            attempts: self.attempts + 1,
            first_last_timestamps: self.read_timestamps.start..ts,
        }
    }
}

impl PackReadState {
    pub fn prepare_read(self, cur_files: &[PackFileId]) -> PackReadPlan {
        let ts = WallTime::now();
        let cur_files = dedupe_pack_files(cur_files);
        let mut to_read = vec![];
        let mut consumed = HashSet::new();
        let mut blocked = HashMap::new();
        // we only insert entries for the current set of files in our
        // to_read, consumed and blocked buckets because if a file
        // no longer exists, we don't care to maintain any state about it
        for f in cur_files.iter() {
            let r = f.get_ref();
            // if we already consumed this pack then we mark it as consumed again
            if self.consumed.contains(&r) {
                consumed.insert(r);
            } else if let Some(blocked_info) = self.blocked.get(&r) {
                // we retry if a blocked pack's generation is bumped
                if f.generation > blocked_info.file.generation
                    // or if it's retry timetstamp is up
                    || blocked_info.next_retry_ts() <= ts
                    // or if all of its parents are consumed
                    // TODO: this is a bit overly conservative because the parents could be scheduled to read in this
                    // round, but we don't have a full dependency tree yet, so for now we'll wait until the next
                    // round to unblock for these cases
                    || blocked_info.parents.iter().all(|p| self.consumed.contains(p))
                {
                    to_read.push(blocked_info.todo(f, ts));
                } else {
                    blocked.insert(r, blocked_info.clone());
                }
            } else {
                to_read.push(PackReadTodo::new(f));
            }
        }
        // order pack files so that we read older ones first
        to_read.sort_by_key(|v| v.file.seqs.end);
        PackReadPlan {
            new_state: PackReadState { consumed, blocked },
            to_read,
        }
    }
}

impl BlockedPackInfo {
    fn next_retry_duration(&self) -> u64 {
        const RETRY_BASE: u128 = Duration::from_secs(10).as_millis();
        // this caps are retry interval at about 4.5 hours
        (RETRY_BASE * 2u128.pow(min(self.attempts, 15) as u32)) as u64
    }

    fn next_retry_ts(&self) -> u64 {
        self.read_timestamps.end + self.next_retry_duration()
    }
}

#[derive(Error, Debug)]
pub enum PackProcessError {
    #[error("pack store error: {0}")]
    Store(#[from] PackStoreError),
    #[error("peer resolve error: {0}")]
    Peer(#[from] PeerResolveError),
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("cipher error: {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("segment decode error: {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("segment verify error: {0}")]
    Verify(#[from] SegmentVerifyError),
    #[error("log validation error: {0}")]
    LogValidation(#[from] LogValidationError),
    #[error("op decode error: {0}")]
    OpDecode(#[from] OpDecodeError),
    #[error("reducer error: {0}")]
    Reducer(BoxError),
    #[error("pending")]
    Pending,
}

impl<R: Reducer> ReplicaInner<R> {
    async fn process_pack(
        &self,
        peer_info: &PeerInfo,
        desc: &PackFileDescriptor,
        store: &PackStore,
        plan: &mut PackReadPlan,
    ) -> Result<(), PackRemoteProcessError> {
        let Some(signed_header) = store.read_header(desc).await? else {
            // if by the time we go to read he pack it is gone,
            // then it has probably been replaced by a newer generation,
            // we'll get back to it later
            return Ok(());
        };

        let mut body: Option<Vec<u8>> = None;

        // TODO: for each self segment and each peer_data segment call try_ingest_segment
        for _segment in signed_header.header.self_segments.iter() {
            todo!("ingest self segment")
        }

        for peer_data in signed_header.header.peer_data.iter() {
            let _peer_info = self.resolve_peer(&peer_data.peer_id).await?;
            for _segment in peer_data.segments.iter() {
                todo!("ingest peer segment")
            }
        }

        Ok(())
    }

    async fn read_segment_bytes<'a>(
        &self,
        pack_desc: &PackFileDescriptor,
        segment_desc: &SegmentDescriptor,
        store: &PackStore,
        body: &'a mut Option<Vec<u8>>,
    ) -> Result<Option<&'a [u8]>, PackStoreError> {
        let body = if let Some(body) = body {
            body.as_slice()
        } else {
            if let Some(b) = store.read_body(pack_desc).await? {
                let bref = b.as_slice();
                *body = Some(b);
                bref
            } else {
                // pack no longer exists
                return Ok(None);
            }
        };
        Ok(Some(&body[segment_desc.body_loc.clone()]))
    }
}

struct PackProcessState {
    peer_info: PeerInfo,
    file: PackFileId,
    header: PackHeader,
    body: Option<Vec<u8>>,
    key_resolver: Arc<dyn CipherKeyResolver>,
}

// #[derive(Error, Debug)]
// #[error("error resolving pack body")]
// struct PackBodyResolveError;

// #[derive(Error, Debug)]
// enum SegmentResolveError {
//     #[error("resolving pack body: {0}")]
//     BodyResolve(#[from] PackBodyResolveError),
//     #[error("segment decode: {0}")]
//     SegmentDecode(#[from] SegmentDecodeError),
//     #[error("verify error: {0}")]
//     Verify(#[from] SegmentVerifyError),
// }
