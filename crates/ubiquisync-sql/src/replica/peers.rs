use sea_query::{Expr, ExprTrait, OnConflict, Query};
use thiserror::Error;
use ubiquisync_core::{
    ids::PeerId,
    init::{InitCommitment, InitEntry, InitVerifyError},
    pack::{PackStore, PackStoreError},
};

use crate::{
    db::{
        DbError,
        sea_query::{insert_cols, select_cols},
    },
    replica::{ReplicaInner, schema::peers},
};

#[derive(Error, Debug)]
pub enum PeerResolveError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    /// A peer stored in our own db no longer verifies.
    #[error("corrupt local peer record: {0}")]
    CorruptLocalPeer(InitVerifyError),
    #[error("remote error: {0}")]
    PackStore(#[from] PackStoreError),
}

/// The result of looking a peer up locally, then on a remote.
pub(crate) enum PeerLookup {
    Found(Box<PeerInfo>),
    /// The remote has no init file for this peer.
    Missing,
    /// The remote's init file doesn't decode or verify: corrupt or truncated, from another app,
    /// or stored under the wrong peer id. Treated like a missing file, so one bad file can't stop
    /// a remote from syncing, and re-read next time in case it was a partial read.
    Invalid(InitVerifyError),
}

pub(crate) struct PeerInfo {
    pub peer: PeerId,
    pub db_id: i64,
    pub commitment: InitCommitment,
    pub init_entry: InitEntry,
}

impl<R> ReplicaInner<R> {
    /// Looks a peer up in our db, then in the remote's `peers/` files. `Err` is only for db or
    /// remote IO failures, which abort the sync round; a bad file on the remote is
    /// `Ok(PeerLookup::Invalid)`.
    pub(crate) async fn resolve_or_init_peer(
        &self,
        peer: &PeerId,
        store: &PackStore,
    ) -> Result<PeerLookup, PeerResolveError> {
        if let Some(info) = self.resolve_peer(peer).await? {
            Ok(PeerLookup::Found(Box::new(info)))
        } else {
            let init_entry = match store.read_peer_init(peer).await {
                Ok(Some(init_entry)) => init_entry,
                Ok(None) => return Ok(PeerLookup::Missing),
                Err(PackStoreError::InitDecode(e)) => return Ok(PeerLookup::Invalid(e.into())),
                Err(e) => return Err(e.into()),
            };
            let commitment = match init_entry.verify(&self.app_id) {
                Ok(commitment) => commitment,
                Err(e) => return Ok(PeerLookup::Invalid(e)),
            };
            let (id,) = insert_cols::<
                (peers::PeerId, peers::CommitmentBytes, peers::Signature),
                (peers::Id,),
            >(
                self.db.as_ref(),
                (
                    init_entry.peer_id.0,
                    init_entry.commitment_bytes.clone(),
                    init_entry.signature,
                ),
                Query::insert()
                    .into_table(peers::Table)
                    // we update here if the row already exists to get the id returned if there is a concurrent insert
                    .on_conflict(
                        OnConflict::column(peers::PeerId)
                            .update_column(peers::PeerId)
                            .to_owned(),
                    )
                    .returning(Query::returning().column(peers::Id)),
            )
            .await?
            .exactly_one()?;
            Ok(PeerLookup::Found(Box::new(PeerInfo {
                peer: *peer,
                db_id: id,
                commitment,
                init_entry,
            })))
        }
    }

    // TODO we could cache this if it ever became a hot path because of signature verification
    pub(crate) async fn resolve_peer(
        &self,
        peer: &PeerId,
    ) -> Result<Option<PeerInfo>, PeerResolveError> {
        if let Some((id, commitment_bytes, signature)) =
            select_cols::<(peers::Id, peers::CommitmentBytes, peers::Signature)>(
                self.db.as_ref(),
                Query::select()
                    .from(peers::Table)
                    .and_where(Expr::col(peers::PeerId).eq(peer.as_ref())),
            )
            .await?
            .one()?
        {
            let init_entry = InitEntry {
                commitment_bytes: commitment_bytes.into(),
                peer_id: *peer,
                signature,
            };
            let commitment = init_entry
                .verify(&self.app_id)
                .map_err(PeerResolveError::CorruptLocalPeer)?;
            Ok(Some(PeerInfo {
                peer: *peer,
                db_id: id,
                commitment,
                init_entry,
            }))
        } else {
            Ok(None)
        }
    }
}
