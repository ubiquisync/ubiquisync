use crate::{
    crypto::{Hash256, Hash256Suite, TaggedHashDomain},
    ids::{ContainerId, PeerId},
};
use thiserror::Error;

pub struct MmrAccumulator {
    hash_suite: Hash256Suite,
    state: MmrState,
    seed: Hash256,
    peer_id: PeerId,
    container_id: ContainerId,
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
        hash_suite: Hash256Suite,
        peer_id: &PeerId,
        container_id: &ContainerId,
        state: MmrState,
    ) -> Result<Self, MmrError> {
        state.validate()?;
        let mut seed_hasher = hash_suite.new_tagged_hasher(super::TaggedHashDomain::MmrSeed);
        seed_hasher.update(&peer_id.0);
        seed_hasher.update(&container_id.0);
        let seed = seed_hasher.finalize();
        Ok(Self {
            hash_suite,
            seed,
            state,
            container_id: *container_id,
            peer_id: *peer_id,
        })
    }

    pub fn append(&mut self, leaf: &Hash256) {
        let mut node = *leaf;
        for _ in 0..self.state.size.trailing_ones() {
            let left = self
                .state
                .peaks
                .pop()
                .expect("invalid MMR state: expected a peak");
            node = self.hash_node(TaggedHashDomain::MmrNode, &left, &node);
        }
        self.state.peaks.push(node);
        self.state.size = self.state.size.checked_add(1).expect("MMR size overflow");
    }

    pub fn root(&self) -> Hash256 {
        let mut acc = self.seed;
        for peak in self.state.peaks.iter().rev() {
            acc = self.hash_node(TaggedHashDomain::MmrBag, peak, &acc);
        }
        acc
    }

    pub fn sign_bytes(&self) -> Hash256 {
        let mut hasher = self
            .hash_suite
            .new_tagged_hasher(TaggedHashDomain::MmrSignBytes);
        hasher.update(&self.peer_id.0);
        hasher.update(&self.container_id.0);
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

    fn hash_node(&self, domain: TaggedHashDomain, left: &Hash256, right: &Hash256) -> Hash256 {
        let mut hasher = self.hash_suite.new_tagged_hasher(domain);
        hasher.update(left);
        hasher.update(right);
        hasher.finalize().into()
    }
}
