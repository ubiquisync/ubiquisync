use std::sync::Arc;

use thiserror::Error;

use crate::{
    ids::PeerId,
    pack::{
        FileRemote, FileRemoteError, FileType, PackData, PackFileDescriptor, PackFileId,
        SignedPackHeader, Topic,
    },
};

pub struct PackStore {
    self_id: PeerId,
    remote: Arc<dyn FileRemote>,
}

#[derive(Error, Debug)]
pub enum PackStoreError {
    #[error("remote error: {0}")]
    Remote(#[from] FileRemoteError),
}

const PEERS_DIR: &str = "peers";

fn parse_peer_id(name: &str) -> Option<PeerId> {
    if name.len() != 64 || name.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let mut id = [0u8; 32];
    hex::decode_to_slice(name, &mut id).ok()?;
    Some(PeerId(id))
}

impl PackStore {
    pub async fn list_peer_inits(&self) -> Result<Vec<PeerId>, PackStoreError> {
        Ok(self
            .remote
            .list(PEERS_DIR)
            .await?
            .into_iter()
            .filter(|f| f.file_type == FileType::File)
            .filter_map(|f| parse_peer_id(&f.file_name))
            .collect())
    }

    pub async fn list_sub_topics(&self, topic: &Topic) -> Result<Vec<Topic>, PackStoreError> {
        todo!()
    }

    pub async fn list_topic_peers(&self, topic: &Topic) -> Result<Vec<PeerId>, PackStoreError> {}

    pub async fn list_packs(
        &self,
        topic: &Topic,
        peer: &PeerId,
    ) -> Result<Vec<PackFileId>, PackStoreError> {
    }

    pub async fn read_header(
        &self,
        file: &PackFileDescriptor,
    ) -> Result<Option<SignedPackHeader>, PackStoreError> {
    }

    pub async fn read_body(
        &self,
        file: &PackFileDescriptor,
    ) -> Result<Option<Vec<u8>>, PackStoreError> {
    }

    pub async fn write_pack(&self, data: &PackData) -> Result<(), PackStoreError> {}

    pub async fn delete_pack(&self, file: &PackFileDescriptor) -> Result<(), PackStoreError> {}
}
