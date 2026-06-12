//! Document operations — the mutation types for collaborative documents.

use crate::uuid::Uuid;

/// A single document mutation. The payload of a
/// [`LogEntry`](crate::log_entry::LogEntry) in the document log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Apply a CRDT update to a document.
    UpdateDoc(UpdateDoc),
    /// Soft-delete a document.
    DeleteDoc(DeleteDoc),
}

/// Applies an opaque CRDT update to the document keyed by `(ns, id)`.
///
/// The protocol never inspects the payload — integration order doesn't
/// matter and duplicate application is a no-op (each CRDT operation carries
/// its own client/clock identity), so updates need no merge timestamp of
/// their own. The entry's HLC timestamp still matters: it is compared
/// against [`DeleteDoc`] tombstones, and a newer update un-deletes the doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDoc {
    /// Document UUID.
    pub id: Uuid,
    /// Application-defined namespace UUID. Documents are keyed by
    /// `(ns, id)`, letting the application partition its document space
    /// (e.g. one namespace per collection or per embedded-doc field).
    pub ns: Uuid,
    /// Encoded CRDT update ([yrs](https://github.com/y-crdt/y-crdt) /
    /// Yjs-compatible v1 update format).
    pub update: Vec<u8>,
}

/// Soft-deletes the document keyed by `(ns, id)` — an LWW tombstone on the
/// enclosing entry's timestamp, exactly like a table row delete. A deleted
/// document's content may be dropped from local storage, but the tombstone
/// stays in the log, and a newer-timestamped [`UpdateDoc`] revives the doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteDoc {
    /// Document UUID.
    pub id: Uuid,
    /// Application-defined namespace UUID (see [`UpdateDoc::ns`]).
    pub ns: Uuid,
}
