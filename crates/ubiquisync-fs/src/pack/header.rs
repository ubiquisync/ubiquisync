use std::ops::Range;

use ubiquisync_core::{
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::Hash256,
    ids::{ContainerId, PeerId},
};

#[derive(Debug, Clone)]
pub struct PackHeader {
    pub parents: Vec<PackRef>,
    /// The list of packs this pack file supersedes directly.
    /// This is used by GC to know when it is safe to delete a pack
    /// which has been compressed.
    /// It is NOT necessary to reference packs which were superseded
    /// by prior compression rounds and doing so just unnecessarily
    /// bloats the header file.
    /// The safe GC condition is simply: is there a pack file which
    /// names the pack ref in its file name (with a later generation)
    /// or in its supersedes list directly, and has a sufficient amount
    /// of time elapsed since that file was written (as a safety buffer).
    pub supersedes: Vec<PackRef>,
    pub self_segments: Vec<SegmentDescriptor>,
    pub peer_data: Vec<PeerData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackRef {
    pub id: u64,
    pub end_seq: u64,
}

#[derive(Debug, Clone)]
pub struct PeerData {
    pub peer_id: PeerId,
    pub supersedes: Vec<PackRef>,
    pub segments: Vec<SegmentDescriptor>,
}

#[derive(Debug, Clone)]
pub struct SegmentDescriptor {
    pub container_id: ContainerId,
    pub idx_range: Range<u64>,
    pub prev_chain: Hash256,
    pub end_chain: Hash256,
    pub body_loc: Range<u64>,
}

impl PackHeader {
    pub fn encode(&self, w: &mut Writer) {
        todo!()
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        todo!()
    }
}

// impl PackRef {
//     pub fn encode(&self, w: &mut Writer) {
//         // fixed length encoding because id is a random number
//         w.write_le_u64(self.id);
//         w.write_var_u64(self.end_seq);
//     }

//     pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
//         let id = r.read_le_u64()?;
//         let end_seq = r.read_var_u64()?;
//         Ok(Self { id, end_seq })
//     }
// }

// impl ForeignSegments {
//     pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
//         w.write_array(&self.peer_id.0);
//         w.write_var_usize(self.segments.len());
//         for s in self.segments.iter() {
//             s.encode(w)?;
//         }
//         Ok(())
//     }

//     pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
//         let peer_id = PeerId(r.read_array()?);
//         let n = r.read_var_usize()?;
//         // NOTE: do not reserve an array to avoid OOM attacks!
//         let mut segments = vec![];
//         for _ in 0..n {
//             segments.push(SegmentDescriptor::decode(r)?);
//         }
//         Ok(Self { peer_id, segments })
//     }
// }

// impl SegmentDescriptor {
//     pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
//         w.write_array(&self.container_id.0);
//         self.prev_chain.encode(w);
//         self.end_chain.encode(w);
//         w.write_range(&self.body_loc)?;
//         Ok(())
//     }

//     pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
//         let container_id = ContainerId(r.read_array()?);
//         let prev_chain = ChainHash::decode(r)?;
//         let end_chain = ChainHash::decode(r)?;
//         let body_loc = r.read_range()?;
//         Ok(Self {
//             container_id,
//             prev_chain,
//             end_chain,
//             body_loc,
//         })
//     }
// }
