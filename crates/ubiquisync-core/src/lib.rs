//! Core protocol types and sync engine for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate contains the storage-agnostic, domain-agnostic core of
//! Ubiquisync: the log entry envelope, opaque UUIDs, the HLC clock, and the
//! wire codec. It has no database driver dependencies and is generic over the
//! op vocabulary it carries — data domains such as tables live in companion
//! crates like `ubiquisync-tables`, and storage backends in crates such as
//! `ubiquisync-sqlite`.
//!
//! Most applications should depend on the [`ubiquisync`](https://crates.io/crates/ubiquisync)
//! facade crate rather than this crate directly.

use crate::rand::rand_fill;

pub mod codec;
pub mod crypto;
pub mod ctl;
pub mod event;
pub mod hlc;
pub mod init;
pub mod keyring;
pub mod log_entry;
pub mod store;
//pub mod sync;
pub mod processor;
pub mod rand;
pub mod reducer;
pub mod storage;
pub mod uuid;
pub mod verifier;

#[derive(Clone, Copy, Debug)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub fn new(genesis_bytes: &[u8], is_server: bool) -> Self {
        let mut digest = blake3::derive_key(DOMAIN_PEER_ID, genesis_bytes);
        if is_server {
            digest[0] |= 0x80;
        } else {
            digest[0] &= 0x7F;
        }
        Self(digest)
    }

    pub fn is_server(&self) -> bool {
        self.0[0] & 0x80 != 0
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

    use crate::{ContainerId, PeerId};

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
