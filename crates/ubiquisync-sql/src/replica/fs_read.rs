use std::{
    cmp::{Reverse, max},
    collections::HashSet,
    ops::Range,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use ubiquisync_core::{
    crypto::{CipherKeyResolver, Hash256, TaggedHashDomain, tagged_hash},
    hlc::WallTime,
    ids::LogId,
    log::LogHashContext,
    pack::{
        PackFileDescriptor, PackFileId, PackHeader, PackHeaderDecodeError, PackRef, PackStore,
        PackStoreError, SegmentDescriptor, SignedPackHeader, dedupe_pack_files,
    },
};

use crate::{
    reducer::Reducer,
    replica::{
        config::PackSyncConfig,
        ReplicaInner,
        fs::PackProcessError,
        fs_sync_schema::{BlockedPackInfo, BlockedReasons, PackReadState},
        ingest::{IngestSource, SegmentBytesResolver, SegmentProcessError},
        peers::PeerInfo,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct PackReadPlan {
    pub next: PackReadState,

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
    fn new(file: PackFileId, ts: WallTime) -> Self {
        let ts = ts.as_millis();
        PackReadTodo {
            file,
            attempts: 0,
            first_last_timestamps: ts..ts,
        }
    }

    fn blocked_again(&self, pack_desc: &PackFileDescriptor) -> BlockedPackInfo {
        let ts = WallTime::now().as_millis();
        BlockedPackInfo {
            file: pack_desc.id.clone(),
            pending_parents: Default::default(),
            read_timestamps: self.first_last_timestamps.start..ts,
            attempts: self.attempts + 1,
            need_keys: Default::default(),
            reasons: BlockedReasons::new(),
            missing_peers: Default::default(),
        }
    }

    pub(crate) fn file(&self) -> &PackFileId {
        &self.file
    }
}

impl BlockedPackInfo {
    fn todo(&self, file: PackFileId, ts: WallTime) -> PackReadTodo {
        PackReadTodo {
            file,
            attempts: self.attempts + 1,
            first_last_timestamps: self.read_timestamps.start..ts.as_millis(),
        }
    }
}

impl PackReadState {
    /// Prepare a plan for this read plan based on what we read previously and what is
    /// available now.
    pub fn prepare_read(
        &self,
        now: WallTime,
        cur_files: &[PackFileId],
        cfg: &PackSyncConfig,
    ) -> PackReadPlan {
        // only retain the highest gen for a given id
        let mut cur_files = dedupe_pack_files(cur_files);
        // oldest first so parents are consumed before children
        cur_files.sort_by_key(|f| f.seqs.end);

        let consumed_refs: HashSet<_> = self.consumed.iter().map(|f| f.get_ref()).collect();
        let covered_seqs = self.covered_seqs();

        let mut to_read = vec![];
        let mut next = PackReadState::default();
        // the refs that were read or will get read in this round as a way to unblock packs transitively depending on parents
        let mut will_get_read = consumed_refs.clone();

        for f in cur_files {
            let r = f.get_ref();

            // 1. a file that was consumed already is still marked as consumed (exact match)
            if self.consumed.contains(&f) {
                next.consumed.insert(f);
                continue;
            }

            // 2. if a new file has the same ref as a consumed file, we can mark it in consumed
            //    if and only if every seq it covers was in some consumed file (if not it covers
            //    some pack we either didn't see or that was blocked)
            if consumed_refs.contains(&r)
                && covered_seqs
                    .iter()
                    .any(|c| c.start <= f.seqs.start && c.end >= f.seqs.end)
            {
                next.consumed.insert(f);
                continue;
            }

            // 3. if a was blocked, we check if its wake condition was met
            //    for now we only check for:
            //      a. generation bump,
            //      b. parents arriving, or
            //      c. time-based retry
            //   there are other wake conditions we can check in the future!
            if let Some(blocked) = self.blocked.get(&r) {
                let wake = f.generation > blocked.file.generation
                    || (!blocked.pending_parents.is_empty()
                        && blocked
                            .pending_parents
                            .iter()
                            .all(|p| will_get_read.contains(p)))
                    || blocked.next_retry_ts(cfg) <= now.as_millis();
                if !wake {
                    next.blocked.insert(r, blocked.clone());
                } else {
                    to_read.push(blocked.todo(f, now));
                    will_get_read.insert(r);
                }
                continue;
            }

            // 4. for any other condition we try to read the pack
            to_read.push(PackReadTodo::new(f, now));
            will_get_read.insert(r);
        }

        PackReadPlan { next, to_read }
    }

    fn covered_seqs(&self) -> Vec<Range<u64>> {
        let mut all_seqs: Vec<Range<u64>> = self.consumed.iter().map(|c| c.seqs.clone()).collect();
        // sort so that the first seqs are at the end, so we can pop them off
        all_seqs.sort_by_key(|r| Reverse(r.start));

        let mut res = vec![];
        let Some(mut l) = all_seqs.pop() else {
            return res;
        };
        while let Some(r) = all_seqs.pop() {
            if r.start <= l.end {
                l.end = max(l.end, r.end);
            } else {
                res.push(l.clone());
                l = r;
            }
        }
        res.push(l); // make sure we pust the last seq we have
        res
    }
}

impl BlockedPackInfo {
    fn next_retry_duration(&self, cfg: &PackSyncConfig) -> Duration {
        let factor = 1u32.checked_shl(self.attempts.min(31) as u32).unwrap_or(u32::MAX);
        cfg.pack_retry_min
            .saturating_mul(factor)
            .min(cfg.pack_retry_max)
    }

    fn next_retry_ts(&self, cfg: &PackSyncConfig) -> u64 {
        self.read_timestamps.end + self.next_retry_duration(cfg).as_millis() as u64
    }
}

impl<R: Reducer> ReplicaInner<R> {
    async fn read_pack_header(
        &self,
        desc: &PackFileDescriptor,
        store: &PackStore,
        peer_info: &PeerInfo,
        todo: &PackReadTodo,
    ) -> Result<SignedPackHeader, PackProcessError> {
        match store.read_header(desc).await {
            Ok(Some(h)) => {
                if h.verify(&peer_info.commitment.sig_verify_key, desc)
                    .is_err()
                {
                    let mut bi = todo.blocked_again(desc);
                    bi.reasons.set_bad_signature(true);
                    return Err(PackProcessError::Blocked(bi));
                }
                Ok(h)
            }
            Ok(None) => Err(PackProcessError::PackGone),
            Err(PackStoreError::HeaderDecode(e)) => {
                let mut bi = todo.blocked_again(desc);
                match e {
                    PackHeaderDecodeError::UnknownVersion(_)
                    | PackHeaderDecodeError::UnknownSignatureType(_) => {
                        bi.reasons.set_needs_upgrade(true)
                    }
                    PackHeaderDecodeError::Read(_) | PackHeaderDecodeError::TrailingBytes => {
                        bi.reasons.set_corrupt_header(true)
                    }
                }
                Err(PackProcessError::Blocked(bi))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub(crate) async fn process_pack(
        &self,
        peer_info: &PeerInfo,
        desc: &PackFileDescriptor,
        remote_id: i64,
        store: &PackStore,
        todo: &PackReadTodo,
        tips: &mut Option<PackTipTracker>,
    ) -> Result<(), PackProcessError> {
        let signed_header = self.read_pack_header(desc, store, peer_info, todo).await?;
        if let Some(tips) = tips {
            tips.update_tips(todo.file.get_ref(), &signed_header.header);
        }

        let mut pack_resolver = PackResolver {
            desc: desc.clone(),
            store,
            body: None,
            body_sha256: signed_header.header.body_sha256,
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
                todo,
            )
            .await?;
        }

        for peer_data in signed_header.header.peer_data.iter() {
            let Some(peer_info) = self.resolve_or_init_peer(&peer_data.peer_id, store).await?
            else {
                // can't find peer info, track and continue
                blocked_info
                    .get_or_insert_with(|| todo.blocked_again(desc))
                    .missing_peers
                    .push(peer_data.peer_id);
                continue;
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
                    todo,
                )
                .await?;
            }
        }

        if let Some(bi) = blocked_info {
            Err(PackProcessError::Blocked(bi))
        } else {
            Ok(())
        }
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
        todo: &PackReadTodo,
    ) -> Result<(), PackProcessError> {
        let hash_ctx = LogHashContext::new(&LogId {
            peer_id: peer_info.peer,
            container_id: segment_desc.container_id,
        });
        let mut resolver = PackSegmentResolver {
            desc: segment_desc.clone(),
            pack_resolver,
        };

        let init_blocked_info = || todo.blocked_again(pack_desc);

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
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .pending_parents = pack_header.parents.iter().cloned().collect();
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
                        .reasons
                        .set_needs_upgrade(true);
                }
                SegmentProcessError::BadMetadata => {
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .reasons
                        .set_bad_metadata(true);
                }
                SegmentProcessError::BodyHashMismatch => {
                    let bi = blocked_info.get_or_insert_with(init_blocked_info);
                    bi.reasons.set_bad_body_hash(true);
                    // we can't process any other segments when we hit this error so just pass the error upwards
                    return Err(PackProcessError::Blocked(bi.clone()));
                }
                SegmentProcessError::Decode(_)
                | SegmentProcessError::Verify(_)
                | SegmentProcessError::Cipher(_)
                | SegmentProcessError::LogValidation(_) => {
                    blocked_info
                        .get_or_insert_with(init_blocked_info)
                        .reasons
                        .set_decode_error(true);
                }
                SegmentProcessError::SegmentGone => return Err(PackProcessError::PackGone),
                // internal errors - push upwards
                SegmentProcessError::Db(e) => return Err(PackProcessError::Db(e)),
                SegmentProcessError::PackStore(e) => return Err(PackProcessError::Store(e)),
                SegmentProcessError::Internal(e) => return Err(PackProcessError::Internal(e)),
                SegmentProcessError::GetSegment(e) => return Err(PackProcessError::GetSegment(e)),
                SegmentProcessError::Encode(e) => return Err(PackProcessError::Encode(e)),
            },
        }
        Ok(())
    }
}

struct PackResolver<'a> {
    desc: PackFileDescriptor,
    store: &'a PackStore,
    body: Option<Vec<u8>>,
    body_sha256: Hash256,
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
            Ok(Some(
                body.get(self.desc.body_loc.clone())
                    .ok_or(SegmentProcessError::BadMetadata)?,
            ))
        } else {
            Ok(None)
        }
    }
}

impl<'a> PackResolver<'a> {
    async fn resolve(&mut self) -> Result<Option<&[u8]>, SegmentProcessError> {
        if self.body.is_none() {
            if let Some(b) = self.store.read_body(&self.desc).await? {
                // check hash
                if tagged_hash(TaggedHashDomain::PackBody, &b) != self.body_sha256 {
                    return Err(SegmentProcessError::BodyHashMismatch);
                }
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

pub(crate) struct PackTipTracker {
    pub last: PackReadState,
    pub tips: HashSet<PackRef>,
}

impl PackTipTracker {
    fn update_tips(&mut self, pack_ref: PackRef, header: &PackHeader) {
        for r in header.parents.iter() {
            self.tips.remove(r);
        }

        for r in header.self_supersedes.iter() {
            self.tips.remove(r);
        }

        // insert pack as a new tip if it isn't already mentioned in blocked packs
        // (meaning we already processed this header, and this is just a retry)
        if !self.last.blocked.contains_key(&pack_ref) {
            self.tips.insert(pack_ref);
        }
    }
}
