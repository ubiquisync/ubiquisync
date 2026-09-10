use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Display,
    ops::Range,
    path::PathBuf,
};

use ubiquisync_core::ids::PeerId;

use crate::pack::PackRef;

pub struct PackFileDescriptor {
    pub topic: PathBuf,
    pub peer_id: PeerId,
    pub name: PackFileName,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackFileName {
    pub seqs: Range<u64>,
    pub id: u64,
    pub generation: u64,
}

impl PackFileName {
    pub fn get_ref(&self) -> PackRef {
        PackRef {
            id: self.id,
            end_seq: self.seqs.end,
        }
    }
}

impl Display for PackFileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04x}-{:04x}-{:016x}-{:02x}",
            self.seqs.start, self.seqs.end, self.id, self.generation
        )
    }
}

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

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use test_case::test_case;

    use crate::pack::PackFileName;

    #[test_case(0..1, 0xabcdef, 0 => "0000-0001-0000000000abcdef-00")]
    #[test_case(0xffff..0xa0000, 0x12345, 0x100 => "ffff-a0000-0000000000012345-100")]
    fn pack_file_name_display(seqs: Range<u64>, id: u64, generation: u64) -> String {
        PackFileName {
            seqs,
            id,
            generation,
        }
        .to_string()
    }
}
