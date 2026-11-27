//! Read access for explorers and debugging tools (the `inspect` feature): the devices a
//! replica knows, the entries of any log, and the files in a sync folder.
//!
//! Normal apps do not need any of this. Nothing here takes a log's lock, so a read that
//! runs while sync writes can see a log a moment out of date.

use std::ops::Range;

use sea_query::{Expr, ExprTrait, Order, Query, value::prelude::Uuid};
use thiserror::Error;
use ubiquisync_core::{
    crypto::Hash256,
    hlc::Timestamp,
    ids::{LogId, PeerId},
    log::{
        EntryBody, LogEntry, LogHashContext, segment::SegmentDecodeError, segment::SegmentReader,
    },
    pack::{PackFileDescriptor, PackFileId, PackStoreError, Topic},
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{
        Replica,
        fs_sync_schema::DEFAULT_TOPIC_ID,
        schema::{branches, peers, segments},
        status::RemoteId,
    },
};

#[derive(Debug, Error)]
pub enum InspectError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("sync folder error: {0}")]
    Store(#[from] PackStoreError),
    #[error("stored segment does not decode: {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("no open sync folder with id {0}")]
    UnknownRemote(RemoteId),
}

/// A device this replica holds an init entry for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub device: PeerId,
    pub is_this_device: bool,
    /// Whether the init entry says this is a server.
    pub server: bool,
    /// False if the stored init entry no longer verifies (should not happen).
    pub valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Op,
    UseKey,
    Expunged,
    Signature,
    /// The entry could not be decrypted or decoded. For display only.
    Unreadable(String),
}

/// One entry of a log, decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryView {
    /// The entry's position in its log, from 0.
    pub index: u64,
    /// The author's clock, for op entries.
    pub at: Option<Timestamp>,
    pub kind: EntryKind,
    /// The plaintext op bytes, for op entries.
    pub op: Option<Vec<u8>>,
    /// The log's chain hash after this entry.
    pub chain_hash: Hash256,
    /// Whether the entry has been applied to the tables on this replica.
    pub applied: bool,
    /// The fork point if this entry is on a fork branch, `None` on the main branch.
    pub fork: Option<u64>,
}

/// One pack file in a sync folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub publisher: PeerId,
    pub file: PackFileId,
    /// Size of the file body, from its header.
    pub bytes: u64,
    /// The log entries it carries. Empty if the header could not be read.
    pub logs: Vec<(LogId, Range<u64>)>,
    /// Whether this replica has read the file in full.
    pub read: bool,
    /// False if the header could not be read or decoded. The header signature is not
    /// checked here.
    pub header_ok: bool,
}

