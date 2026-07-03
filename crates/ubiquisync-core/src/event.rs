//! In-process fan-out of change events to subscribers, keyed by target.
//!
//! [`event_bus`] returns an [`EventBusPublisher`]/[`EventBus`] pair sharing one
//! mutex-guarded map: the producer holds the publisher and emits events; subscribers
//! hold `EventBus` and [`subscribe`](EventBus::subscribe) to a
//! [`RoutableEvent::Target`], receiving a [`Subscription`] (a [`Stream`]) of the
//! events routed there. Targets left with no subscribers are removed eagerly, so
//! the map holds exactly the targets currently being watched.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

use futures_channel::mpsc;
use futures_core::Stream;

/// An event that knows the set of targets it should be delivered to.
pub trait RoutableEvent: Clone {
    /// The routing key subscribers select — the granularity of a subscription.
    type Target: Eq + Hash + Clone;

    /// The targets this event fans out to.
    fn targets(&self) -> impl Iterator<Item = Self::Target>;
}

/// A sink events are published into. The write end of an [`event_bus`] is one;
/// a producer holds `dyn Publisher`/`impl Publisher` so the bus — or a no-op —
/// can be swapped behind it. `Send + Sync` is required by the sharer (e.g. a
/// cross-thread producer), not here.
pub trait Publisher<E> {
    /// Deliver `event` to whatever this publisher feeds.
    fn publish(&self, event: E);
}

/// A [`Publisher`] that discards every event — for producers with nothing
/// listening (a headless replica) or that opt out of change events.
pub struct NoopPublisher;

impl<E> Publisher<E> for NoopPublisher {
    fn publish(&self, _event: E) {}
}

/// Constructs the read side of an event stream, paired with its [`Publisher`]
/// write end. A producer holds `impl EventHandler` so it can both emit (through
/// `Publish`) and hand out subscriptions (through the handler itself), with no
/// bound on the event type. `Send + Sync` is required by the sharer (a
/// cross-thread producer), not here.
pub trait EventHandler<E> {
    /// The write end paired with this handler.
    type Publish: Publisher<E>;

    /// Build a fresh `(publisher, handler)` sharing one stream.
    fn init() -> (Self::Publish, Self);
}

/// An [`EventHandler`] that discards events — pairs [`NoopPublisher`] with itself
/// for a producer with nothing listening.
pub struct NoopHandler;

impl<E> EventHandler<E> for NoopHandler {
    type Publish = NoopPublisher;
    fn init() -> (Self::Publish, Self) {
        (NoopPublisher, NoopHandler)
    }
}

/// Per-subscriber channel depth. A subscriber this far behind is dropped.
const SUBSCRIBER_BUFFER: usize = 256;

/// The subscribers watching one target, keyed by subscription id.
type Subscribers<T> = HashMap<u64, mpsc::Sender<T>>;

struct Inner<T: RoutableEvent> {
    targets: HashMap<T::Target, Subscribers<T>>,
    next_id: u64,
}

/// The write end: emits events into the bus. Held by the producer.
pub struct EventBusPublisher<T: RoutableEvent> {
    inner: Arc<Mutex<Inner<T>>>,
}

/// The read end: hands out [`Subscription`]s. Cheap to clone and share.
pub struct EventBus<T: RoutableEvent> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T: RoutableEvent> Clone for EventBus<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Create an event bus, returning its `(write, read)` handles.
pub fn event_bus<T: RoutableEvent>() -> (EventBusPublisher<T>, EventBus<T>) {
    let inner = Arc::new(Mutex::new(Inner {
        targets: HashMap::new(),
        next_id: 0,
    }));
    (
        EventBusPublisher {
            inner: Arc::clone(&inner),
        },
        EventBus { inner },
    )
}

impl<T: RoutableEvent> EventHandler<T> for EventBus<T> {
    type Publish = EventBusPublisher<T>;
    fn init() -> (Self::Publish, Self) {
        event_bus()
    }
}

impl<T: RoutableEvent> Publisher<T> for EventBusPublisher<T> {
    /// Deliver `event` to every subscriber of each of its targets, pruning any
    /// whose channel is full or gone and dropping targets left with none.
    fn publish(&self, event: T) {
        let mut inner = lock(&self.inner);
        for target in event.targets() {
            if let Entry::Occupied(mut e) = inner.targets.entry(target) {
                e.get_mut()
                    .retain(|_, tx| tx.try_send(event.clone()).is_ok());
                if e.get().is_empty() {
                    e.remove();
                }
            }
        }
    }
}

