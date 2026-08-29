/// A failure while ingesting an entry, tagged by the stage that produced it.
#[derive(Debug, thiserror::Error)]
pub enum ExecError<E> {
    /// The reducer failed; `E` is its own error type.
    // No `#[from]`: a blanket `From<E>` would clash with `From<DbError>` when a
    // reducer sets `Error = DbError`. Mapped explicitly at the call sites.
    #[error("reducer error: {0}")]
    Reducer(E),
    /// Advancing or persisting the HLC failed.
    #[error("hlc error: {0}")]
    Hlc(#[from] HlcError<DbError>),
    /// The tracker failed to record the entry.
    #[error("tracker error: {0}")]
    Tracker(#[from] LogTrackerError),
    /// A backend operation failed — e.g. the batch commit was rejected.
    #[error("db error: {0}")]
    Db(#[from] DbError),
    /// A [`SyncError`] surfaced from the sync layer.
    #[error("sync error: {0}")]
    Sync(#[from] SyncError),
}