impl<R: crate::reducer::Reducer> Replica<R> {
    /// Every device this replica holds an init entry for, this device first.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, DbError> {
        let inner = self.inner.as_ref();
        let rows = select_cols::<(peers::Id, peers::PeerId)>(
            inner.db.as_ref(),
            Query::select()
                .from(peers::Table)
                .order_by(peers::Id, Order::Asc),
        )
        .await?;
        let mut res = vec![];
        for row in rows.iter() {
            let (_, peer) = row?;
            let device = PeerId(peer);
            let info = inner.resolve_peer(&device).await.ok().flatten();
            res.push(DeviceInfo {
                device,
                is_this_device: device == inner.self_init.peer_id,
                server: info.as_ref().is_some_and(|i| i.commitment.server),
                valid: info.is_some(),
            });
        }
        Ok(res)
    }

    /// The entries of `log` whose index is in `range`, on every branch, sorted by branch
    /// (main branch first) then index.
    pub async fn entries(
        &self,
        log: LogId,
        range: Range<u64>,
    ) -> Result<Vec<EntryView>, InspectError> {
        let inner = self.inner.as_ref();
        let db = inner.db.as_ref();
        let Some((peer_db_id,)) = select_cols::<(peers::Id,)>(
            db,
            Query::select()
                .from(peers::Table)
                .and_where(Expr::col(peers::PeerId).eq(log.peer_id.as_ref())),
        )
        .await?
        .one()?
        else {
            return Ok(vec![]);
        };
        let branch_rows = select_cols::<(branches::Id, branches::CommitSize, branches::ForkSize)>(
            db,
            Query::select()
                .from(branches::Table)
                .and_where(Expr::col(branches::PeerId).eq(peer_db_id))
                .and_where(
                    Expr::col(branches::ContainerId).eq(Uuid::from_bytes(log.container_id.0)),
                )
                .order_by(branches::Id, Order::Asc),
        )
        .await?
        .to_vec()?;

        // sizes are stored as signed 64-bit integers
        let max = i64::MAX as u64;
        let range = Ord::min(range.start, max)..Ord::min(range.end, max);
        let hash_ctx = LogHashContext::new(&log);
        let mut res = vec![];
        for (branch_id, commit_size, fork_size) in branch_rows {
            let bodies = select_cols::<(segments::Body,)>(
                db,
                Query::select()
                    .from(segments::Table)
                    .and_where(Expr::col(segments::BranchId).eq(branch_id))
                    .and_where(Expr::col(segments::EndSize).gt(range.start))
                    .and_where(Expr::col(segments::StartIdx).lt(range.end))
                    .order_by(segments::StartIdx, Order::Asc),
            )
            .await?;
            for row in bodies.iter() {
                let (body,) = row?;
                let decoded = SegmentReader::start(body)?
                    .read(inner.key_resolver.as_ref(), &hash_ctx)
                    .await?;
                let mut next_index = decoded.header.prev_chain.size;
                for e in decoded
                    .to_plaintext(inner.key_resolver.as_ref())
                    .await
                    .entries
                {
                    let view = match e {
                        Ok((entry, meta)) => {
                            let index = meta.chain_hash.size.saturating_sub(1);
                            next_index = meta.chain_hash.size;
                            let (kind, at, op) = match entry {
                                LogEntry::IndexedEntry(EntryBody::Op(op)) => {
                                    (EntryKind::Op, Some(op.timestamp), Some(op.op.0.to_vec()))
                                }
                                LogEntry::IndexedEntry(EntryBody::UseKey(_)) => {
                                    (EntryKind::UseKey, None, None)
                                }
                                LogEntry::IndexedEntry(EntryBody::Expunged(_)) => {
                                    (EntryKind::Expunged, None, None)
                                }
                                LogEntry::Signature(_) => (EntryKind::Signature, None, None),
                            };
                            EntryView {
                                index,
                                at,
                                kind,
                                op,
                                chain_hash: meta.chain_hash.hash,
                                applied: index < commit_size,
                                fork: fork_size,
                            }
                        }
                        Err(error) => EntryView {
                            index: next_index,
                            at: None,
                            kind: EntryKind::Unreadable(error.to_string()),
                            op: None,
                            chain_hash: [0; 32],
                            applied: false,
                            fork: fork_size,
                        },
                    };
                    if range.contains(&view.index) {
                        res.push(view);
                    }
                }
            }
        }
        Ok(res)
    }

    /// The pack files in a sync folder, read straight from the folder, with whether this
    /// replica has read each one. Reads every header, so it costs one file read per pack.
    pub async fn remote_files(&self, remote: RemoteId) -> Result<Vec<RemoteFile>, InspectError> {
        let inner = self.inner.as_ref();
        let store = inner
            .pack_remotes
            .lock()
            .await
            .get(&remote)
            .cloned()
            .ok_or(InspectError::UnknownRemote(remote))?;
        let topic = Topic::default();
        let mut res = vec![];
        for publisher in store.list_topic_peers(&topic).await? {
            let read_state = match inner.resolve_peer(&publisher).await {
                Ok(Some(info)) => Some(
                    inner
                        .get_pack_read_state(remote, DEFAULT_TOPIC_ID, info.db_id)
                        .await?,
                ),
                _ => None,
            };
            let mut files = store.list_packs(&topic, &publisher).await?;
            files.sort_by_key(|f| (f.seqs.start, f.id, f.generation));
            for file in files {
                let desc = PackFileDescriptor {
                    topic: topic.clone(),
                    peer_id: publisher,
                    id: file.clone(),
                };
                let header = store.read_header(&desc).await.ok().flatten();
                let mut logs = vec![];
                let mut bytes = 0;
                if let Some(h) = &header {
                    for s in &h.header.self_segments {
                        bytes = Ord::max(bytes, s.body_loc.end as u64);
                        logs.push((
                            LogId {
                                peer_id: publisher,
                                container_id: s.container_id,
                            },
                            s.idx_range.clone(),
                        ));
                    }
                    for p in &h.header.peer_data {
                        for s in &p.segments {
                            bytes = Ord::max(bytes, s.body_loc.end as u64);
                            logs.push((
                                LogId {
                                    peer_id: p.peer_id,
                                    container_id: s.container_id,
                                },
                                s.idx_range.clone(),
                            ));
                        }
                    }
                }
                res.push(RemoteFile {
                    publisher,
                    read: read_state
                        .as_ref()
                        .is_some_and(|s| s.consumed.contains(&file)),
                    file,
                    bytes,
                    logs,
                    header_ok: header.is_some(),
                });
            }
        }
        Ok(res)
    }
}
