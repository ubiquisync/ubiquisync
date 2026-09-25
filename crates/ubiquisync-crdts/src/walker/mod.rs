use std::collections::HashMap;

use ubiquisync_core::{hlc::Timestamp, ids::PeerId, log::EntryOrigin};

use crate::id::{ElementId, EntryId, PeerAlias};

#[derive(Debug, Clone, Copy)]
pub enum View {
    Effect,
    Prepare,
}

pub trait Walkable<Id> {
    type LogicalOp: CountableOp;
    type PhysicalOp: CountableOp;
    type PrepareOp;
    type LogicalOpErr;
    type EffectErr;
    type PrepareErr;

    fn logical_to_physical(
        &self,
        view: View,
        op: Self::LogicalOp,
    ) -> Result<Self::PhysicalOp, Self::LogicalOpErr>;

    fn apply(
        &mut self,
        timestamp: Timestamp,
        id: ElementId<Id>,
        op: Self::PhysicalOp,
    ) -> Result<Self::PrepareOp, Self::EffectErr>;

    fn prepare_advance(&mut self, op: &Self::PrepareOp) -> Result<(), Self::PrepareErr>;
    fn prepare_retract(&mut self, op: &Self::PrepareOp) -> Result<(), Self::PrepareErr>;
}

pub trait CountableOp {
    fn num_elements(&self) -> u64;
}

pub struct WalkerOp<Op: CountableOp> {
    pub peer_aliases: Vec<PeerAlias>,
    pub observations: Vec<EntryId>,
    pub ops: Vec<Op>,
}

pub struct Walker<W: Walkable<EntryId>> {
    data: W,
    peers_by_id: HashMap<PeerId, PeerIdx>,
    peer_state: Vec<PeerState>,
    ops: Vec<OpNode<W::PrepareOp>>,
}

#[derive(Default)]
struct PeerState {
    // maps peer declared aliases to indexes
    peer_aliases: HashMap<u64, PeerIdx>,
    heads: Vec<OpIdx>,
}

#[derive(Clone, Copy)]
struct PeerIdx(usize);

#[derive(Clone, Copy)]
struct OpIdx(usize);

struct OpNode<PrepareOp> {
    parent: OpIdx,
    children: Vec<OpIdx>,
    op: PrepareOp,
}

impl<W: Walkable<EntryId>> Walker<W> {
    pub fn apply(&mut self, origin: &EntryOrigin, op: WalkerOp<W::LogicalOp>) {
        // collect peer aliases
        let new_aliases = op
            .peer_aliases
            .iter()
            .map(|PeerAlias { peer_id, alias }| (*alias, self.get_or_insert_peer(peer_id)))
            .collect::<Vec<_>>(); // we need to collect in order to avoid &mut self borrow conflict
        let peer_state = self.peer_state_mut(&origin.log.peer_id);
        for (alias, idx) in new_aliases.iter() {
            peer_state.peer_aliases.insert(*alias, *idx);
        }

        // collect observations
    }

    fn get_or_insert_peer(&mut self, peer: &PeerId) -> PeerIdx {
        if let Some(idx) = self.peers_by_id.get(peer) {
            *idx
        } else {
            let idx = PeerIdx(self.peer_state.len());
            self.peers_by_id.insert(*peer, idx);
            self.peer_state.push(Default::default());
            idx
        }
    }

    fn peer_state_mut(&mut self, peer: &PeerId) -> &mut PeerState {
        let idx = self.get_or_insert_peer(peer);
        &mut self.peer_state[idx.0]
    }
}
