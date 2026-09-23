use std::ops::Range;

use crate::{
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::Hash256,
    ids::{ContainerId, PeerId},
};

use crate::{log::ChainHash, pack::PackFileName};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
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
    pub self_supersedes: Vec<PackRef>,
    pub self_segments: Vec<SegmentDescriptor>,
    pub peer_data: Vec<PeerData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct PackRef {
    pub id: u64,
    pub end_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct PeerData {
    pub peer_id: PeerId,
    /// Names precisely a peer file this pack supersedes.
    /// Note we MUST include generation because there
    /// could be race condition in which the peer compresses
    /// two packs into the PackRef we're targetting - we
    /// need to know _which_ actual file range/generation
    /// we're targetting.
    pub supersedes: Vec<PackFileName>,
    pub segments: Vec<SegmentDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct SegmentDescriptor {
    pub container_id: ContainerId,
    pub idx_range: Range<u64>,
    pub prev_chain: Hash256,
    pub end_chain: Hash256,
    pub body_loc: Range<u64>,
}

impl PackHeader {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_vec(&self.parents, |w, x| x.encode(w))?;
        w.write_vec(&self.self_supersedes, |w, x| x.encode(w))?;
        w.write_vec(&self.self_segments, |w, x| x.encode(w))?;
        w.write_vec(&self.peer_data, |w, x| x.encode(w))?;
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        let parents = r.read_vec(|r| PackRef::decode(r))?;
        let self_supersedes = r.read_vec(|r| PackRef::decode(r))?;
        let self_segments = r.read_vec(|r| SegmentDescriptor::decode(r))?;
        let peer_data = r.read_vec(|r| PeerData::decode(r))?;
        Ok(Self {
            parents,
            self_supersedes,
            self_segments,
            peer_data,
        })
    }
}

impl PackRef {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        // fixed length encoding because id is a random number
        w.write_le_u64(self.id);
        w.write_var_u64(self.end_seq);
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        let id = r.read_le_u64()?;
        let end_seq = r.read_var_u64()?;
        Ok(Self { id, end_seq })
    }
}

impl PeerData {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_array(&self.peer_id.0);
        w.write_vec(&self.segments, |w, s| s.encode(w))?;
        w.write_vec(&self.supersedes, |w, s| s.encode(w))?;

        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        let peer_id = PeerId(r.read_array()?);
        let segments = r.read_vec(|r| SegmentDescriptor::decode(r))?;
        let supersedes = r.read_vec(|r| PackFileName::decode(r))?;

        Ok(Self {
            peer_id,
            segments,
            supersedes,
        })
    }
}

impl SegmentDescriptor {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_array(&self.container_id.0);
        w.write_range(&self.idx_range)?;
        w.write_array(&self.prev_chain);
        w.write_array(&self.end_chain);
        w.write_range(&self.body_loc)?;
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        let container_id = ContainerId(r.read_array()?);
        let idx_range = r.read_range()?;
        let prev_chain = r.read_array()?;
        let end_chain = r.read_array()?;
        let body_loc = r.read_range()?;
        Ok(Self {
            container_id,
            prev_chain,
            end_chain,
            idx_range,
            body_loc,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    #[cfg(test)]
    use crate::codec::{Reader, Writer};
    use crate::pack::PackHeader;

    #[proptest]
    fn roundtrip_pack_header(header: PackHeader) {
        let mut w = Writer::new();
        header.encode(&mut w).unwrap();
        let encoded = w.finalize();
        let mut r = Reader::new(&encoded);
        let decoded = PackHeader::decode(&mut r).unwrap();
        assert_eq!(header, decoded);
    }
}
