use std::{collections::HashSet, ops::Range};

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    hlc::{HlcError, wall_ms},
    ids::LogId,
    log::{
        ChainSeed, LogEntry, LogValidationError, OpOrExpunge, SegmentCipherError,
        segment::{
            PlaintextSegmentEncoding, SegmentDecodeError, SegmentEncoding, SegmentReader,
            SegmentVerifyError, VerifiedSegment,
        },
    },
};
use ubiquisync_fs::pack::{PackFileName, PackHeader, PackRef, SegmentDescriptor};

use crate::{
    BoxError,
    db::{
        DbError,
        sea_query::{insert_cols_batch, update_cols_batch},
    },
    op::OpDecodeError,
    reducer::Reducer,
    replica::{
        Replica,
        peers::{PeerInfo, PeerResolveError},
        schema::{CommitErr, segments, streams},
        stream_lock::KeyedLockGuard,
        streams::{StreamInfo, StreamLog},
    },
};

#[derive(Debug, Clone)]
pub struct PackReadState {
    /// The packs we have already inspected either
    /// directly or in a prior generation.
    /// These get saved in state.
    pub consumed: HashSet<PackRef>,

    pub blocked: Vec<BlockedPackRef>,
}

#[derive(Debug, Clone)]
pub struct PackReadResult {
    pub new_state: PackReadState,

    /// The files we need to inspect. We only retain this
    /// transiently between directory listings and add files
    /// to the consumed state as we inspect them.
    pub to_read: Vec<PackFileName>,
}

#[derive(Debug, Clone)]
pub struct BlockedPackRef {
    pub file: PackFileName,
    pub parents: HashSet<PackRef>,
    pub read_timestamps: Range<u64>,
    pub retries: u64,
}

impl PackReadState {
    pub fn update(&self, ts: u64, cur_files: &[PackFileName]) -> PackReadResult {
        let mut to_read = vec![];
        let mut new_consumed = HashSet::new();
        for f in cur_files.iter() {
            let r = f.get_ref();
            if self.consumed.contains(&r) {
                new_consumed.insert(r);
            } else {
                to_read.push(f.clone());
            }
        }
        to_read.sort_by_key(|v| v.seqs.end);
        // TODO handled blocked packs
        PackReadResult {
            new_state: PackReadState {
                consumed: new_consumed,
                blocked: self.blocked.clone(),
            },
            to_read,
        }
    }
}

#[derive(Error, Debug)]
pub enum ProcessPackError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("peer not initialized: {0:?}")]
    PeerResolve(#[from] PeerResolveError),
}

impl<R: Reducer> Replica<R> {
    async fn process_pack(&self, state: &mut PackProcessState) -> Result<(), ProcessPackError> {
        // TODO maybe we want this to be a stateful consumption of segments to resume after shutdown
        for segment in state.header.self_segments.iter() {
            self.process_pack_segment(state, &state.peer_info, segment)?;
        }

        // TODO peer segments
        for peer_data in state.header.peer_data.iter() {
            let peer_info = self.resolve_peer(&peer_data.peer_id)?;
            for segment in peer_data.segments.iter() {
                self.process_pack_segment(state, &peer_info, segment)?;
            }
        }

        Ok(())
    }

