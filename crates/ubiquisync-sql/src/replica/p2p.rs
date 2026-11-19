//! The serving side of direct peer-to-peer sync: the read-only calls a replica answers for a
//! connected peer.

use std::cmp::min;

use sea_query::{Expr, ExprTrait, Func, Order, Query, value::prelude::Uuid};
use thiserror::Error;
use ubiquisync_core::{
    ids::{ContainerId, PeerId},
    init::InitEntry,
    log::ChainHash,
};

use crate::{
    db::{
        DbError,
        sea_query::{build_sql, select_cols},
    },
    replica::{
        ReplicaInner,
        branches::LogKey,
        schema::{branches, peers, segments},
    },
};

/// The most branches one `changes_since` call returns.
pub const MAX_CHANGES_LIMIT: u32 = 1000;
/// The most body bytes one `fetch` call returns (it can return more when a single body is larger).
pub const MAX_FETCH_BYTES: u32 = 4 * 1024 * 1024;
/// The most segment rows one `fetch` call looks at.
const MAX_FETCH_ROWS: u64 = 1024;

/// One branch whose head changed, as reported by the serving replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchChange {
    /// The serving replica's local branch id. Only a handle for `fetch` within the same
    /// session: ids differ between replicas and must not be stored by the caller.
    pub branch_id: i64,
    pub author: PeerId,
    pub container: ContainerId,
    pub head: ChainHash,
    pub changed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPage {
    /// Encoded segment bodies exactly as stored, in log order.
    pub bodies: Vec<Vec<u8>>,
    /// Where to continue with the next `fetch` call for the same branch; None once the
    /// branch head has been reached.
    pub next_start: Option<u64>,
}

/// What a calling peer is told when a call fails.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pError {
    /// Not a branch of the serving replica: the id is stale or from another session.
    #[error("unknown branch: {0}")]
    UnknownBranch(i64),
    /// The serving replica failed. The cause is logged there and not sent to the caller.
    #[error("internal error")]
    Internal,
}

/// Why a call failed, for the serving replica's own log.
#[derive(Error, Debug)]
enum HandlerError {
    #[error("unknown branch: {0}")]
    UnknownBranch(i64),
    /// A branch's parent is missing, or it has a parent but no fork size.
    #[error("corrupt branch: {0}")]
    CorruptBranch(i64),
    /// The stored segments of a branch don't cover its log up to its head.
    #[error("branch {branch_id} has no segment at size {size}")]
    MissingSegment { branch_id: i64, size: u64 },
    #[error("db error: {0}")]
    Db(#[from] DbError),
}

impl From<HandlerError> for P2pError {
    fn from(e: HandlerError) -> Self {
        match e {
            HandlerError::UnknownBranch(id) => P2pError::UnknownBranch(id),
            e => {
                tracing::error!(error = %e, "p2p call failed");
                P2pError::Internal
            }
        }
    }
}

#[async_trait::async_trait]
pub trait P2pHandler: Send + Sync {
    /// Branches with `changed_at > stamp` that hold at least one entry of their own, ordered
    /// by `changed_at`, at most `limit` (capped at `MAX_CHANGES_LIMIT`) of them. Page by
    /// passing the last returned `changed_at` as the next `stamp`, until fewer than `limit`
    /// come back.
    ///
    /// This doesn't guarantee the caller has seen every change: a stamp is taken before its
    /// write is committed, so a change can become visible below a stamp the caller has
    /// already paged past (see `ReplicaInner::next_changed_at`). A caller needs another way
    /// to catch up on what it missed.
    async fn changes_since(&self, stamp: u64, limit: u32) -> Result<Vec<BranchChange>, P2pError>;

    /// The stored segment bodies from log size `start` up to the branch's head, including the
    /// ones held by its ancestors when `start` is below its fork point. A body can start below
    /// `start`, and one held by an ancestor can run past the fork point into the ancestor's own
    /// line. Stops before `max_bytes` (capped at `MAX_FETCH_BYTES`) would be passed, but returns
    /// at least one body if any remain.
    async fn fetch(
        &self,
        branch_id: i64,
        start: u64,
        max_bytes: u32,
    ) -> Result<FetchPage, P2pError>;

