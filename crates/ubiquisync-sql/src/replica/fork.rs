use std::cmp::min;

use sea_query::Query;
use ubiquisync_core::{
    crypto::CipherInfo,
    log::{ChainHash, LogHashContext, segment::DecodedSegment},
};

use crate::{
    db::{DbError, sea_query::insert_cols},
    reducer::Reducer,
    replica::{
        ReplicaInner,
        branches::{BranchInfo, LogKey},
        ingest::{IngestSource, SegmentProcessError},
        keyed_lock::KeyedLockGuard,
        schema::branches,
    },
};

impl<R: Reducer> ReplicaInner<R> {
    /// Search for a fork of the branch (or the branch itself) upon which we can place new_segment.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fork_search(
        &self,
        guard: &KeyedLockGuard<LogKey>,
        mut branch: BranchInfo,
        all_branches: &[BranchInfo],
        mut existing_segment: DecodedSegment<'_>,
        new_segment: &DecodedSegment<'_>,
        hash_ctx: &LogHashContext,
        src: &IngestSource,
    ) -> Result<Option<BranchInfo>, SegmentProcessError> {
        let mut cmp_res = compare_segments(
            &existing_segment,
            new_segment,
            new_segment.header.prev_chain,
        );
        'outer: loop {
            match cmp_res {
                CompareSegmentsResult::ExistingExhaused => {
                    let Some(next_segment) = self
                        .get_segment_by_index(
                            guard,
                            hash_ctx,
                            branch.id,
                            existing_segment.head_chain.size,
                        )
                        .await?
                    else {
                        // we probably shouldn't end up here, but if we do it just means we actually don't have a fork
                        return Ok(Some(branch));
                    };
                    cmp_res =
                        compare_segments(&next_segment, new_segment, existing_segment.head_chain);
                    existing_segment = next_segment;
                }
                CompareSegmentsResult::NewExhausted => {
                    // we probably shouldn't end up here because our other ingest code
                    // should check that this segment doesn't already eixt
                    // but if later some other code path does something different we handle this case
                    self.update_published(src, branch.id, new_segment.head_chain.size)
                        .await?;
                    return Ok(None);
                }
                CompareSegmentsResult::Divergent {
                    last_common,
                    cipher_info,
                } => {
                    // look in all_branches to see if we have an existing fork to follow at this point
                    let fork_candidates =
                        find_fork_candidates(&branch, all_branches, last_common.size);
                    let mut dangling = None;
                    for fork in fork_candidates.iter() {
                        if let Some(next_segment) = self
                            .get_segment_by_index(guard, hash_ctx, fork.id, last_common.size)
                            .await?
                        {
                            let cmp = compare_segments(&next_segment, new_segment, last_common);
                            match cmp {
                                CompareSegmentsResult::Divergent {
                                    last_common: new_last_common,
                                    ..
                                } if new_last_common.size == last_common.size => {
                                    // this is not a matching fork
                                    continue;
                                }
                                _ => {
                                    // this matches enough and we can go back to the outer loop
                                    branch = fork.clone();
                                    cmp_res = cmp;
                                    existing_segment = next_segment;
                                    continue 'outer;
                                }
                            }
                        } else {
                            // we found a dangling branch with no segments on top
                            dangling = Some(fork.clone());
                        }
                    }
                    if let Some(dangling) = dangling
                        && dangling.head_chain == last_common
                    {
                        return Ok(Some(dangling));
                    } else {
                        let fork = self
                            .create_fork(guard, &branch, last_common, cipher_info)
                            .await?;
                        return Ok(Some(fork));
                    }
                }
            }
        }
    }

    async fn create_fork(
        &self,
        guard: &KeyedLockGuard<LogKey>,
        parent: &BranchInfo,
        last_common: ChainHash,
        cipher_info: Option<CipherInfo>,
    ) -> Result<BranchInfo, DbError> {
        if guard.key().peer_db_id == self.self_db_id {
            // local writes carry on from the root branch, see `exec`
            tracing::warn!(
                container = ?guard.key().container_id,
                fork_size = last_common.size,
                "our own log has forked: this device was restored from a backup or its db was copied"
            );
        }
        let commit_size = min(parent.commit_size, last_common.size);
        let (id,) = insert_cols::<
            (
                branches::PeerId,
                branches::ContainerId,
                branches::HeadSize,
                branches::HeadHash,
                branches::HeadCipher,
                branches::CommitSize,
                branches::ParentId,
                branches::ForkSize,
            ),
            (branches::Id,),
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
            Query::insert().into_table(branches::Table),
        )
        .await?
        .exactly_one()?;
        Ok(BranchInfo {
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
    mut last_common: ChainHash,
) -> CompareSegmentsResult {
    let start = last_common.size;
    let mut new_it = new.chain_meta_iter(start);
    let mut existing_it = existing.chain_meta_iter(start);
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
        last_common = existing_ch.chain_hash;
        // a segment that starts with a UseKey entry leaves start_cipher out of its header, so
        // at a segment's first point one side can be None while the other has the real cipher
        cipher_info = existing_ch.cipher_info.or(new_ch.cipher_info);
    }
}

pub(crate) enum CompareSegmentsResult {
    ExistingExhaused,
    NewExhausted,
    Divergent {
        last_common: ChainHash,
        cipher_info: Option<CipherInfo>,
    },
}

fn find_fork_candidates(
    parent: &BranchInfo,
    all_branches: &[BranchInfo],
    fork_size: u64,
) -> Vec<BranchInfo> {
    all_branches
        .iter()
        .filter(|s| s.parent_id == Some(parent.id) && s.fork_size == Some(fork_size))
        .cloned()
        .collect()
}