    // TODO: i think we only want these results from this fn:
    // - Pending - mark whole pack as pending so we come back to trying to place this segment later
    // - Ok - we did everything we could with this segment for better or worse
    // - TransientError - some db state error occurred that may resolve later
    // probably all other errors we want to collapse into these or track in the streams table
    async fn process_pack_segment(
        &self,
        state: &mut PackProcessState,
        peer_info: &PeerInfo,
        segment: &SegmentDescriptor,
    ) -> Result<(), SegmentProcessError> {
        // first aquire the lock for this log
        let stream_guard = self
            .stream_locks
            .lock(&StreamLog::new(peer_info.peer_db_id, segment.container_id))
            .await;

        let Range { start, end } = segment.idx_range;
        // first check if we can immediately place this segment or likely already have it
        let streams = self.resolve_streams(&stream_guard).await?;
        if streams.is_empty() {
            // we don't have anything yet so check if this segment starts at 0
            if start != 0 {
                // TODO mark this pack as pending since we can't place this segment yet
            } else {
                // TODO we can immediately place this segment
                // TODO in the future do an authz check for this log
            }
        } else {
            let mut candidate_streams = vec![];
            let mut probably_have_segment = false;
            for s in streams.iter() {
                if s.head_err.is_some() {
                    // TODO in the future we can check the err and see if
                    // we can maybe place some of this segment but we avoid
                    // that complexity for now
                    continue;
                }

                let size = s.head_chain.size;
                if size == end {
                    // TODO we can check directly if we have this segment already by hash
                } else if size > end {
                    // we don't have bloom filters on chain hashes yet,
                    // so we just skip this segment and assume we already have it
                    // at the cost of not detecting forks eagerly
                    // in the future we can probabalistically increase the chance of catching forks early
                    // with cheap bloom filters
                    probably_have_segment = true;
                    break;
                } else if size == start {
                    if s.head_chain.hash == segment.prev_chain {
                        // this is the happy path where we can place the segment immediately
                        return self
                            .place_pack_segment_direct(&stream_guard, state, segment, &s)
                            .await;
                    } else {
                        // this is a sad path this segment does not go on this stream, just keep going
                    }
                } else if size > start {
                    // in this case we may be able to place this segment on this stream
                    // so we mark it as a candidate
                    candidate_streams.push(s);
                } else {
                    // this is a sad path where the segment is ahead of this stream, just keep going
                }
            }
            if probably_have_segment {
                // skip
                return Ok(());
            }
            if candidate_streams.is_empty() {
                // mark this pack as pending since we can't place this segment yet
                return Err(SegmentProcessError::Pending);
            }

            // TODO check the candidate streams to see if we can place this segment
            // look for segment where start_idx <= segment.idx_range.start < end
            // select_cols::<(segments::StreamId, segments::Body)>(
            //     self.db.as_ref(),
            //     Query::select()
            //         .from(segments::Table)
            //         .inner_join(streams::Table, Expr::col((segments::StreamId, streams::Id))),
            //         .and_where(Expr::col(segments::StartIdx).lt(segment.idx_range.start))
            //         .and_where(Expr::col(segments::EndSize).gte(segment.idx_range.start))
            //         .and_where()
            // )
        }

        Ok(())
    }

    async fn place_pack_segment_direct(
        &self,
        _guard: &KeyedLockGuard<StreamLog>,
        pack_state: &mut PackProcessState,
        segment_desc: &SegmentDescriptor,
        stream: &StreamInfo,
    ) -> Result<(), SegmentProcessError> {
        debug_assert_eq!(segment_desc.idx_range.start, stream.head_chain.size);
        debug_assert_eq!(segment_desc.prev_chain, stream.head_chain.hash);
        debug_assert!(stream.head_err.is_none());
        debug_assert!(stream.head_cipher.is_none());

        let segment = pack_state.resolve_segment(segment_desc)?;
        let chain_hash = segment.verified.chain_hash;

        // we always commit in 2 phases:
        // 1. save the segment and advance stream head
        // 2. iterate over each entry we CAN decode and commit

        // 1. save the segment
        let mut batch = self.db.new_batch();
        // TODO should we use original bytes from the pack or re-encode?
        insert_cols_batch::<(
            segments::StreamId,
            segments::StartIdx,
            segments::EndSize,
            segments::Body,
        )>(
            batch.as_mut(),
            (
                stream.id,
                segment_desc.idx_range.start,
                chain_hash.size,
                segment.bytes.to_vec(),
            ),
            Query::insert().into_table(segments::Table),
        )?;

        update_cols_batch::<(streams::HeadSize, streams::HeadHash)>(
            batch.as_mut(),
            (chain_hash.size, chain_hash.hash),
            Query::update()
                .table(streams::Table)
                .and_where(Expr::column(streams::Id).eq(stream.id)),
        )?;

        batch.commit().await?;

        // TODO at this point we can mark this segment as done within any durable PackProcessState
        // commit will auto-resume later via other periodic processes

        // 2. attempt to commit the segment

        if stream.commit_err.is_some() {
            // we actually can't commit now be committing is stalled with an error
            return Ok(());
        }

        if stream.commit_cipher.is_some() {
            todo!("support encryption")
        }

        let mut commit_size = stream.commit_size;
        if commit_size != stream.head_chain.size {
            todo!("internal error, stream sizes don't match expected based on no err")
        }

        let decoded = segment.verified.to_plaintext(&None)?;

        let local_ts = wall_ms();
        for entry in decoded {
            match entry {
                LogEntry::IndexedEntry(entry) => {
                    commit_size += 1;
                    let mut batch = self.db.new_batch();
                    let apply_state = match entry {
                        ubiquisync_core::log::EntryBody::OpBatch(op_batch) => {
                            let ts = op_batch.get_timestamp()?;
                            match self.hlc.observe(ts, local_ts, batch.as_mut()) {
                                Ok(_) => {}
                                Err(HlcError::Skew(_)) => todo!("hlc skew stall"),
                                Err(HlcError::Storage(e)) => {
                                    return Err(e.into());
                                }
                            }

                            let ops = op_batch
                                .ops
                                .into_iter()
                                .filter_map(|e| match e {
                                    OpOrExpunge::Op(o) => Some(o),
                                    OpOrExpunge::Expunge(_) => None,
                                })
                                .collect::<Vec<_>>();

                            let op = self
                                .reducer
                                .codec()
                                .decode(&segment_desc.container_id, &ops)?;
                            // TODO decode errors are either:
                            // - incompatible software version
                            // - frozen stall

                            let read_state = self
                                .reducer
                                .prepare(self.db.as_ref(), &op)
                                .await
                                .map_err(|e| SegmentProcessError::Reducer(Box::new(e)))?;

                            let apply_state = self
                                .reducer
                                .apply(batch.as_mut(), ts, &op, read_state)
                                .map_err(|e| SegmentProcessError::Reducer(Box::new(e)))?;
                            Some(apply_state)
                        }
                        ubiquisync_core::log::EntryBody::UseKey(cipher_info) => {
                            update_cols_batch::<(
                                streams::CommitSize,
                                streams::CommitCipher,
                                streams::CommitErr,
                            )>(
                                batch.as_mut(),
                                (commit_size, Some(cipher_info), Some(CommitErr::NeedKey)),
                                Query::update()
                                    .table(streams::Table)
                                    .and_where(Expr::column(streams::Id).eq(stream.id)),
                            )?;

                            let _ = batch.commit().await?;
                            // we can't resolve encryption keys let so we're done processing
                            // and commit is stalled until encryption support arrives
                            return Ok(());
                        }
                        ubiquisync_core::log::EntryBody::Expunged(_) => {
                            // NOTE: maybe we could skip updating commit index for expunged entries
                            // but for now we don't because an expunge at the end of a segment
                            // could cause an auto-resume proc to loop
                            None
                        }
                    };

                    update_cols_batch::<(streams::CommitSize,)>(
                        batch.as_mut(),
                        (commit_size,),
                        Query::update()
                            .table(streams::Table)
                            .and_where(Expr::column(streams::Id).eq(stream.id)),
                    )?;

                    let batch_result = batch.commit().await?;

                    if let Some(apply_state) = apply_state {
                        self.reducer
                            .post_apply(apply_state, &batch_result)
                            .map_err(|e| SegmentProcessError::Reducer(Box::new(e)))?;
                    }
                }
                LogEntry::Signature(_) => {} // do nothing
            }
        }

        Ok(())
    }
}