    /// The init entry of a peer this replica knows.
    async fn peer_init(&self, peer: PeerId) -> Result<Option<InitEntry>, P2pError>;
}

/// One branch on the path from the log's root to a fetched branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathHop {
    branch_id: i64,
    /// Where the path leaves this branch: the fork size of the next branch down, or the head
    /// size of the fetched branch itself.
    upper: u64,
}

/// The path from the log's root down to `branch_id`, root first. `log_branches` is every
/// branch of the log as `(id, parent_id, fork_size)`.
fn branch_path(
    branch_id: i64,
    head_size: u64,
    log_branches: &[(i64, Option<i64>, Option<u64>)],
) -> Result<Vec<PathHop>, HandlerError> {
    let mut path = vec![];
    let mut cur = branch_id;
    let mut upper = head_size;
    loop {
        // a path can't be longer than the log has branches, unless parents loop
        if path.len() >= log_branches.len() {
            return Err(HandlerError::CorruptBranch(branch_id));
        }
        path.push(PathHop {
            branch_id: cur,
            upper,
        });
        let Some((_, parent_id, fork_size)) = log_branches.iter().find(|(id, _, _)| *id == cur)
        else {
            return Err(HandlerError::CorruptBranch(cur));
        };
        match (parent_id, fork_size) {
            (None, _) => break,
            (Some(parent_id), Some(fork_size)) => {
                cur = *parent_id;
                upper = *fork_size;
            }
            (Some(_), None) => return Err(HandlerError::CorruptBranch(cur)),
        }
    }
    path.reverse();
    Ok(path)
}

/// Picks the rows of one `fetch` page, offered in path order.
struct Page {
    max_bytes: u64,
    bytes: u64,
    rows: usize,
    /// The log size the rows taken so far reach along the path.
    reached: u64,
    full: bool,
}

impl Page {
    fn new(start: u64, max_bytes: u32) -> Self {
        Self {
            max_bytes: min(max_bytes, MAX_FETCH_BYTES) as u64,
            bytes: 0,
            rows: 0,
            reached: start,
            full: false,
        }
    }

    /// Offers the next row of a hop. Returns false, leaving the row out, once the page is full.
    /// A row held by an ancestor can end past the hop's upper bound, the path only reaches the
    /// upper bound with it.
    fn offer(
        &mut self,
        hop: &PathHop,
        start_idx: u64,
        end_size: u64,
        len: u64,
    ) -> Result<bool, HandlerError> {
        if self.full {
            return Ok(false);
        }
        if start_idx > self.reached {
            return Err(HandlerError::MissingSegment {
                branch_id: hop.branch_id,
                size: self.reached,
            });
        }
        if self.rows > 0 && self.bytes + len > self.max_bytes {
            self.full = true;
            return Ok(false);
        }
        self.bytes += len;
        self.rows += 1;
        self.reached = min(end_size, hop.upper);
        Ok(true)
    }

    fn next_start(&self, head_size: u64) -> Option<u64> {
        (self.reached < head_size).then_some(self.reached)
    }
}

#[async_trait::async_trait]
impl<R: Send + Sync> P2pHandler for ReplicaInner<R> {
    async fn changes_since(&self, stamp: u64, limit: u32) -> Result<Vec<BranchChange>, P2pError> {
        Ok(self.branch_changes_since(stamp, limit).await?)
    }

    async fn fetch(
        &self,
        branch_id: i64,
        start: u64,
        max_bytes: u32,
    ) -> Result<FetchPage, P2pError> {
        Ok(self.fetch_branch_path(branch_id, start, max_bytes).await?)
    }

    async fn peer_init(&self, peer: PeerId) -> Result<Option<InitEntry>, P2pError> {
        Ok(self.stored_peer_init(peer).await?)
    }
}

