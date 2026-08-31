use std::ops::Range;

use crate::crypto::{Hash256, Hash256Suite, TaggedHashDomain};
use thiserror::Error;

pub struct MmrAccumulator {
    seed: Hash256,
    size: u64,
    peaks: Vec<Node>,
    observer: Option<Box<dyn NodeObserver>>,
}

#[derive(Clone, Default, Debug)]
pub struct MmrState {
    pub size: u64,
    pub peaks: Vec<Hash256>,
}

#[derive(Clone, Debug)]
struct Node {
    id: Range<u64>,
    hash: Hash256,
}

#[derive(Clone, Debug)]
struct Bag {
    id: Range<u64>,
    hash: Hash256,
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

#[async_trait::async_trait]
pub trait NodeStore: Send + Sync {
    async fn lookup_node(&self, id: &Range<u64>) -> Option<Hash256>;
}

pub trait NodeObserver {
    fn on_create(&self, id: &Range<u64>, hash: &Hash256);
}

impl MmrState {
    pub fn validate(&self) -> Result<(), MmrError> {
        let expected = peak_count(self.size);
        let actual = self.peaks.len();
        if expected != actual {
            return Err(MmrError::InvalidPeakCount { expected, actual });
        }
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum MmrError {
    #[error("expected {expected} peaks, got {actual}")]
    InvalidPeakCount { expected: usize, actual: usize },
}

const HASH_SUITE: Hash256Suite = Hash256Suite::Sha256;

impl MmrAccumulator {
    pub fn new(seed: Hash256, state: MmrState) -> Result<Self, MmrError> {
        state.validate()?;
        let size = state.size;
        let peaks = peaks_to_nodes(size, &state.peaks).ok_or(MmrError::InvalidPeakCount {
            expected: peak_count(size),
            actual: state.peaks.len(),
        })?;
        Ok(Self {
            seed,
            size,
            peaks,
            observer: None,
        })
    }

    pub fn set_observer(&mut self, observer: Box<dyn NodeObserver>) {
        self.observer = Some(observer)
    }

    pub fn append(&mut self, leaf: &Hash256) {
        let i = self.size;
        let mut node = Node {
            hash: *leaf,
            id: i..i + 1,
        };
        if let Some(ref observer) = self.observer {
            observer.on_create(&node.id, &node.hash);
        }
        for _ in 0..self.size.trailing_ones() {
            let left = self
                .peaks
                .pop()
                .expect("invalid MMR state: expected a peak");
            node = node_hash(&left, &node);
            if let Some(ref observer) = self.observer {
                observer.on_create(&node.id, &node.hash);
            }
        }
        self.peaks.push(node);
        self.size = self.size.checked_add(1).expect("MMR size overflow");
    }

    pub fn root(&self) -> Hash256 {
        let root_bag = root_fold(self.size, &self.seed, &self.peaks);
        root_bag.hash
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn peaks(&self) -> impl Iterator<Item = Hash256> {
        self.peaks.iter().map(|n| n.hash)
    }

    #[allow(dead_code)]
    pub(crate) fn advance_with_cover(
        &mut self,
        end: u64,
        cover: &[Hash256],
    ) -> Result<(), InvalidCoverError> {
        PrefixProof::apply_cover(&mut self.peaks, cover, self.size, end)?;
        self.size = end;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn seed(&self) -> &Hash256 {
        &self.seed
    }
}

#[derive(Error, Debug)]
pub enum InvalidCoverError {
    #[error("invalid cover from {m} to {n}, got {len} hashes")]
    BadCover { m: u64, n: u64, len: usize },
    #[error("invalid cover range {m} to {n}")]
    BadCoverRange { m: u64, n: u64 },
}

fn node_hash(left: &Node, right: &Node) -> Node {
    let left_size = left.size();
    assert!(left_size >= 1);
    let right_size = right.size();
    assert!(right_size >= 1);
    assert_eq!(left.id.end, right.id.start);
    let hash = tagged_hash_node(
        TaggedHashDomain::MmrNode,
        left_size,
        &left.hash,
        right_size,
        &right.hash,
    );
    Node {
        id: left.id.start..right.id.end,
        hash,
    }
}

fn bag_hash(left: &Node, right: &Bag) -> Bag {
    let left_size = left.size();
    assert!(left_size >= 1);
    let right_size = right.size();
    // right size can be 0 for seed
    assert_eq!(left.id.end, right.id.start);
    let hash = tagged_hash_node(
        TaggedHashDomain::MmrBag,
        left_size,
        &left.hash,
        right_size,
        &right.hash,
    );
    Bag {
        id: left.id.start..right.id.end,
        hash,
    }
}

fn seed_bag(seed: &Hash256, size: u64) -> Bag {
    Bag {
        id: size..size,
        hash: *seed,
    }
}

fn tagged_hash_node(
    domain: TaggedHashDomain,
    left_size: u64,
    left: &Hash256,
    right_size: u64,
    right: &Hash256,
) -> Hash256 {
    let mut hasher = HASH_SUITE.new_tagged_hasher(domain);
    hasher.update(&left_size.to_le_bytes());
    hasher.update(left);
    hasher.update(&right_size.to_le_bytes());
    hasher.update(right);
    hasher.finalize()
}

fn root_fold(size: u64, seed: &Hash256, peaks: &[Node]) -> Bag {
    let mut acc = seed_bag(seed, size);
    for peak in peaks.iter().rev() {
        acc = bag_hash(peak, &acc);
    }
    acc
}

impl Node {
    fn size(&self) -> u64 {
        self.id
            .end
            .checked_sub(self.id.start)
            .expect("valid node id")
    }
}

impl Bag {
    fn size(&self) -> u64 {
        self.id
            .end
            .checked_sub(self.id.start)
            .expect("valid node id")
    }
}

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

fn peak_ids(size: u64) -> impl Iterator<Item = Range<u64>> {
    let mut cur = 0;
    std::iter::from_fn(move || {
        if cur < size {
            let left = size - cur;
            let w = 1 << left.ilog2();
            let start = cur;
            let end = start + w;
            cur = end;
            Some(start..end)
        } else {
            None
        }
    })
}

fn peak_ids_rev(size: u64) -> impl Iterator<Item = Range<u64>> {
    let mut cur = size;
    std::iter::from_fn(move || {
        if cur > 0 {
            let w = 1 << cur.trailing_zeros();
            let start = cur - w;
            let end = cur;
            cur = start;
            Some(cur..end)
        } else {
            None
        }
    })
}

fn peaks_to_nodes(size: u64, peaks: &[Hash256]) -> Option<Vec<Node>> {
    let mut nodes = vec![];
    let ids = peak_ids(size);
    let n = peaks.len();
    for (i, id) in ids.enumerate() {
        if i >= n {
            return None;
        }
        nodes.push(Node {
            id: id.clone(),
            hash: peaks[i],
        })
    }
    if n > nodes.len() {
        // don't tolerate trailing peaks
        return None;
    }
    Some(nodes)
}

impl InclusionProof {
    pub fn verify(&self, leaf: &Hash256, root: &Hash256, seed: &Hash256) -> bool {
        let Some(witness_ids) = Self::witness_ids(self.i, self.size) else {
            return false;
        };

        let n = self.witnesses.len();
        if n != witness_ids.len() {
            return false;
        }

        let mut cur = Node {
            id: self.i..self.i + 1,
            hash: *leaf,
        };
        let mut i = 0;

        let climb_node_ids = witness_ids.climb_node_ids;
        while i < climb_node_ids.len() {
            let witness = self.witnesses[i];
            let witness_id = &climb_node_ids[i];
            let w = cur.size();
            if (cur.id.start / w).is_multiple_of(2) {
                // even -> cur LHS, witness RHS
                cur = node_hash(
                    &cur,
                    &Node {
                        id: witness_id.clone(),
                        hash: witness,
                    },
                );
            } else {
                // odd -> witness LHS, cur RHS
                cur = node_hash(
                    &Node {
                        id: witness_id.clone(),
                        hash: witness,
                    },
                    &cur,
                );
            }
            i += 1;
        }

        let mut acc = if let Some(id) = witness_ids.bag_id {
            let bag = Bag {
                id: id.clone(),
                hash: self.witnesses[i],
            };
            i += 1;
            bag
        } else {
            seed_bag(seed, self.size)
        };

        acc = bag_hash(&cur, &acc);

        for j in (i..n).rev() {
            acc = bag_hash(
                &Node {
                    hash: self.witnesses[j],
                    id: witness_ids.lhs_peaks[j - i].clone(),
                },
                &acc,
            );
        }

        acc.hash == *root
    }

    pub async fn generate(
        i: u64,
        size: u64,
        store: &dyn NodeStore,
        seed: &Hash256,
    ) -> Option<Self> {
        let witness_ids = Self::witness_ids(i, size)?;
        let mut witnesses = vec![];
        for id in witness_ids.climb_node_ids.iter() {
            let node = resolve_node(store, id).await?;
            witnesses.push(node.hash);
        }

        if let Some(bag_id) = witness_ids.bag_id {
            let bag = resolve_bag(store, &bag_id, seed).await?;
            witnesses.push(bag.hash);
        }

        for id in witness_ids.lhs_peaks.iter() {
            let node = resolve_node(store, id).await?;
            witnesses.push(node.hash);
        }

        Some(Self { i, size, witnesses })
    }

    fn witness_ids(i: u64, size: u64) -> Option<InclusionProofIds> {
        if i >= size {
            return None;
        }
        let mut climb_node_ids = vec![];
        let mut cur = i..i + 1;
        // phase 1: climb until we reach a peak (which would be present in MMR peak state for this size)
        loop {
            let w = cur.end - cur.start;
            if (cur.start / w) % 2 == 0 {
                let Some(new_end) = cur.end.checked_add(w) else {
                    // this would overflow u64 so there can't be a node this big
                    break;
                };
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
        Some(InclusionProofIds {
            climb_node_ids,
            bag_id,
            lhs_peaks: lhs_peaks.collect::<Vec<_>>(),
        })
    }
}

impl PrefixProof {
    fn apply_cover(
        peaks: &mut Vec<Node>,
        cover: &[Hash256],
        m: u64,
        n: u64,
    ) -> Result<(), InvalidCoverError> {
        let cover_len = cover.len();
        let mut i = 0;
        for id in Self::cover_ids(m, n)? {
            if i >= cover_len {
                // bad cover size
                return Err(InvalidCoverError::BadCover {
                    m,
                    n,
                    len: cover_len,
                });
            }
            reduce_peaks(peaks, Node { id, hash: cover[i] });
            i += 1;
        }
        if i != cover_len {
            // don't tolerate trailing garbage in the cover
            return Err(InvalidCoverError::BadCover {
                m,
                n,
                len: cover_len,
            });
        }
        Ok(())
    }

    pub fn verify(&self, root_m: &Hash256, root_n: &Hash256, seed: &Hash256) -> bool {
        if self.m > self.n {
            return false;
        }

        let Some(mut peaks) = peaks_to_nodes(self.m, &self.peaks_m) else {
            return false;
        };
        if root_fold(self.m, seed, &peaks).hash != *root_m {
            return false;
        }

        if Self::apply_cover(&mut peaks, &self.cover, self.m, self.n).is_err() {
            return false;
        }

        let root_bag_n = root_fold(self.n, seed, &peaks);
        root_bag_n.hash == *root_n
    }

    pub async fn generate(m: u64, n: u64, store: &dyn NodeStore) -> Option<Self> {
        if m > n {
            return None;
        }

        let peaks_ids_m = peak_ids(m);
        let mut peaks_m = vec![];
        for id in peaks_ids_m {
            let node = resolve_node(store, &id).await?;
            peaks_m.push(node.hash);
        }

        let mut cover = vec![];
        let Ok(cover_ids) = Self::cover_ids(m, n) else {
            return None;
        };
        for id in cover_ids {
            let node = resolve_node(store, &id).await?;
            cover.push(node.hash);
        }

        Some(Self {
            m,
            n,
            peaks_m,
            cover,
        })
    }

    // assumes we already have peaks(m)
    fn cover_ids(m: u64, n: u64) -> Result<impl Iterator<Item = Range<u64>>, InvalidCoverError> {
        if m > n {
            return Err(InvalidCoverError::BadCoverRange { m, n });
        }
        let mut p = m;
        Ok(std::iter::from_fn(move || {
            if p < n {
                let cover_width = Self::cover_width(p, n);
                let start = p;
                p += cover_width;
                Some(start..p)
            } else {
                None
            }
        }))
    }

    fn cover_width(p: u64, n: u64) -> u64 {
        assert!(p < n);
        let rhs_width = if p == 0 {
            // this is the largest RHS node that could pair with p
            u64::MAX
        } else {
            1 << p.trailing_zeros()
        };
        let remaining = n - p;
        let fit_width = 1 << (63 - remaining.leading_zeros());
        std::cmp::min(rhs_width, fit_width)
    }
}

fn reduce_peaks(peaks: &mut Vec<Node>, mut right: Node) {
    while let Some(left) = peaks.last() {
        if left.size() != right.size() {
            break;
        }
        let left = peaks.pop().expect("non-empty node");
        right = node_hash(&left, &right);
    }
    peaks.push(right);
}

#[async_recursion::async_recursion]
async fn resolve_node(store: &dyn NodeStore, id: &Range<u64>) -> Option<Node> {
    if let Some(hash) = store.lookup_node(id).await {
        return Some(Node {
            id: id.clone(),
            hash,
        });
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
    let lhs = resolve_node(store, &(id.start..m)).await?;
    let rhs = resolve_node(store, &(m..id.end)).await?;
    Some(node_hash(&lhs, &rhs))
}

async fn resolve_bag(store: &dyn NodeStore, id: &Range<u64>, seed: &Hash256) -> Option<Bag> {
    let mut acc = seed_bag(seed, id.end);
    // lookup all peak ids that are >= id.start for size id.end
    for peak_id in peak_ids_rev(id.end).filter(|p| p.start >= id.start) {
        let peak = resolve_node(store, &peak_id).await?;
        acc = bag_hash(&peak, &acc);
    }
    Some(acc)
}

#[cfg(test)]
mod test {
    use std::fmt::Write;
    use std::{collections::HashMap, ops::Range};

    use insta::assert_snapshot;
    use sha2::{Digest, Sha256};

    use crate::crypto::tests::hex;
    use crate::crypto::{
        Hash256,
        mmr::{
            InclusionProof, MmrAccumulator, MmrState, NodeStore, PrefixProof, peak_count, peak_ids,
            peak_ids_rev,
        },
    };

    #[test]
    fn test_peak_ids() {
        for i in 0..=64 {
            let n = peak_count(i);
            let ids = peak_ids(i).collect::<Vec<_>>();
            let mut ids_rev = peak_ids_rev(i).collect::<Vec<_>>();
            assert_eq!(n, ids.len());
            assert_eq!(n, ids_rev.len());
            // reverse and check they're equal
            ids_rev.reverse();
            assert_eq!(ids, ids_rev);

            if i > 0 {
                assert_eq!(ids[0].start, 0);
                assert_eq!(ids[n - 1].end, i);
            }
        }
    }

    fn deterministic_hash(i: u64) -> Hash256 {
        Sha256::digest(i.to_be_bytes()).into()
    }

    // ensures that our hashes are deterministic and that there is no regression in root hashes or peaks
    #[test]
    fn test_regression() {
        let seed = deterministic_hash(0);
        let mut acc = MmrAccumulator::new(seed, MmrState::default()).unwrap();
        let mut root_snapshot = String::new();
        for i in 0..64 {
            let leaf = deterministic_hash(i + 1);
            acc.append(&leaf);
            let root = hex(&acc.root());
            let size = acc.size();
            let peaks = acc.peaks().map(|h| hex(&h)).collect::<Vec<_>>();
            writeln!(root_snapshot, "{size} {root} {peaks:?}").unwrap();
        }

        assert_snapshot!(root_snapshot);
    }

    #[futures_test::test]
    async fn test_inclusion_proofs() {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).unwrap();
        let mut acc = MmrAccumulator::new(seed, MmrState::default()).unwrap();
        let mut node_store = TestNodeStore::default();
        let mut leaves = vec![];
        for i in 0..=64 {
            if i % 7 == 0 {
                // sometimes reset accumulator
                acc = MmrAccumulator::new(
                    seed,
                    MmrState {
                        size: acc.size(),
                        peaks: acc.peaks().collect(),
                    },
                )
                .unwrap();
            }

            let mut leaf = [0; 32];
            getrandom::fill(&mut leaf).unwrap();
            acc.append(&leaf);
            leaves.push(leaf);
            node_store.insert(i..i + 1, leaf);
            assert_eq!(acc.peaks.len(), peak_count(acc.size));
            if i % 5 == 0 {
                // don't insert all peaks to test how nodes get regenerated
                for node in acc.peaks.iter() {
                    if !node_store.contains_key(&node.id) {
                        node_store.insert(node.id.clone(), node.hash);
                    }
                }
            }

            let root = acc.root();
            for j in 0..=i {
                let mut proof = InclusionProof::generate(j, acc.size(), &node_store, &seed)
                    .await
                    .expect("valid proof");
                let leaf = &leaves[j as usize];

                // muck with the args and verify it fails and doesn't panic!
                assert!(!proof.verify(&muck_bit(leaf), &root, &seed));
                assert!(!proof.verify(leaf, &muck_bit(&root), &seed));
                // NOTE: mucking with the seed MAY or MAY NOT corrupt the proof, the seed isn't always used
                // to prove inclusion of seed, prefix proofs must be used instead!

                // proper args should pass
                assert!(proof.verify(leaf, &root, &seed));

                // muck with witness bits
                for i in 0..proof.witnesses.len() {
                    let w = proof.witnesses[i];
                    proof.witnesses[i] = muck_bit(&w);
                    assert!(!proof.verify(leaf, &root, &seed));
                    proof.witnesses[i] = w;
                    assert!(proof.verify(leaf, &root, &seed)); // should work again
                }
                // muck with u64's
                let proof_i = proof.i;
                proof.i = rand_u64();
                assert!(!proof.verify(leaf, &root, &seed));
                proof.i = proof_i;
                assert!(proof.verify(leaf, &root, &seed)); // should work again
                let proof_size = proof.size;
                proof.size = rand_u64();
                assert!(!proof.verify(leaf, &root, &seed));
                proof.size = proof_size;
                assert!(proof.verify(leaf, &root, &seed)); // should work again
            }
        }
    }

    #[futures_test::test]
    async fn test_prefix_proofs() {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).unwrap();
        let mut acc = MmrAccumulator::new(seed, MmrState::default()).unwrap();
        let mut node_store = TestNodeStore::default();
        let mut roots = vec![];
        for i in 0..64 {
            let mut leaf = [0; 32];
            getrandom::fill(&mut leaf).unwrap();
            acc.append(&leaf);
            node_store.insert(i..i + 1, leaf);
            assert_eq!(acc.peaks.len(), peak_count(acc.size));
            if i % 5 == 0 {
                // don't insert all peaks to test how nodes get regenerated
                for node in acc.peaks.iter() {
                    if !node_store.contains_key(&node.id) {
                        node_store.insert(node.id.clone(), node.hash);
                    }
                }
            }

            let root = acc.root();
            roots.push(root);
            let size = acc.size();
            assert_eq!(i + 1, size);

            for j in 0..=size {
                let mut proof = PrefixProof::generate(j, size, &node_store)
                    .await
                    .expect("valid proof");
                let root_j = if j == 0 {
                    // this tests proofs that the full tree is rooted to the seed
                    &seed
                } else {
                    &roots[(j - 1) as usize]
                };

                // muck with the args and verify it fails and doesn't panic!
                assert!(!proof.verify(&muck_bit(root_j), &root, &seed));
                assert!(!proof.verify(root_j, &muck_bit(&root), &seed));
                assert!(!proof.verify(root_j, &root, &muck_bit(&seed)));

                // proper args should pass
                assert!(proof.verify(root_j, &root, &seed), "m:{j} n:{size}");

                // muck with peak bits
                for i in 0..proof.peaks_m.len() {
                    let w = proof.peaks_m[i];
                    proof.peaks_m[i] = muck_bit(&w);
                    assert!(!proof.verify(root_j, &root, &seed));
                    proof.peaks_m[i] = w;
                    assert!(proof.verify(root_j, &root, &seed)); // should work again
                }
                // muck with cover bits
                for i in 0..proof.cover.len() {
                    let w = proof.cover[i];
                    proof.cover[i] = muck_bit(&w);
                    assert!(!proof.verify(root_j, &root, &seed));
                    proof.cover[i] = w;
                    assert!(proof.verify(root_j, &root, &seed)); // should work again
                }
                // muck with u64's
                let proof_m = proof.m;
                proof.m = rand_u64();
                assert!(!proof.verify(root_j, &root, &seed));
                proof.m = proof_m;
                assert!(proof.verify(root_j, &root, &seed)); // should work again
                let proof_n = proof.n;
                proof.n = rand_u64();
                assert!(!proof.verify(root_j, &root, &seed));
                proof.n = proof_n;
                assert!(proof.verify(root_j, &root, &seed)); // should work again
            }
        }
    }

    fn muck_bit(hash: &Hash256) -> Hash256 {
        let mut hash = *hash;
        let mut bit = [0; 1];
        getrandom::fill(&mut bit).unwrap();
        let byte = bit[0] / 8;
        let bit = bit[0] - (byte * 8);
        hash[byte as usize] ^= 1 << bit;
        hash
    }

    fn rand_u64() -> u64 {
        let mut buf = [0; 8];
        getrandom::fill(&mut buf).unwrap();
        u64::from_le_bytes(buf)
    }

    type TestNodeStore = HashMap<Range<u64>, Hash256>;

    #[async_trait::async_trait]
    impl NodeStore for HashMap<Range<u64>, Hash256> {
        async fn lookup_node(&self, id: &Range<u64>) -> Option<Hash256> {
            self.get(id).cloned()
        }
    }
}
