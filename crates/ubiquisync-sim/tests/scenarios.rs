//! Scripted sync scenarios between a few replicas on a shared folder remote: stalled commits
//! that the commit retry loop has to finish, and forked logs. (`sim.rs` is the randomized
//! counterpart.)

use std::{
    collections::HashMap,
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use secrecy::SecretBox;
use test_case::test_case;
use ubiquisync_core::{
    crypto::{NullCipherKeyResolver, credentials::software::SoftwareCredentials},
    event::event_bus,
    hlc::Timestamp,
    ids::{AppId, ContainerId, LogId, PeerId},
    log::{ChainHash, LogHashContext, segment::SegmentReader},
    pack::StdFsRemoteProvider,
};
use ubiquisync_sql::{
    Exec, SqlQueryStore,
    db::{Db, DbBatch, DbStatementResult, DbValue},
    op::OpCodec,
    reducer::{ApplyError, PrepareError, Reducer as SqlReducer},
    replica::{
        CommitRetryConfig, PackSyncConfig, Replica, ReplicaConfig, ReplicaDeps,
        p2p::{BranchChange, FetchPage, MAX_FETCH_BYTES, P2pError, P2pHandler},
        test_support::{BranchHeads, P2pIngest},
    },
};
use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::{
    col_type::ColType,
    id::{ColumnId, TableId},
    op::{ColumnSet, Op, Upsert, Value},
    reducer::Reducer,
    schema::{ColumnSchema, TableSchema},
};

const APP_ID: AppId = AppId([7; 16]);
const CONTAINER_ID: ContainerId = ContainerId([9; 16]);
const TABLE: TableId = TableId::new(&[ColType::I64], 1);
const COL_V: ColumnId = ColumnId::new(0, ColType::Text);
const FOLDER: &str = "folder";
const TICK: Duration = Duration::from_millis(20);

#[test_case(Failure::Prepare, TICK, false ; "once the backoff passes")]
#[test_case(Failure::Prepare, Duration::from_hours(1), true ; "right after a restart, whatever the backoff")]
#[test_case(Failure::DbWrite, Duration::from_hours(1), true ; "a failed db write backs off too")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_commits_catch_up(failure: Failure, retry_delay: Duration, restart: bool) {
    let root = temp_root(&format!("stall-{failure:?}-{restart}"));
    let (writer, reader) = (
        Peer::new(&root, 1, retry_delay),
        Peer::new(&root, 2, retry_delay),
    );
    let writer_replica = writer.open_attached().await;
    let mut reader_replica = reader.open_attached().await;

    // the reader stalls on the first entry, so the later ones land on a branch whose commit
    // is already behind and are stored without being committed
    reader.faults.fail(Some(failure));
    for keys in [0..3, 3..5] {
        write(&writer_replica, keys).await;
        wait_until_synced(&reader_replica, &writer_replica).await;
    }
    assert!(rows(&reader_replica).await.is_empty());

    if restart {
        // the backoff is far away: nothing is tried again until the restart
        let attempts = reader.faults.attempts();
        tokio::time::sleep(10 * TICK).await;
        assert_eq!(reader.faults.attempts(), attempts);
        reader.faults.fail(None);
        reader_replica.shutdown().await;
        reader_replica = reader.open().await;
    } else {
        reader.faults.fail(None);
    }
    wait_until_committed(&reader_replica, &writer_replica).await;

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// The clock moves when an op is first tried, so an op that only commits on a retry still has
/// to save it, or a restart would bring back a clock behind an op that was applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retried_commit_saves_the_clock() {
    let root = temp_root("clock");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));
    let writer_replica = writer.open_attached().await;
    let reader_replica = reader.open_attached().await;

    reader.faults.fail(Some(Failure::Prepare));
    write(&writer_replica, 0..1).await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    reader.faults.fail(None);
    wait_until_committed(&reader_replica, &writer_replica).await;

    // the writer's saved clock is its op's timestamp
    assert!(saved_clock(&reader_replica).await > saved_clock(&writer_replica).await);

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A fork that arrives while its parent's commit is behind has to wait for the parent: the
/// entries below the fork point are the parent's to commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_of_a_stalled_log_catches_up() {
    let root = temp_root("fork");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db, carries on, and publishes entries 0..5
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, 3..5).await;

    reader.faults.fail(Some(Failure::Prepare));
    let reader_replica = reader.open_attached().await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    writer_replica.shutdown().await;

    // the backup writes different entries on top of the first three: a fork at entry 3
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..7).await;
    restored.attach(&restored_replica).await;
    wait_until_forked_at(&reader_replica, 3).await;
    assert!(rows(&reader_replica).await.is_empty());

    reader.faults.fail(None);
    wait_until("the reader commits both branches", async || {
        rows(&reader_replica).await.len() == 7
    })
    .await;

    restored_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A device restored from a backup forks its own log as soon as it writes and syncs. Both
