//! Shared, persistent hybrid logical clock service.
//!
//! [`HlcService`] is the clock handle subsystems actually hold: it wraps the
//! pure [`Hlc`](crate::hlc::Hlc) in a lock so every log domain draws from one causal clock domain,
//! and persists the clock state through [`HlcStorage`] so monotonicity survives restarts — a peer
//! must never reissue a timestamp it already wrote, even after a crash.
//!
//! Construct once per database and hand a clone/`Arc` of it to each
//! subsystem.

use std::sync::Mutex;

use crate::hlc::{Hlc, SkewError, Timestamp, wall_ms};

/// Durable storage for the clock state: a single packed-`u64` register.
///
/// Implemented by storage backend crates (e.g. as one row in a metadata
/// table). The contract is a plain register — `load` returns whatever was
/// last `save`d, or `None` if nothing ever was. Durability must match the
/// data it timestamps: if log entries survive a crash, the saved clock
/// state that covered them must too.
pub trait HlcStorage {
    /// Backend error type surfaced through the service's results.
    type Error;

    /// Load the last persisted clock state, or `None` for a fresh store.
    fn load(&self) -> Result<Option<u64>, Self::Error>;

    /// Durably replace the persisted clock state.
    fn save(&self, raw: u64) -> Result<(), Self::Error>;
}

/// Error from a clock operation: either the storage backend failed, or a
/// remote timestamp was rejected by the skew bound.
#[derive(Debug)]
pub enum HlcError<E> {
    /// The remote timestamp is too far ahead of the local wall clock — the
    /// entry carrying it must be rejected. See [`crate::hlc::SkewError`].
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
pub struct HlcService<S: HlcStorage> {
    state: Mutex<Hlc>,
    storage: S,
}

impl<S: HlcStorage> HlcService<S> {
    /// Seed the in-memory clock from the persisted state (0 if none) and
    /// return the service. The persisted state is the last-observed clock
    /// position, so causal monotonicity survives crashes.
    pub fn open(storage: S) -> Result<Self, S::Error> {
        let seed = storage.load()?.unwrap_or(0);
        Ok(Self {
            state: Mutex::new(Hlc::new(seed)),
            storage,
        })
    }

    /// Generate a fresh timestamp for a local write and persist the new
    /// state. Always advances; always writes — the state must be durable
    /// before the timestamp is used, or a crash could reissue it.
    pub fn now(&self) -> Result<Timestamp, S::Error> {
        let mut hlc = self.state.lock().unwrap();
        let ts = hlc.tick(wall_ms());
        self.storage.save(hlc.state().raw())?;
        Ok(ts)
    }

    /// Absorb a remote timestamp, enforcing the skew bound (see
    /// [`Hlc::observe`]). Persists only when the in-memory state actually
    /// advances — observe is a no-op for stale receipts and we don't want
    /// to spam the register.
    ///
    /// `local_wall_ms` is supplied by the caller — typically a single
    /// [`wall_ms`](crate::hlc::wall_ms) reading taken once when a received
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
    ) -> Result<(), HlcError<S::Error>> {
        let mut hlc = self.state.lock().unwrap();
        let before = hlc.state();
        hlc.observe(received, local_wall_ms)
            .map_err(HlcError::Skew)?;
        let after = hlc.state();
        if after != before {
            self.storage.save(after.raw()).map_err(HlcError::Storage)?;
        }
        Ok(())
    }

    /// Current clock state without advancing it. Cheap snapshot for callers
    /// that only need to peek (e.g. tests, diagnostics).
    pub fn state(&self) -> Timestamp {
        self.state.lock().unwrap().state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::{MAX_SKEW_MS, wall_ms};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// In-memory register standing in for a backend metadata row.
    #[derive(Default)]
    struct MemStorage {
        // u64::MAX sentinel = never saved; clock state can't reach it
        // because from_parts truncates millis to 48 bits.
        value: AtomicU64,
        saves: AtomicUsize,
        present: std::sync::atomic::AtomicBool,
    }

    impl HlcStorage for &MemStorage {
        type Error = std::convert::Infallible;

        fn load(&self) -> Result<Option<u64>, Self::Error> {
            Ok(self
                .present
                .load(Ordering::SeqCst)
                .then(|| self.value.load(Ordering::SeqCst)))
        }

        fn save(&self, raw: u64) -> Result<(), Self::Error> {
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
        let t1 = svc.now().unwrap();
        let t2 = svc.now().unwrap();
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
            svc.now().unwrap()
        };
        // When: reopening over the same storage and ticking.
        let svc2 = HlcService::open(&mem).unwrap();
        let t = svc2.now().unwrap();
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
        svc.observe(ahead, local).unwrap();
        let saves_after_advance = mem.saves.load(Ordering::SeqCst);
        assert_eq!(saves_after_advance, 1, "advancing observe persists");
        // When: observing something older. Then: no new save.
        svc.observe(Timestamp::from_parts(local, 0), local).unwrap();
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
        let err = svc.observe(too_far, local).unwrap_err();
        assert!(matches!(err, HlcError::Skew(_)));
        assert_eq!(svc.state(), Timestamp::from_raw(0));
        assert_eq!(mem.saves.load(Ordering::SeqCst), 0);
    }
}
