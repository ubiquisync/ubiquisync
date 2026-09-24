use std::ops::Range;

use thiserror::Error;

use crate::crypto::{SignatureVerifyError, SigningError};
use crate::pack::PackFileId;
use crate::{
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{CryptoDecodeError, Hash256, Signature, VerifyingKey, new_tagged_hasher},
    ids::{ContainerId, PeerId},
    pack::PackFileDescriptor,
};

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
    pub body_sha256: Hash256,
}

pub struct SignedPackHeader {
    pub header: PackHeader,
    pub signature: Signature,
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
    pub supersedes: Vec<PackFileId>,
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

#[derive(Error, Debug)]
pub enum PackHeaderDecodeError {
    #[error("read error: {0}")]
    Read(#[from] ReadError),
    #[error("unknown version {0}")]
    UnknownVersion(u8),
    #[error("unknown signature algorithm {0}")]
    UnknownSignatureType(u8),
    #[error("trailing bytes")]
    TrailingBytes,
}

const PACK_HEADER_VERSION: u8 = 0;

impl PackHeader {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_byte(PACK_HEADER_VERSION); // version byte
        w.write_vec(&self.parents, |w, x| x.encode(w))?;
        w.write_vec(&self.self_supersedes, |w, x| x.encode(w))?;
        w.write_vec(&self.self_segments, |w, x| x.encode(w))?;
        w.write_vec(&self.peer_data, |w, x| x.encode(w))?;
        w.write_array(&self.body_sha256);
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, PackHeaderDecodeError> {
        let version = r.read_byte()?;
        if version != PACK_HEADER_VERSION {
            return Err(PackHeaderDecodeError::UnknownVersion(version));
        }
        let parents = r.read_vec(|r| PackRef::decode(r))?;
        let self_supersedes = r.read_vec(|r| PackRef::decode(r))?;
        let self_segments = r.read_vec(|r| SegmentDescriptor::decode(r))?;
        let peer_data = r.read_vec(|r| PeerData::decode(r))?;
        let body_sha256 = r.read_array()?;
        Ok(Self {
            parents,
            self_supersedes,
            self_segments,
            peer_data,
            body_sha256,
        })
    }

    pub fn sign_bytes(&self, file_desc: &PackFileDescriptor) -> Result<Hash256, WriteError> {
        let mut hasher = new_tagged_hasher(crate::crypto::TaggedHashDomain::PackHeader);
        file_desc.hash(&mut hasher)?;
        let mut w = Writer::new();
        self.encode(&mut w)?;
        let header_bytes = w.finalize();
        hasher.update_len_prefixed(&header_bytes);
        Ok(hasher.finalize())
    }

    pub fn sign(
        &self,
        signing_key: &dyn crate::crypto::SigningKey,
        file_desc: &PackFileDescriptor,
    ) -> Result<SignedPackHeader, PackSignError> {
        let sig_bytes = self.sign_bytes(file_desc)?;
        let signature = signing_key.sign(&sig_bytes)?;
        Ok(SignedPackHeader {
            header: self.clone(),
            signature,
        })
    }
}

impl SignedPackHeader {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        self.header.encode(w)?;
        self.signature.encode(w);
        Ok(())
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PackHeaderDecodeError> {
        let mut r = Reader::new(buf);
        let header = PackHeader::decode(&mut r)?;
        let signature = Signature::decode(&mut r).map_err(|e| match e {
            CryptoDecodeError::ReadError(e) => PackHeaderDecodeError::Read(e),
            CryptoDecodeError::UnknownAlgorithm(b) => {
                PackHeaderDecodeError::UnknownSignatureType(b)
            }
        })?;
        if !r.is_empty() {
            return Err(PackHeaderDecodeError::TrailingBytes);
        }
        Ok(Self { header, signature })
    }

    pub fn verify(
        &self,
        key: &VerifyingKey,
        file_desc: &PackFileDescriptor,
    ) -> Result<(), PackVerifyError> {
        let sig_bytes = self.header.sign_bytes(file_desc)?;
        key.verify_signature(&sig_bytes, &self.signature)?;
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum PackVerifyError {
    #[error("error encoding header: {0}")]
    Encode(#[from] WriteError),
    #[error("signature verification failed: {0}")]
    Signature(#[from] SignatureVerifyError),
}

#[derive(Error, Debug)]
pub enum PackSignError {
    #[error("error encoding header: {0}")]
    Encode(#[from] WriteError),
    #[error("error signing header: {0}")]
    Sign(#[from] SigningError),
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
        let supersedes = r.read_vec(|r| PackFileId::decode(r))?;

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
    use test_case::test_case;
    use test_strategy::proptest;

    #[cfg(test)]
    use crate::codec::{Reader, Writer};
    use crate::crypto::SIG_ALGO_ED25519;
    use crate::pack::{PackHeader, PackHeaderDecodeError};

    #[proptest]
    fn roundtrip_pack_header(header: PackHeader) {
        let mut w = Writer::new();
        header.encode(&mut w).unwrap();
        let encoded = w.finalize();
        let mut r = Reader::new(&encoded);
        let decoded = PackHeader::decode(&mut r).unwrap();
        assert_eq!(header, decoded);
    }

    fn empty_header_bytes() -> Vec<u8> {
        let mut b = vec![0u8; 5 + 32]; // version, 4 empty vecs, body hash
        b.push(SIG_ALGO_ED25519);
        b.extend([0u8; 64]);
        b
    }

    #[test_case(|_| {} => matches Ok(_) ; "empty header")]
    #[test_case(|b| b[0] = 1 => matches Err(PackHeaderDecodeError::UnknownVersion(1)) ; "unknown version")]
    #[test_case(|b| b.push(0) => matches Err(PackHeaderDecodeError::TrailingBytes) ; "trailing byte")]
    #[test_case(|b| b.truncate(3) => matches Err(PackHeaderDecodeError::Read(_)) ; "truncated")]
    #[test_case(|b| b[37] = 0xff => matches Err(PackHeaderDecodeError::UnknownSignatureType(0xff)) ; "unknown sig algo")]
    fn decode_pack_header(patch: fn(&mut Vec<u8>)) -> Result<PackHeader, PackHeaderDecodeError> {
        let mut b = empty_header_bytes();
        patch(&mut b);
        let mut r = Reader::new(&b);
        PackHeader::decode(&mut r)
    }
}