/// copies keep writing to their own lineage, each gets the other's entries as a fork, and every
/// device ends up with the same rows.
#[test_case(3 ; "backup taken after three entries")]
#[test_case(0 ; "backup taken before the log had any entry")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forked_own_log_keeps_writing(backed_up: i64) {
    let root = temp_root(&format!("own-fork-{backed_up}"));
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db, carries on, and publishes entries 0..5
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..backed_up).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, backed_up..5).await;
    let reader_replica = reader.open_attached().await;

    // the backup writes before it syncs, so what the original wrote since is a fork of its log
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..7).await;
    restored.attach(&restored_replica).await;
    for replica in [&restored_replica, &writer_replica, &reader_replica] {
        wait_until_forked_at(replica, backed_up as u64).await;
    }

    // both copies of the forked log can still write
    write(&restored_replica, 7..9).await;
    write(&writer_replica, 9..11).await;
    for replica in [&restored_replica, &writer_replica, &reader_replica] {
        wait_until("every device has every row", async || {
            rows(replica).await.len() == 11
        })
        .await;
    }
    assert_eq!(rows(&restored_replica).await, rows(&reader_replica).await);
    assert_eq!(rows(&writer_replica).await, rows(&reader_replica).await);

    for replica in [restored_replica, writer_replica, reader_replica] {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `changed_at` is a replica's own record of when each of its branches last changed. Local
/// writes and ingested segments both move a branch's stamp past every stamp the replica handed
/// out before. A branch whose head didn't move keeps its stamp, also when its stored entries
/// are committed later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_at_follows_branch_heads() {
    let root = temp_root("changed-at");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));
    let started = wall_ms();
    let writer_replica = writer.open_attached().await;
    let writer_id = writer_replica.sync_snapshot().await.unwrap().self_peer;

    // every local write moves the stamp of the log it wrote to
    write(&writer_replica, 0..1).await;
    let first = changed_at(&writer_replica).await[&writer_id];
    assert!(first >= started);
    write(&writer_replica, 1..2).await;
    let second = changed_at(&writer_replica).await[&writer_id];
    assert!(second > first);

    // the reader stamps the writer's log when it stores it, not when it was written
    let reader_opened = wall_ms();
    let reader_replica = reader.open_attached().await;
    let reader_id = reader_replica.sync_snapshot().await.unwrap().self_peer;
    wait_until_synced(&reader_replica, &writer_replica).await;
    let ingested = changed_at(&reader_replica).await[&writer_id];
    assert!(ingested >= reader_opened);

    // the reader's own write is stamped after that, and leaves the writer's log alone
    write(&reader_replica, 10..11).await;
    let stamps = changed_at(&reader_replica).await;
    assert_eq!(stamps[&writer_id], ingested);
    assert!(stamps[&reader_id] > ingested);

    // more from the writer moves its log past everything else on the reader, here stored
    // without being committed
    reader.faults.fail(Some(Failure::Prepare));
    write(&writer_replica, 2..3).await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    let stamps = changed_at(&reader_replica).await;
    assert!(stamps[&writer_id] > stamps[&reader_id]);

    // committing the stored entry later doesn't move the head, so the stamp stays
    reader.faults.fail(None);
    wait_until("the reader commits the stored entry", async || {
        rows(&reader_replica).await.len() == 4
    })
    .await;
    assert_eq!(changed_at(&reader_replica).await, stamps);

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A fork is a new branch row, stamped when it is first stored, and the branch it forked from
/// keeps its stamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_at_stamps_a_new_fork() {
    let root = temp_root("changed-at-fork");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db after three entries, carries on to five, and the reader syncs
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, 3..5).await;
    let reader_replica = reader.open_attached().await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    writer_replica.shutdown().await;
    let before = reader_replica.branch_changes().await.unwrap();
    assert_eq!(before.len(), 1);
    let (_, _, _, root_stamp) = before[0];

    // the backup writes three entries of its own on top of the first three: a fork with head 6
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..8).await;
    restored.attach(&restored_replica).await;
    wait_until("the reader stores the whole fork", async || {
        let changes = reader_replica.branch_changes().await.unwrap();
        changes.iter().any(|(_, _, head_size, _)| *head_size == 6)
    })
    .await;

    // ordered by stamp: the branch the reader already had, then the fork
    let after = reader_replica.branch_changes().await.unwrap();
    let stamps: Vec<(u64, u64)> = after.iter().map(|(_, _, size, at)| (*size, *at)).collect();
    assert_eq!(stamps.len(), 2);
    assert_eq!(stamps[0], (5, root_stamp));
    assert_eq!(stamps[1].0, 6);
    assert!(stamps[1].1 > root_stamp);

    restored_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A local write to a log whose commit is stalled is stored without being committed, and
/// still moves the stamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_at_moves_on_a_write_to_a_stalled_log() {
    let root = temp_root("changed-at-stalled");
    let writer = Peer::new(&root, 1, Duration::from_hours(1));

    // a restored copy of the writer gets the entries the original wrote after the backup; they
    // extend its own log, and its commit of them stalls
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, 3..5).await;
    restored.faults.fail(Some(Failure::Prepare));
    let restored_replica = restored.open_attached().await;
    wait_until_synced(&restored_replica, &writer_replica).await;
    writer_replica.shutdown().await;
    assert_eq!(rows(&restored_replica).await.len(), 3);
    let writer_id = restored_replica.sync_snapshot().await.unwrap().self_peer;
    let stalled = changed_at(&restored_replica).await[&writer_id];

    // the write goes on top of the stalled entries
    write(&restored_replica, 5..6).await;
    assert_eq!(rows(&restored_replica).await.len(), 3);
    assert!(changed_at(&restored_replica).await[&writer_id] > stalled);

    restored_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// `changes_since` reports every branch once, in stamp order, however small the pages, and a
