use sea_query::Query;
use ubiquisync_core::{
    crypto::Hash256,
    ids::{LogId, PeerId},
};

use crate::{
    db::DbError,
    replica::{replica::Replica, schema::Segments},
};

impl<Op> Replica<Op> {
    async fn insert_segment_local(&self) {
        Query::insert().into_table(Segments::Table).columns(
            Segments::LogId,
            Segments::StartIdx,
            Segments::EndIdx,
            Segments::EndHash,
            Segments::Body,
        )
    }
}
