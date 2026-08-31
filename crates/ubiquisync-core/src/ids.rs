#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct AppId(pub [u8; 16]);

#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct LogId {
    pub peer_id: PeerId,
    pub container_id: ContainerId,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct PeerId(pub [u8; 32]);

#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct ContainerId(pub [u8; 16]);
