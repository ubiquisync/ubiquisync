use std::{collections::HashSet, ops::Range};

use sea_query::{Expr, ExprTrait, Query, value::prelude::Uuid};
use thiserror::Error;
use ubiquisync_core::{
    ids::{LogId, PeerId},
    log::segment::{
        DecodedSegment, PlaintextSegmentEncoding, SegmentDecodeError, SegmentEncoding,
        SegmentReader,
    },
};
use ubiquisync_fs::pack::{PackFileName, PackHeader, PackRef, SegmentDescriptor};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{
        Replica,
        peers::{PeerInfo, PeerResolveError},
        schema::{peers, segments, streams},
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

impl<R> Replica<R> {
    async fn process_pack(&self, state: &mut PackProcessState) -> Result<(), ProcessPackError> {
        // TODO maybe we want this to be a stateful consumption of segments to resume after shutdown
        for segment in state.header.self_segments.iter() {
            // first aquire the lock for this log
            let stream_guard = self
                .stream_locks
                .lock(&StreamLog::new(state.peer_db_id, segment.container_id))
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
                            // TODO: this is the happy path where we can place the segment immediately
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
                    continue;
                }
                if candidate_streams.is_empty() {
                    // TODO mark this pack as pending since we can't place this segment yet
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
        }

        // TODO peer segments

        Ok(())
    }

    async fn place_pack_segment_direct(
        &self,
        pack_state: &mut PackProcessState,
        segment: &SegmentDescriptor,
        stream: &StreamInfo,
    ) {
        debug_assert_eq!(segment.idx_range.start, stream.head_chain.size);
        debug_assert_eq!(segment.prev_chain, stream.head_chain.hash);

        // let mut batch = self.db.new_batch();
        // let timestamp = self.hlc.observe(batch.as_mut())?;

        // let entry = PlaintextLogEntry::IndexedEntry(EntryBody::OpBatch(OpBatch::new(
        //     timestamp,
        //     server_user_id,
        //     op_bytes,
        // )));
        // let entries = vec![entry];

        // let next_chain_head = chain_head.compute_next_plaintext(&seed, &None, entries.iter())?;

        // let sign_bytes = next_chain_head.sign_bytes(&seed);

        // let signature = self.credentials.signing_key().sign(&sign_bytes)?;

        // let segment = encode_segment_plaintext(&signature, &chain_head, &None, &entries)?;

        // insert_cols_batch::<(
        //     segments::StreamId,
        //     segments::StartIdx,
        //     segments::EndSize,
        //     segments::Body,
        // )>(
        //     batch.as_mut(),
        //     (stream_id, chain_head.size, next_chain_head.size, segment),
        //     Query::insert().into_table(segments::Table),
        // )?;

        // if commit_err.is_none() {
        //     // TODO does prepare indicate stall conditions?
        //     // somewhere in here maybe prepare, for ctl ops
        //     // we need to enrich them with observe & key wrap ops when needed
        //     // and also return a stall condition if waiting on another ctl
        //     // log from another peer
        //     let read_state = self
        //         .reducer
        //         .prepare(self.db.as_ref(), &op)
        //         .await
        //         .map_err(|e| ExecError::Reducer(Box::new(e)))?;

        //     update_cols_batch::<(streams::HeadSize, streams::HeadHash, streams::CommitSize)>(
        //         batch.as_mut(),
        //         (
        //             next_chain_head.size,
        //             next_chain_head.hash,
        //             next_chain_head.size,
        //         ), // head and commit sizes match
        //         Query::update()
        //             .table(streams::Table)
        //             .and_where(Expr::column(streams::Id).eq(stream_id)),
        //     )?;

        //     let apply_state = self
        //         .reducer
        //         .apply(batch.as_mut(), timestamp, &op, read_state)
        //         .map_err(|e| ExecError::Reducer(Box::new(e)))?;

        //     let batch_result = batch.commit().await?;

        //     self.reducer
        //         .post_apply(apply_state, &batch_result)
        //         .map_err(|e| ExecError::Reducer(Box::new(e)))?;
        // } else {
        //     // we cannot commit because our commit status is non-Ok, so we just update the head size and hash
        //     update_cols_batch::<(streams::HeadSize, streams::HeadHash)>(
        //         batch.as_mut(),
        //         (next_chain_head.size, next_chain_head.hash),
        //         Query::update()
        //             .table(streams::Table)
        //             .and_where(Expr::column(streams::Id).eq(stream_id)),
        //     )?;

        //     batch.commit().await?;

        //     // TODO should we return any kind of pending status to the caller? maybe not and this would be more of an alerting watch channel that could propogate to UI sync status or something
        // }
    }
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
    ) -> Result<DecodedSegment<'a>, SegmentResolveError> {
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
        let segment = reader.read(&None)?;
        Ok(segment)
    }
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
}
