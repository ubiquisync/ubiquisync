use std::borrow::Cow;

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::{CipherKeyResolveError, RootKey256Fingerprint},
    ids::LogId,
    log::{
        ChainHash, LogDecodeError, LogEntry, LogHashContext, LogValidationError,
        SegmentCipherError,
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

use super::{commit::CommitError, schema::HeadErr};

#[derive(Error, Debug)]
pub(crate) enum SegmentProcessError {
    #[error("pending other data")]
    Pending,
    #[error("corrupt segment metadata, actual hashes don't match descriptor")]
    BadMetadata,
    #[error("need key: {0:?}")]
    NeedKey(RootKey256Fingerprint),
    #[error("segment gone")]
    SegmentGone,
    /// The segment uses a format (encoding, compression, cipher suite, entry type,
    /// signature algorithm) this software version doesn't know; retry after an upgrade.
    #[error("unsupported by this software version: {0}")]
    Unsupported(SegmentDecodeError),

    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("internal error: {0}")]
    Internal(String),
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
    #[error("get segment error: {0}")]
    GetSegment(#[from] GetSegmentError),
}

#[async_trait::async_trait]
pub(crate) trait SegmentBytesResolver {
    async fn fetch_segment_bytes<'a>(&'a mut self)
    -> Result<Option<&'a [u8]>, SegmentProcessError>;
}

pub(crate) enum IngestSource {
    P2p,
    Pack { remote_id: i64 },
}