/// branch whose head moves comes back with a later stamp. `peer_init` hands out the init
/// entries the replica knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_changes_since_pages_over_every_branch() {
    let root = temp_root("p2p-changes");
    let (server, a, b) = (
        Peer::new(&root, 1, TICK),
        Peer::new(&root, 2, TICK),
        Peer::new(&root, 3, TICK),
    );
    let server_replica = server.open_attached().await;
    let a_replica = a.open_attached().await;
    let b_replica = b.open_attached().await;
    write(&a_replica, 0..2).await;
    write(&b_replica, 2..5).await;
    write(&server_replica, 5..6).await;
    for replica in [&a_replica, &b_replica] {
        wait_until_synced(&server_replica, replica).await;
    }
    let handler = server_replica.p2p_handler();

    // one branch per author, in stamp order, matching what the replica stores
    let changes = handler.changes_since(0, 1000).await.unwrap();
    assert_eq!(changes.len(), 3);
    let snapshot = server_replica.sync_snapshot().await.unwrap();
    for change in &changes {
        assert_eq!(change.container, CONTAINER_ID);
        assert_eq!(
            snapshot.branches[&(change.author, change.container)],
            vec![(change.head.size, change.head.hash, None)]
        );
    }
    // the same branches and stamps the replica stores
    let stored = server_replica.branch_changes().await.unwrap();
    let reported: Vec<_> = changes
        .iter()
        .map(|c| (c.author, c.container, c.head.size, c.changed_at))
        .collect();
    assert_eq!(reported, stored);
    assert!(
        changes
            .windows(2)
            .all(|w| w[0].changed_at < w[1].changed_at)
    );

    // paging visits every branch exactly once
    for limit in [1, 2] {
        assert_eq!(changes_paged(handler.as_ref(), 0, limit).await, changes);
    }
    assert!(handler.changes_since(0, 0).await.unwrap().is_empty());
    let last = changes.last().unwrap().changed_at;
    for stamp in [last, last + 1000, u64::MAX] {
        assert!(handler.changes_since(stamp, 1000).await.unwrap().is_empty());
    }

    // a branch whose head moves comes back with a later stamp
    let a_id = a_replica.sync_snapshot().await.unwrap().self_peer;
    write(&a_replica, 6..7).await;
    wait_until_synced(&server_replica, &a_replica).await;
    let moved = handler.changes_since(last, 1000).await.unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].author, a_id);
    assert_eq!(moved[0].head.size, 3);
    assert!(moved[0].changed_at > last);

    // init entries of our own peer and of another known peer, and none for an unknown one
    for peer in [snapshot.self_peer, a_id] {
        let init = handler.peer_init(peer).await.unwrap().unwrap();
        assert_eq!(init.peer_id, peer);
        init.verify(&APP_ID).unwrap();
    }
    assert!(
        handler
            .peer_init(PeerId([0xee; 32]))
            .await
            .unwrap()
            .is_none()
    );

    for replica in [server_replica, a_replica, b_replica] {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `fetch` on a root branch: the bodies form one chain from at or below `start` up to the head,
/// paging returns the same bodies, and past the head there is nothing to fetch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_fetch_root_branch() {
    let root = temp_root("p2p-fetch-root");
    let (server, writer) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer's first five entries are published together, so the server stores them as
    // one segment, then two more
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..5).await;
    writer.attach(&writer_replica).await;
    let server_replica = server.open_attached().await;
    wait_until_synced(&server_replica, &writer_replica).await;
    write(&writer_replica, 5..7).await;
    wait_until_synced(&server_replica, &writer_replica).await;
    // local writes are stored one entry per segment
    write(&server_replica, 10..13).await;
    let writer_id = writer_replica.sync_snapshot().await.unwrap().self_peer;
    let server_id = server_replica.sync_snapshot().await.unwrap().self_peer;
    let handler = server_replica.p2p_handler();
    let changes = handler.changes_since(0, 1000).await.unwrap();
    let theirs = changes.iter().find(|c| c.author == writer_id).unwrap();
    let ours = changes.iter().find(|c| c.author == server_id).unwrap();
    assert_eq!(theirs.head.size, 7);
    assert_eq!(ours.head.size, 3);

    let all = handler
        .fetch(theirs.branch_id, 0, MAX_FETCH_BYTES)
        .await
        .unwrap();
    assert_eq!(all.next_start, None);
    let (starts, head) = follow_path(writer_id, &all.bodies).await;
    assert_eq!(starts[..2], [0, 5]);
    assert_eq!(head, theirs.head);

    // from a segment boundary, and from inside the first segment
    let (starts, head) = follow_path(
        writer_id,
        &fetch_bodies(&*handler, theirs.branch_id, 5).await,
    )
    .await;
    assert_eq!(starts[0], 5);
    assert_eq!(head, theirs.head);
    let (starts, head) = follow_path(
        writer_id,
        &fetch_bodies(&*handler, theirs.branch_id, 2).await,
    )
    .await;
    assert_eq!(starts[0], 0);
    assert_eq!(head, theirs.head);

    // one body per page gives the same bodies
    for change in [theirs, ours] {
        let all = handler
            .fetch(change.branch_id, 0, MAX_FETCH_BYTES)
            .await
            .unwrap();
        let (paged, next_starts) = fetch_paged(&*handler, change.branch_id, 0, 1).await;
        assert_eq!(paged, all.bodies);
        assert_eq!(next_starts.len(), all.bodies.len());
    }
    let (_, next_starts) = fetch_paged(&*handler, ours.branch_id, 0, 1).await;
    assert_eq!(next_starts, [Some(1), Some(2), None]);

    // at or past the head there is nothing to fetch
    for start in [7, 100] {
        let page = handler
            .fetch(theirs.branch_id, start, MAX_FETCH_BYTES)
            .await
            .unwrap();
        assert_eq!(
            page,
            FetchPage {
                bodies: vec![],
                next_start: None
            }
        );
    }
    let unknown = changes.iter().map(|c| c.branch_id).max().unwrap() + 1000;
    assert!(matches!(
        handler.fetch(unknown, 0, MAX_FETCH_BYTES).await,
        Err(P2pError::UnknownBranch(id)) if id == unknown
    ));

    writer_replica.shutdown().await;
    server_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// `fetch` on a fork returns the segments its parent holds below the fork point first, then