impl<T: RoutableEvent> EventBus<T> {
    /// Subscribe to every event routed to `target`.
    pub fn subscribe(&self, target: T::Target) -> Subscription<T> {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_BUFFER);
        let mut inner = lock(&self.inner);
        let id = inner.next_id;
        inner.next_id += 1;
        inner
            .targets
            .entry(target.clone())
            .or_default()
            .insert(id, tx);
        Subscription {
            inner: Arc::clone(&self.inner),
            target,
            id,
            rx,
        }
    }
}

/// A live subscription to one target: a [`Stream`] of the events routed there.
/// Dropping it unsubscribes (and drops the target if it was the last one).
pub struct Subscription<T: RoutableEvent> {
    inner: Arc<Mutex<Inner<T>>>,
    target: T::Target,
    id: u64,
    rx: mpsc::Receiver<T>,
}

impl<T: RoutableEvent> Stream for Subscription<T>
where
    T::Target: Unpin,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        Pin::new(&mut self.get_mut().rx).poll_next(cx)
    }
}

impl<T: RoutableEvent> Drop for Subscription<T> {
    fn drop(&mut self) {
        let mut inner = lock(&self.inner);
        if let Entry::Occupied(mut e) = inner.targets.entry(self.target.clone()) {
            e.get_mut().remove(&self.id);
            if e.get().is_empty() {
                e.remove();
            }
        }
    }
}

/// Lock, recovering from a poisoned guard: the map can't be left inconsistent by
/// a panic (the critical sections only clone-and-send), so recovering beats
/// propagating poison to every publisher and subscriber.
fn lock<G>(m: &Mutex<G>) -> MutexGuard<'_, G> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use futures::{FutureExt, StreamExt};

    use super::*;

    #[derive(Clone)]
    struct Ev {
        n: i32,
        to: Vec<u32>,
    }

    impl RoutableEvent for Ev {
        type Target = u32;
        fn targets(&self) -> impl Iterator<Item = u32> {
            self.to.clone().into_iter()
        }
    }

    fn ev(n: i32, to: &[u32]) -> Ev {
        Ev { n, to: to.to_vec() }
    }

    /// Poll the stream once without blocking: `Some` if an event is ready now.
    fn recv(sub: &mut Subscription<Ev>) -> Option<i32> {
        sub.next().now_or_never().flatten().map(|e| e.n)
    }

    fn target_count(bus: &EventBus<Ev>) -> usize {
        lock(&bus.inner).targets.len()
    }

    #[test]
    fn delivers_to_subscriber() {
        let (p, bus) = event_bus::<Ev>();
        let mut s = bus.subscribe(1);
        p.publish(ev(10, &[1]));
        assert_eq!(recv(&mut s), Some(10));
    }

    #[test]
    fn fans_out_to_all_targets_of_an_event() {
        let (p, bus) = event_bus::<Ev>();
        let mut a = bus.subscribe(1);
        let mut b = bus.subscribe(2);
        p.publish(ev(7, &[1, 2]));
        assert_eq!(recv(&mut a), Some(7));
        assert_eq!(recv(&mut b), Some(7));
    }

    #[test]
    fn other_targets_are_not_delivered() {
        let (p, bus) = event_bus::<Ev>();
        let mut a = bus.subscribe(1);
        p.publish(ev(1, &[2]));
        assert_eq!(recv(&mut a), None);
    }

    #[test]
    fn last_unsubscribe_removes_the_target() {
        let (_p, bus) = event_bus::<Ev>();
        let a = bus.subscribe(1);
        let b = bus.subscribe(1);
        assert_eq!(target_count(&bus), 1);
        drop(a);
        assert_eq!(target_count(&bus), 1); // b still watching
        drop(b);
        assert_eq!(target_count(&bus), 0); // eager removal
    }

    #[test]
    fn overflowing_subscriber_is_pruned_on_publish() {
        let (p, bus) = event_bus::<Ev>();
        let _s = bus.subscribe(1); // held, never drained
        for i in 0..(SUBSCRIBER_BUFFER as i32 + 8) {
            p.publish(ev(i, &[1]));
        }
        assert_eq!(target_count(&bus), 0); // buffer filled -> pruned -> target dropped
    }
}
