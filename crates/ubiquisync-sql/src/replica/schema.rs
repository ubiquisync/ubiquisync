use std::ops::Range;

use thiserror::Error;
use ubiquisync_core::{
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{CipherInfo, RootKey256Fingerprint, Signature},
    hlc::Timestamp,
};

use crate::{
    codeable_col_repr, db::CreateTableDef, def_table, def_table_with_auto_id,
    try_from_into_col_repr,
};

def_table_with_auto_id!(peers as __replica_peers (id) => {
    peer_id: [u8; 32],
    commitment_bytes: Vec<u8>,
    signature: super::Signature
});

// TODO when we intialize a container we should resolve its topic
def_table_with_auto_id!(containers as __replica_containers (id) => {
    container_id: [u8; 16],
    // NULL means the default topic, a container with no row is also in the default topic
    topic_id: Option<i64>,
});

// TODO should we rename streams to something like branches or logs?
def_table_with_auto_id!(streams as __replica_streams (id) => {
   peer_id: i64, // TODO ref peers
   container_id: [u8; 16], // TODO should we ref containers.id as i64 or not here?
   head_size: u64, // TODO default 0
   head_hash: [u8; 32], // TODO could be non-null and default to seed
   head_cipher: Option<super::CipherInfo>,
   head_err: Option<super::HeadErr>,
   commit_size: u64, // TODO default 0
   commit_err: Option<super::CommitErr>,
   parent_id: Option<i64>, // TODO ref streams
   fork_size: Option<u64>,
   // TODO CHECK(commit_size <= head_size)
});

def_table!(segments as __replica_segments (stream_id: i64, end_size: u64) => { // TODO ref streams
    start_idx: u64,
    body: Vec<u8>,
    // TODO we actually need a rowid and want to have an auto ID anyway for p2p sync tracking
});

def_table!(stream_deps as __replica_stream_deps (
    stream_id: i64,
    peer: i64,
    container: i64,
    index: u64,
    hash_prefix: Vec<u8>,
) => {});

def_table!(hlc as __replica_hlc (id:i64) => {timestamp: super::Timestamp});
try_from_into_col_repr!(Timestamp, i64);

pub(crate) fn table_defs() -> Vec<CreateTableDef> {
    vec![
        peers::create_table_def().with_unique(&["peer_id"]),
        containers::create_table_def().with_unique(&["container_id"]),
        streams::create_table_def(),
        segments::create_table_def(),
        hlc::create_table_def(),
    ]
}

// TODO: CREATE UNIQUE INDEX streams_root ON streams(peer_id, container_id) WHERE parent_id IS NULL;
// TODO: we might also want a unique on (parent_id, fork_idx, fork_hash) to avoid races

try_from_into_col_repr!([u8; 32], Vec<u8>);
codeable_col_repr!(CipherInfo);
codeable_col_repr!(CommitErr);
codeable_col_repr!(HeadErr);
codeable_col_repr!(Signature);

