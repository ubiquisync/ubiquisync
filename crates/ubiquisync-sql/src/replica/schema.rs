use thiserror::Error;
use ubiquisync_core::{
    codec::{ReadError, Reader, Writer},
    crypto::{CipherInfo, RootKey256Fingerprint, Signature},
    ids::{ContainerId, PeerId},
    log::ChainHash,
};

use crate::{
    codeable_col_repr,
    db::{ColRepr, CreateTableDef, Db, DbError},
    def_table, def_table_with_auto_id,
    dialect::SqlDialect,
    try_from_into_col_repr,
};

def_table_with_auto_id!(peers as __replica_peers (id) => {
    peer_id: [u8; 32],
    commitment_bytes: Vec<u8>,
    signature: super::Signature
});

def_table_with_auto_id!(streams as __replica_streams (id) => {
   peer_id: i64, // TODO ref peers
   container_id: [u8;16],
   head_size: u64, // TODO default 0
   head_hash: [u8; 32], // TODO could be non-null and default to seed
   head_cipher: Option<super::CipherInfo>,
   head_err: Option<super::HeadErr>,
   commit_size: u64, // TODO default 0
   commit_cipher: Option<super::CipherInfo>,
   commit_err: Option<super::CommitErr>,
   parent_id: Option<i64>, // TODO ref streams
   fork_idx: Option<u64>,
   fork_hash: Option<[u8;32]>,
   // TODO CHECK(commit_size <= head_size)
});

def_table!(segments as __replica_segments (stream_id: i64, end_size: u64) => { // TODO ref streams
    start_idx: u64,
    body: Vec<u8>,
    // WITH ROWID!
});

pub(crate) async fn create_tables(db: &dyn Db) -> Result<(), DbError> {
    let mut batch = db.new_batch();
    for st in create_table_sql(db.dialect()) {
        batch.add_statement(&st, &[]);
    }
    batch.commit().await?;
    Ok(())
}

fn create_table_sql(dialect: SqlDialect) -> Vec<String> {
    table_defs()
        .iter()
        .map(|d| d.create_table_sql(dialect))
        .collect::<Vec<_>>()
}

fn table_defs() -> Vec<CreateTableDef> {
    vec![
        peers::create_table_def().with_unique(&["peer_id"]),
        streams::create_table_def(),
        segments::create_table_def(),
    ]
}

// TODO: CREATE UNIQUE INDEX streams_root ON streams(peer_id, container_id) WHERE parent_id IS NULL;
// TODO: we might also want a unique on (parent_id, fork_idx, fork_hash) to avoid races

try_from_into_col_repr!([u8; 32], Vec<u8>);
codeable_col_repr!(CipherInfo);
codeable_col_repr!(CommitErr);
codeable_col_repr!(HeadErr);
codeable_col_repr!(Signature);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum HeadErr {
    Todo,
    //Sealed(u64)
    //Closed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CommitErr {
    NeedKey(RootKey256Fingerprint),
    HLCForwardSkew,
    NeedPeerCommit {
        peer_id: PeerId,
        // None if same container
        container_id: Option<ContainerId>,
        head: ChainHash,
    },
    IncompatibleSoftware(UnknownSoftwareVersion),
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
    pub fn encode(&self, _: &mut Writer) {}

    pub fn decode<'a>(_: &mut Reader<'a>) -> Result<Self, ReadError> {
        Ok(HeadErr::Todo)
    }
}

impl CommitErr {
    pub fn encode(&self, writer: &mut Writer) {
        match self {
            CommitErr::NeedKey(root_key256_fingerprint) => {
                writer.write_byte(0);
                writer.write_array(&root_key256_fingerprint.0);
            }
            CommitErr::HLCForwardSkew => writer.write_byte(1),
            CommitErr::NeedPeerCommit {
                peer_id,
                container_id,
                head,
            } => {
                if let Some(container_id) = container_id {
                    writer.write_byte(2);
                    writer.write_array(&container_id.0);
                } else {
                    writer.write_byte(3);
                }
                writer.write_array(&peer_id.0);
                head.encode(writer);
            }
            CommitErr::IncompatibleSoftware(unknown_software_version) => {
                writer.write_byte(4);
                unknown_software_version.encode(writer);
            }
        }
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, StatusDecodeError> {
        Ok(match reader.read_byte()? {
            0 => Self::NeedKey(RootKey256Fingerprint(reader.read_array()?)),
            1 => Self::HLCForwardSkew,
            b @ (2 | 3) => {
                let container_id = if b == 2 {
                    Some(ContainerId(reader.read_array()?))
                } else {
                    None
                };
                let peer_id = PeerId(reader.read_array()?);
                let head = ChainHash::decode(reader)?;
                Self::NeedPeerCommit {
                    peer_id,
                    container_id,
                    head,
                }
            }
            4 => Self::IncompatibleSoftware(UnknownSoftwareVersion::decode(reader)?),
            b => return Err(StatusDecodeError::UnknownTag(b)),
        })
    }
}

impl UnknownSoftwareVersion {
    pub fn encode(&self, writer: &mut Writer) {
        let (tag, version) = match self {
            UnknownSoftwareVersion::EntryType(b) => (0, b),
            UnknownSoftwareVersion::OpType(b) => (1, b),
            UnknownSoftwareVersion::CipherSuite(b) => (2, b),
        };
        writer.write_byte(tag);
        writer.write_byte(*version);
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
    use insta::assert_snapshot;
    use test_strategy::proptest;

    use crate::{
        db::ColRepr,
        replica::schema::{CommitErr, create_table_sql},
    };

    #[test]
    fn schema_snapshot() {
        assert_snapshot!(
            "sqlite",
            create_table_sql(crate::dialect::SqlDialect::Sqlite).join(";\n")
        );
    }

    #[proptest]
    fn roundtrip_commit_err(e: CommitErr) {
        let e2 = CommitErr::from_repr(&e.to_repr()).unwrap();
        assert_eq!(e, e2);
    }
}
