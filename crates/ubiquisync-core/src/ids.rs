use crate::rand::{self, rand_fill};

#[derive(Clone, Copy, Debug)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub fn new(genesis_bytes: &[u8], is_server: bool) -> Self {
        let mut digest = blake3::derive_key(DOMAIN_PEER_ID, genesis_bytes);
        if is_server {
            digest[31] |= 0x80;
        } else {
            digest[31] &= 0x7F;
        }
        Self(digest)
    }

    pub fn is_server(&self) -> bool {
        self.0[31] & 0x80 != 0
    }
}

const DOMAIN_PEER_ID: &str = "ubiquisync/v1/peer-id";

#[derive(Clone, Copy, Debug)]
pub struct ContainerId(pub [u8; 16]);

impl ContainerId {
    pub fn generate(app_type: u8, is_encrypted: bool) -> Result<Self, rand::Error> {
        let mut buf = [0; 16];
        if is_encrypted {
            buf[0] |= 0x1; // set encrypted flag
            // other flags in byte 0 are reserved for futre flags
        }
        buf[1] = app_type;
        rand_fill(&mut buf[2..])?;
        Ok(Self(buf))
    }

    pub fn is_encrypted(&self) -> bool {
        return self.0[0] & 0x1 == 0x1;
    }

    pub fn app_type(&self) -> u8 {
        return self.0[1];
    }
}

#[cfg(test)]
pub mod tests {
    use test_strategy::proptest;

    #[cfg(test)]
    use crate::ids::{ContainerId, PeerId};

    #[proptest]
    fn test_peer_id(genesis_bytes: Vec<u8>, is_server: bool) {
        let peer_id = PeerId::new(&genesis_bytes, is_server);
        assert_eq!(is_server, peer_id.is_server())
    }

    #[proptest]
    fn test_container_id(app_type: u8, is_encrypted: bool) {
        let container_id = ContainerId::generate(app_type, is_encrypted).unwrap();
        assert_eq!(container_id.is_encrypted(), is_encrypted);
        assert_eq!(container_id.app_type(), app_type);
        // check that other flag bits are unset
        assert_eq!(container_id.0[0] & !0x1, 0x0);
    }
}
