//! [`Replica`]: a store readable and writable for any origin — the oplog.

use crate::log_entry::OpEntry;

use super::processor::LogProcessor;
use super::source::LogSource;

/// A full replica: readable ([`LogSource`]) and writable for any origin
/// ([`LogProcessor`]). The SQL oplog is this, so one type serves, relays, and
/// merges. Contrast [`FileLogReplica`](super::FileLogReplica), which writes only
/// its own origin.
pub trait Replica<E>: LogProcessor<E> + LogSource<OpEntry<E>> {}

/// Anything that is both is a [`Replica`].
impl<E, T: LogProcessor<E> + LogSource<OpEntry<E>>> Replica<E> for T {}
