use crate::rand::{self, rand_fill};

#[derive(Clone, Copy, Debug)]
pub struct AppId(pub [u8; 16]);

#[derive(Clone, Copy, Debug)]
pub struct LogId {
    pub peer_id: PeerId,
    pub container_id: ContainerId,
}

#[derive(Clone, Copy, Debug)]
pub struct PeerId(pub [u8; 32]);

#[derive(Clone, Copy, Debug)]
pub struct ContainerId(pub [u8; 16]);

#[derive(Clone, Copy, Debug)]
pub struct AudienceId(pub [u8; 15]);

impl ContainerId {
    pub fn new(audience: &AudienceId, commit_unit: u8) -> Self {
        let mut id = [0; 16];
        id[..15].copy_from_slice(&audience.0);
        id[15] = commit_unit;
        Self(id)
    }

    pub fn audience(&self) -> AudienceId {
        let mut id = [0; 15];
        id.copy_from_slice(&self.0);
        AudienceId(id)
    }

    pub fn commit_unit(&self) -> u8 {
        self.0[15]
    }
}
