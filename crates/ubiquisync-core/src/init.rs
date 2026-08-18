use bitfield_struct::bitfield;
use thiserror::Error;

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::{
        EncapsulationKey, Hash256, Hash256Suite, Signature, SignatureVerificationError,
        SigningError, SigningKey, TaggedHashDomain, VerifyingKey,
    },
    ids::PeerId,
    log_entry::DecodeError,
};

pub struct InitEntry {
    pub commitment_bytes: Vec<u8>,
    pub peer_id: PeerId,
    pub signature: Signature,
    /// Opaque server endorsement to be used when server mode is supported.
    pub server_endorsement: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct InitCommitment {
    pub version: Version,
    pub hash_suite: Hash256Suite,
    pub sig_verify_key: VerifyingKey,
    pub encrypt_wrap_key: EncapsulationKey,
    pub server: bool,
    /// For transplant resistance, unset indicates this peer is not a member of any
    /// existing workspace when it is initialized. Such a peer is considered
    /// a workspace "root" peer.
    /// When set, this field indicates that the peer is joining the workspace of some root peer.
    /// Without this logs from any peer could just be haphazardly merged with those of other peers.
    /// Note that other layers will allow for joining two peers who were not joined at initialization
    /// but that is outside the scope of [InitCommitment].
    pub workspace_join: Option<PeerId>,
}

#[bitfield(u8)]
#[derive(PartialEq, Eq)]
pub struct Version {
    #[bits(4)]
    pub major: u8,
    #[bits(4)]
    pub minor: u8,
}

#[bitfield(u8)]
#[derive(PartialEq, Eq)]
struct Flags {
    #[bits(1)]
    server: bool,
    #[bits(1)]
    workspace_join: bool,
    #[bits(6)]
    reserved: u8,
}

static DOMAIN_INIT_COMMITMENT: TaggedHashDomain =
    TaggedHashDomain::new("ubiquisync/v1/init-commitment");

impl InitEntry {
    pub fn create(
        commitment: InitCommitment,
        app_magic: &Hash256,
        signing_key: &dyn SigningKey,
    ) -> Result<InitEntry, InitCreationError> {
        if commitment.version.major() != 0 || commitment.version.minor() != 0 {
            return Err(InitCreationError::UnsupportedVersion(commitment.version));
        }
        let mut w = Writer::new();
        commitment.encode(&mut w);
        let commitment_bytes = w.finalize();
        let mut hasher = commitment.hash_suite.tagged_hasher(DOMAIN_INIT_COMMITMENT);
        hasher.update(&app_magic[..]);
        hasher.update(&commitment_bytes);
        let peer_hash = hasher.finalize();
        let peer_id = PeerId(peer_hash);
        let signature = signing_key.sign(&peer_hash)?;
        let entry = InitEntry {
            commitment_bytes,
            peer_id,
            signature,
            server_endorsement: None,
        };
        entry.verify(app_magic)?;
        Ok(entry)
    }

    pub fn commitment_data(&self) -> Result<InitCommitment, DecodeError> {
        // TODO check size
        InitCommitment::decode(&self.commitment_bytes)
    }

    pub fn verify(&self, app_magic: &Hash256) -> Result<InitCommitment, InitVerifyError> {
        let commitment = self.commitment_data()?;
        let mut hasher = commitment.hash_suite.tagged_hasher(DOMAIN_INIT_COMMITMENT);
        hasher.update(&app_magic[..]);
        hasher.update(&self.commitment_bytes);
        let peer_hash = hasher.finalize();
        if peer_hash != self.peer_id.0 {
            return Err(InitVerifyError::HashMismatch);
        }
        commitment
            .sig_verify_key
            .verify_signature(&peer_hash, &self.signature)?;
        Ok(commitment)
    }
}

/// Limits [InitCommitment] entries to 32kb.
/// Way more than we need now but could accomodate quantum-resistant cryptography in the future.
pub const MAX_INIT_COMMITMENT_LEN: usize = 32768;
/// Limits [InitEntry] entries to 64kb.
/// Way more than we need now but could accomodate quantum-resistant cryptography in the future.
pub const MAX_INIT_ENTRY_LEN: usize = 65536;

#[derive(Error, Debug)]
pub enum InitCreationError {
    #[error("signing error: {0}")]
    SigningError(#[from] SigningError),
    #[error("unsupported version: {0:?}")]
    UnsupportedVersion(Version),
    #[error("unsupported version: {0:?}")]
    InitVerifyError(#[from] InitVerifyError),
}

#[derive(Error, Debug)]
pub enum InitVerifyError {
    #[error("decode error: {0}")]
    DecodeError(#[from] DecodeError),
    #[error("hash mismatch")]
    HashMismatch,
    #[error("signature verification error: {0}")]
    SignatureError(#[from] SignatureVerificationError),
}

impl InitCommitment {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte(self.version.into());
        self.hash_suite.encode(writer);
        let flags = Flags::new()
            .with_server(self.server)
            .with_workspace_join(self.workspace_join.is_some());
        writer.write_byte(flags.into_bits());

        self.sig_verify_key.encode(writer);
        self.encrypt_wrap_key.encode(writer);
        if let Some(workspace_id) = self.workspace_join {
            writer.write_array(&workspace_id.0);
        }
    }

    pub fn decode(commitment_bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(commitment_bytes);
        let version: Version = reader.read_byte()?.into();
        if version.major() > 0 {
            return Err(DecodeError::UnsupportedVersion(version.major()));
        }
        let hash_suite = Hash256Suite::decode(&mut reader)?;
        let flags = Flags::from_bits(reader.read_byte()?);
        if flags.reserved() != 0 {
            return Err(DecodeError::UnknownInitFlags(flags.0));
        }
        let sig_verify_key = VerifyingKey::decode(&mut reader)?;
        let encrypt_wrap_key = EncapsulationKey::decode(&mut reader)?;
        let workspace_join = if flags.workspace_join() {
            Some(PeerId(reader.read_array()?))
        } else {
            None
        };
        let remaining = reader.remaining().len();
        if remaining != 0 && version.minor() == 0 {
            // error if we are at the current supported minor version (0) and we have remaining data
            // if we are at another minor version, but the same major (0), we tolerate extra remaining data
            // without parsing it - basically minor communicates that this data is non-critical
            return Err(DecodeError::UnknownInitData { version, remaining });
        }

        Ok(Self {
            version,
            hash_suite,
            sig_verify_key,
            encrypt_wrap_key,
            server: flags.server(),
            workspace_join,
        })
    }
}
