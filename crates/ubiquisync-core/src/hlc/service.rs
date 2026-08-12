//! Shared, persistent hybrid logical clock service.
//!
//! [`HlcService`] is the clock handle subsystems actually hold: it wraps the
//! pure [`Hlc`](super::Hlc) in a lock so every log domain draws from one causal clock domain,
//! and persists the clock state through [`HlcStorage`] so monotonicity survives restarts — a peer
//! must never reissue a timestamp it already wrote, even after a crash.
//!
//! It is not `Clone`; construct it once per database, wrap it in an `Arc`,
//! and hand a clone of that `Arc` to each subsystem.

use std::sync::Mutex;

use crate::storage::{Storage, StorageError};

use super::{Hlc, SkewError, Timestamp, wall_ms};

/// Error from a clock operation: either the storage backend failed, or a
/// remote timestamp was rejected by the skew bound.
#[derive(Debug)]
pub enum HlcError<E> {
    /// The remote timestamp is too far ahead of the local wall clock — the
    /// entry carrying it must be rejected. See [`SkewError`](super::SkewError).
    Skew(SkewError),
    /// The storage backend failed to load or save clock state.
    Storage(E),
}

impl<E: std::fmt::Display> std::fmt::Display for HlcError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HlcError::Skew(e) => write!(f, "{e}"),
            HlcError::Storage(e) => write!(f, "hlc storage: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for HlcError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HlcError::Skew(e) => Some(e),
            HlcError::Storage(e) => Some(e),
        }
    }
}

/// Shared HLC service: one lock-protected clock plus its persistence.
/// Serializes `now()`/`observe()` across subsystems so they share a single
/// causal clock domain in memory and on disk.
pub struct HlcService<S: Storage> {
    state: Mutex<Hlc>,
    storage: S,
}

impl<S: Storage> HlcService<S> {
    /// Seed the in-memory clock from the persisted state (0 if none) and
    /// return the service. The persisted state is the last-observed clock
    /// position, so causal monotonicity survives crashes.
    pub fn open(storage: S) -> Result<Self, StorageError> {
        let seed = storage.load_hlc()?.unwrap_or(0);
        Ok(Self {
            state: Mutex::new(Hlc::new(seed)),
            storage,
        })
    }

    /// Lock the clock, recovering the guard if a previous holder panicked.
    /// Recovery is safe because the protected [`Hlc`] state can never be left
    /// logically corrupt by a panic: it is only ever *replaced* with a fully
    /// validated, strictly larger [`Timestamp`], so whether the panic came
    /// from a `storage` call or from `tick`'s wall-ceiling assert, the
    /// recovered state is an intact, monotonic prior value. A recovered clock
    /// can only tick forward, so it can't hand back an out-of-order or
    /// duplicate timestamp, and the underlying failure resurfaces on the next
    /// operation. This beats `unwrap()`, which would let one subsystem's
    /// panic crash every subsystem sharing the clock.
    fn lock(&self) -> std::sync::MutexGuard<'_, Hlc> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Generate a fresh timestamp for a local write and enqueue the new state
    /// into `sink`. Always advances; always writes — the state must reach the
    /// committing batch, or a crash could reissue it.
    pub fn now(&self, sink: &mut S::Batch) -> Result<Timestamp, S::Error> {
        let mut hlc = self.lock();
        let ts = hlc.tick(wall_ms());
        self.storage.save(sink, hlc.state().raw())?;
        Ok(ts)
    }

    /// Absorb a remote timestamp, enforcing the skew bound (see
    /// [`Hlc::observe`]). Saves into `sink` only when the in-memory state
    /// actually advances — observe is a no-op for stale receipts and we don't
    /// want to spam the register.
    ///
    /// `local_wall_ms` is supplied by the caller — typically a single
    /// [`wall_ms`](super::wall_ms) reading taken once when a received
    /// batch starts replaying — rather than read here per call. This judges a
    /// whole batch against one reference instant, so entries don't drift in
    /// and out of the skew window depending on where they fall in the loop,
    /// and it keeps the skew tests deterministic. The wall clock only moves
    /// forward across a batch, so a shared start-of-batch reading is at worst
    /// conservative (it may reject a borderline entry that a fresher reading
    /// would admit) — never unsound, since it can't widen the window.
    pub fn observe(
        &self,
        received: Timestamp,
        local_wall_ms: u64,
        sink: &mut S::Sink,
    ) -> Result<(), HlcError<S::Error>> {
        let mut hlc = self.lock();
        let before = hlc.state();
        hlc.observe(received, local_wall_ms)
            .map_err(HlcError::Skew)?;
        let after = hlc.state();
        if after != before {
            self.storage
                .save(sink, after.raw())
                .map_err(HlcError::Storage)?;
        }
        Ok(())
    }

    /// Current clock state without advancing it. Cheap snapshot for callers
    /// that only need to peek (e.g. tests, diagnostics).
    pub fn state(&self) -> Timestamp {
        self.lock().state()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MAX_SKEW_MS, wall_ms};
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// In-memory register standing in for a backend metadata row.
    #[derive(Default)]
    struct MemStorage {
        /// Last saved clock state; meaningful only when `present` is set.
        value: AtomicU64,
        /// Count of `save` calls, so tests can assert persistence cadence.
        saves: AtomicUsize,
        /// Whether anything has been saved yet — drives `load` returning
        /// `None` for a fresh store rather than relying on a value sentinel.
        present: std::sync::atomic::AtomicBool,
    }

    impl HlcStorage for &MemStorage {
        type Error = std::convert::Infallible;
        type Sink = ();

