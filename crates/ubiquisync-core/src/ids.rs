use std::{fmt::Display, str::FromStr};

use thiserror::Error;

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct AppId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct LogId {
    pub peer_id: PeerId,
    pub container_id: ContainerId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct PeerId(pub [u8; 32]);

impl AsRef<[u8]> for PeerId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Formatted as lowercase hex.
impl Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

#[derive(Debug, Error)]
#[error("invalid peer id")]
pub struct ParsePeerIdError;

/// Accepts only the canonical form written by Display, all lowercase hex, exactly 64 chars.
impl FromStr for PeerId {
    type Err = ParsePeerIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(ParsePeerIdError);
        }
        let mut id = [0u8; 32];
        hex::decode_to_slice(s, &mut id).map_err(|_| ParsePeerIdError)?;
        Ok(Self(id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct ContainerId(pub [u8; 16]);

impl AsRef<[u8]> for ContainerId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;
    use test_strategy::proptest;

    use crate::ids::PeerId;

    #[proptest]
    fn peer_id_roundtrips(id: PeerId) {
        let parsed: PeerId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test_case("" ; "empty")]
    #[test_case(&"ab".repeat(31) ; "too short")]
    #[test_case(&"ab".repeat(33) ; "too long")]
    #[test_case(&"AB".repeat(32) ; "uppercase")]
    #[test_case(&"zz".repeat(32) ; "not hex")]
    fn peer_id_rejects(s: &str) {
        assert!(s.parse::<PeerId>().is_err());
    }
}
