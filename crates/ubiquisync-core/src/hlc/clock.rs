//! The pure hybrid logical clock ([`Hlc`]), its wall-clock source
//! ([`wall_ms`]), and the skew bound that protects it ([`MAX_SKEW_MS`] /
//! [`SkewError`]).
//!
//! Each tick the HLC generates is strictly greater than the last, and ticks
//! respect causality across peers: after a peer observes a remote timestamp,
//! every tick it generates is strictly greater. Entries written in one atomic
//! transaction share a tick, so a peer's entry stream is monotonically
//! non-decreasing. All LWW merge decisions in the protocol compare these
//! timestamps.
//!
//! [`Hlc`] is the pure in-memory clock; the shared, persistent service that
//! subsystems actually hold lives in [`super::service`].

use super::timestamp::Timestamp;

/// Maximum acceptable forward clock skew for remote entries, in milliseconds.
/// Entries whose wall-clock component is more than this far ahead of the
/// local wall clock are rejected to prevent HLC poisoning. Within-window
/// skew is self-healing (the local clock catches up, the HLC re-aligns).
///
/// HLC timestamps are UTC (Unix epoch ms), so DST and timezone handling
/// are irrelevant. 60s absorbs NTP settling and devices waking from sleep,
/// while decisively rejecting clocks that are actually wrong (usually off
/// by minutes, hours, or more).
pub const MAX_SKEW_MS: u64 = 60_000;

/// Hybrid logical clock — the pure, in-memory state machine. Holds the
/// greatest timestamp seen so far (locally generated or remotely observed)
/// and derives the next tick from it plus the wall clock.
///
/// This type does no I/O and is single-threaded; almost all callers want
/// [`HlcService`](super::service::HlcService), which wraps one of these
/// in a lock and persists its state so monotonicity survives restarts.
#[derive(Debug)]
pub struct Hlc {
    state: Timestamp,
}

impl Hlc {
    /// Construct from the last persisted state (packed u64), or 0 for a
    /// brand-new clock.
    pub fn new(seed: u64) -> Self {
        Self {
            state: Timestamp::from_raw(seed),
        }
    }

    /// Generate a new timestamp for a local write. Guarantees:
    /// - Result > all previously generated timestamps.
    /// - Result > all timestamps absorbed by `observe` so far.
    /// - Result >= `local_wall_ms`.
    ///
    /// `local_wall_ms` (typically from [`wall_ms`]) is passed in rather than
    /// read here for the same reason as [`observe`](Self::observe): [`Hlc`] is
    /// a pure state machine that does no I/O, which keeps the tick logic fully
    /// testable against a controlled clock. The service layer reads the wall
    /// clock once and supplies it. (Named `tick` rather than `now` precisely
    /// because it does not read the clock — it advances state from a reading
    /// the caller provides.)
    pub fn tick(&mut self, local_wall_ms: u64) -> Timestamp {
        let old_wall = self.state.millis();
        let old_counter = self.state.counter();

        // The wall component can never regress: take the larger of the
        // caller's wall reading and the wall we last emitted. When the wall
        // clock has ticked into a new millisecond it wins and we get a fresh
        // wall; when it's stalled within a millisecond — or running behind a
        // remote wall we already absorbed via `observe` — the stored wall
        // holds the line so we never hand back time.
        let mut new_wall = local_wall_ms.max(old_wall);

        let new_counter = if new_wall == old_wall {
            // The wall reading did not pass the wall we last used, so the
            // wall alone can't order this tick ahead of the previous one. The
            // counter does that work. `saturating_add` caps at u16::MAX rather
            // than wrapping to 0, which would silently move time backwards.
            let c = old_counter.saturating_add(1);
            if c == old_counter {
                // The add was a no-op, so we were already at u16::MAX: the
                // counter is exhausted for this millisecond. Borrow a
                // millisecond from the future and restart the counter, keeping
                // the result strictly greater than the old state. 65_536 ticks
                // inside one millisecond is far beyond any real workload — this
                // is a correctness backstop, not an expected path.
                new_wall += 1;
                0
            } else {
                c
            }
        } else {
            // The wall reading advanced into a new millisecond, which by
            // itself already orders this tick ahead of the last. Start the
            // counter fresh for the new millisecond.
            0
        };

        self.state = Timestamp::from_parts(new_wall, new_counter);
        self.state
    }

    /// Absorb a timestamp received from a peer. Does not generate a new
    /// timestamp — ensures that the next [`tick`](Self::tick) will return
    /// something strictly greater than `received`.
    ///
    /// The skew bound is enforced here, at the clock itself, so no caller
    /// can poison the clock by forgetting to check: a timestamp beyond
    /// [`MAX_SKEW_MS`] ahead of `local_wall_ms` is rejected and the state
    /// does not advance. Callers should also pre-validate batches with
    /// [`is_within_skew`](Self::is_within_skew) so a bad entry is rejected
    /// before any work is done on it; this check is the backstop.
    ///
    /// `local_wall_ms` (typically from [`wall_ms`]) is passed in rather than
    /// read here because [`Hlc`] is a pure state machine that does no I/O —
    /// the reference instant is an input like any other (see [`tick`](Self::tick)).
    /// Threading it through also lets a batch of received entries be checked
    /// against one consistent wall-clock reading and lets tests pin the skew
    /// window exactly.
    pub fn observe(&mut self, received: Timestamp, local_wall_ms: u64) -> Result<(), SkewError> {
        if !Self::is_within_skew(received, local_wall_ms) {
            return Err(SkewError {
                received,
                local_wall_ms,
            });
        }
        if received > self.state {
            self.state = received;
        }
        Ok(())
    }

