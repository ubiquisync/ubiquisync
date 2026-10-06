use std::{
    borrow::Borrow,
    cmp::min,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::{CipherKeyResolver, NullCipherKeyResolver},
    hlc::WallTime,
    ids::LogId,
    log::{
        LogEntry, LogHashContext, LogValidationError, SegmentCipherError,
        segment::{SegmentDecodeError, SegmentVerifyError},
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
        fs::PackProcessError,
        fs_sync_schema::{BlockedPackInfo, PackReadState},
        ingest::{IngestSource, SegmentBytesResolver, SegmentProcessError},
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
        // let ts = WallTime::now();
        // let cur_files = dedupe_pack_files(cur_files);
        // let mut to_read = vec![];
        // let mut consumed = HashSet::new();
        // let mut blocked = HashMap::new();
        // // we only insert entries for the current set of files in our
        // // to_read, consumed and blocked buckets because if a file
        // // no longer exists, we don't care to maintain any state about it
        // for f in cur_files.iter() {
        //     let r = f.get_ref();
        //     // if we already consumed this pack then we mark it as consumed again
        //     if self.consumed.contains(&f) {
        //         consumed.insert(r);
        //     } else if let Some(blocked_info) = self.blocked.get(&r) {
        //         // we retry if a blocked pack's generation is bumped
        //         if f.generation > blocked_info.file.generation
        //             // or if it's retry timetstamp is up
        //             || blocked_info.next_retry_ts() <= ts
        //             // or if all of its parents are consumed
        //             // TODO: this is a bit overly conservative because the parents could be scheduled to read in this
        //             // round, but we don't have a full dependency tree yet, so for now we'll wait until the next
        //             // round to unblock for these cases
        //             || blocked_info.parents.iter().all(|p| self.consumed.contains(p))
        //         {
        //             to_read.push(blocked_info.todo(f, ts));
        //         } else {
        //             blocked.insert(r, blocked_info.clone());
        //         }
        //     } else {
        //         to_read.push(PackReadTodo::new(f));
        //     }
        // }
        // // order pack files so that we read older ones first
        // to_read.sort_by_key(|v| v.file.seqs.end);
        // PackReadPlan {
        //     new_state: PackReadState { consumed, blocked },
        //     to_read,
        // }
        todo!()
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

// #[derive(Error, Debug)]
// pub enum PackProcessError {
//     #[error("pack store error: {0}")]
//     Store(#[from] PackStoreError),
//     #[error("peer resolve error: {0}")]
//     Peer(#[from] PeerResolveError),
//     #[error("db error: {0}")]
//     Db(#[from] DbError),
//     #[error("cipher error: {0}")]
//     Cipher(#[from] SegmentCipherError),
//     #[error("segment decode error: {0}")]
//     Decode(#[from] SegmentDecodeError),
//     #[error("segment verify error: {0}")]
//     Verify(#[from] SegmentVerifyError),
//     #[error("log validation error: {0}")]
//     LogValidation(#[from] LogValidationError),
//     #[error("op decode error: {0}")]
//     OpDecode(#[from] OpDecodeError),
//     #[error("reducer error: {0}")]
//     Reducer(BoxError),
// }

impl<R: Reducer> ReplicaInner<R> {
    async fn process_pack(
        &self,
        peer_info: &PeerInfo,
        desc: &PackFileDescriptor,
        remote_id: i64,
        store: &PackStore,
    ) -> Result<Option<BlockedPackInfo>, PackProcessError> {
        let Some(signed_header) = store.read_header(desc).await? else {
            // if by the time we go to read he pack it is gone,
            // then it has probably been replaced by a newer generation,
            // we'll get back to it later
            return Ok(None);
        };

        let mut pack_resolver = PackResolver {
            desc: desc.clone(),
            store,
            body: None,
        };

        let mut blocked_info = None;

        for segment in signed_header.header.self_segments.iter() {
            self.process_pack_segment(
                desc,
                &signed_header.header,
                remote_id,
                &mut pack_resolver,
                peer_info,
                segment,
                &mut blocked_info,
            )
            .await?;
        }

        for peer_data in signed_header.header.peer_data.iter() {
            let Some(peer_info) = self.resolve_peer(&peer_data.peer_id).await? else {
                todo!()
            };
            for segment in peer_data.segments.iter() {
                self.process_pack_segment(
                    desc,
                    &signed_header.header,
                    remote_id,
                    &mut pack_resolver,
                    &peer_info,
                    segment,
                    &mut blocked_info,
                )
                .await?;
            }
        }

        Ok(blocked_info)
    }

    async fn process_pack_segment(
        &self,
        pack_desc: &PackFileDescriptor,
        pack_header: &PackHeader,
        remote_id: i64,
        pack_resolver: &mut PackResolver<'_>,
        peer_info: &PeerInfo,
        segment_desc: &SegmentDescriptor,
        blocked_info: &mut Option<BlockedPackInfo>,
    ) -> Result<(), PackProcessError> {
        let hash_ctx = LogHashContext::new(&LogId {
            peer_id: peer_info.peer,
            container_id: segment_desc.container_id,
        });
        let mut resolver = PackSegmentResolver {
            desc: segment_desc.clone(),
            pack_resolver,
        };

        let init_blocked_info = || {
            let ts = WallTime::now().as_millis();
            BlockedPackInfo {
                file: pack_desc.id.clone(),
                parents: pack_header.parents.iter().cloned().collect(),
                read_timestamps: ts..ts,
                attempts: 1,
                need_keys: Default::default(),
                needs_upgrade: false,
                pending: false,
                bad_metadata: false,
                decode_error: false,
            }
        };

        match self
            .try_ingest_segment(
                &hash_ctx,
                peer_info,
                segment_desc,
                &mut resolver,
                &IngestSource::Pack { remote_id },
            )
            .await
        {
            Ok(_) => {}
            Err(e) => match e {
                SegmentProcessError::Pending => {
                    blocked_info.get_or_insert_with(init_blocked_info).pending = true;
                }
                SegmentProcessError::NeedKey(k) => {
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .need_keys
                        .insert(k);
                }
                SegmentProcessError::Unsupported(_) => {
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .needs_upgrade = true;
                }
                SegmentProcessError::SegmentGone => return Err(PackProcessError::PackGone),
                // internal errors - push upwards
                SegmentProcessError::Db(e) => return Err(PackProcessError::Db(e)),
                SegmentProcessError::PackStore(e) => return Err(PackProcessError::Store(e)),
                SegmentProcessError::Internal(e) => return Err(PackProcessError::Internal(e)),
                SegmentProcessError::GetSegment(e) => return Err(PackProcessError::GetSegment(e)),
                // bad segment errors, flag on BlockedPackInfo in such a way that we can figure out if all segments are corrupted or not
                SegmentProcessError::BadMetadata => {
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .bad_metadata = true;
                }
                SegmentProcessError::Decode(_)
                | SegmentProcessError::Encode(_)
                | SegmentProcessError::Verify(_)
                | SegmentProcessError::Cipher(_)
                | SegmentProcessError::LogValidation(_) => {
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .decode_error = true;
                }
            },
        }
        Ok(())
    }
}

struct PackResolver<'a> {
    desc: PackFileDescriptor,
    store: &'a PackStore,
    body: Option<Vec<u8>>,
}

struct PackSegmentResolver<'a: 'b, 'b> {
    desc: SegmentDescriptor,
    pack_resolver: &'b mut PackResolver<'a>,
}

#[async_trait]
impl<'a: 'b, 'b> SegmentBytesResolver for PackSegmentResolver<'a, 'b> {
    async fn fetch_segment_bytes<'c>(
        &'c mut self,
    ) -> Result<Option<&'c [u8]>, SegmentProcessError> {
        if let Some(body) = self.pack_resolver.resolve().await? {
            Ok(Some(&body[self.desc.body_loc.clone()]))
        } else {
            Ok(None)
        }
    }
}

impl<'a> PackResolver<'a> {
    async fn resolve<'b>(&'b mut self) -> Result<Option<&'b [u8]>, SegmentProcessError> {
        if self.body.is_none() {
            if let Some(b) = self.store.read_body(&self.desc).await? {
                self.body = Some(b);
            } else {
                // pack no longer exists
                return Ok(None);
            }
        }
        if let Some(ref body) = self.body {
            Ok(Some(body.as_slice()))
        } else {
            Ok(None)
        }
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
