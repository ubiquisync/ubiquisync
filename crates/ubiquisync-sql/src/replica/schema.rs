use pastey::paste;
use sea_query::Iden;

use crate::db::{DbTableDescriptor, DbType::*, col, table, table_with_auto_id};

fn table_schemas() -> Vec<DbTableDescriptor> {
    vec![
        table_with_auto_id(
            "peer",
            "id",
            &[
                col("peer_id", Blob),
                col("commitment", Blob),
                col("signature", Blob),
            ],
        )
        .with_unique(&["peer_id"]),
        table_with_auto_id("containers", "id", &[col("container_id", Uuid)])
            .with_unique(&["container_id"]),
        table_with_auto_id(
            "streams",
            "id",
            &[
                col("peer_id", Integer),
                col("container_id", Integer),
                col("parent_id", Integer).nullable(),
                col("fork_idx", Integer).nullable(),
                col("fork_hash", Blob).nullable(),
                col("ready_idx", Integer).default_zero(),
                col("ready_status", Integer).default_zero(),
                col("ready_status_data", Blob).default_zero(),
                col("commit_idx", Integer).default_zero(),
                col("commit_status", Integer).default_zero(),
                col("commit_status_data", Blob).default_zero(),
            ],
        ),
        table(
            "segments",
            &[col("log_id", Integer), col("end_idx", Integer)],
            &[
                col("start_idx", Integer),
                col("end_hash", Blob),
                col("body", Blob),
            ],
        ),
    ]
}

#[derive(Iden)]
pub enum Peer {
    Table,
    Id,
    PeerId,
    Commitment,
    Signature,
}

#[derive(Iden)]
pub enum Container {
    Table,
    Id,
    ContainerId,
}

#[derive(Iden)]
pub enum Streams {
    Table,
    Id,
    PeerId,
    ContainerId,
}

#[derive(Iden)]
pub enum Segments {
    Table,
    LogId,
    StartIdx,
    EndIdx,
    EndHash,
    Body,
}

// macro_rules! table {
//     (
//         $name:ident
//         ( $($pk_name:ident),+ )
//         => { $($col_name:ident),* }
//     ) => {
//         paste! {
//             #[derive(sea_query::Iden)]
//             pub enum [< $name:camel >] {
//                 Table,
//                 $( [< $pk_name:camel >] ),+
//                 $( [< $col_name:camel >] ),*
//             }
//         }
//     }
// }

// table!(
//     peer (id) => { peer_id, commitment, signature }
// );