    /// The greatest timestamp seen so far. This is what gets persisted.
    pub fn state(&self) -> Timestamp {
        self.state
    }

    /// Returns true if the received timestamp's wall-clock component is
    /// within the acceptable forward skew window of `local_wall_ms`.
    /// Past timestamps are always allowed (they lose LWW harmlessly); only
    /// far-future timestamps are rejected to prevent HLC poisoning.
    pub fn is_within_skew(received: Timestamp, local_wall_ms: u64) -> bool {
        let remote_wall = received.millis();
        remote_wall <= local_wall_ms.saturating_add(MAX_SKEW_MS)
    }
}

/// Current wall clock as Unix epoch milliseconds — the wall component fed
/// into the HLC. Saturates to 0 if the system clock reads before the epoch.
pub fn wall_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Error returned by [`Hlc::observe`] when a remote timestamp's wall-clock
/// component is more than [`MAX_SKEW_MS`] ahead of the local wall clock.
/// One of the two clocks is wrong — the remote ahead or the local behind;
/// this signal alone cannot say which. The offending entry must be rejected
/// (not applied), pausing sync from that peer until a clock is corrected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkewError {
    /// The rejected remote timestamp.
    pub received: Timestamp,
    /// The local wall clock (Unix epoch ms) it was checked against.
    pub local_wall_ms: u64,
}

impl std::fmt::Display for SkewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "remote timestamp {} ms ahead of local wall clock (max skew {} ms)",
            self.received.millis().saturating_sub(self.local_wall_ms),
            MAX_SKEW_MS
        )
    }
}

impl std::error::Error for SkewError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_monotone() {
        // Goal: consecutive local ticks strictly increase.
        // Given: a fresh clock. When: ticking repeatedly. Then: each tick
        // beats the last.
        let mut hlc = Hlc::new(0);
        let t1 = hlc.tick(wall_ms());
        let t2 = hlc.tick(wall_ms());
        let t3 = hlc.tick(wall_ms());
        assert!(t2 > t1);
        assert!(t3 > t2);
    }

    #[test]
    fn tick_after_observe_beats_observed() {
        // Goal: causality across peers — a write made after seeing a remote
        // entry always wins LWW against it.
        // Given: a clock that observed a remote timestamp ahead of local
        // wall time (within the skew window). When: ticking. Then: the tick
        // is strictly greater than the observed timestamp.
        let mut hlc = Hlc::new(0);
        let local = wall_ms();
        let remote = Timestamp::from_parts(local + MAX_SKEW_MS, 999);
        hlc.observe(remote, local).unwrap();
        let t = hlc.tick(wall_ms());
        assert!(t > remote);
    }

    #[test]
    fn counter_saturation_advances_wall() {
        // Goal: monotonicity survives counter exhaustion.
        // Given: a clock whose counter sits at u16::MAX at a far-future
        // wall (so wall_ms() doesn't dominate; must fit in 48 bits).
        let far_future = (1u64 << 48) - 2;
        let saturated = Timestamp::from_parts(far_future, u16::MAX);
        let mut hlc = Hlc::new(saturated.raw());
        // When: ticking. Then: the wall advances by 1 and the counter resets.
        let t = hlc.tick(wall_ms());
        assert!(t > saturated, "must advance past saturated counter");
        assert_eq!(t.millis(), far_future + 1);
        assert_eq!(t.counter(), 0);
    }

    #[test]
    fn observe_stale_is_noop() {
        // Goal: old remote timestamps never move the clock backward.
        // Given: a clock at state 1000. When: observing 500. Then: state
        // is unchanged (and the observe itself succeeds — past is fine).
        let mut hlc = Hlc::new(1000);
        hlc.observe(Timestamp::from_raw(500), wall_ms()).unwrap();
        assert_eq!(hlc.state(), Timestamp::from_raw(1000));
    }

    #[test]
    fn observe_rejects_beyond_skew_without_advancing() {
        // Goal: the clock itself refuses poisoning — no caller can advance
        // it past the skew window by forgetting a check.
        // Given: a remote timestamp one ms beyond the window.
        let local = wall_ms();
        let too_far = Timestamp::from_parts(local + MAX_SKEW_MS + 1, 0);
        let mut hlc = Hlc::new(0);
        // When: observing it. Then: an error identifying the timestamp, and
        // the clock state has not moved.
        let err = hlc.observe(too_far, local).unwrap_err();
        assert_eq!(err.received, too_far);
        assert_eq!(hlc.state(), Timestamp::from_raw(0));
    }

    #[test]
    fn skew_accepts_past_timestamps() {
        // Past timestamps are always within skew — they just lose LWW harmlessly.
        let local = wall_ms();
        let past = Timestamp::from_parts(local.saturating_sub(10 * 365 * 86_400_000), 0); // 10 years ago
        assert!(Hlc::is_within_skew(past, local));
    }

    #[test]
    fn skew_accepts_within_window() {
        // Anything up to MAX_SKEW_MS ahead is accepted, including the
        // current instant and the window edge itself.
        let local = wall_ms();
        assert!(Hlc::is_within_skew(Timestamp::from_parts(local, 0), local));
        let edge = Timestamp::from_parts(local + MAX_SKEW_MS, 0);
        assert!(Hlc::is_within_skew(edge, local));
    }

    #[test]
    fn skew_rejects_beyond_window() {
        // One ms beyond the window is rejected; so is a clock set years ahead.
        let local = wall_ms();
        let too_far = Timestamp::from_parts(local + MAX_SKEW_MS + 1, 0);
        assert!(!Hlc::is_within_skew(too_far, local));
        let far_future = Timestamp::from_parts(local + (365 * 86_400_000), 0); // one year ahead
        assert!(!Hlc::is_within_skew(far_future, local));
    }
}
