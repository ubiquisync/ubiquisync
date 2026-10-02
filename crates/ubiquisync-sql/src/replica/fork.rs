use std::cmp::{max, min};

use async_recursion::async_recursion;
use sea_query::Query;
use ubiquisync_core::{
    crypto::CipherInfo,
    log::{ChainHash, LogHashContext, segment::DecodedSegment},
};

use crate::{
    db::{DbError, sea_query::insert_cols},
    replica::{
        ReplicaInner,
        ingest::SegmentProcessError,
        peers::PeerInfo,
        schema::streams,
        stream_lock::KeyedLockGuard,
        streams::{StreamInfo, StreamLog},
    },
};

pub(crate) enum ForkSearchResult {
    NoAnchor,
    NewFork(StreamInfo),
}

impl<R> ReplicaInner<R> {
    #[async_recursion(Sync)]
    pub(crate) async fn fork_search(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        stream: &StreamInfo,
        all_streams: &[StreamInfo],
        existing_segment: &DecodedSegment<'_>,
        new_segment: &DecodedSegment<'_>,
        peer_info: &PeerInfo,
        hash_ctx: &LogHashContext,
    ) -> Result<ForkSearchResult, SegmentProcessError> {
        match compare_segments(&existing_segment, &new_segment) {
            CompareSegmentsResult::ExistingExhaused => {
                let Some(next_segment) = self
                    .get_segment_by_index(
                        guard,
                        hash_ctx,
                        stream.id,
                        existing_segment.head_chain.size,
                    )
                    .await?
                else {
                    todo!(
                        "this is a weird case because if we end up here but don't have a next segment, we should have must on the chain head"
                    )
                };
                return self
                    .fork_search(
                        guard,
                        stream,
                        all_streams,
                        &next_segment,
                        new_segment,
                        peer_info,
                        hash_ctx,
                    )
                    .await;
            }
            CompareSegmentsResult::NewExhausted => {
                todo!("no fork, we have the entry, but we should have found it before...")
            }
            CompareSegmentsResult::Divergent {
                last_common,
                cipher_info,
            } => {
                if let Some(last_common) = last_common {
                    // look in all_streams to see if we have an existing fork to follow at this point
                    let fork_candidates =
                        find_fork_candidates(stream, all_streams, last_common.size);
                    if fork_candidates.is_empty() {
                        let fork = self
                            .create_fork(guard, stream, last_common, cipher_info)
                            .await?;
                        return Ok(ForkSearchResult::NewFork(fork));
                    } else {
                        for fork in fork_candidates.iter() {
                            if let Some(next_segment) = self
                                .get_segment_by_index(guard, hash_ctx, fork.id, last_common.size)
                                .await?
                            {
                                match self
                                    .fork_search(
                                        guard,
                                        fork,
                                        all_streams,
                                        &next_segment,
                                        new_segment,
                                        peer_info,
                                        hash_ctx,
                                    )
                                    .await?
                                {
                                    ForkSearchResult::NoAnchor => continue,
                                    r @ ForkSearchResult::NewFork(_) => return Ok(r),
                                }
                            } else {
                                // TODO should this happen? it probably doesn't matter either way
                            }
                        }
                    }
                }
                return Ok(ForkSearchResult::NoAnchor);
            }
        }
    }

    async fn create_fork(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        parent: &StreamInfo,
        last_common: ChainHash,
        cipher_info: Option<CipherInfo>,
    ) -> Result<StreamInfo, DbError> {
        let commit_size = min(parent.commit_size, last_common.size);
        let (id,) = insert_cols::<
            (
                streams::PeerId,
                streams::ContainerId,
                streams::HeadSize,
                streams::HeadHash,
                streams::HeadCipher,
                streams::CommitSize,
                streams::ParentId,
                streams::ForkSize,
            ),
            (streams::Id,),
        >(
            self.db.as_ref(),
            (
                guard.key().peer_db_id,
                guard.key().container_id.0,
                last_common.size,
                last_common.hash,
                cipher_info,
                // if parent.commit_size < last_common.size, the commit retry loop needs to check that the parent
                // commits first, before updating the fork
                commit_size,
                Some(parent.id),
                Some(last_common.size),
            ),
            Query::insert().into_table(streams::Table),
        )
        .await?
        .exactly_one()?;
        Ok(StreamInfo {
            id,
            head_chain: last_common,
            head_cipher: cipher_info,
            head_err: None, // TODO: should we set something here??
            commit_size,
            commit_err: None,
            parent_id: Some(parent.id),
            fork_size: Some(last_common.size),
        })
    }
}

pub(crate) fn compare_segments(
    existing: &DecodedSegment<'_>,
    new: &DecodedSegment<'_>,
) -> CompareSegmentsResult {
    let start = max(existing.header.prev_chain.size, new.header.prev_chain.size);
    let mut new_it = new.chain_meta_iter(start);
    let mut existing_it = existing.chain_meta_iter(start);
    let mut last_common = None;
    let mut cipher_info = None;
    loop {
        // check new first, because if they are exhausted at the same time, we mainly care that new is exhausted
        let Some(new_ch) = new_it.next() else {
            return CompareSegmentsResult::NewExhausted;
        };
        let Some(existing_ch) = existing_it.next() else {
            return CompareSegmentsResult::ExistingExhaused;
        };
        debug_assert_eq!(
            existing_ch.chain_hash.size, new_ch.chain_hash.size,
            "it should be impossible for iterators not to iterate the same sizes in order"
        );
        if existing_ch.chain_hash.hash != new_ch.chain_hash.hash {
            return CompareSegmentsResult::Divergent {
                last_common,
                cipher_info,
            };
        }
        last_common = Some(existing_ch.chain_hash);
        debug_assert_eq!(existing_ch.cipher_info, new_ch.cipher_info);
        cipher_info = existing_ch.cipher_info;
    }
}

pub(crate) enum CompareSegmentsResult {
    ExistingExhaused,
    NewExhausted,
    Divergent {
        last_common: Option<ChainHash>,
        cipher_info: Option<CipherInfo>,
    },
}

fn find_fork_candidates(
    parent: &StreamInfo,
    all_streams: &[StreamInfo],
    fork_size: u64,
) -> Vec<StreamInfo> {
    all_streams
        .iter()
        .filter(|s| s.parent_id == Some(parent.id) && s.fork_size == Some(fork_size))
        .cloned()
        .collect()
}
