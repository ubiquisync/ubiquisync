use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    ops::Range,
};

use crate::pack::{PackFileName, PackRef};

#[derive(Debug, Clone)]
pub struct PackReadState {
    /// The packs we have already inspected either
    /// directly or in a prior generation.
    /// These get saved in state.
    pub consumed: HashSet<PackRef>,

    pub blocked: Vec<BlockedPackRef>,
}

#[derive(Debug, Clone)]
pub struct PackReadResult {
    pub new_state: PackReadState,

    /// The files we need to inspect. We only retain this
    /// transiently between directory listings and add files
    /// to the consumed state as we inspect them.
    pub to_read: Vec<PackFileName>,
}

#[derive(Debug, Clone)]
pub struct BlockedPackRef {
    pub file: PackFileName,
    pub parents: HashSet<PackRef>,
    pub read_timestamps: Range<u64>,
    pub retries: u64,
}

impl PackReadState {
    pub fn update(&self, ts: u64, cur_files: &[PackFileName]) -> PackReadResult {
        let mut to_read = vec![];
        let mut new_consumed = HashSet::new();
        for f in cur_files.iter() {
            let r = f.get_ref();
            if self.consumed.contains(&r) {
                new_consumed.insert(r);
            } else {
                to_read.push(f.clone());
            }
        }
        to_read.sort_by_key(|v| v.seqs.end);
        // TODO handled blocked packs
        PackReadResult {
            new_state: PackReadState {
                consumed: new_consumed,
                blocked: self.blocked.clone(),
            },
            to_read,
        }
    }
}

// /// `consumed` and `cur` must be deduped already
// pub fn compute_read_state(consumed: &[PackRef], cur: &[PackFileName]) -> PackReadState {
//     let consumed_set = consumed.iter().collect::<HashSet<_>>();
//     let mut to_read = vec![];
//     let mut new_consumed = vec![];
//     for f in cur.iter() {
//         let r = f.get_ref();
//         if consumed_set.contains(&r) {
//             new_consumed.push(r);
//         } else {
//             to_read.push(f.clone());
//         }
//     }
//     to_read.sort_by_key(|v| v.seqs.end);
//     PackReadState {
//         consumed: new_consumed,
//         to_read,
//     }
// }
