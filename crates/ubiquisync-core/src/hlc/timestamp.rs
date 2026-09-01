//! The [`Timestamp`] value type: the packed HLC instant every log entry
//! carries. This is just the representation and its ordering — the clock that
//! generates timestamps lives in [`super::clock`].

/// HLC timestamp — packs `(wall_ms: 48, counter: 16)` into a single `u64`.
/// This is the canonical in-memory and on-wire representation of a
/// `LogEntry.timestamp` value. Plain numeric comparison on the raw u64
/// preserves causal order (wall dominates, counter tie-breaks).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct Timestamp {
    data: u64,
}

impl Timestamp {
    /// Compose from `(millis, counter)` parts. `millis` must fit in the top
    /// 48 bits — Unix-epoch ms stays under 2^48 until roughly year 10895.
    /// A larger value would shift its high bits off the top of the `u64`
    /// (`<<` discards them rather than panicking) and silently wrap the wall
    /// component back toward zero, shattering monotonicity — so we panic
    /// instead. Unreachable for any real timestamp.
    pub fn from_parts(millis: u64, counter: u16) -> Self {
        assert!(
            millis >> (u64::BITS - COUNTER_BITS) == 0,
            "millis {millis} exceeds the 48-bit wall field"
        );
        Self {
            data: (millis << COUNTER_BITS) | counter as u64,
        }
    }

    /// Reconstruct from the packed u64 representation.
    pub fn from_raw(data: u64) -> Self {
        Self { data }
    }

    /// The packed u64 representation: `(millis << 16) | counter`.
    pub fn raw(&self) -> u64 {
        self.data
    }

    /// Wall-clock component: Unix epoch milliseconds. Top 48 bits.
    pub fn millis(&self) -> u64 {
        self.data >> COUNTER_BITS
    }

    /// Monotonic counter within a given millisecond. Low 16 bits.
    pub fn counter(&self) -> u16 {
        (self.data & COUNTER_MASK) as u16
    }
}

impl From<u64> for Timestamp {
    fn from(data: u64) -> Self {
        Self { data }
    }
}

impl From<Timestamp> for u64 {
    fn from(ts: Timestamp) -> u64 {
        ts.data
    }
}

const COUNTER_BITS: u32 = 16;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_parts_roundtrip() {
        // Given wall + counter, from_parts + accessors must agree.
        let t = Timestamp::from_parts(1_700_000_000_000, 42);
        assert_eq!(t.millis(), 1_700_000_000_000);
        assert_eq!(t.counter(), 42);
        // raw() returns the packed form: (millis << 16) | counter.
        assert_eq!(t.raw(), (1_700_000_000_000u64 << 16) | 42);
    }

    #[test]
    fn timestamp_ordering_matches_raw_u64() {
        // Counter breaks ties within the same millis.
        let earlier = Timestamp::from_parts(100, 5);
        let later_counter = Timestamp::from_parts(100, 6);
        assert!(later_counter > earlier);
        // Millis strictly dominates counter — even max counter at a lower
        // millis loses to counter 0 at the next millis.
        let next_ms = Timestamp::from_parts(101, 0);
        let max_counter_prev = Timestamp::from_parts(100, u16::MAX);
        assert!(next_ms > max_counter_prev);
    }

    #[test]
    #[should_panic = "exceeds the 48-bit wall field"]
    fn from_parts_rejects_wall_past_ceiling() {
        // The wall field is 48 bits. A millis value at the ceiling would
        // shift its top bit out and wrap the timestamp back toward zero,
        // breaking monotonicity — we panic instead. (Year ~10895; never a
        // real timestamp.) `counter_saturation_advances_wall` (in the clock
        // module) deliberately tops out one ms below this so it exercises
        // saturation safely.
        let _ = Timestamp::from_parts(1 << (u64::BITS - COUNTER_BITS), 0);
    }
}
