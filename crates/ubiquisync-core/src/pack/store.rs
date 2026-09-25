use thiserror::Error;

use crate::{
    ids::PeerId,
    pack::{FileRemote, FileRemoteError, PackData, PackFileId, SignedPackHeader, Topic},
};

pub struct PackStore {
    remote: Box<dyn FileRemote>,
}

#[derive(Error, Debug)]
pub enum PackStoreError {
    #[error("remote error: {0}")]
    Remote(#[from] FileRemoteError),
}

impl PackStore {
    pub fn list_peer_inits(&self) -> Result<Vec<PeerId>, PackStoreError> {}

    pub fn list_topics(&self) -> Result<Vec<Topic>, PackStoreError> {}

    pub fn list_topic_peers(&self, topic: &Topic) -> Result<Vec<PeerId>, PackStoreError> {}

    pub fn list_packs(
        &self,
        topic: &Topic,
        peer: &PeerId,
    ) -> Result<Vec<PackFileId>, PackStoreError> {
    }

    pub fn read_header(
        &self,
        topic: &Topic,
        peer: &PeerId,
        id: &PackFileId,
    ) -> Result<Option<SignedPackHeader>, PackStoreError> {
    }

    pub fn read_body(
        &self,
        topic: &Topic,
        peer: &PeerId,
        id: &PackFileId,
    ) -> Result<Option<Vec<u8>>, PackStoreError> {
    }

    pub fn write_pack(&self, data: &PackData) -> Result<(), PackStoreError> {}

    pub fn delete_pack(
        &self,
        topic: &Topic,
        peer: &PeerId,
        id: &PackFileId,
    ) -> Result<(), PackStoreError> {
    }
}