/// its own. The parent's segment runs past the fork point, so paging has to carry on from the
/// fork point and not from that segment's end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_fetch_fork_branch() {
    let root = temp_root("p2p-fetch-fork");
    let (writer, server) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db after three entries, carries on to five and publishes them
    // together, so the server stores entries 0..5 as one segment
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open().await;
    write(&writer_replica, 3..5).await;
    writer.attach(&writer_replica).await;
    let server_replica = server.open_attached().await;
    wait_until_synced(&server_replica, &writer_replica).await;
    writer_replica.shutdown().await;

    // the backup writes three entries of its own on top of the first three: a fork at 3
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..8).await;
    restored.attach(&restored_replica).await;
    wait_until_forked_at(&server_replica, 3).await;
    wait_until("the server stores the whole fork", async || {
        let heads = heads(&server_replica).await;
        heads.values().flatten().any(|head| head.0 == 6)
    })
    .await;

    let handler = server_replica.p2p_handler();
    let changes = handler.changes_since(0, 1000).await.unwrap();
    assert_eq!(changes.len(), 2);
    let parent = changes.iter().find(|c| c.head.size == 5).unwrap();
    let fork = changes.iter().find(|c| c.head.size == 6).unwrap();
    let author = parent.author;

    // the parent holds 0..5 in one segment, past the fork point
    let parent_bodies = fetch_bodies(&*handler, parent.branch_id, 0).await;
    assert_eq!(parent_bodies.len(), 1);
    let (starts, head) = follow_path(author, &parent_bodies).await;
    assert_eq!((starts, head), (vec![0], parent.head));

    // from below the fork point: the parent's segment unchanged, then the fork's own
    for start in [0, 2] {
        let bodies = fetch_bodies(&*handler, fork.branch_id, start).await;
        assert_eq!(bodies[0], parent_bodies[0]);
        let (starts, head) = follow_path(author, &bodies).await;
        assert_eq!(starts, [0, 3]);
        assert_eq!(head, fork.head);
    }
    // from the fork point or above it: only the fork's own
    for start in [3, 4] {
        let bodies = fetch_bodies(&*handler, fork.branch_id, start).await;
        let (starts, head) = follow_path(author, &bodies).await;
        assert_eq!(starts, [3]);
        assert_eq!(head, fork.head);
    }

    // one body per page carries on from the fork point
    let all = fetch_bodies(&*handler, fork.branch_id, 0).await;
    let (paged, next_starts) = fetch_paged(&*handler, fork.branch_id, 0, 1).await;
    assert_eq!(paged, all);
    assert_eq!(next_starts, [Some(3), None]);
    let page = handler.fetch(fork.branch_id, 6, 1).await.unwrap();
    assert_eq!(
        page,
        FetchPage {
            bodies: vec![],
            next_start: None
        }
    );

    restored_replica.shutdown().await;
    server_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A reader that fetched from the server ends up holding the server's branches of a forked