enum PlaceSegmentDescResult {
    /// An empty segment which can't be placed anywhere.
    Empty,
    /// The first segment in a stream.
    FirstSegment,
    /// Can be placed exactly on top of an existing stream.
    PlaceExact(StreamInfo),
    /// We already have the full segment.
    AlreadyExists(StreamInfo),
    /// There is no existing segment which this segment extends.
    NoAnchor,
    /// This segment may overlap with one or more segments we already have.
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
        if segment_desc.idx_range.start == 0 {
            return PlaceSegmentDescResult::FirstSegment;
        } else {
            return PlaceSegmentDescResult::NoAnchor;
        }
    }

    let prev_chain = segment_desc.prev_chain();
    let end_chain = segment_desc.end_chain();

    if let Some(stream) = streams.iter().find(|s| s.head_chain == end_chain) {
        return PlaceSegmentDescResult::AlreadyExists(stream.clone());
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
        src: &IngestSource,
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
                self.resolve_and_place_segment(
                    &guard,
                    stream,
                    peer_info,
                    hash_ctx,
                    segment_resolver,
                    src,
                )
                .await
            }
            PlaceSegmentDescResult::PlaceExact(stream) => {
                self.resolve_and_place_segment(
                    &guard,
                    stream,
                    peer_info,
                    hash_ctx,
                    segment_resolver,
                    src,
                )
                .await
            }
            PlaceSegmentDescResult::AlreadyExists(stream) => {
                self.update_published(src, stream.id, segment_desc.idx_range.end)
                    .await?;
                Ok(())
            }
            PlaceSegmentDescResult::NoAnchor => Err(SegmentProcessError::Pending),
            PlaceSegmentDescResult::Overlaps(overlapping) => {
                self.try_place_overlapping_segment_desc(
                    &guard,
                    segment_desc,
                    overlapping,
                    &streams,
                    peer_info,
                    hash_ctx,
                    segment_resolver,
                    src,
                )
                .await
            }
        }
    }

    async fn resolve_and_place_segment(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        stream: StreamInfo,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
        src: &IngestSource,
    ) -> Result<(), SegmentProcessError> {
        let segment = self
            .fetch_and_verify_remote_segment(peer_info, hash_ctx, segment_resolver)
            .await?;

        self.place_segment(guard, stream, segment, src).await
    }

    async fn place_segment(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        mut stream: StreamInfo,
        segment: ResolvedSegment<'_>,
        src: &IngestSource,
    ) -> Result<(), SegmentProcessError> {
        if let Some(HeadErr::Blocked(_size)) = stream.head_err {
            todo!(
                "if the stream is blocked, then we need to only ingest up to the specified size, we'll need to add logic for truncating segments to do this"
            )
        }

        let header = &segment.decoded.header;
        let chain_hash = segment.decoded.head_chain;

        // we always commit in 2 phases:
        // 1. save the segment and advance stream head
        // 2. iterate over each entry we CAN decode and commit

        // 1. save the segment

        let body = if header.prev_chain.size < stream.head_chain.size {
            segment
                .decoded
                .reencode_suffix(self.key_resolver.as_ref(), stream.head_chain.size)
                .await?
        } else {
            // the segment places directly on top of our existing stream

            // first check that actual hashes line up
            if header.prev_chain.hash != stream.head_chain.hash {
                return Err(SegmentProcessError::BadMetadata);
            }
            segment.bytes.to_vec()
        };

        let mut batch = self.db.new_batch();
        insert_cols_batch::<(
            segments::StreamId,
            segments::StartIdx,
            segments::EndSize,
            segments::Body,
        )>(
            batch.as_mut(),
            (stream.id, stream.head_chain.size, chain_hash.size, body),
            Query::insert().into_table(segments::Table),
        )?;

        update_cols_batch::<(streams::HeadSize, streams::HeadHash, streams::HeadCipher)>(
            batch.as_mut(),
            (
                chain_hash.size,
                chain_hash.hash,
                segment.decoded.head_cipher,
            ),
            Query::update()
                .table(streams::Table)
                .and_where(Expr::column(streams::Id).eq(stream.id)),
        )?;

        self.update_published_batch(batch.as_mut(), src, stream.id, chain_hash.size)?;

        batch.commit().await?;

        // at this point we could mark this segment as done within any durable pack process state
        // to auto-resume later via other periodic processes, but for now packs succeed or fail completely

        // we should only call try commit when commit size already matched head size, otherwise commit is behind and we'll fail
        if stream.head_chain.size == stream.commit_size {
            // make sure to update the stream based on our latest updates
            stream.head_chain = chain_hash;
            stream.head_cipher = segment.decoded.head_cipher;
            self.try_commit(guard, &mut stream, segment.decoded)
                .await
                .map_err(|e| match e {
                    CommitError::Db(e) => SegmentProcessError::Db(e),
                    CommitError::Internal(e) => SegmentProcessError::Internal(e),
                })?;
        }

        Ok(())
    }

    async fn fetch_and_verify_remote_segment<'a>(
        &self,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &'a mut dyn SegmentBytesResolver,
    ) -> Result<ResolvedSegment<'a>, SegmentProcessError> {
        let Some(segment_bytes) = segment_resolver.fetch_segment_bytes().await? else {
            // pack no longer exists
            return Err(SegmentProcessError::SegmentGone);
        };

        let reader = SegmentReader::start(segment_bytes).map_err(classify_decode_error)?;
        let decoded = reader
            .read(self.key_resolver.as_ref(), hash_ctx)
            .await
            .map_err(classify_decode_error)?;
        decoded.verify(&peer_info.commitment.sig_verify_key)?;
        Ok(ResolvedSegment {
            bytes: segment_bytes,
            decoded,
        })
    }

    async fn try_place_overlapping_segment_desc(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        segment_desc: &SegmentDescriptor,
        candidate_streams: Vec<StreamInfo>,
        all_streams: &[StreamInfo],
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
        src: &IngestSource,
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
                        self.update_published(src, stream.id, end_chain.size)
                            .await?;
                        return Ok(());
                    }
                } else {
                    // this can happen if the stream is a fork and the segment is held by it's parents
                    continue;
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
                        .try_place_overlapping_segment(
                            guard,
                            stream,
                            all_streams,
                            segment,
                            peer_info,
                            hash_ctx,
                            segment_resolver,
                            src,
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

    async fn try_place_overlapping_segment(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        stream: &StreamInfo,
        all_streams: &[StreamInfo],
        existing_segment: DecodedSegment<'_>,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
        segment_resolver: &mut dyn SegmentBytesResolver,
        src: &IngestSource,
    ) -> Result<(), SegmentProcessError> {
        let new_segment = self
            .fetch_and_verify_remote_segment(peer_info, hash_ctx, segment_resolver)
            .await?;

        // first we're going to check and see if this segment extends
        // any existing head
        let new_head = new_segment.decoded.head_chain;
        let stream = if let Some(extends) = all_streams
            .iter()
            // make sure the new segment extens the candidate, otherwise we'd have an empty segment
            // technically we should have filtered this out already, but this is safer
            .filter(|s| new_head.size > s.head_chain.size)
            // make sure this isn't an empty fork branch
            // if we did place on an empty fork branch, we may actually overlap with a real parent
            // so we never want to select empty forks here (they could however be selected below in the actual fork search)
            .filter(|s| s.fork_size != Some(s.head_chain.size))
            .find(|s| {
                new_segment
                    .decoded
                    .entries
                    .iter()
                    .any(|e| e.chain_hash == s.head_chain)
            }) {
            // now we know that this segment extends this stream at some, it just has an overlapping prefix
            // note that we don't need to worry about forks here because a fork cannot happen at the head of any stream
            extends.clone()
        } else if let Some(fork) = self
            .fork_search(
                guard,
                stream.clone(),
                all_streams,
                existing_segment,
                &new_segment.decoded,
                hash_ctx,
            )
            .await?
        {
            fork
        } else {
            // fork search only returns none when we have every entry
            return Ok(());
        };

        self.place_segment(guard, stream, new_segment, src).await
    }
}

fn classify_decode_error(e: SegmentDecodeError) -> SegmentProcessError {
    match e {
        // missing key
        SegmentDecodeError::MissingSegmentKey(ci) => SegmentProcessError::NeedKey(ci.fingerprint),
        SegmentDecodeError::SegmentCipher(SegmentCipherError::KeyResolve(
            CipherKeyResolveError::NotFound(k),
        )) => SegmentProcessError::NeedKey(k),
        // some unknown tag or algorithm
        e @ (SegmentDecodeError::UnknownSegmentEncoding(_)
        | SegmentDecodeError::UnknownCompression(_)
        | SegmentDecodeError::UnknownSignatureAlgorithm(_)
        | SegmentDecodeError::UnknownCipherSuite(_)
        | SegmentDecodeError::SegmentCipher(SegmentCipherError::KeyResolve(
            CipherKeyResolveError::UnknownSuite(_),
        ))
        | SegmentDecodeError::LogDecodeError(
            LogDecodeError::UndecodableEntryType(_) | LogDecodeError::UnknownSignatureAlgorithm(_),
        )) => SegmentProcessError::Unsupported(e),
        // another error that can't be handled
        e => SegmentProcessError::Decode(e),
    }
}

struct ResolvedSegment<'a> {
    pub bytes: &'a [u8],
    pub decoded: DecodedSegment<'a>,
}
