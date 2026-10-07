use std::sync::{Arc, atomic::AtomicU64};

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::{CryptoDecodeError, NullCipherKeyResolver, credentials::Credentials},
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
    dialect::SqlDialect,
    reducer::Reducer,
    replica::{
        HlcError, Replica, ReplicaInner, fs_sync_schema,
        hlc::load_hlc,
        schema::{self, peers},
        stream_lock::KeyedLock,
    },
};

impl<R: Reducer> Replica<R> {
    pub async fn new(
        app_id: AppId,
        db: Box<dyn Db>,
        reducer: R,
        credentials: Box<dyn Credentials>,
    ) -> Result<Self, InitError> {
        // TODO support prefixes

        const SELF_DB_ID: i64 = 1;

        // initialize schema, in the future we need some more proper migrations
        create_tables(db.as_ref()).await?;

        let hlc = load_hlc(db.as_ref()).await?;

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
            let self_id = PeerId(self_id);
            let init_entry = InitEntry {
                commitment_bytes: commitment_bytes.into(),
                peer_id: self_id,
                signature,
            };
            init_entry.verify(&app_id)?;
            let commit_data = init_entry.commitment_data()?;
            if commit_data.sig_verify_key != credentials.signing_key().verifying_key() {
                return Err(InitError::Internal(
                    "signing key mismatch, invalid state".to_owned(),
                ));
            }
            if commit_data.encrypt_wrap_key != credentials.decapsulation_key().encapsulation_key() {
                return Err(InitError::Internal(
                    "encryption key mismatch, invalid state".to_owned(),
                ));
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
            let init_entry = InitEntry::create(commitment, &app_id, credentials.signing_key())?;

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
            inner: Arc::new(ReplicaInner {
                app_id,
                self_id,
                self_db_id: SELF_DB_ID,
                credentials,
                db,
                reducer,
                hlc: AtomicU64::new(hlc.into()),
                stream_locks: KeyedLock::new(),
                pack_remotes: Default::default(),
                key_resolver: Arc::new(NullCipherKeyResolver),
            }),
            tasks: Default::default(),
            cancel: Default::default(),
        })
    }
}

async fn create_tables(db: &dyn Db) -> Result<(), DbError> {
    let mut batch = db.new_batch();
    for st in create_table_sql(db.dialect()) {
        batch.add_statement(&st, &[]);
    }
    batch.commit().await?;
    Ok(())
}

fn create_table_sql(dialect: SqlDialect) -> Vec<String> {
    schema::table_defs()
        .iter()
        .chain(fs_sync_schema::table_defs().iter())
        .map(|d| d.create_table_sql(dialect))
        .collect::<Vec<_>>()
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
    #[error("timestamp error")]
    Hlc(#[from] HlcError),
}
