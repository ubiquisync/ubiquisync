//! The engine's public API: apply local writes and watch the events they emit.

use crate::{
    event::{RoutableEvent, Subscription},
    uuid::Uuid,
};

/// Apply local writes and observe the change events they produce.
///
/// `Op`/`Event` are the engine's own vocabulary. Projecting an app- or
/// domain-specific slice out of them (e.g. a `-tables` store over a bundled
/// reducer) is a layer *above* this trait, via `Into`/`TryFrom` on the wrapper —
/// not something the engine implements.
#[async_trait::async_trait]
pub trait Store<Op, Err, Event: RoutableEvent> {
    /// Apply a local write, minting a fresh log entry for it.
    async fn exec(&self, server_user_id: Option<Uuid>, op: Op) -> Result<(), Err>;

    /// Subscribe to events routed to `target`; dropping the subscription
    /// unsubscribes.
    fn watch(&self, target: Event::Target) -> Subscription<Event>;
}
