use num_enum::{IntoPrimitive, TryFromPrimitive};
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
    enum_col_repr, try_from_into_col_repr,
};

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
        peers::create_table_def(),
        streams::create_table_def(),
        segments::create_table_def(),
    ]
}

def_table_with_auto_id!(peers as __replica_peers (id) => {
    peer_id: [u8; 32], // TODO UNIQUE
    commitment_bytes: Vec<u8>,
    signature: super::Signature
});

def_table_with_auto_id!(streams __replica_streams (id) => {
   peer_id: i64, // TODO ref peers
   container_id: [u8;16],
   head_size: u64, // TODO default 0
   head_hash: [u8; 32], // TODO could be non-null and default to seed
   head_cipher: Option<super::CipherInfo>,
   head_status: Option<super::HeadStatus>,
   commit_size: u64, // TODO default 0
   commit_cipher: Option<super::CipherInfo>,
   commit_status: super::CommitStatus, // TODO default 0
   commit_status_data: Option<super::CommitStatusData>,
   parent_id: Option<i64>, // TODO ref streams
   fork_idx: Option<u64>,
   fork_hash: Option<[u8;32]>,
   // TODO CHECK(commit_size <= head_size)
});

#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum CommitStatus {
    Ok = 0,
    NeedKey = 1,
    HLCForwardSkew = 2,
    NeedPeerCommit = 3,
    IncompatibleSoftware = 4,
    CantDecodeOp = 5,
    Frozen = 6,
}

enum_col_repr!(CommitStatus);
try_from_into_col_repr!([u8; 32], Vec<u8>);
codeable_col_repr!(CipherInfo);
codeable_col_repr!(CommitStatusData);
codeable_col_repr!(HeadStatus);
codeable_col_repr!(Signature);

// TODO: CREATE UNIQUE INDEX streams_root ON streams(peer_id, container_id) WHERE parent_id IS NULL;
// TODO: we might also want a unique on (parent_id, fork_idx, fork_hash) to avoid races

def_table!(segments as __replica_segments (stream_id: i64, end_size: u64) => { // TODO ref streams
    start_idx: u64,
    body: Vec<u8>,
    // WITH ROWID!
});

pub enum HeadStatus {
    Ok,
}

pub enum CommitStatusData {
    Other,
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

pub enum UnknownSoftwareVersion {
    EntryType(u8),
    OpType(u8),
    CipherSuite(u8),
}

impl HeadStatus {
    pub fn encode(&self, writer: &mut Writer) {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}

impl CommitStatusData {
    pub fn encode(&self, writer: &mut Writer) {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}

impl UnknownSoftwareVersion {
    pub fn encode(&self, writer: &mut Writer) {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use crate::replica::schema::create_table_sql;

    #[test]
    fn schema_snapshot() {
        assert_snapshot!(
            "sqlite",
            create_table_sql(crate::dialect::SqlDialect::Sqlite).join(";\n")
        );
    }
}
