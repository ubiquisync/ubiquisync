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
        table(
            "streams",
            &[col("id", Blob)],
            &[
                col("peer_id", Integer),
                col("container_id", Uuid),
                col("receive_idx", Integer).default_zero(),
                col("receive_mmr_peaks", Blob).nullable(),
                col("commit_idx", Integer).default_zero(),
                // fork handling
                col("parent_id", Integer).nullable(),
                col("fork_idx", Integer).nullable(),
                col("fork_root", Blob).nullable(),
            ],
        ),
        table(
            "segments",
            &[col("stream_id", Blob), col("start_idx", Integer)],
            &[
                col("data", Blob),
                col("end_idx", Integer),
                col("root_hash", Blob),
            ],
        ),
    ]
}