// TODO: CREATE UNIQUE INDEX streams_root ON streams(peer_id, container_id) WHERE parent_id IS NULL;
// TODO: we might also want a unique on (parent_id, fork_idx, fork_hash) to avoid races

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum HeadErr {
    /// Entry admission is blocked at the specified size.
    /// If the specified size is less than the current head size,
    /// more entries may be admitted up to that point.
    Blocked(u64),
    /// Key rotation is needed and the next entry MUST be
    /// a UseKey op with the specified key.
    /// In the case of local operations, the system will insert this.
    /// For remote peers, the state machine will quarantine peers who
    /// don't rotate keys when expected.
    NeedUseKey(RootKey256Fingerprint),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CommitErr {
    /// Internal errors may or may not be transient - there may be a temporary database
    /// availability issue or it could be a software bug.
    /// Since we don't have a good way of knowing this, we use a retry backoff algorithm.
    /// Failure reasons aren't included here because they should be included in logs.
    Internal {
        retry_time_span: Range<u64>,
        retry_count: u64,
    },
    NeedKey(RootKey256Fingerprint),
    HLCForwardSkew(Timestamp),
    /// Awaiting dependencies from other logs.
    /// These will be specified in the stream_deps table.
    AwaitingDeps,
    IncompatibleSoftware(UnknownSoftwareVersion),
    Frozen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum UnknownSoftwareVersion {
    EntryType(u8),
    OpType(u8),
    CipherSuite(u8),
}

#[derive(Error, Debug)]
pub enum StatusDecodeError {
    #[error("read error: {0}")]
    Read(#[from] ReadError),
    #[error("unknown tag: {0}")]
    UnknownTag(u8),
}

impl HeadErr {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        match self {
            HeadErr::Blocked(size) => {
                writer.write_byte(0);
                writer.write_var_u64(*size);
            }
            HeadErr::NeedUseKey(root_key256_fingerprint) => {
                writer.write_byte(1);
                writer.write_array(&root_key256_fingerprint.0);
            }
        }
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, StatusDecodeError> {
        Ok(match reader.read_byte()? {
            0 => Self::Blocked(reader.read_var_u64()?),
            1 => Self::NeedUseKey(RootKey256Fingerprint(reader.read_array()?)),
            b => return Err(StatusDecodeError::UnknownTag(b)),
        })
    }
}

impl CommitErr {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        match self {
            CommitErr::NeedKey(root_key256_fingerprint) => {
                writer.write_byte(0);
                writer.write_array(&root_key256_fingerprint.0);
            }
            CommitErr::HLCForwardSkew(timestamp) => {
                writer.write_byte(1);
                writer.write_timestamp(*timestamp);
            }
            CommitErr::AwaitingDeps => writer.write_byte(2),
            CommitErr::IncompatibleSoftware(unknown_software_version) => {
                writer.write_byte(3);
                unknown_software_version.encode(writer);
            }
            CommitErr::Internal {
                retry_time_span,
                retry_count,
            } => {
                writer.write_byte(4);
                writer.write_range(retry_time_span)?;
                writer.write_var_u64(*retry_count);
            }
            CommitErr::Frozen => writer.write_byte(5),
        }
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, StatusDecodeError> {
        Ok(match reader.read_byte()? {
            0 => Self::NeedKey(RootKey256Fingerprint(reader.read_array()?)),
            1 => Self::HLCForwardSkew(reader.read_timestamp()?),
            2 => Self::AwaitingDeps,
            3 => Self::IncompatibleSoftware(UnknownSoftwareVersion::decode(reader)?),
            4 => {
                let retry_time_span = reader.read_range()?;
                let retry_count = reader.read_var_u64()?;
                Self::Internal {
                    retry_time_span,
                    retry_count,
                }
            }
            5 => Self::Frozen,
            b => return Err(StatusDecodeError::UnknownTag(b)),
        })
    }
}

impl UnknownSoftwareVersion {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        let (tag, version) = match self {
            UnknownSoftwareVersion::EntryType(b) => (0, b),
            UnknownSoftwareVersion::OpType(b) => (1, b),
            UnknownSoftwareVersion::CipherSuite(b) => (2, b),
        };
        writer.write_byte(tag);
        writer.write_byte(*version);
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, StatusDecodeError> {
        let tag = reader.read_byte()?;
        let version = reader.read_byte()?;
        Ok(match tag {
            0 => Self::EntryType(version),
            1 => Self::OpType(version),
            2 => Self::CipherSuite(version),
            _ => return Err(StatusDecodeError::UnknownTag(tag)),
        })
    }
}

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    use crate::{
        db::ColRepr,
        replica::schema::{CommitErr, HeadErr},
    };

    #[proptest]
    fn roundtrip_commit_err(e: CommitErr) {
        let enc = e.clone().to_repr().unwrap();
        let e2 = CommitErr::from_repr(&enc).unwrap();
        assert_eq!(e, e2);
    }

    #[proptest]
    fn roundtrip_head_err(e: HeadErr) {
        let e2 = HeadErr::from_repr(&e.to_repr().unwrap()).unwrap();
        assert_eq!(e, e2);
    }
}
