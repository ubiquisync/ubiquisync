use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use ubiquisync_core::{
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::RootKey256Fingerprint,
    ids::PeerId,
    pack::{PackFileId, PackRef},
};

use bitfield_struct::bitfield;

use super::schema::StatusDecodeError;
use crate::{codeable_col_repr, def_table, def_table_with_auto_id};

def_table_with_auto_id!(remotes as __replica_remotes (id) => {});
def_table_with_auto_id!(topics as __pack_topics (id) => {
    topic: String,
});

def_table!(topic_state as __pack_topic_state (remote_id: i64, topic_id: i64) => {
    // the dirty flag indicates that we have a membership change on this topic
    // which requires resetting stream published sizes to 0 for containers which moved out of the topic
    // and then 1) rewriting those in the new topic and 2) compacting old topic packs to remove the old container data
    dirty: bool,
    write_state: super::PackWriteState,
});

def_table!(published as __pack_published (
    remote_id: i64, // TODO ref remotes
    stream_id: i64, // TODO ref streams
) => {
    published_size: u64, // TODO default 0
    // tracks when peer segments aren't published in a pack remote
    pending_since: Option<u64>
});

def_table!(pack_read_state as __pack_read_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
    peer_id: i64, // TODO ref peers
) => {
    state: super::PackReadState,
});

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackReadState {
    pub consumed: HashSet<PackFileId>,
    pub blocked: HashMap<PackRef, BlockedPackInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackWriteState {
    pub tips: HashSet<PackRef>,
}

codeable_col_repr!(PackReadState);
codeable_col_repr!(PackWriteState);

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct BlockedPackInfo {
    pub file: PackFileId,
    pub read_timestamps: Range<u64>,
    pub attempts: u64,

    // Set to the list of missing parents when any segment does not have an anchor (meaning we have a missing dependency).
    pub pending_parents: HashSet<PackRef>,
    /// Set when we are missing keys for decoding some segment with outer encryption.
    pub need_keys: HashSet<RootKey256Fingerprint>,
    #[cfg_attr(
        test,
        strategy(proptest::strategy::Strategy::prop_map(
            proptest::arbitrary::any::<u8>(),
            |b| BlockedReasons::from_bits(b).with_reserved(0)
        ))
    )]
    pub reasons: BlockedReasons,
    // List of peers referenced in peer data for whom we cannot find an init entry.
    pub missing_peers: Vec<PeerId>,
}

#[bitfield(u8)]
#[derive(PartialEq, Eq)]
pub struct BlockedReasons {
    /// Any case where we hit some decode error which suggests MAYBE a software upgrade is needed (could also mean corrupt data).
    pub needs_upgrade: bool,
    /// The metadata for this segment did not match the actual segment body.
    pub bad_metadata: bool,
    /// Some decode error which most likely means that retrying will continue to fail.
    /// If this error is the only error blocked condition after retrying, it means
    /// we should probably permanently mark this pack as failed.
    pub decode_error: bool,
    /// The body hash did not match the expect hash. This could mean the file
    /// wasn't fully transferred yet, so we should retry. If there are repeated
    /// errors we can mark this pack as failed, but for now we'll retry later.
    pub bad_body_hash: bool,
    /// The header signature failed verification. Possibly a partially synced file,
    /// so we retry with backoff.
    pub bad_signature: bool,
    /// The header could not be decoded. Possibly a partially synced file,
    /// so we retry with backoff.
    pub corrupt_header: bool,
    #[bits(2)]
    reserved: u8,
}

