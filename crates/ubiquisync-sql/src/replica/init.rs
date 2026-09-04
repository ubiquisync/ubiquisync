use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::{CryptoDecodeError, credentials::Credentials},
    hlc::HlcService,
    ids::{AppId, PeerId},
    init::{
        InitCommitment, InitCreationError, InitDecodeError, InitEntry, InitVerifyError, Version,
    },
};

use crate::{
    db::{
        Db, DbError,
        sea_query::{insert_cols, select_cols},
    },
    hlc_storage::SqlHlcStorage,
    reducer::Reducer,
    replica::{replica::Replica, schema::peers},
};

impl<R: Reducer> Replica<R> {
    pub async fn new(
        app_magic: AppId,
        db: Box<dyn Db>,
        reducer: R,
        credentials: Box<dyn Credentials>,
    ) -> Result<Self, InitError> {
        // TODO support prefixes

        const SELF_DB_ID: i64 = 0;

        let hlc = HlcService::open(SqlHlcStorage::open(db.as_ref(), "").await?)?;

        // TODO: initialize replica schema

        let self_id = if let Some((self_id, commitment_bytes, signature)) =
            select_cols::<(peers::PeerId, peers::CommitmentBytes, peers::Signature)>(
                db.as_ref(),
                Query::select()
                    .from(peers::Table)
                    .and_where(Expr::column(peers::Id).eq(SELF_DB_ID)),
            )
            .await?
            .one()?
        {
            let self_id: PeerId = PeerId(self_id.try_into().map_err(|_| {
                InitError::Internal(format!("invalid peer id length: {0}", self_id.len()))
            })?);
            let init_entry = InitEntry {
                commitment_bytes: commitment_bytes.into(),
                peer_id: self_id,
                signature,
                outer_endorsement: None,
            };
            init_entry.verify(&app_magic)?;
            let commit_data = init_entry.commitment_data()?;
            if commit_data.sig_verify_key != credentials.signing_key().verifying_key() {
                todo!()
            }
            if commit_data.encrypt_wrap_key != credentials.decapsulation_key().encapsulation_key() {
                todo!()
            }

            self_id
        } else {
            let commitment = InitCommitment {
                version: Version::default(),
                hash_suite: ubiquisync_core::crypto::Hash256Suite::Sha256,
                sig_verify_key: credentials.signing_key().verifying_key(),
                encrypt_wrap_key: credentials.decapsulation_key().encapsulation_key(),
                // TODO: support servers
                server: false,
                // TODO: support workspace join
                workspace_join: None,
                endorsement: vec![],
            };
            let init_entry = InitEntry::create(commitment, &app_magic, credentials.signing_key())?;

            let (self_db_id,) = insert_cols::<
                (peers::PeerId, peers::CommitmentBytes, peers::Signature),
                (peers::Id,),
            >(
                db.as_ref(),
                (
                    init_entry.peer_id.0,
                    init_entry.commitment_bytes,
                    init_entry.signature,
                ),
                Query::insert().into_table(peers::Table),
            )
            .await?
            .exactly_one()?;

            if self_db_id != SELF_DB_ID {
                return Err(InitError::Internal(format!(
                    "self_db_id mismatch: got {self_db_id}, expected {}",
                    SELF_DB_ID
                )));
            }

            init_entry.peer_id
        };

        Ok(Self {
            self_id,
            self_db_id: SELF_DB_ID,
            credentials,
            db,
            reducer,
            hlc,
        })
    }
}

#[derive(Error, Debug)]
pub enum InitError {
    #[error("internal error: {0}")]
    Internal(String),
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("init decode error: {0}")]
    InitDecode(#[from] InitDecodeError),
    #[error("init verify error: {0}")]
    InitVerify(#[from] InitVerifyError),
    #[error("init creation error: {0}")]
    InitCreation(#[from] InitCreationError),
    #[error("signature decode error: {0}")]
    SigDecode(#[from] CryptoDecodeError),
}
