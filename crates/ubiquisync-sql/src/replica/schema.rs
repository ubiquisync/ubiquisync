use sea_query::Iden;

use crate::db::{CreateTableDef, DbType::*, col, table, table_with_auto_id};

fn table_schemas() -> Vec<CreateTableDef> {
    vec![
        table_with_auto_id(
            &Peers::Table,
            &Peers::Id,
            &[
                col(&Peers::PeerId, Blob),
                col(&Peers::Commitment, Blob),
                col(&Peers::Signature, Blob),
            ],
        )
        .with_unique(&["peer_id"]),
        table_with_auto_id(
            &Containers::Table,
            &Containers::Id,
            &[col(&Containers::ContainerId, Uuid)],
        )
        .with_unique(&["container_id"]),
        table_with_auto_id(
            &Streams::Table,
            &Streams::Id,
            &[
                col(&Streams::PeerId, Integer),
                col(&Streams::ContainerId, Integer),
                col(&Streams::HeadIdx, Integer).default_zero(),
                col(&Streams::HeadHash, Blob).default_zero(),
                col(&Streams::HeadCipher, Blob).nullable(),
                col(&Streams::HeadStatus, Blob).nullable(),
                col(&Streams::ReadyIdx, Integer).default_zero(),
                col(&Streams::ReadyStatus, Integer).default_zero(),
                col(&Streams::ReadyStatusData, Blob).default_zero(),
                col(&Streams::CommitIdx, Integer).default_zero(),
                col(&Streams::CommitStatus, Integer).default_zero(),
                col(&Streams::CommitStatusData, Blob).default_zero(),
                col(&Streams::ParentId, Integer).nullable(),
                col(&Streams::ForkIdx, Integer).nullable(),
                col(&Streams::ForkHash, Blob).nullable(),
            ],
        ),
        table(
            &Segments::Table,
            &[
                col(&Segments::LogId, Integer),
                col(&Segments::EndIdx, Integer),
            ],
            &[
                col(&Segments::StartIdx, Integer),
                col(&Segments::EndHash, Blob),
                col(&Segments::Body, Blob),
            ],
        ),
    ]
}

#[derive(Iden)]
pub enum Peers {
    Table,
    Id,
    PeerId,
    Commitment,
    Signature,
}

#[derive(Iden)]
pub enum Containers {
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
    HeadIdx,
    HeadHash,
    HeadCipher,
    HeadStatus,
    ReadyIdx,
    ReadyStatus,
    ReadyStatusData,
    CommitIdx,
    CommitStatus,
    CommitStatusData,
    ParentId,
    ForkIdx,
    ForkHash, // is this even needed if we know the parent id definitively?
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
