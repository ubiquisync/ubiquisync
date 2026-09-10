use std::{fmt::Display, ops::Range, path::PathBuf};

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
