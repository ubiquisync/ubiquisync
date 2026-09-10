use std::{
    collections::{
        HashMap, HashSet,
        hash_map::{self, Entry},
    },
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

/// Dedupes files and chooses the latest valid generation of a pack.
/// NOTE: if a generation file is corrupted garbage we could inspect
/// earlier generations at read time, but if this happens in a shared folder
/// there are bigger problems (noting this here so that reviewers don't keep
/// flagging a thread that doesn't match the threat model).
pub fn dedupe_pack_files(files: &[PackFileName]) -> Vec<PackFileName> {
    let mut files_by_ref = HashMap::<PackRef, PackFileName>::new();
    for f in files.iter() {
        match files_by_ref.entry(f.get_ref()) {
            Entry::Occupied(mut e) => {
                if e.get().generation < f.generation {
                    e.insert(f.clone());
                }
            }
            Entry::Vacant(e) => {
                e.insert(f.clone());
            }
        }
    }
    files_by_ref.into_values().collect::<Vec<_>>()
}