impl<R> ReplicaInner<R> {
    async fn branch_changes_since(
        &self,
        stamp: u64,
        limit: u32,
    ) -> Result<Vec<BranchChange>, HandlerError> {
        let limit = min(limit, MAX_CHANGES_LIMIT);
        if limit == 0 {
            return Ok(vec![]);
        }
        // stamps are stored as i64, nothing is past i64::MAX
        let stamp = min(stamp, i64::MAX as u64);
        let rows = select_cols::<(
            branches::Id,
            peers::PeerId,
            branches::ContainerId,
            branches::HeadSize,
            branches::HeadHash,
            branches::ChangedAt,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(branches::Table)
                .inner_join(
                    peers::Table,
                    Expr::col((branches::Table, branches::PeerId))
                        .eq(Expr::col((peers::Table, peers::Id))),
                )
                .and_where(Expr::col((branches::Table, branches::ChangedAt)).gt(stamp))
                // leave out branches with no entry of their own yet
                .and_where(
                    Expr::col((branches::Table, branches::HeadSize)).gt(Func::coalesce([
                        Expr::col((branches::Table, branches::ForkSize)),
                        Expr::val(0),
                    ])),
                )
                .order_by((branches::Table, branches::ChangedAt), Order::Asc)
                .limit(limit as u64),
        )
        .await?;
        let mut res = vec![];
        for row in rows.iter() {
            let (branch_id, peer, container, head_size, head_hash, changed_at) = row?;
            res.push(BranchChange {
                branch_id,
                author: PeerId(peer),
                container: ContainerId(container),
                head: ChainHash {
                    hash: head_hash,
                    size: head_size,
                },
                changed_at,
            });
        }
        Ok(res)
    }

    async fn fetch_branch_path(
        &self,
        branch_id: i64,
        start: u64,
        max_bytes: u32,
    ) -> Result<FetchPage, HandlerError> {
        let db = self.db.as_ref();
        // which log the branch belongs to never changes, so it can be read before locking
        let Some((peer_db_id, container_id)) =
            select_cols::<(branches::PeerId, branches::ContainerId)>(
                db,
                Query::select()
                    .from(branches::Table)
                    .and_where(Expr::col(branches::Id).eq(branch_id)),
            )
            .await?
            .one()?
        else {
            return Err(HandlerError::UnknownBranch(branch_id));
        };
        // The log lock keeps the log's segment rows as they are for the reads below: a page is
        // picked by row sizes first and its bodies are loaded after, and anything that rewrites
        // stored rows (joining them into larger ones) has to hold this lock too.
        let _guard = self
            .log_locks
            .lock(&LogKey::new(peer_db_id, ContainerId(container_id)))
            .await;
        let Some((head_size, parent_id)) = select_cols::<(branches::HeadSize, branches::ParentId)>(
            db,
            Query::select()
                .from(branches::Table)
                .and_where(Expr::col(branches::Id).eq(branch_id)),
        )
        .await?
        .one()?
        else {
            return Err(HandlerError::UnknownBranch(branch_id));
        };
        if start >= head_size {
            return Ok(FetchPage {
                bodies: vec![],
                next_start: None,
            });
        }

        let path = if parent_id.is_none() {
            vec![PathHop {
                branch_id,
                upper: head_size,
            }]
        } else {
            let log_branches =
                select_cols::<(branches::Id, branches::ParentId, branches::ForkSize)>(
                    db,
                    Query::select()
                        .from(branches::Table)
                        .and_where(Expr::col(branches::PeerId).eq(peer_db_id))
                        .and_where(
                            Expr::col(branches::ContainerId).eq(Uuid::from_bytes(container_id)),
                        ),
                )
                .await?
                .to_vec()?;
            branch_path(branch_id, head_size, &log_branches)?
        };

        // first pick the rows by their sizes, then load only the bodies of those
        let mut page = Page::new(start, max_bytes);
        let mut row_budget = MAX_FETCH_ROWS;
        // per hop: (branch_id, end_size of the last row taken from it)
        let mut taken: Vec<(i64, u64)> = vec![];
        for hop in path.iter().filter(|hop| hop.upper > start) {
            if page.full || row_budget == 0 {
                break;
            }
            let (sql, params) = build_sql(
                Query::select()
                    .column(segments::StartIdx)
                    .column(segments::EndSize)
                    .expr(Func::cust("LENGTH").arg(Expr::col(segments::Body)))
                    .from(segments::Table)
                    .and_where(Expr::col(segments::BranchId).eq(hop.branch_id))
                    .and_where(Expr::col(segments::EndSize).gt(start))
                    .and_where(Expr::col(segments::StartIdx).lt(hop.upper))
                    .order_by(segments::EndSize, Order::Asc)
                    .limit(row_budget),
                db.dialect(),
            )?;
            let rows = db.query(&sql, &params).await?;
            row_budget -= rows.len() as u64;
            let mut last_end = None;
            for row in rows.iter() {
                let start_idx = row.get_at::<u64>(0)?;
                let end_size = row.get_at::<u64>(1)?;
                let len = row.get_at::<u64>(2)?;
                if !page.offer(hop, start_idx, end_size, len)? {
                    break;
                }
                last_end = Some(end_size);
            }
            if let Some(last_end) = last_end {
                taken.push((hop.branch_id, last_end));
            }
        }
        if page.rows == 0 {
            return Err(HandlerError::MissingSegment {
                branch_id,
                size: start,
            });
        }

        let mut bodies = Vec::with_capacity(page.rows);
        for (hop_branch_id, last_end) in taken {
            let rows = select_cols::<(segments::Body,)>(
                db,
                Query::select()
                    .from(segments::Table)
                    .and_where(Expr::col(segments::BranchId).eq(hop_branch_id))
                    .and_where(Expr::col(segments::EndSize).gt(start))
                    .and_where(Expr::col(segments::EndSize).lte(last_end))
                    .order_by(segments::EndSize, Order::Asc),
            )
            .await?;
            for row in rows.iter() {
                let (body,) = row?;
                bodies.push(body.to_vec());
            }
        }
        // can't happen while the log lock is held
        if bodies.len() != page.rows {
            return Err(DbError::UnexpectedRowCount { got: bodies.len() }.into());
        }

        Ok(FetchPage {
            bodies,
            next_start: page.next_start(head_size),
        })
    }

