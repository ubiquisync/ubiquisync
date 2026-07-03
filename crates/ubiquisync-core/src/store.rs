//! The engine's public API: apply local writes and watch the events they emit.

use futures_core::Stream;

use crate::{event::RoutableEvent, uuid::Uuid};

/// Apply local writes and observe the change events they produce.
#[async_trait::async_trait]
pub trait Store<Op, Err, Event: RoutableEvent> {
    /// Apply a local write, minting a fresh log entry for it.
    async fn exec(&self, server_user_id: Option<Uuid>, op: Op) -> Result<(), Err>;

    /// Stream events routed to `target`; dropping the stream unsubscribes.
    fn watch(&self, target: Event::Target) -> impl Stream<Item = Event>;
}