/// log, whatever it already held, however the fork is cut into segments and pages, and in
/// whichever order it takes the branches.
#[test_case(false, false, false ; "fork in one segment, fork first")]
#[test_case(false, false, true ; "fork in one segment, parent first")]
#[test_case(false, true, false ; "fork in one segment, a body per page, fork first")]
#[test_case(false, true, true ; "fork in one segment, a body per page, parent first")]
#[test_case(true, false, false ; "fork an entry per segment, fork first")]
#[test_case(true, false, true ; "fork an entry per segment, parent first")]
#[test_case(true, true, false ; "fork an entry per segment, a body per page, fork first")]
#[test_case(true, true, true ; "fork an entry per segment, a body per page, parent first")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_reader_gets_a_forked_log(per_entry: bool, body_per_page: bool, parent_first: bool) {
    let name = format!("p2p-reader-{per_entry}-{body_per_page}-{parent_first}");
    let (root, server_replica, restored_replica) = forked_server(&name, per_entry).await;
    let handler = server_replica.p2p_handler();
    let changes = handler.changes_since(0, 1000).await.unwrap();
    assert_eq!(changes.len(), 2);
    let parent = changes.iter().find(|c| c.head.size == 5).unwrap();
    let fork = changes.iter().find(|c| c.head.size == 6).unwrap();
    let max_bytes = if body_per_page { 1 } else { MAX_FETCH_BYTES };

    let reader_replica = Peer::new(&root, 9, TICK).open().await;
    let order = if parent_first {
        [parent, fork]
    } else {
        [fork, parent]
    };
    for change in order {
        read_branch(&reader_replica, &*handler, change, max_bytes).await;
        assert!(holds_head(&reader_replica, change).await);
    }
    let log = (fork.author, CONTAINER_ID);
    assert_eq!(
        heads(&reader_replica).await[&log],
        heads(&server_replica).await[&log]
    );

    for replica in [reader_replica, server_replica, restored_replica] {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A reader that holds the parent line and part of the fork carries on from its own fork
/// head when the server's fork grows. Its largest head is on the parent line, which the
/// fork's later segments don't attach to, so it has to fall back to its next head.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_reader_carries_on_from_its_own_fork_head() {
    let (root, server_replica, restored_replica) = forked_server("p2p-reader-grow", true).await;
    let handler = server_replica.p2p_handler();
    let changes = handler.changes_since(0, 1000).await.unwrap();
    let fork = changes.iter().find(|c| c.head.size == 6).unwrap();
    let init = handler.peer_init(fork.author).await.unwrap().unwrap();

    // the reader takes the first two pages only: the parent's 0..5 and the fork's entry 3
    let reader_replica = Peer::new(&root, 9, TICK).open().await;
    let first = handler.fetch(fork.branch_id, 0, 1).await.unwrap();
    let second = handler
        .fetch(fork.branch_id, first.next_start.unwrap(), 1)
        .await
        .unwrap();
    for body in first.bodies.iter().chain(second.bodies.iter()) {
        let res = reader_replica
            .ingest_p2p_body(&init, CONTAINER_ID, body)
            .await;
        assert_eq!(res, P2pIngest::Placed);
    }
    let mut sizes: Vec<u64> = heads(&reader_replica).await[&(fork.author, CONTAINER_ID)]
        .iter()
        .map(|head| head.0)
        .collect();
    sizes.sort();
    assert_eq!(sizes, [4, 5]);

    // the server's fork grows to 8
    write(&restored_replica, 8..10).await;
    wait_until_fork_head(&server_replica, 8).await;
    let changes = handler.changes_since(0, 1000).await.unwrap();
    let fork = changes.iter().find(|c| c.head.size == 8).unwrap();
    let tried = read_branch(&reader_replica, &*handler, fork, 1).await;
    assert_eq!(tried, [5, 4]);
    assert!(holds_head(&reader_replica, fork).await);

    for replica in [reader_replica, server_replica, restored_replica] {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A fork of a fork: the writer's log 0..5, a fork at 3, and a fork of that fork at 4. The
/// path to the inner fork's head goes through all three branches.
#[test_case(false ; "all at once")]
#[test_case(true ; "a body per page")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_reader_gets_a_fork_of_a_fork(body_per_page: bool) {
    let root = temp_root(&format!("p2p-fork-of-fork-{body_per_page}"));
    let (writer, server) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up after three entries and carries on to five
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let outer = writer.backup();
    let writer_replica = writer.open().await;
    write(&writer_replica, 3..5).await;
    writer.attach(&writer_replica).await;
    let server_replica = server.open_attached().await;
    wait_until_synced(&server_replica, &writer_replica).await;
    writer_replica.shutdown().await;

    // the backup writes one entry, is backed up again, and carries on to six: a fork at 3
    let outer_replica = outer.open().await;
    write(&outer_replica, 10..11).await;
    outer_replica.shutdown().await;
    let inner = Peer {
        db_path: outer.db_path.with_extension("inner.db"),
        remote_dir: outer.remote_dir.clone(),
        faults: Default::default(),
        ..outer
    };
    std::fs::copy(&outer.db_path, &inner.db_path).unwrap();
    let outer_replica = outer.open().await;
    write(&outer_replica, 11..13).await;
    outer.attach(&outer_replica).await;
    wait_until_fork_head(&server_replica, 6).await;

    // the second backup writes three entries on top of its four: a fork of the fork at 4
    let inner_replica = inner.open().await;
    write(&inner_replica, 20..23).await;
    inner.attach(&inner_replica).await;
    wait_until_forked_at(&server_replica, 4).await;
    wait_until_fork_head(&server_replica, 7).await;

    let handler = server_replica.p2p_handler();
    let changes = handler.changes_since(0, 1000).await.unwrap();
    assert_eq!(changes.len(), 3);
    let innermost = changes.iter().find(|c| c.head.size == 7).unwrap();
    let max_bytes = if body_per_page { 1 } else { MAX_FETCH_BYTES };

    // the innermost branch first, from nothing, then the others
    let reader_replica = Peer::new(&root, 9, TICK).open().await;
    read_branch(&reader_replica, &*handler, innermost, max_bytes).await;
    assert!(holds_head(&reader_replica, innermost).await);
    for change in changes.iter() {
        read_branch(&reader_replica, &*handler, change, max_bytes).await;
        assert!(holds_head(&reader_replica, change).await);
    }
    let log = (innermost.author, CONTAINER_ID);
    assert_eq!(
        heads(&reader_replica).await[&log],
        heads(&server_replica).await[&log]
    );

    for replica in [reader_replica, server_replica, outer_replica, inner_replica] {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A server holding a forked log: the writer's 0..5 stored as one segment, and a fork at 3
/// with head 6 written by a restored backup of the writer (returned still running, attached).
/// The fork reaches the server as one segment, or with `per_entry` as one segment per entry.
async fn forked_server(
    name: &str,
    per_entry: bool,
) -> (PathBuf, Replica<FlakyReducer>, Replica<FlakyReducer>) {
    let root = temp_root(name);
    let (writer, server) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open().await;
    write(&writer_replica, 3..5).await;
    writer.attach(&writer_replica).await;
    let server_replica = server.open_attached().await;
    wait_until_synced(&server_replica, &writer_replica).await;
    writer_replica.shutdown().await;

    let restored_replica = restored.open().await;
    if per_entry {
        restored.attach(&restored_replica).await;
        for key in 5..8 {
            write(&restored_replica, key..key + 1).await;
            wait_until_fork_head(&server_replica, key as u64 - 1).await;
        }
    } else {
        write(&restored_replica, 5..8).await;
        restored.attach(&restored_replica).await;
        wait_until_fork_head(&server_replica, 6).await;
    }
    (root, server_replica, restored_replica)
}

/// Waits until the replica holds a fork branch with this head size.
async fn wait_until_fork_head(replica: &Replica<FlakyReducer>, head_size: u64) {
    wait_until("the replica stores the fork's head", async || {
        let heads = heads(replica).await;
        heads
            .values()
            .flatten()
            .any(|head| head.0 == head_size && head.2.is_some())
    })
    .await;
}

/// Whether the replica has a branch with exactly this head.
async fn holds_head(replica: &Replica<FlakyReducer>, change: &BranchChange) -> bool {
    let heads = heads(replica).await;
    heads
        .get(&(change.author, change.container))
        .is_some_and(|branches| {
            branches
                .iter()
                .any(|head| (head.0, head.1) == (change.head.size, change.head.hash))
        })
}

/// What a p2p reader does with one change from the server: nothing if it holds that head,
/// otherwise fetch the path to it and ingest the bodies in order. It starts from its own
/// largest head of that log below the server's head; when a body doesn't attach to what it
/// holds, it starts again from its next smaller head, and from 0 last. Returns the starts it
/// tried.
async fn read_branch(
    reader: &Replica<FlakyReducer>,
    handler: &dyn P2pHandler,
    change: &BranchChange,
    max_bytes: u32,
) -> Vec<u64> {
    if holds_head(reader, change).await {
        return vec![];
    }
    let init = handler.peer_init(change.author).await.unwrap().unwrap();
    let own = heads(reader).await;
    let mut starts: Vec<u64> = own
        .get(&(change.author, change.container))
        .into_iter()
        .flatten()
        .map(|head| head.0)
        .filter(|size| *size > 0 && *size < change.head.size)
        .collect();
    starts.sort_by(|a, b| b.cmp(a));
    starts.dedup();
    starts.push(0);

    let mut tried = vec![];
    'starts: for first_start in starts {
        tried.push(first_start);
        let mut start = first_start;
        loop {
            let page = handler
                .fetch(change.branch_id, start, max_bytes)
                .await
                .unwrap();
            for body in page.bodies.iter() {
                let res = reader.ingest_p2p_body(&init, change.container, body).await;
                if res == P2pIngest::NoAnchor {
                    assert!(first_start > 0, "no anchor for a path fetched from 0");
                    continue 'starts;
                }
            }
            match page.next_start {
                Some(next) => start = next,
                None => return tried,
            }
        }
    }
    unreachable!("the last start is 0")
}

/// Every `changes_since` page from `stamp` on, `limit` at a time.
async fn changes_paged(handler: &dyn P2pHandler, mut stamp: u64, limit: u32) -> Vec<BranchChange> {
    let mut all = vec![];
    loop {
        let page = handler.changes_since(stamp, limit).await.unwrap();
        all.extend(page.iter().cloned());
        if page.len() < limit as usize {
            return all;
        }
        stamp = page.last().unwrap().changed_at;
    }
}

/// The bodies of one big `fetch` call, which has to reach the head.
async fn fetch_bodies(handler: &dyn P2pHandler, branch_id: i64, start: u64) -> Vec<Vec<u8>> {
    let page = handler
        .fetch(branch_id, start, MAX_FETCH_BYTES)
        .await
        .unwrap();
    assert_eq!(page.next_start, None);
    page.bodies
}

/// Follows `next_start` until the head, returning every body and every page's `next_start`.
async fn fetch_paged(
    handler: &dyn P2pHandler,
    branch_id: i64,
    mut start: u64,
    max_bytes: u32,
) -> (Vec<Vec<u8>>, Vec<Option<u64>>) {
    let (mut bodies, mut next_starts) = (vec![], vec![]);
    loop {
        assert!(next_starts.len() < 100, "paging doesn't reach the head");
        let page = handler.fetch(branch_id, start, max_bytes).await.unwrap();
        bodies.extend(page.bodies);
        next_starts.push(page.next_start);
        match page.next_start {
            Some(next) => start = next,
            None => return (bodies, next_starts),
        }
    }
}

/// Decodes fetched bodies and follows them as one path through `author`'s log: every body has
/// to start on the path so far. A body held by a fork's parent can run past the fork point, the
/// fork's first body then starts below its end and replaces the rest. Returns the size each body
/// starts at and the head the path ends at.
async fn follow_path(author: PeerId, bodies: &[Vec<u8>]) -> (Vec<u64>, ChainHash) {
    let ctx = LogHashContext::new(&LogId {
        peer_id: author,
        container_id: CONTAINER_ID,
    });
    // the chain hash at each size, from the first body's start on
    let mut path: Vec<Option<ChainHash>> = vec![];
    let mut starts = vec![];
    for body in bodies {
        let segment = SegmentReader::start(body)
            .unwrap()
            .read(&NullCipherKeyResolver, &ctx)
            .await
            .unwrap();
        let prev = segment.header.prev_chain;
        let at = prev.size as usize;
        if starts.is_empty() {
            if prev.size == 0 {
                assert_eq!(prev, ChainHash::empty(&ctx));
            }
            path.resize(at + 1, None);
            path[at] = Some(prev);
        }
        assert_eq!(
            path.get(at).copied().flatten(),
            Some(prev),
            "a body doesn't start on the path"
        );
        starts.push(prev.size);
        path.truncate(at + 1);
        for entry in &segment.entries {
            assert_eq!(entry.chain_meta.chain_hash.size as usize, path.len());
            path.push(Some(entry.chain_meta.chain_hash));
        }
        assert_eq!(path.last().copied().flatten(), Some(segment.head_chain));
    }
    (starts, path.last().copied().flatten().unwrap())
}

/// How a peer's reducer fails while its commits should stall.
#[derive(Debug, Clone, Copy)]
enum Failure {
    /// `prepare` returns an error.
    Prepare,
    /// The entry's db batch fails when it is committed.
    DbWrite,
}

#[derive(Default)]
struct Faults {
    failure: Mutex<Option<Failure>>,
    attempts: AtomicU64,
}

impl Faults {
    fn fail(&self, failure: Option<Failure>) {
        *self.failure.lock().unwrap() = failure;
    }

    fn failure(&self) -> Option<Failure> {
        *self.failure.lock().unwrap()
    }

    /// How many times an entry's commit was tried.
    fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }
}

struct Peer {
    db_path: PathBuf,
    remote_dir: PathBuf,
    key_seed: u8,
    retry_delay: Duration,
    faults: Arc<Faults>,
}

impl Peer {
    fn new(root: &Path, key_seed: u8, retry_delay: Duration) -> Self {
        Self {
            db_path: root.join(format!("peer-{key_seed}.db")),
            remote_dir: root.join("remote"),
            key_seed,
            retry_delay,
            faults: Default::default(),
        }
    }

    /// The same peer on a copy of its db as it is now (the replica must be shut down).
    fn backup(&self) -> Self {
        let db_path = self.db_path.with_extension("backup.db");
        std::fs::copy(&self.db_path, &db_path).unwrap();
        Self {
            db_path,
            remote_dir: self.remote_dir.clone(),
            faults: Default::default(),
            ..*self
        }
    }

    async fn open_attached(&self) -> Replica<FlakyReducer> {
        let replica = self.open().await;
        self.attach(&replica).await;
        replica
    }

    /// Attach the shared remote, which the db remembers across reopens.
    async fn attach(&self, replica: &Replica<FlakyReducer>) {
        replica
            .add_remote(FOLDER, self.remote_dir.to_str().unwrap())
            .await
            .unwrap();
    }

    /// Open (or reopen) this peer's replica on its db file.
    async fn open(&self) -> Replica<FlakyReducer> {
        let db = SqliteDb::open(&self.db_path).unwrap();
        let (events, _) = event_bus();
        let reducer = FlakyReducer {
            inner: Reducer::new(CONTAINER_ID, "app", &[table_schema()], &db, events)
                .await
                .unwrap(),
            faults: self.faults.clone(),
        };
        let credentials = SoftwareCredentials::new(
            SecretBox::new(Box::new([self.key_seed; 32])),
            SecretBox::new(Box::new([self.key_seed + 100; 32])),
        );
        let config = ReplicaConfig {
            pack_sync: PackSyncConfig {
                poll_interval: TICK,
                pack_retry_min: TICK,
                remote_retry_min: TICK,
                ..Default::default()
            },
            commit_retry: CommitRetryConfig {
                poll_interval: TICK,
                retry_min: self.retry_delay,
                retry_max: self.retry_delay,
            },
        };
        let deps = ReplicaDeps {
            db: Box::new(db),
            reducer,
            credentials: Box::new(credentials),
            pack_remote_providers: [(FOLDER.into(), Box::new(StdFsRemoteProvider) as _)].into(),
        };
        Replica::new(APP_ID, deps, config).await.unwrap()
    }
}

/// The tables reducer, failing on demand.
struct FlakyReducer {
    inner: Reducer,
    faults: Arc<Faults>,
}

#[async_trait]
impl SqlReducer for FlakyReducer {
    type Op = Op;
    type ReadState = <Reducer as SqlReducer>::ReadState;
    type ApplyState = <Reducer as SqlReducer>::ApplyState;

    fn codec(&self) -> &dyn OpCodec<Op> {
        self.inner.codec()
    }

    async fn prepare(&self, db: &dyn Db, op: &Op) -> Result<Self::ReadState, PrepareError> {
        self.faults.attempts.fetch_add(1, Ordering::Relaxed);
        if let Some(Failure::Prepare) = self.faults.failure() {
            return Err(PrepareError::Internal("injected prepare failure".into()));
        }
        self.inner.prepare(db, op).await
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        op: &Op,
        read: Self::ReadState,
    ) -> Result<Self::ApplyState, ApplyError> {
        let apply_state = self.inner.apply(batch, timestamp, op, read)?;
        if let Some(Failure::DbWrite) = self.faults.failure() {
            batch.add_statement(r#"INSERT INTO "no_such_table" VALUES (1)"#, &[]);
        }
        Ok(apply_state)
    }

    fn post_apply(&self, apply_state: Self::ApplyState, batch_result: &[DbStatementResult]) {
        self.inner.post_apply(apply_state, batch_result)
    }
}

fn table_schema() -> TableSchema {
    TableSchema::new(
        TABLE,
        "t".into(),
        vec!["k0".into()],
        vec![ColumnSchema {
            name: "v".into(),
            id: COL_V,
        }],
    )
    .unwrap()
}

fn upsert(key: i64) -> Op {
    Op::Upsert(Upsert {
        table_id: TABLE,
        primary_key: vec![Value::I64(key)],
        sets: vec![ColumnSet {
            column_id: COL_V,
            value: Value::Text(format!("v{key}")),
        }],
        nulls: vec![],
    })
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ubiquisync-commit-retry-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn write(replica: &Replica<FlakyReducer>, keys: Range<i64>) {
    for key in keys {
        replica.exec(None, upsert(key)).await.unwrap();
    }
}

/// Every branch head the replica has stored, comparable across replicas.
async fn heads(replica: &Replica<FlakyReducer>) -> HashMap<(PeerId, ContainerId), BranchHeads> {
    replica.sync_snapshot().await.unwrap().branches
}

/// The replica's `changed_at` stamp for each author's log. Only for scenarios where every
/// peer has a single branch (one container, no forks).
async fn changed_at(replica: &Replica<FlakyReducer>) -> HashMap<PeerId, u64> {
    let changes = replica.branch_changes().await.unwrap();
    let stamps: HashMap<PeerId, u64> = changes
        .iter()
        .map(|(peer, _, _, changed_at)| (*peer, *changed_at))
        .collect();
    assert_eq!(
        stamps.len(),
        changes.len(),
        "a peer has more than one branch"
    );
    stamps
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The table rows the replica has committed.
async fn rows(replica: &Replica<FlakyReducer>) -> Vec<Vec<DbValue>> {
    replica
        .query(r#"SELECT "k0", "v" FROM "t" ORDER BY "k0""#, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.values)
        .collect()
}

/// The replica's saved clock, if it has saved one.
async fn saved_clock(replica: &Replica<FlakyReducer>) -> Option<i64> {
    let rows = replica
        .query(r#"SELECT "timestamp" FROM "__replica_hlc""#, &[])
        .await
        .unwrap();
    match rows.first()?.values[0] {
        DbValue::Integer(clock) => Some(clock),
        ref other => panic!("unexpected clock value: {other:?}"),
    }
}

/// Waits until the replica holds a branch that forked off its parent at `fork_size`.
async fn wait_until_forked_at(replica: &Replica<FlakyReducer>, fork_size: u64) {
    wait_until("the replica stores the fork", async || {
        let heads = heads(replica).await;
        heads
            .values()
            .flatten()
            .any(|head| head.2 == Some(fork_size))
    })
    .await;
}

async fn wait_until_synced(reader: &Replica<FlakyReducer>, writer: &Replica<FlakyReducer>) {
    wait_until("the reader stores the writer's entries", async || {
        heads(reader).await == heads(writer).await
    })
    .await;
}

async fn wait_until_committed(reader: &Replica<FlakyReducer>, writer: &Replica<FlakyReducer>) {
    wait_until("the reader commits the stored entries", async || {
        rows(reader).await == rows(writer).await
    })
    .await;
}

async fn wait_until<F: Future<Output = bool>>(what: &str, mut done: impl FnMut() -> F) {
    let started = Instant::now();
    while !done().await {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting until {what}"
        );
        tokio::time::sleep(TICK).await;
    }
}
