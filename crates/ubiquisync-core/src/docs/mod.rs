//! Collaborative rich-text documents.
//!
//! Documents are the protocol's third data domain, alongside system tables
//! and user-defined tables. They sync through their own log (same
//! [`LogEntry`](crate::log_entry::LogEntry) envelope, own op vocabulary) and
//! merge as CRDTs rather than LWW: update payloads are opaque to the
//! protocol and integrate commutatively and idempotently, so concurrent
//! edits to the same document combine without losing anyone's work.
//!
//! This module currently holds the op vocabulary; the document store
//! (caching, flushing, subscriptions) lands with the engine port.

pub mod op;
