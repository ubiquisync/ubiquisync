use std::sync::Arc;

use thiserror::Error;

use crate::{
    codec::{WriteError, Writer},
    ids::PeerId,
    pack::{
        FileRemote, FileRemoteError, FileType, HEADER_EXT, PackData, PackFileDescriptor,
        PackFileId, PackHeaderDecodeError, SignedPackHeader, Topic,
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
    #[error("header decode error: {0}")]
    Decode(#[from] PackHeaderDecodeError),
    #[error("write error: {0}")]
    Write(#[from] WriteError),
    #[error("can't author packs for other peers")]
    InvalidAuthor,
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
        Ok(self
            .remote
            .list(&topic.dir())
            .await?
            .into_iter()
            .filter(|f| f.file_type == FileType::Dir)
            .filter_map(|f| {
                f.file_name
                    .strip_prefix("_")
                    .and_then(|n| topic.sub_topic(n).ok())
            })
            .collect())
    }

    pub async fn list_topic_peers(&self, topic: &Topic) -> Result<Vec<PeerId>, PackStoreError> {
        Ok(self
            .remote
            .list(&topic.dir())
            .await?
            .into_iter()
            .filter(|f| f.file_type == FileType::Dir)
            .filter_map(|f| parse_peer_id(&f.file_name))
            .collect())
    }

    pub async fn list_packs(
        &self,
        topic: &Topic,
        peer: &PeerId,
    ) -> Result<Vec<PackFileId>, PackStoreError> {
        Ok(self
            .remote
            .list(&topic.peer_dir(peer))
            .await?
            .into_iter()
            .filter(|f| f.file_type == FileType::File)
            .filter_map(|f| f.file_name.strip_suffix(HEADER_EXT)?.parse().ok())
            .collect())
    }

    pub async fn read_header(
        &self,
        file: &PackFileDescriptor,
    ) -> Result<Option<SignedPackHeader>, PackStoreError> {
        Ok(
            if let Some(buf) = self.remote.read(&file.header_path()).await? {
                Some(SignedPackHeader::decode(&buf)?)
            } else {
                None
            },
        )
    }

    pub async fn read_body(
        &self,
        file: &PackFileDescriptor,
    ) -> Result<Option<Vec<u8>>, PackStoreError> {
        let buf = self.remote.read(&file.body_path()).await?;
        Ok(buf)
    }

    pub async fn write_pack(&self, data: &PackData) -> Result<(), PackStoreError> {
        if data.descriptor.peer_id != self.self_id {
            return Err(PackStoreError::InvalidAuthor);
        }

        let mut w = Writer::new();
        data.header.encode(&mut w)?;
        let header = w.finalize();
        // body first, header second
        self.remote
            .write(&data.descriptor.body_path(), &data.body)
            .await?;
        self.remote
            .write(&data.descriptor.header_path(), &header)
            .await?;
        Ok(())
    }

    pub async fn delete_pack(&self, file: &PackFileDescriptor) -> Result<(), PackStoreError> {
        // header first, body second
        self.remote.delete(&file.header_path()).await?;
        self.remote.delete(&file.body_path()).await?;
        Ok(())
    }
}
