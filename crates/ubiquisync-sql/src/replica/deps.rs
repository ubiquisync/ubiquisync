use std::collections::HashMap;

use ubiquisync_core::{crypto::credentials::Credentials, pack::FileRemoteProvider};

use crate::db::Db;

/// The pack remote backends a replica can attach, keyed by the provider name passed to
/// `add_remote`.
pub type PackRemoteProviders = HashMap<String, Box<dyn FileRemoteProvider>>;

/// The services a replica runs on, supplied by the host application. Its settings are
/// separate, in [`ReplicaConfig`](super::ReplicaConfig).
pub struct ReplicaDeps<R> {
    pub db: Box<dyn Db>,
    pub reducer: R,
    pub credentials: Box<dyn Credentials>,
    pub pack_remote_providers: PackRemoteProviders,
}
