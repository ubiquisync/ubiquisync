use std::collections::BTreeMap;

use thiserror::Error;

use crate::crypto::{TaggedHashDomain, tagged_hash};
use crate::hlc::WallTime;
use crate::log::LogHashContext;
use crate::pack::{PackFileDescriptor, PackSignError, SignedPackHeader};
use crate::{
    codec::Writer,
    crypto::{CipherKeyResolver, SigningKey},
    ids::PeerId,
    log::segment::{JoinSegmentsError, join_segments},
    pack::{PackHeader, PackRef, PeerData, SegmentDescriptor},
};

pub struct PackBuilder {
    parents: Vec<PackRef>,
    self_supersedes: Vec<PackRef>,
    self_segments: Vec<SegmentDescriptor>,
    peer_data: BTreeMap<PeerId, PeerData>,
    body_writer: Writer,
    descriptor: PackFileDescriptor,
}

#[derive(Debug, Error)]
pub enum PackBuildError {
    #[error("join segments error: {0}")]
    Join(#[from] JoinSegmentsError),
    #[error("empty segment range")]
    EmptySegmentRange,
}

impl PackBuilder {
    pub fn new(descriptor: PackFileDescriptor, parents: Vec<PackRef>) -> Self {
        Self {
            descriptor,
            parents,
            self_supersedes: vec![],
            self_segments: vec![],
            peer_data: BTreeMap::new(),
            body_writer: Writer::new(),
        }
    }

    pub async fn add_container_segments<'a, B: AsRef<[u8]> + 'a>(
        &mut self,
        key_resolver: &dyn CipherKeyResolver,
        hash_ctx: &LogHashContext,
        start_size: u64,
        segments: &'a [B],
    ) -> Result<u64, PackBuildError> {
        // TODO we can skip joining segments when there's only one segment
        let body_start = self.body_writer.len();
        // note that a failure here may leave some body bytes written, but this is mostly harmless and in most cases any error will cause the caller to abandon building the pack anyway
        let joined = join_segments(
            key_resolver,
            hash_ctx,
            start_size,
            segments,
            &mut self.body_writer,
        )
        .await?;
        let body_end = self.body_writer.len();
        let idx_range = joined.prev_chain.size..joined.chain_hash.size;
        if idx_range.is_empty() {
            return Err(PackBuildError::EmptySegmentRange);
        }
        let desc = SegmentDescriptor {
            container_id: hash_ctx.log_id().container_id,
            idx_range,
            prev_chain_hash: joined.prev_chain.hash,
            end_chain_hash: joined.chain_hash.hash,
            body_loc: body_start..body_end,
        };
        let peer_id = hash_ctx.log_id().peer_id;
        if peer_id == self.descriptor.peer_id {
            self.self_segments.push(desc);
        } else {
            self.peer_data
                .entry(peer_id)
                .or_insert_with(|| PeerData {
                    peer_id,
                    supersedes: vec![],
                    segments: vec![],
                })
                .segments
                .push(desc);
        }
        Ok(joined.chain_hash.size)
    }

    pub fn build(self, signing_key: &dyn SigningKey) -> Result<PackData, PackSignError> {
        let body = self.body_writer.finalize();
        let body_sha256 = tagged_hash(TaggedHashDomain::PackBody, &body);
        let header = PackHeader {
            timestamp: WallTime::now(),
            parents: self.parents,
            self_supersedes: self.self_supersedes,
            self_segments: self.self_segments,
            peer_data: self.peer_data.into_values().collect(),
            body_sha256,
        };
        let header = header.sign(signing_key, &self.descriptor)?;
        Ok(PackData {
            descriptor: self.descriptor,
            header,
            body,
        })
    }
}

pub struct PackData {
    pub descriptor: PackFileDescriptor,
    pub header: SignedPackHeader,
    pub body: Vec<u8>,
}