    /// The init entry as stored. It isn't verified again here, the caller has to verify it
    /// anyway.
    async fn stored_peer_init(&self, peer: PeerId) -> Result<Option<InitEntry>, HandlerError> {
        Ok(select_cols::<(peers::CommitmentBytes, peers::Signature)>(
            self.db.as_ref(),
            Query::select()
                .from(peers::Table)
                .and_where(Expr::col(peers::PeerId).eq(peer.as_ref())),
        )
        .await?
        .one()?
        .map(|(commitment_bytes, signature)| InitEntry {
            commitment_bytes: commitment_bytes.into(),
            peer_id: peer,
            signature,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{HandlerError, MAX_FETCH_BYTES, P2pError, Page, PathHop, branch_path};

    fn hop(branch_id: i64, upper: u64) -> PathHop {
        PathHop { branch_id, upper }
    }

    #[test]
    fn path_of_a_root() {
        let path = branch_path(1, 5, &[(1, None, None)]).unwrap();
        assert_eq!(path, vec![hop(1, 5)]);
    }

    #[test]
    fn path_of_a_fork_of_a_fork() {
        // 1 is the root, 2 forks off it at 3, 3 forks off 2 at 6, 4 is a sibling of 2
        let branches = [
            (1, None, None),
            (2, Some(1), Some(3)),
            (3, Some(2), Some(6)),
            (4, Some(1), Some(2)),
        ];
        let path = branch_path(3, 9, &branches).unwrap();
        assert_eq!(path, vec![hop(1, 3), hop(2, 6), hop(3, 9)]);
        let path = branch_path(4, 4, &branches).unwrap();
        assert_eq!(path, vec![hop(1, 2), hop(4, 4)]);
    }

    #[test]
    fn path_with_a_missing_parent() {
        // 2's parent 1 isn't a branch of the log, 5 is an unrelated root
        let res = branch_path(2, 5, &[(2, Some(1), Some(3)), (5, None, None)]);
        assert!(matches!(res, Err(HandlerError::CorruptBranch(1))));
    }

    #[test]
    fn path_with_a_parent_loop() {
        let res = branch_path(1, 5, &[(1, Some(2), Some(3)), (2, Some(1), Some(1))]);
        assert!(matches!(res, Err(HandlerError::CorruptBranch(1))));
    }

    #[test]
    fn path_without_a_fork_size() {
        let res = branch_path(2, 5, &[(1, None, None), (2, Some(1), None)]);
        assert!(matches!(res, Err(HandlerError::CorruptBranch(2))));
    }

    #[test]
    fn caller_only_sees_unknown_branch() {
        assert_eq!(
            P2pError::from(HandlerError::UnknownBranch(7)),
            P2pError::UnknownBranch(7)
        );
        assert_eq!(
            P2pError::from(HandlerError::CorruptBranch(7)),
            P2pError::Internal
        );
    }

    /// Offers `rows` as `(hop, start_idx, end_size, len)`, returns how many were taken and the
    /// next start.
    fn page(
        start: u64,
        head: u64,
        max_bytes: u32,
        rows: &[(PathHop, u64, u64, u64)],
    ) -> (usize, Option<u64>) {
        let mut page = Page::new(start, max_bytes);
        for (hop, start_idx, end_size, len) in rows {
            if !page.offer(hop, *start_idx, *end_size, *len).unwrap() {
                break;
            }
        }
        (page.rows, page.next_start(head))
    }

    #[test]
    fn page_takes_everything_that_fits() {
        let rows = [(hop(1, 6), 0, 2, 10), (hop(1, 6), 2, 6, 10)];
        assert_eq!(page(0, 6, 100, &rows), (2, None));
        assert_eq!(page(0, 6, 20, &rows), (2, None));
    }

    #[test]
    fn page_stops_before_max_bytes() {
        let rows = [(hop(1, 6), 0, 2, 10), (hop(1, 6), 2, 6, 10)];
        assert_eq!(page(0, 6, 19, &rows), (1, Some(2)));
    }

    #[test]
    fn page_always_takes_one_body() {
        let rows = [(hop(1, 6), 0, 2, 10), (hop(1, 6), 2, 6, 10)];
        assert_eq!(page(0, 6, 0, &rows), (1, Some(2)));
        assert_eq!(page(0, 6, 5, &rows), (1, Some(2)));
    }

    #[test]
    fn page_caps_max_bytes() {
        let big = MAX_FETCH_BYTES as u64;
        let rows = [(hop(1, 6), 0, 2, big), (hop(1, 6), 2, 6, 1)];
        assert_eq!(page(0, 6, u32::MAX, &rows), (1, Some(2)));
    }

    /// A row held by the parent runs past the fork point: the path only reaches the fork point
    /// with it, so the next page starts there and not at the row's end.
    #[test]
    fn page_stops_a_straddling_row_at_the_fork_point() {
        let rows = [(hop(1, 3), 0, 5, 10), (hop(2, 6), 3, 6, 10)];
        assert_eq!(page(0, 6, 0, &rows), (1, Some(3)));
        assert_eq!(page(0, 6, 100, &rows), (2, None));
        // starting inside the straddling row
        assert_eq!(page(2, 6, 0, &rows), (1, Some(3)));
    }

    #[test]
    fn page_with_a_gap() {
        let mut page = Page::new(0, 100);
        assert!(page.offer(&hop(1, 6), 0, 2, 10).unwrap());
        let res = page.offer(&hop(1, 6), 3, 6, 10);
        assert!(matches!(
            res,
            Err(HandlerError::MissingSegment {
                branch_id: 1,
                size: 2
            })
        ));
    }

    #[test]
    fn page_with_a_first_row_above_start() {
        let mut page = Page::new(1, 100);
        let res = page.offer(&hop(1, 6), 2, 4, 10);
        assert!(matches!(
            res,
            Err(HandlerError::MissingSegment {
                branch_id: 1,
                size: 1
            })
        ));
    }
}
