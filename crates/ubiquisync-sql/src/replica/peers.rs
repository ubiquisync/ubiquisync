use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    ids::PeerId,
    init::{InitCommitment, InitDecodeError, InitEntry, InitVerifyError},
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{Replica, schema::peers},
};

#[derive(Error, Debug)]
pub enum PeerResolveError {
    #[error("peer not initialized: {0:?}")]
    NotInitialized(PeerId),
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("error verifying peer init entry: {0}")]
    InitVerify(#[from] InitVerifyError),
    #[error("error decoding peer init data: {0}")]
    InitDecode(#[from] InitDecodeError),
}

pub(crate) struct PeerInfo {
    pub peer: PeerId,
    pub peer_db_id: i64,
    pub commitment: InitCommitment,
}

impl<R> Replica<R> {
    // TODO we could cache this if it ever became a hot path because of signature verification
    pub(crate) async fn resolve_peer(&self, peer: &PeerId) -> Result<PeerInfo, PeerResolveError> {
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
                outer_endorsement: None,
            };
            init_entry.verify(&self.app_id)?;
            let commitment = init_entry.commitment_data()?;
            Ok(PeerInfo {
                peer: *peer,
                peer_db_id: id,
                commitment,
            })
        } else {
            Err(PeerResolveError::NotInitialized(*peer))
        }
    }
}