#[derive(Error, Debug)]
enum SegmentProcessError {
    #[error("pending other data")]
    Pending,
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("resolve error: {0}")]
    Resolve(#[from] SegmentResolveError),
    #[error("cipher error: {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("log validation error: {0}")]
    LogValidation(#[from] LogValidationError),
    #[error("op decode error: {0}")]
    OpDecode(#[from] OpDecodeError),
    #[error("reducer error: {0}")]
    Reducer(BoxError),
}

struct PackProcessState {
    peer_info: PeerInfo,
    file: PackFileName,
    header: PackHeader,
    body: Option<Vec<u8>>,
}

impl PackProcessState {
    fn resolve_body<'a>(&mut self) -> Result<&'a [u8], PackBodyResolveError> {
        todo!()
    }

    fn resolve_segment<'a>(
        &'a mut self,
        segment_desc: &SegmentDescriptor,
    ) -> Result<ResolvedSegment<'a>, SegmentResolveError> {
        let body = self.resolve_body()?;
        let segment_bytes = &body[segment_desc.body_loc.clone()];
        let reader = SegmentReader::start(segment_bytes)?;
        let segment_header = reader.header();
        if let SegmentEncoding::Plaintext(PlaintextSegmentEncoding {
            outer_encryption: Some(_),
            ..
        }) = &segment_header.encoding
        {
            todo!("encryption not supported yet!");
        }
        let log_id = LogId {
            peer_id: self.peer_info.peer,
            container_id: segment_desc.container_id,
        };
        let seed = ChainSeed::new(&log_id);
        let segment = reader.verify(
            &self.peer_info.commitment.sig_verify_key,
            &None,
            &None,
            &seed,
        )?;
        Ok(ResolvedSegment {
            bytes: segment_bytes,
            verified: segment,
        })
    }
}

struct ResolvedSegment<'a> {
    pub bytes: &'a [u8],
    pub verified: VerifiedSegment<'a>,
}

#[derive(Error, Debug)]
#[error("error resolving pack body")]
struct PackBodyResolveError;

#[derive(Error, Debug)]
enum SegmentResolveError {
    #[error("resolving pack body: {0}")]
    BodyResolve(#[from] PackBodyResolveError),
    #[error("segment decode: {0}")]
    SegmentDecode(#[from] SegmentDecodeError),
    #[error("verify error: {0}")]
    Verify(#[from] SegmentVerifyError),
}
