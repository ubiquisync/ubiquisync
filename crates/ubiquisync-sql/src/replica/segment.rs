use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    bytes::ToStatic,
    log::{
        LogHashContext,
        segment::{DecodedSegment, SegmentDecodeError, SegmentReader},
    },
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{ReplicaInner, schema::segments, stream_lock::KeyedLockGuard, streams::StreamLog},
};

#[derive(Error, Debug)]
pub(crate) enum GetSegmentError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("segment decode error: {0}")]
    Decode(#[from] SegmentDecodeError),
}

impl<R> ReplicaInner<R> {
    pub(crate) async fn get_segment_for_size(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        hash_ctx: &LogHashContext,
        stream_id: i64,
        size: u64,
    ) -> Result<Option<DecodedSegment<'static>>, GetSegmentError> {
        if size == 0 {
            return Ok(None);
        }

        self.get_segment_by_index(guard, hash_ctx, stream_id, size - 1)
            .await
    }

    pub(crate) async fn get_segment_by_index(
        &self,
        _guard: &KeyedLockGuard<StreamLog>,
        hash_ctx: &LogHashContext,
        stream_id: i64,
        index: u64,
    ) -> Result<Option<DecodedSegment<'static>>, GetSegmentError> {
        if let Some((body,)) = select_cols::<(segments::Body,)>(
            self.db.as_ref(),
            Query::select()
                .from(segments::Table)
                .and_where(Expr::column(segments::StreamId).eq(stream_id))
                .and_where(Expr::column(segments::StartIdx).lte(index))
                .and_where(Expr::column(segments::EndSize).gt(index)),
        )
        .await?
        .one()?
        {
            let reader = SegmentReader::start(body)?;
            let decoded = reader.read(self.key_resolver.as_ref(), hash_ctx).await?;
            Ok(Some(decoded.to_static()))
        } else {
            Ok(None)
        }
    }
}
