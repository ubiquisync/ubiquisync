#[derive(Clone, Copy, Debug)]
pub struct AppId(pub [u8; 16]);

#[derive(Clone, Copy, Debug)]
pub struct LogId {
    pub peer_id: PeerId,
    pub container_id: ContainerId,
}

#[derive(Clone, Copy, Debug)]
pub struct PeerId(pub [u8; 32]);

impl AsRef<[u8]> for PeerId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ContainerId(pub [u8; 16]);

impl AsRef<[u8]> for ContainerId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