        fn load(&self) -> Result<Option<u64>, Self::Error> {
            Ok(self
                .present
                .load(Ordering::SeqCst)
                .then(|| self.value.load(Ordering::SeqCst)))
        }

        fn save(&self, _sink: &mut (), raw: u64) -> Result<(), Self::Error> {
            self.value.store(raw, Ordering::SeqCst);
            self.present.store(true, Ordering::SeqCst);
            self.saves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn now_persists_every_tick() {
        // Goal: a timestamp is never handed out without its covering state
        // being saved — a crash must not reissue one.
        // Given: a fresh service over empty storage.
        let mem = MemStorage::default();
        let svc = HlcService::open(&mem).unwrap();
        // When: ticking twice. Then: storage holds the latest tick and was
        // written once per tick.
        let t1 = svc.now(&mut ()).unwrap();
        let t2 = svc.now(&mut ()).unwrap();
        assert!(t2 > t1);
        assert_eq!(mem.value.load(Ordering::SeqCst), t2.raw());
        assert_eq!(mem.saves.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reopen_resumes_past_persisted_state() {
        // Goal: monotonicity survives a restart.
        // Given: a service that ticked, then was dropped.
        let mem = MemStorage::default();
        let last = {
            let svc = HlcService::open(&mem).unwrap();
            svc.now(&mut ()).unwrap()
        };
        // When: reopening over the same storage and ticking.
        let svc2 = HlcService::open(&mem).unwrap();
        let t = svc2.now(&mut ()).unwrap();
        // Then: the new tick beats everything from the previous life.
        assert!(t > last);
    }

    #[test]
    fn observe_persists_only_on_advance() {
        // Goal: stale receipts don't spam the storage register.
        // Given: a service whose clock has advanced past some remote ts.
        let mem = MemStorage::default();
        let svc = HlcService::open(&mem).unwrap();
        let local = wall_ms();
        let ahead = Timestamp::from_parts(local + MAX_SKEW_MS, 7);
        svc.observe(ahead, local, &mut ()).unwrap();
        let saves_after_advance = mem.saves.load(Ordering::SeqCst);
        assert_eq!(saves_after_advance, 1, "advancing observe persists");
        // When: observing something older. Then: no new save.
        svc.observe(Timestamp::from_parts(local, 0), local, &mut ())
            .unwrap();
        assert_eq!(mem.saves.load(Ordering::SeqCst), saves_after_advance);
        assert_eq!(svc.state(), ahead);
    }

    #[test]
    fn observe_beyond_skew_errors_and_persists_nothing() {
        // Goal: the service surfaces the clock's skew rejection and leaves
        // both memory and storage untouched.
        // Given: a remote timestamp beyond the skew window.
        let mem = MemStorage::default();
        let svc = HlcService::open(&mem).unwrap();
        let local = wall_ms();
        let too_far = Timestamp::from_parts(local + MAX_SKEW_MS + 1, 0);
        // When: observing it. Then: HlcError::Skew, state still 0, no save.
        let err = svc.observe(too_far, local, &mut ()).unwrap_err();
        assert!(matches!(err, HlcError::Skew(_)));
        assert_eq!(svc.state(), Timestamp::from_raw(0));
        assert_eq!(mem.saves.load(Ordering::SeqCst), 0);
    }

    /// Storage whose `save` always fails — exercises the fallible path that
    /// the `Infallible` `MemStorage` can't reach.
    struct FailingStorage;

    impl HlcStorage for FailingStorage {
        type Error = &'static str;
        type Sink = ();

        fn load(&self) -> Result<Option<u64>, Self::Error> {
            Ok(None)
        }

        fn save(&self, _sink: &mut (), _raw: u64) -> Result<(), Self::Error> {
            Err("save failed")
        }
    }

    #[test]
    fn now_propagates_save_failure() {
        // A timestamp must never be handed back when its covering state could
        // not be persisted — the storage error propagates in its place.
        let svc = HlcService::open(FailingStorage).unwrap();
        assert!(svc.now(&mut ()).is_err());
    }

    #[test]
    fn advancing_observe_surfaces_save_failure_as_storage_error() {
        // An advancing observe that can't persist must report Storage, not
        // Skew — the timestamp was within the window; only the save failed.
        let svc = HlcService::open(FailingStorage).unwrap();
        let local = wall_ms();
        let ahead = Timestamp::from_parts(local + MAX_SKEW_MS, 1);
        assert!(matches!(
            svc.observe(ahead, local, &mut ()),
            Err(HlcError::Storage(_))
        ));
    }

    #[test]
    fn lock_recovers_from_a_poisoned_clock() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        // Goal: a panic that poisons the clock mutex (as a panicking
        // `storage.save` would) does not brick the clock for everyone else.
        let mem = MemStorage::default();
        let svc = HlcService::open(&mem).unwrap();
        let t1 = svc.now(&mut ()).unwrap();
        // Poison the mutex by panicking while holding the guard.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = svc.lock();
            panic!("poison the clock");
        }));
        // A bare `.lock().unwrap()` would panic here; `lock()` recovers the
        // guard, so the clock stays usable and still strictly monotone.
        let t2 = svc.now(&mut ()).unwrap();
        assert!(t2 > t1);
    }

    #[test]
    fn concurrent_ticks_are_all_distinct() {
        // Goal: the mutex actually serializes ticks — under contention every
        // issued timestamp is unique (hence strictly ordered), none reused.
        let mem = MemStorage::default();
        let svc = HlcService::open(&mem).unwrap();
        let mut all: Vec<Timestamp> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        (0..100)
                            .map(|_| svc.now(&mut ()).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        let total = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), total, "every concurrent tick must be unique");
    }
}
