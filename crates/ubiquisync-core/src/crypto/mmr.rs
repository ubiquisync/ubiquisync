use blake3::Hasher;

use crate::{
    crypto::Hash256,
    ids::{ContainerId, PeerId},
};
use thiserror::Error;

pub struct MmrAccumulator {
    state: MmrState,
    seed: Hash256,
}

#[derive(Clone)]
pub struct MmrState {
    pub size: u64,
    pub peaks: Vec<Hash256>,
}

impl MmrState {
    pub fn validate(&self) -> Result<(), MmrError> {
        let expected = self.size.count_ones();
        let actual = self.peaks.len();
        if (expected as usize) != actual {
            return Err(MmrError::InvalidPeakCount { expected, actual });
        }
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum MmrError {
    #[error("expected {expected} peaks, got {actual}")]
    InvalidPeakCount { expected: u32, actual: usize },
}

impl MmrAccumulator {
    pub fn new(
        peer_id: &PeerId,
        container_id: &ContainerId,
        state: MmrState,
    ) -> Result<Self, MmrError> {
        state.validate()?;
        let mut seed_hasher = Hasher::new_derive_key(DOMAIN_MMR_SEED);
        seed_hasher.update(&peer_id.0);
        seed_hasher.update(&container_id.0);
        let seed = seed_hasher.finalize().into();
        Ok(Self { seed, state })
    }

    pub fn append(&mut self, leaf: &Hash256) {
        let mut node = *leaf;
        for _ in 0..self.state.size.trailing_ones() {
            let left = self
                .state
                .peaks
                .pop()
                .expect("invalid MMR state: expected a peak");
            node = hash_node(DOMAIN_MMR_NODE, &left, &node);
        }
        self.state.peaks.push(node);
        self.state.size = self.state.size.checked_add(1).expect("MMR size overflow");
    }

    pub fn root(&self) -> Hash256 {
        let mut acc = self.seed;
        for peak in self.state.peaks.iter().rev() {
            acc = hash_node(DOMAIN_MMR_BAG, peak, &acc);
        }
        acc
    }

    pub fn sign_bytes(&self) -> Hash256 {
        let mut hasher = Hasher::new_derive_key(DOMAIN_SIGN_BYTES);
        // TODO add container & peer id here so it's trivial to bind this signature to a log height without even having the full MMR proof
        hasher.update(&self.state.size.to_le_bytes());
        hasher.update(&self.root());
        hasher.finalize().into()
    }

    pub fn size(&self) -> u64 {
        self.state.size
    }

    pub fn state(&self) -> &MmrState {
        &self.state
    }
}

fn hash_node(domain: &str, left: &Hash256, right: &Hash256) -> Hash256 {
    let mut hasher = Hasher::new_derive_key(domain);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

const DOMAIN_MMR_NODE: &str = "ubiquisync/v1/mmr-node";
const DOMAIN_MMR_BAG: &str = "ubiquisync/v1/mmr-bag";
const DOMAIN_MMR_SEED: &str = "ubiquisync/v1/mmr-seed";
const DOMAIN_SIGN_BYTES: &str = "ubiquisync/v1/sign-bytes";
