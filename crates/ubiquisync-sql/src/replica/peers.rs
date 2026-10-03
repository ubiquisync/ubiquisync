use sea_query::{Expr, ExprTrait, OnConflict, Query, Returning};
use thiserror::Error;
use ubiquisync_core::{
    ids::PeerId,
    init::{InitCommitment, InitDecodeError, InitEntry, InitVerifyError},
    pack::{PackStore, PackStoreError},
};

use crate::{
    db::{
        DbError,
        sea_query::{insert_cols, select_cols},
    },
    replica::{Replica, ReplicaInner, schema::peers},
};

#[derive(Error, Debug)]
pub enum PeerResolveError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("error verifying peer init entry: {0}")]
    InitVerify(#[from] InitVerifyError),
    #[error("error decoding peer init data: {0}")]
    InitDecode(#[from] InitDecodeError),
    #[error("error decoding peer init data: {0}")]
    PackStore(#[from] PackStoreError),
}

pub(crate) struct PeerInfo {
    pub peer: PeerId,
    pub db_id: i64,
    pub commitment: InitCommitment,
}

impl<R> ReplicaInner<R> {
    pub(crate) async fn resolve_or_init_peer(
        &self,
        peer: &PeerId,
        store: &PackStore,
    ) -> Result<PeerInfo, PeerResolveError> {
        if let Some(info) = self.resolve_peer(peer).await? {
            Ok(info)
        } else {
            let init_entry = store.read_peer_init(peer).await?;
            init_entry.verify(&self.app_id)?;
            let commitment = init_entry.commitment_data()?;
            let (id,) = insert_cols::<
                (peers::PeerId, peers::CommitmentBytes, peers::Signature),
                (peers::Id,),
            >(
                self.db.as_ref(),
                (
                    init_entry.peer_id.0,
                    init_entry.commitment_bytes,
                    init_entry.signature,
                ),
                Query::insert()
                    .into_table(peers::Table)
                    .on_conflict(OnConflict::column(peers::PeerId).do_nothing().to_owned())
                    .returning(Query::returning().column(peers::Id)),
            )
            .await?
            .exactly_one()?;
            Ok(PeerInfo {
                peer: *peer,
                db_id: id,
                commitment,
            })
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
            init_entry.verify(&self.app_id)?;
            let commitment = init_entry.commitment_data()?;
            Ok(Some(PeerInfo {
                peer: *peer,
                db_id: id,
                commitment,
            }))
        } else {
            Ok(None)
        }
    }
}
