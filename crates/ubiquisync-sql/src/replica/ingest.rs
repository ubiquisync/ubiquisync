use std::{borrow::Borrow, ops::Range};

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    ids::LogId,
    log::{
        ChainHash, LogEntry, LogHashContext, LogValidationError, SegmentCipherError,
        segment::{
            DecodedEntries, SegmentDecodeError, SegmentReader, SegmentVerifyError, VerifiedSegment,
        },
    },
    pack::SegmentDescriptor,
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
        ReplicaInner,
        peers::PeerInfo,
        schema::{CommitErr, segments, streams},
        stream_lock::KeyedLockGuard,
        streams::{StreamInfo, StreamLog},
    },
};

#[derive(Error, Debug)]
pub(crate) enum SegmentProcessError {
    #[error("pending other data")]
    Pending,
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("segment decode error: {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("segment verify error: {0}")]
    Verify(#[from] SegmentVerifyError),
    #[error("cipher error: {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("log validation error: {0}")]
    LogValidation(#[from] LogValidationError),
    #[error("op decode error: {0}")]
    OpDecode(#[from] OpDecodeError),
    #[error("reducer error: {0}")]
    Reducer(BoxError),
}

#[async_trait::async_trait]
pub(crate) trait SegmentBytesResolver {
    async fn fetch_segment_bytes<'a>(&'a mut self)
    -> Result<Option<&'a [u8]>, SegmentProcessError>;
}

impl<R: Reducer> ReplicaInner<R> {
    async fn try_ingest_segment(
        &self,
        segment_desc: &SegmentDescriptor,
        peer_info: &PeerInfo,
        segment_resolver: &mut dyn SegmentBytesResolver,
    ) -> Result<(), SegmentProcessError>
where {
        // first aquire the lock for this log
        let stream_guard = self
            .stream_locks
            .lock(&StreamLog::new(peer_info.db_id, segment_desc.container_id))
            .await;

        let Range {
            start: segment_start,
            end: segment_end,
        } = segment_desc.idx_range;
        // first check if we can immediately place this segment or likely already have it
        let streams = self.resolve_streams(&stream_guard).await?;
        if streams.is_empty() {
            // we don't have anything yet so check if this segment starts at 0
            if segment_start == 0 {
                // we can place this segment immediately
                // TODO in the future do an authz check for this log
                let hash_ctx = LogHashContext::new(&LogId {
                    peer_id: peer_info.peer,
                    container_id: segment_desc.container_id,
                });
                let stream = self.create_stream(&stream_guard, &hash_ctx).await?;
                return self
                    .place_segment(
                        &stream_guard,
                        &stream,
                        peer_info,
                        &hash_ctx,
                        segment_resolver,
                    )
                    .await;
            } else {
                // mark this pack as pending since we can't place this segment yet
                return Err(SegmentProcessError::Pending);
            }
        } else {
            let mut candidate_streams = vec![];
            let mut probably_have_segment = false;
            for stream in streams.iter() {
                if stream.head_err.is_some() {
                    // TODO in the future we can check the err and see if
                    // we can maybe place some of this segment but we avoid
                    // that complexity for now
                    continue;
                }

                let stream_size = stream.head_chain.size;
                if stream_size == segment_end {
                    if stream.head_chain.hash == segment_desc.end_chain {
                        // we already have this segment and can just mark it as done
                        return Ok(());
                    } else {
                        // this is definitely a fork, let's see if we can place it
                        let segment_start = segment_desc.idx_range.start;
                        let (segment,) = select_cols::<(segments::Body,)>(
                            self.db.as_ref(),
                            Query::select()
                                .from(segments::Table)
                                .and_where(Expr::column(segments::StartIdx).eq(stream.id))
                                .and_where(Expr::column(segments::StartIdx).lt(segment_start))
                                .and_where(Expr::column(segments::EndSize).gte(segment_start)),
                        )
                        .await?
                        .exactly_one()?;

                        todo!("insert fork")
                    }
                } else if stream_size > segment_end {
                    // we don't have bloom filters on chain hashes yet,
                    // so we just skip this segment and assume we already have it
                    // at the cost of not detecting forks eagerly
                    // in the future we can probabalistically increase the chance of catching forks early
                    // with cheap bloom filters
                    probably_have_segment = true;
                    break;
                } else if stream_size == segment_start {
                    if stream.head_chain.hash == segment_desc.prev_chain {
                        // this is the happy path where we can place the segment immediately
                        return self
                            .place_segment(
                                &stream_guard,
                                stream,
                                peer_info,
                                &LogHashContext::new(&LogId {
                                    peer_id: peer_info.peer,
                                    container_id: segment_desc.container_id,
                                }),
                                segment_resolver,
                            )
                            .await;
                    } else {
                        // this is a sad path this segment does not go on this stream, just keep going
                        // maybe there's a different stream it goes on
                        continue;
                    }
                } else if stream_size > segment_start {
                    // in this case we may be able to place this segment on this stream
                    // so we mark it as a candidate
                    candidate_streams.push(stream);
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

    //     pub(crate) async fn try_place_segment(
    //         &self,
    //         stream: &StreamInfo,
    //         segment_desc: &SegmentDescriptor,
    //     ) -> Result<bool, SegmentProcessError> {
    //         if stream.head_err.is_some() {
    //             // TODO in the future we can check the err and see if
    //             // we can maybe place some of this segment but we avoid
    //             // that complexity for now
    //             todo!(
    //                 "this check should actually be placed where after we know whether we _could_ place the segment"
    //             )
    //         }

    //         let segment_start = segment_desc.idx_range.start;
    //         let segment_end = segment_desc.idx_range.end;
    //         let stream_size = stream.head_chain.size;
    //         if stream_size == segment_end && stream.head_chain.hash == segment_desc.end_chain {
    //             // we already have this segment and can just mark it as done
    //             return Ok(true);
    //         }

    //         let (segment,) = select_cols::<(segments::Body,)>(
    //             self.db.as_ref(),
    //             Query::select()
    //                 .from(segments::Table)
    //                 .and_where(Expr::column(segments::StartIdx).eq(stream.id))
    //                 .and_where(Expr::column(segments::StartIdx).lt(segment_start))
    //                 .and_where(Expr::column(segments::EndSize).gte(segment_start)),
    //         )
    //         .await?
    //         .exactly_one()?;
    //     }

    async fn place_segment(
        &self,
        _guard: &KeyedLockGuard<StreamLog>,
        stream: &StreamInfo,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
    ) -> Result<(), SegmentProcessError> {
        debug_assert!(stream.head_err.is_none());
        debug_assert!(stream.head_cipher.is_none());

        let Some(segment) = self
            .fetch_segment(peer_info, hash_ctx, segment_resolver)
            .await?
        else {
            todo!("pack disappeared")
        };
        let header = &segment.verified.decoded.header;
        let chain_hash = segment.verified.head_chain;

        // we always commit in 2 phases:
        // 1. save the segment and advance stream head
        // 2. iterate over each entry we CAN decode and commit

        // 1. save the segment

        let mut batch = self.db.new_batch();
        if header.prev_chain.size < stream.head_chain.size {
            // we need to drop a prefix from the segment because it starts
            // before our existing head
            todo!()
        } else {
            // the segment places directly on top of our existing stream

            // first check that actual hashes line up
            if header.prev_chain.hash != stream.head_chain.hash {
                todo!("hash error")
            }

            insert_cols_batch::<(
                segments::StreamId,
                segments::StartIdx,
                segments::EndSize,
                segments::Body,
            )>(
                batch.as_mut(),
                (
                    stream.id,
                    header.prev_chain.size,
                    chain_hash.size,
                    segment.bytes.to_vec(),
                ),
                Query::insert().into_table(segments::Table),
            )?;
        }

        update_cols_batch::<(streams::HeadSize, streams::HeadHash)>(
            batch.as_mut(),
            (chain_hash.size, chain_hash.hash),
            Query::update()
                .table(streams::Table)
                .and_where(Expr::column(streams::Id).eq(stream.id)),
        )?;

        batch.commit().await?;

        // at this point we could mark this segment as done within any durable pack process state
        // to auto-resume later via other periodic processes, but for now packs succeed or fail completely
        // TODO: we still do need a auto-resume process setup

        // 2. attempt to commit the segment

        if stream.commit_err.is_some() {
            // we actually can't commit now be committing is stalled with an error
            return Ok(());
        }

        let mut commit_size = stream.commit_size;
        if commit_size != stream.head_chain.size {
            todo!("internal error, stream sizes don't match expected based on no err")
        }

        let decoded = match segment.verified.decoded.entries {
            DecodedEntries::Opaque(items) => todo!(),
            DecodedEntries::Plaintext(items) => items,
        };

        let local_ts = wall_ms();
        for entry in decoded {
            match entry {
                LogEntry::IndexedEntry(entry) => {
                    commit_size += 1;
                    let mut batch = self.db.new_batch();
                    let apply_state = match entry {
                        ubiquisync_core::log::EntryBody::Op(op_entry) => {
                            let ts = op_entry.timestamp;
                            match self.hlc.observe(ts, local_ts, batch.as_mut()) {
                                Ok(_) => {}
                                Err(HlcError::Skew(_)) => todo!("hlc skew stall"),
                                Err(HlcError::Storage(e)) => {
                                    return Err(e.into());
                                }
                            }

                            let op = self
                                .reducer
                                .codec()
                                .decode(&hash_ctx.log_id().container_id, op_entry.op.borrow())?;
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
                            todo!("try to resolve the key!");
                            update_cols_batch::<(streams::CommitSize, streams::CommitErr)>(
                                batch.as_mut(),
                                (
                                    commit_size,
                                    Some(CommitErr::NeedKey(cipher_info.fingerprint)),
                                ),
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

    async fn fetch_segment<'a>(
        &self,
        peer_info: &PeerInfo,
        // TODO we don't actually need LogHashContext here because segment.verify constructs one anyway
        hash_ctx: &LogHashContext,
        segment_resolver: &'a mut dyn SegmentBytesResolver,
        prev_chain: &ChainHash,
    ) -> Result<Option<ResolvedSegment<'a>>, SegmentProcessError> {
        let Some(segment_bytes) = segment_resolver.fetch_segment_bytes().await? else {
            // pack no longer exists
            return Ok(None);
        };

        let reader = SegmentReader::start(segment_bytes)?;
        let segment = reader
            .read(self.key_resolver.as_ref(), hash_ctx.log_id())
            .await?;
        let verified = segment
            .verify(
                &peer_info.commitment.sig_verify_key,
                self.key_resolver.as_ref(),
                prev_chain,
            )
            .await?;
        Ok(Some(ResolvedSegment {
            bytes: segment_bytes,
            verified,
        }))
    }
}

struct ResolvedSegment<'a> {
    pub bytes: &'a [u8],
    pub verified: VerifiedSegment<'a>,
}
