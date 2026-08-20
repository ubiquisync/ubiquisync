use std::ops::Range;

use crate::crypto::{Hash256, Hash256Suite, TaggedHashDomain};
use thiserror::Error;

pub struct MmrAccumulator {
    state: MmrState,
    seed: Hash256,
    // peer_id: PeerId,
    // container_id: ContainerId,
}

#[derive(Clone, Default)]
pub struct MmrState {
    pub size: u64,
    pub peaks: Vec<Hash256>,
}

pub struct InclusionProof {
    pub i: u64,
    pub size: u64,
    pub witnesses: Vec<Hash256>,
}

pub struct PrefixProof {
    pub m: u64,
    pub n: u64,
    pub peaks_m: Vec<Hash256>,
    pub cover: Vec<Hash256>,
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

    pub fn node_ids(&self) -> Vec<Range<u64>> {
        peak_ids(self.size)
    }
}

#[derive(Error, Debug)]
pub enum MmrError {
    #[error("expected {expected} peaks, got {actual}")]
    InvalidPeakCount { expected: u32, actual: usize },
}

const HASH_SUITE: Hash256Suite = Hash256Suite::Sha256;

impl MmrAccumulator {
    pub fn new(seed: Hash256, state: MmrState) -> Result<Self, MmrError> {
        state.validate()?;
        // let mut seed_hasher = HASH_SUITE.new_tagged_hasher(super::TaggedHashDomain::MmrSeed);
        // seed_hasher.update(&peer_id.0);
        // seed_hasher.update(&container_id.0);
        // let seed = seed_hasher.finalize();
        Ok(Self {
            seed,
            state,
            // container_id: *container_id,
            // peer_id: *peer_id,
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
        root_fold(&self.seed, &self.state.peaks)
    }

    // pub fn sign_bytes(&self) -> Hash256 {
    //     let mut hasher = HASH_SUITE.new_tagged_hasher(TaggedHashDomain::MmrSignBytes);
    //     hasher.update(&self.peer_id.0);
    //     hasher.update(&self.container_id.0);
    //     hasher.update(&self.state.size.to_le_bytes());
    //     hasher.update(&self.root());
    //     hasher.finalize().into()
    // }

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

fn root_fold(seed: &Hash256, peaks: &[Hash256]) -> Hash256 {
    let mut acc = *seed;
    for peak in peaks.iter().rev() {
        acc = hash_node(TaggedHashDomain::MmrBag, peak, &acc);
    }
    acc
}

// #[derive(Debug, Clone, PartialEq, Eq)]
// enum WitnessId {
//     Node(Range<u64>),
//     Bag(Range<u64>),
// }

#[derive(Debug, Clone, PartialEq, Eq)]
struct InclusionProofIds {
    climb_node_ids: Vec<Range<u64>>,
    bag_id: Option<Range<u64>>,
    lhs_peaks: Vec<Range<u64>>,
}

impl InclusionProofIds {
    fn len(&self) -> usize {
        let mut n = self.climb_node_ids.len() + self.lhs_peaks.len();
        if self.bag_id.is_some() {
            n += 1
        }
        n
    }
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
        let witness_ids = Self::witness_ids(self.i, self.size);
        let n = self.witnesses.len();
        if n != witness_ids.len() {
            return false;
        }
        let mut cur = *leaf;
        let mut cur_id = self.i..self.i + 1;
        let mut i = 0;

        let climb_node_ids = witness_ids.climb_node_ids;
        while i < climb_node_ids.len() {
            let witness = self.witnesses[i];
            let witness_id = &climb_node_ids[i];
            let w = cur_id.end - cur_id.start;
            if (cur_id.start / w) % 2 == 0 {
                // even -> cur LHS, witness RHS
                cur = hash_node(TaggedHashDomain::MmrNode, &cur, &witness);
                cur_id = cur_id.start..witness_id.end;
            } else {
                // odd -> witness LHS, cur RHS
                cur = hash_node(TaggedHashDomain::MmrNode, &witness, &cur);
                cur_id = witness_id.start..cur_id.end;
            }
            i += 1;
        }

        if witness_ids.bag_id.is_some() {
            cur = hash_node(TaggedHashDomain::MmrBag, &cur, &self.witnesses[i]);
            i += 1;
        } else {
            cur = hash_node(TaggedHashDomain::MmrBag, &cur, &seed);
        }

        for j in (i..n).rev() {
            cur = hash_node(TaggedHashDomain::MmrBag, &self.witnesses[j], &cur);
        }

        cur == *root
    }

    pub fn generate(i: u64, size: u64, store: &dyn NodeStore, seed: &Hash256) -> Option<Self> {
        let witness_ids = Self::witness_ids(i, size);
        let mut witnesses = vec![];
        for id in witness_ids.climb_node_ids.iter() {
            let node = resolve_node(store, &id)?;
            witnesses.push(node);
        }

        if let Some(bag_id) = witness_ids.bag_id {
            let bag = resolve_bag(store, &bag_id, seed)?;
            witnesses.push(bag);
        }

        for id in witness_ids.lhs_peaks.iter() {
            let node = resolve_node(store, &id)?;
            witnesses.push(node);
        }

        Some(Self { i, size, witnesses })
    }

    fn witness_ids(i: u64, size: u64) -> InclusionProofIds {
        assert!(i < size);
        let mut climb_node_ids = vec![];
        let mut cur = i..i + 1;
        // phase 1: climb until we reach a peak (which would be present in MMR peak state for this size)
        loop {
            let w = cur.end - cur.start;
            if (cur.start / w) % 2 == 0 {
                // even -> need RHS
                let new_end = cur
                    .end
                    .checked_add(w)
                    .expect("proof shouldn't overflow u64");
                if new_end > size {
                    break;
                }
                let rhs = cur.end..new_end;
                climb_node_ids.push(rhs);
                cur = cur.start..new_end;
            } else {
                // odd -> need LHS
                let start = cur.start - w;
                let lhs = start..cur.start;
                climb_node_ids.push(lhs);
                cur = start..cur.end;
            }
        }

        // phase 2: is there an initial bag in the proof or do we use the seed
        let bag_id = if cur.end < size {
            Some(cur.end..size)
        } else {
            None
        };

        // phase 3: collect any remaining LHS peaks that must be bagged (rather than hashed as nodes), this all would have been in the peak state for this size
        // will be empty if cur.start == 0
        let lhs_peaks = peak_ids(cur.start);
        InclusionProofIds {
            climb_node_ids,
            bag_id,
            lhs_peaks,
        }
    }
}

impl PrefixProof {
    pub fn verify(&self, root_m: &Hash256, root_n: &Hash256, seed: &Hash256) -> bool {
        if self.m > self.n {
            return false;
        }

        let peaks_ids_m = peak_ids(self.m);
        if self.peaks_m.len() != peak_count(self.m) {
            return false;
        }
        let cover_ids = Self::cover_ids(self.m, self.n);
        if cover_ids.len() != self.cover.len() {
            return false;
        }
        if root_fold(seed, &self.peaks_m) != *root_m {
            return false;
        }

        let mut peaks = self.peaks_m.clone();
        let mut peak_ids = peaks_ids_m.clone();
        for (i, id) in cover_ids.iter().enumerate() {
            let span = id.end - id.start;

            if let Some(last) = peak_ids.last().cloned() {
                let last_span = last.end - last.start;
                if last_span == span {
                    peak_ids.pop();
                    let node = hash_node(
                        TaggedHashDomain::MmrNode,
                        &peaks.pop().expect("non-empty node"),
                        &self.cover[i],
                    );
                    peaks.push(node);
                    peak_ids.push(last.start..id.end);
                }
            } else {
                peaks.push(self.cover[i]);
                peak_ids.push(id.clone());
            }
        }

        root_fold(seed, &peaks) == *root_n
    }

    fn reduce_peaks(
        peaks: &mut Vec<Hash256>,
        peak_ids: &mut Vec<Range<u64>>,
        id: Range<u64>,
        node: Hash256,
    ) {
        let span = id.end - id.start;
        if let Some(last) = peak_ids.last().cloned() {
            let last_span = last.end - last.start;
            if last_span == span {
                peak_ids.pop();
                let node = hash_node(
                    TaggedHashDomain::MmrNode,
                    &peaks.pop().expect("non-empty node"),
                    &node,
                );
                peaks.push(node);
                peak_ids.push(last.start..id.end);
            }
        } else {
            peaks.push(node);
            peak_ids.push(id);
        }
    }

    pub fn generate(m: u64, n: u64, store: &dyn NodeStore) -> Option<Self> {
        let peaks_ids_m = peak_ids(m);
        let cover_ids = Self::cover_ids(m, n);
        let mut peaks_m = vec![];
        for id in peaks_ids_m.iter() {
            let node = store.lookup_node(id)?;
            peaks_m.push(node);
        }

        let mut cover = vec![];
        for id in cover_ids.iter() {
            let node = store.lookup_node(id)?;
            cover.push(node);
        }

        Some(Self {
            m,
            n,
            peaks_m,
            cover,
        })
    }

    // assumes we already have peaks(m)
    fn cover_ids(m: u64, n: u64) -> Vec<Range<u64>> {
        assert!(m < n);
        let mut cover = vec![];
        let mut p = m;
        while p < n {
            let mut width = if p == 0 {
                // this is the largest RHS node that could pair with p
                u64::MAX
            } else {
                1 << p.trailing_zeros()
            };
            let remaining = n - p;
            if width > remaining {
                // if such a node is too wide to fit within n
                width = 1 << (63 - remaining.leading_zeros());
            }
            let end = p + width;
            cover.push(p..end);
            p = end;
        }
        cover
    }

    // fn breakdown_peak_cover(id: &Range<u64>, m: u64, cover: &mut Vec<Range<u64>>) {
    //     if id.start >= m {
    //         // no part of peak is in m
    //         cover.push(id.clone());
    //     } else {
    //         if id.end <= m {
    //             // peak is already in peaks(m)
    //             return;
    //         } else {
    //             // some sub-tree of peak is in peaks(m)
    //             let span = id.end - id.start;
    //             assert_ne!(
    //                 span, 1,
    //                 "leaves should be covered 100% by the id.start >= m case"
    //             );
    //             let mid = id.start + (span / 2);
    //             // lhs
    //             Self::breakdown_peak_cover(&(id.start..mid), m, cover);
    //             // rhs
    //             Self::breakdown_peak_cover(&(mid..id.end), m, cover);
    //         }
    //     }
    // }
}

pub trait NodeStore {
    fn lookup_node(&self, id: &Range<u64>) -> Option<Hash256>;
}

fn resolve_node(store: &dyn NodeStore, id: &Range<u64>) -> Option<Hash256> {
    if let Some(node) = store.lookup_node(id) {
        return Some(node);
    }
    let span = id.end.checked_sub(id.start).expect("valid range");
    if span <= 1 {
        return None;
    }
    if span.count_ones() != 1 {
        // quick check that is a power of 2
        return None;
    }
    let m = id.start + span / 2;
    let lhs = resolve_node(store, &(id.start..m))?;
    let rhs = resolve_node(store, &(m..id.end))?;
    Some(hash_node(TaggedHashDomain::MmrNode, &lhs, &rhs))
}

fn resolve_bag(store: &dyn NodeStore, id: &Range<u64>, seed: &Hash256) -> Option<Hash256> {
    let mut acc = *seed;
    // lookup all peak ids that are >= id.start for size id.end
    for peak_id in peak_ids(id.end)
        .iter()
        .filter(|p| p.start >= id.start)
        .rev()
    {
        let peak = resolve_node(store, peak_id)?;
        acc = hash_node(TaggedHashDomain::MmrBag, &peak, &acc);
    }
    Some(acc)
}

#[cfg(test)]
mod test {
    use std::{collections::HashMap, ops::Range};

    use crate::{
        crypto::{
            Hash256,
            mmr::{
                InclusionProof, MmrAccumulator, MmrState, NodeStore, PrefixProof, peak_count,
                peak_ids,
            },
        },
        rand::rand_fill,
    };

    #[test]
    fn test_peak_id_count() {
        for i in 0..=64 {
            assert_eq!(peak_count(i), peak_ids(i).len())
        }
    }

    #[test]
    fn test_inclusion_proofs() {
        let mut seed = [0; 32];
        rand_fill(&mut seed).unwrap();
        let mut acc = MmrAccumulator::new(seed.clone(), MmrState::default()).unwrap();
        let mut node_store = TestNodeStore::default();
        let mut leaves = vec![];
        for i in 0..=64 {
            let mut leaf = [0; 32];
            rand_fill(&mut leaf).unwrap();
            acc.append(&leaf);
            leaves.push(leaf.clone());
            node_store.insert(i..i + 1, leaf.clone());
            let state = acc.state();
            assert_eq!(state.peaks.len(), peak_count(state.size));
            for (i, id) in state.node_ids().iter().enumerate() {
                if !node_store.contains_key(id) {
                    node_store.insert(id.clone(), state.peaks[i]);
                }
            }

            let root = acc.root();
            for j in 0..=i {
                let proof = InclusionProof::generate(j, acc.size(), &node_store, &seed)
                    .expect("valid proof");
                let leaf = &leaves[j as usize];
                assert!(proof.verify(leaf, &root, &seed));
            }
        }
    }

    #[test]
    fn test_prefix_proofs() {
        let mut seed = [0; 32];
        rand_fill(&mut seed).unwrap();
        let mut acc = MmrAccumulator::new(seed.clone(), MmrState::default()).unwrap();
        let mut node_store = TestNodeStore::default();
        let mut roots = vec![];
        for i in 0..=64 {
            let mut leaf = [0; 32];
            rand_fill(&mut leaf).unwrap();
            acc.append(&leaf);
            node_store.insert(i..i + 1, leaf.clone());
            let state = acc.state();
            assert_eq!(state.peaks.len(), peak_count(state.size));
            for (i, id) in state.node_ids().iter().enumerate() {
                if !node_store.contains_key(id) {
                    node_store.insert(id.clone(), state.peaks[i]);
                }
            }

            let root = acc.root();
            roots.push(root.clone());

            for j in 0..=i {
                let proof = PrefixProof::generate(j, acc.size(), &node_store).expect("valid proof");
                let root_j = &roots[j as usize];
                assert!(proof.verify(root_j, &root, &seed));
            }
        }
    }

    type TestNodeStore = HashMap<Range<u64>, Hash256>;

    impl NodeStore for HashMap<Range<u64>, Hash256> {
        fn lookup_node(&self, id: &Range<u64>) -> Option<Hash256> {
            self.get(id).cloned()
        }
    }

    #[test]
    fn test_cover() {
        print_cover(4, 7);
        print_cover(2, 12);
        print_cover(0, 8);
        print_cover(0, 15);
    }

    fn print_cover(m: u64, n: u64) {
        let cover = PrefixProof::cover_ids(m, n);
        println!("{m} {n} {cover:?}")
    }
}
