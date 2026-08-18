use crate::rand::{self, rand_fill};

#[derive(Clone, Copy, Debug)]
pub struct PeerId(pub [u8; 32]);

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
    use crate::ids::ContainerId;

    #[proptest]
    fn test_container_id(app_type: u8, is_encrypted: bool) {
        let container_id = ContainerId::generate(app_type, is_encrypted).unwrap();
        assert_eq!(container_id.is_encrypted(), is_encrypted);
        assert_eq!(container_id.app_type(), app_type);
        // check that other flag bits are unset
        assert_eq!(container_id.0[0] & !0x1, 0x0);
    }
}