impl PackReadState {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        writer.write_iter(self.consumed.iter(), |w, f| f.encode(w))?;
        // the map key can be inferred from the values, so only write the values
        writer.write_iter(self.blocked.values(), |w, b| b.encode(w))
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, StatusDecodeError> {
        let consumed = reader.read_collect(|r| PackFileId::decode(r))?;
        let blocked = reader.read_collect(|r| {
            let b = BlockedPackInfo::decode(r)?;
            Ok::<_, StatusDecodeError>((b.file.get_ref(), b))
        })?;
        Ok(Self { consumed, blocked })
    }
}
impl PackWriteState {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        writer.write_iter(self.tips.iter(), |w, f| f.encode(w))?;
        Ok(())
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, StatusDecodeError> {
        let tips = reader.read_collect(|r| PackRef::decode(r))?;
        Ok(Self { tips })
    }
}

impl BlockedPackInfo {
    fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        self.file.encode(writer)?;
        writer.write_range(&self.read_timestamps)?;
        writer.write_var_u64(self.attempts);
        writer.write_iter(self.pending_parents.iter(), |w, p| p.encode(w))?;
        writer.write_iter(self.need_keys.iter(), |w, k| {
            w.write_array(&k.0);
            Ok(())
        })?;
        writer.write_byte(self.reasons.into_bits());
        writer.write_vec(&self.missing_peers, |w, p| {
            w.write_array(&p.0);
            Ok(())
        })
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, StatusDecodeError> {
        let file = PackFileId::decode(reader)?;
        let read_timestamps = reader.read_range()?;
        let attempts = reader.read_var_u64()?;
        let pending_parents = reader.read_collect(|r| PackRef::decode(r))?;
        let need_keys =
            reader.read_collect(|r| Ok::<_, ReadError>(RootKey256Fingerprint(r.read_array()?)))?;
        let flags = reader.read_byte()?;
        let reasons = BlockedReasons::from_bits(flags);
        if reasons.reserved() != 0 {
            return Err(StatusDecodeError::UnknownTag(flags));
        }
        let missing_peers = reader.read_vec(|r| Ok::<_, ReadError>(PeerId(r.read_array()?)))?;
        Ok(Self {
            file,
            read_timestamps,
            attempts,
            pending_parents,
            need_keys,
            reasons,
            missing_peers,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;
    use test_strategy::proptest;
    use ubiquisync_core::pack::PackFileId;

    use super::{BlockedPackInfo, BlockedReasons, PackReadState};
    use crate::db::ColRepr;

    #[proptest]
    fn roundtrip_read_state(consumed: Vec<PackFileId>, blocked: Vec<BlockedPackInfo>) {
        let state = PackReadState {
            consumed: consumed.into_iter().collect(),
            blocked: blocked.into_iter().map(|b| (b.file.get_ref(), b)).collect(),
        };
        let state2 = PackReadState::from_repr(&state.clone().to_repr().unwrap()).unwrap();
        assert_eq!(state, state2);
    }

    /// One blocked entry with no flags set and no missing peers.
    fn single_blocked_bytes() -> Vec<u8> {
        let file = PackFileId {
            seqs: 0..1,
            id: 1,
            generation: 0,
        };
        let state = PackReadState {
            consumed: Default::default(),
            blocked: [(
                file.get_ref(),
                BlockedPackInfo {
                    file,
                    read_timestamps: 0..0,
                    attempts: 0,
                    pending_parents: Default::default(),
                    need_keys: Default::default(),
                    reasons: BlockedReasons::new(),
                    missing_peers: vec![],
                },
            )]
            .into(),
        };
        state.to_repr().unwrap()
    }

    // the flags byte is second to last, before the empty missing_peers vec
    #[test_case(|_| {} => matches Ok(_) ; "valid")]
    #[test_case(|b| { let i = b.len() - 2; b[i] = 0x80 } => matches Err(_) ; "unknown flag")]
    #[test_case(|b| b.truncate(b.len() - 1) => matches Err(_) ; "truncated")]
    fn decode_read_state(patch: fn(&mut Vec<u8>)) -> Result<PackReadState, crate::db::DbError> {
        let mut b = single_blocked_bytes();
        patch(&mut b);
        PackReadState::from_repr(&b)
    }
}
