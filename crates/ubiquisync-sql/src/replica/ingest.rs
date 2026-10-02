use std::{borrow::Borrow, ops::Range};

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    ids::LogId,
    log::{
        ChainHash, LogEntry, LogHashContext, LogValidationError, SegmentCipherError,
        segment::{
            DecodedSegment, SegmentDecodeError, SegmentEncodeError, SegmentReader,
            SegmentVerifyError, encode_segment_plaintext,
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
        segment::GetSegmentError,
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
    #[error("segment encode error: {0}")]
    Encode(#[from] SegmentEncodeError),
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
    #[error("get segment error: {0}")]
    GetSegment(#[from] GetSegmentError),
}

#[async_trait::async_trait]
pub(crate) trait SegmentBytesResolver {
    async fn fetch_segment_bytes<'a>(&'a mut self)
    -> Result<Option<&'a [u8]>, SegmentProcessError>;
}

enum PlaceSegmentDescResult {
    Empty,
    FirstSegment,
    PlaceExact(StreamInfo),
    AlreadyExists,
    NoAnchor,
    Overlaps(Vec<StreamInfo>),
}

fn evaluate_segment_desc(
    segment_desc: &SegmentDescriptor,
    streams: &[StreamInfo],
) -> PlaceSegmentDescResult {
    if segment_desc.idx_range.is_empty() {
        // pathological case of an empty segment
        return PlaceSegmentDescResult::Empty;
    }

    if streams.is_empty() {
        return PlaceSegmentDescResult::FirstSegment;
    }

    let prev_chain = segment_desc.prev_chain();
    let end_chain = segment_desc.end_chain();

    if streams.iter().any(|s| s.head_chain == end_chain) {
        return PlaceSegmentDescResult::AlreadyExists;
    }

    if let Some(stream) = streams.iter().find(|s| s.head_chain == prev_chain) {
        return PlaceSegmentDescResult::PlaceExact(stream.clone());
    }

    if streams.iter().all(|s| s.head_chain.size < prev_chain.size) {
        return PlaceSegmentDescResult::NoAnchor;
    }

    let overlapping = streams
        .iter()
        .filter(|s| prev_chain.size < s.head_chain.size)
        .cloned()
        .collect::<Vec<_>>();

    if overlapping.is_empty() {
        PlaceSegmentDescResult::NoAnchor
    } else {
        PlaceSegmentDescResult::Overlaps(overlapping)
    }
}

impl<R: Reducer> ReplicaInner<R> {
    async fn try_ingest_segment(
        &self,
        hash_ctx: &LogHashContext,
        peer_info: &PeerInfo,
        segment_desc: &SegmentDescriptor,
        segment_resolver: &mut dyn SegmentBytesResolver,
    ) -> Result<(), SegmentProcessError>
where {
        // first aquire the lock for this log
        let guard = self
            .stream_locks
            .lock(&StreamLog::new(peer_info.db_id, segment_desc.container_id))
            .await;

        let streams = self.resolve_streams(&guard).await?;
        match evaluate_segment_desc(segment_desc, &streams) {
            PlaceSegmentDescResult::Empty => Ok(()),
            PlaceSegmentDescResult::FirstSegment => {
                let stream = self.create_stream(&guard, hash_ctx).await?;
                self.place_segment(&guard, &stream, peer_info, hash_ctx, segment_resolver)
                    .await
            }
            PlaceSegmentDescResult::PlaceExact(stream) => {
                self.place_segment(&guard, &stream, peer_info, hash_ctx, segment_resolver)
                    .await
            }
            PlaceSegmentDescResult::AlreadyExists => Ok(()),
            PlaceSegmentDescResult::NoAnchor => Err(SegmentProcessError::Pending),
            PlaceSegmentDescResult::Overlaps(overlapping) => {
                self.try_place_overlapping_segment(
                    &guard,
                    segment_desc,
                    overlapping,
                    &streams,
                    peer_info,
                    hash_ctx,
                    segment_resolver,
                )
                .await
            }
        }
    }

    async fn place_segment(
        &self,
        _guard: &KeyedLockGuard<StreamLog>,
        stream: &StreamInfo,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
    ) -> Result<(), SegmentProcessError> {
        if stream.head_err.is_some() {
            todo!("see if we can place part of the segment")
        }

        let Some(segment) = self
            .fetch_and_verify_remote_segment(peer_info, hash_ctx, segment_resolver)
            .await?
        else {
            todo!("pack disappeared")
        };
        let header = &segment.decoded.header;
        let chain_hash = segment.decoded.head_chain;

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

        // TODO call commit

        Ok(())
    }

    async fn fetch_and_verify_remote_segment<'a>(
        &self,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &'a mut dyn SegmentBytesResolver,
    ) -> Result<Option<ResolvedSegment<'a>>, SegmentProcessError> {
        let Some(segment_bytes) = segment_resolver.fetch_segment_bytes().await? else {
            // pack no longer exists
            return Ok(None);
        };

        let reader = SegmentReader::start(segment_bytes)?;
        let decoded = reader.read(self.key_resolver.as_ref(), hash_ctx).await?;
        decoded.verify(&peer_info.commitment.sig_verify_key)?;
        Ok(Some(ResolvedSegment {
            bytes: segment_bytes,
            decoded,
        }))
    }

    async fn try_place_overlapping_segment(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        segment_desc: &SegmentDescriptor,
        candidate_streams: Vec<StreamInfo>,
        all_streams: &[StreamInfo],
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
    ) -> Result<(), SegmentProcessError> {
        let prev_chain = segment_desc.prev_chain();
        let end_chain = segment_desc.end_chain();
        // first check if we have the segment already
        // this is the common case, so we do it first
        for stream in candidate_streams.iter() {
            if stream.head_chain.size > end_chain.size {
                if let Some(segment) = self
                    .get_segment_for_size(guard, hash_ctx, stream.id, end_chain.size)
                    .await?
                {
                    if segment.entries.iter().any(|e| e.chain_hash == end_chain) {
                        // we already have the segment and we found where it is
                        return Ok(());
                    }
                } else {
                    // TODO we shouldn't end up here, but if we do, what should we do??
                }
            }
        }

        // now we have proved that at least we don't have the end of this segment,
        // so we try to see if we can place the start of the segment anywhere
        for stream in candidate_streams.iter() {
            if let Some(segment) = self
                .get_segment_for_size(guard, hash_ctx, stream.id, prev_chain.size)
                .await?
            {
                if segment.entries.iter().any(|e| e.chain_hash == prev_chain) {
                    return self
                        .place_overlapping_segment(
                            guard,
                            stream,
                            all_streams,
                            segment,
                            peer_info,
                            hash_ctx,
                            segment_resolver,
                        )
                        .await;
                }
            } else {
                // this can happen if the stream we're checking is a fork, and its parent contains this entry
                continue;
            }
        }

        // if we end up here, it's a fork that we can't place and we mark it as pending
        Err(SegmentProcessError::Pending)
    }

    async fn place_overlapping_segment(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        stream: &StreamInfo,
        all_streams: &[StreamInfo],
        existing_segment: DecodedSegment<'_>,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
    ) -> Result<(), SegmentProcessError> {
        let Some(new_segment) = self
            .fetch_and_verify_remote_segment(peer_info, hash_ctx, segment_resolver)
            .await?
        else {
            todo!("pack disappeared")
        };

        // first we're going to check and see if this segment extends
        // any existing head
        let mut extends = all_streams
            .iter()
            .filter(|s| {
                new_segment
                    .decoded
                    .entries
                    .iter()
                    .any(|e| e.chain_hash == s.head_chain)
            })
            .collect::<Vec<_>>();
        extends.sort_by_key(|s| s.head_chain.size);
        if let Some(extends) = extends.last() {
            // TODO check first if there are any forks from this point, because there's a rare chance
            // that some fork stream contains a later entry in the segment
        }
        // let mut suffix = new_segment
        //     .decoded
        //     .entries
        //     .iter()
        //     .skip_while(|e| e.chain_hash.size < stream.head_chain.size);
        // if let Some(head) = suffix.next()
        //     && head.chain_hash == stream.head_chain
        // {
        //     // we can just place this suffix now
        //     // first we re-encode it
        //     // if we can decode plaintext we attempt that first (better compression)
        //     // if not we fall back to opaque encoding
        //     // let suffix = suffix.collect::<Vec<_>>();
        //     // let body = encode_segment_plaintext(
        //     //     &new_segment.decoded.header.signature,
        //     //     &stream.head_chain,
        //     //     &stream.head_cipher,
        //     //     hash_ctx.log_id(),
        //     //     self.key_resolver.as_ref(),
        //     //     &suffix,
        //     // )
        //     // .await?;
        //     todo!("place segment")
        // }

        // now we use the real data from the verified segment
        let prev_chain = new_segment.decoded.header.prev_chain;

        // this is the sad path, we have a fork
        let mut it = existing_segment.entries.iter();
        while let Some(e) = it.next() {
            if e.chain_hash.size < prev_chain.size {
                continue;
            } else if e.chain_hash.size == prev_chain.size {
                if e.chain_hash.hash != prev_chain.hash {
                    // this means the segment we decoded didn't match the header
                    break;
                } else {
                    // now we found the right starting point
                    // we need to walk both iterators forward comparing hashes
                    // until we either:
                    // 1. find new parts of
                    todo!()
                }
            }
        }

        // this means the segment we decoded didn't match the header
        // we could try created a SegmentDescriptor from the real body
        // and placing it again
        todo!()
    }
}

struct ResolvedSegment<'a> {
    pub bytes: &'a [u8],
    pub decoded: DecodedSegment<'a>,
}
