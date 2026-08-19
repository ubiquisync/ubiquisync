use std::ops::Range;

use crate::{
    crypto::{Hash256, Hash256Suite, TaggedHashDomain},
    ids::{ContainerId, PeerId},
};
use thiserror::Error;

pub struct MmrAccumulator {
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

const HASH_SUITE: Hash256Suite = Hash256Suite::Sha256;

impl MmrAccumulator {
    pub fn new(
        peer_id: &PeerId,
        container_id: &ContainerId,
        state: MmrState,
    ) -> Result<Self, MmrError> {
        state.validate()?;
        let mut seed_hasher = HASH_SUITE.new_tagged_hasher(super::TaggedHashDomain::MmrSeed);
        seed_hasher.update(&peer_id.0);
        seed_hasher.update(&container_id.0);
        let seed = seed_hasher.finalize();
        Ok(Self {
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
            node = hash_node(TaggedHashDomain::MmrNode, &left, &node);
        }
        self.state.peaks.push(node);
        self.state.size = self.state.size.checked_add(1).expect("MMR size overflow");
    }

    pub fn root(&self) -> Hash256 {
        let mut acc = self.seed;
        for peak in self.state.peaks.iter().rev() {
            acc = hash_node(TaggedHashDomain::MmrBag, peak, &acc);
        }
        acc
    }

    pub fn sign_bytes(&self) -> Hash256 {
        let mut hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::MmrSignBytes);
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
}

fn hash_node(domain: TaggedHashDomain, left: &Hash256, right: &Hash256) -> Hash256 {
    let mut hasher = HASH_SUITE.new_tagged_hasher(domain);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

pub struct InclusionProof {
    pub i: u64,
    pub size: u64,
    pub witnesses: Vec<Hash256>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WitnessId {
    Node(Range<u64>),
    Bag(Range<u64>),
}

fn peak_count(size: u64) -> usize {
    size.count_ones() as usize
}

fn peak_ids(mut size: u64) -> Vec<Range<u64>> {
    let mut ids = vec![];
    let mut n = 0;
    let mut end = size;
    while size > 0 {
        if size & 0x1 == 0x1 {
            let start = (size & !0x1) << n;
            ids.push(start..end);
            end = start;
        }
        size = size >> 1;
        n += 1;
    }
    ids.reverse();
    ids
}

impl InclusionProof {
    pub fn verify(&self, leaf: &Hash256, root: &Hash256, seed: &Hash256) -> bool {
        if self.i >= self.size {
            return false;
        }
        let (witness_ids, need_seed) = Self::witness_ids(self.i, self.size);
        let n = self.witnesses.len();
        if n != witness_ids.len() {
            return false;
        }
        let mut cur = *leaf;
        let mut cur_id = self.i..self.i + 1;
        for i in 0..n {
            let witness = self.witnesses[i];
            match &witness_ids[i] {
                WitnessId::Node(witness_id) => {
                    let w = cur_id.end - cur_id.end;
                    if (cur_id.start / w) % 2 == 0 {
                        // even -> cur LHS, witness RHS
                        cur = hash_node(TaggedHashDomain::MmrNode, &cur, &witness);
                        cur_id = cur_id.start..witness_id.end;
                    } else {
                        // odd -> witness LHS, cur RHS
                        cur = hash_node(TaggedHashDomain::MmrNode, &witness, &cur);
                        cur_id = witness_id.start..cur_id.end;
                    }
                }
                WitnessId::Bag(witness_id) => {
                    cur = hash_node(TaggedHashDomain::MmrBag, &cur, &witness);
                    cur_id = cur_id.start..witness_id.end;
                }
            }
        }
        if need_seed {
            cur = hash_node(TaggedHashDomain::MmrBag, &cur, &seed);
        }
        cur == *root
    }

    fn witness_ids(i: u64, size: u64) -> (Vec<WitnessId>, bool) {
        assert!(i < size);
        let mut witnesses = vec![];
        let mut cur = i..i + 1;
        loop {
            let w = cur.end - cur.start;
            if (cur.start / w) % 2 == 0 {
                // even -> need RHS
                let new_end = cur
                    .end
                    .checked_add(w)
                    .expect("proof shouldn't overflow u64");
                if new_end > size {
                    if cur.start != 0 {
                        let mut lhs_peaks: Vec<WitnessId> = peak_ids(cur.start)
                            .iter()
                            .map(|r| WitnessId::Node(r.clone()))
                            .collect();
                        witnesses.append(&mut lhs_peaks);
                    }
                    let need_bag = cur.end < size;
                    if need_bag {
                        witnesses.push(WitnessId::Bag(cur.end..size));
                    }
                    let need_seed = !need_bag;
                    return (witnesses, need_seed);
                }
                let rhs = cur.end..new_end;
                witnesses.push(WitnessId::Node(rhs));
                cur = cur.start..new_end;
            } else {
                // odd -> need LHS
                let start = cur.start - w;
                let lhs = start..cur.start;
                witnesses.push(WitnessId::Node(lhs));
                cur = start..cur.end;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::crypto::mmr::{InclusionProof, peak_count, peak_ids};

    #[test]
    fn test1() {
        for i in 0..=16 {
            testn(i);
        }
    }

    fn testn(size: u64) {
        let n = peak_count(size);
        let peaks = peak_ids(size);
        assert_eq!(n, peaks.len());
        println!("{size} {n} {peaks:?}")
    }

    #[test]
    fn test2() {
        testi(7, 21);
        testi(17, 21);
        testi(20, 21);
    }

    fn testi(i: u64, size: u64) {
        let (w, need_seed) = InclusionProof::witness_ids(i, size);
        println!("{i} in {size} -> {w:?} {need_seed}")
    }
}
