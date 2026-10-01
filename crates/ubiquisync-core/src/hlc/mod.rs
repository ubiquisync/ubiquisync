use thiserror::Error;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct Timestamp {
    #[cfg_attr(feature = "proptest", strategy(0..=i64::MAX as u64))]
    value: u64,
}

#[derive(Error, Debug)]
#[error("timestamp overflowed (either did not fit into i64 or was negative)")]
pub struct TimestampOverflow;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct WallTime {
    millis: u64,
}

impl Timestamp {
    pub fn from_parts(timestamp: WallTime, counter: u16) -> Result<Self, TimestampOverflow> {
        Self::from_millis_and_counter(timestamp.millis, counter)
    }

    fn from_millis_and_counter(millis: u64, counter: u16) -> Result<Self, TimestampOverflow> {
        if millis > MILLIS_MAX {
            return Err(TimestampOverflow);
        }
        let mut value = millis << COUNTER_BITS;
        value |= counter as u64;
        Ok(Self { value })
    }

    pub fn next_local(&self, now: WallTime) -> Result<Self, TimestampOverflow> {
        let new = Self::from_parts(now, 0)?;
        if &new > self { Ok(new) } else { self.bump() }
    }

    pub fn next_after(&self, other: Timestamp) -> Result<Self, TimestampOverflow> {
        if &other >= self {
            other.bump()
        } else {
            Ok(*self)
        }
    }

    fn millis(&self) -> u64 {
        self.value >> COUNTER_BITS
    }

    pub fn wall(&self) -> WallTime {
        WallTime {
            millis: self.millis(),
        }
    }

    fn counter(&self) -> u16 {
        self.value as u16
    }

    fn bump(&self) -> Result<Self, TimestampOverflow> {
        let counter = self.counter();
        if counter == u16::MAX {
            Self::from_millis_and_counter(self.millis() + 1, 0)
        } else {
            Self::from_millis_and_counter(self.millis(), counter + 1)
        }
    }
}

const COUNTER_BITS: u32 = 16;
const MILLIS_BITS: u32 = 63 - COUNTER_BITS;
const MILLIS_MAX: u64 = (1 << MILLIS_BITS) - 1;

impl From<Timestamp> for u64 {
    fn from(value: Timestamp) -> Self {
        value.value
    }
}

impl From<Timestamp> for i64 {
    fn from(value: Timestamp) -> Self {
        // the constructors for Timestamp ensures it is convertible to i64
        value.value as i64
    }
}

impl TryFrom<u64> for Timestamp {
    type Error = TimestampOverflow;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value > i64::MAX as u64 {
            return Err(TimestampOverflow);
        }
        Ok(Self { value })
    }
}

impl TryFrom<i64> for Timestamp {
    type Error = TimestampOverflow;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            // in this case we use the same error for negative
            return Err(TimestampOverflow);
        }
        Ok(Self {
            value: value as u64,
        })
    }
}

impl WallTime {
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        WallTime {
            millis: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    pub fn as_millis(&self) -> u64 {
        self.millis
    }

    pub fn from_millis(millis: u64) -> Self {
        Self { millis }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use test_case::test_case;
    use test_strategy::{Arbitrary, proptest};

    use super::*;

    fn ts(ms: u64, counter: u16) -> Timestamp {
        Timestamp::from_parts(WallTime::from_millis(ms), counter).unwrap()
    }

    #[test_case(5, 3, 0 => (5, 4) ; "wall behind bumps counter")]
    #[test_case(5, 3, 5 => (5, 4) ; "same ms bumps counter")]
    #[test_case(5, u16::MAX, 0 => (6, 0) ; "saturated counter borrows a ms")]
    #[test_case(5, 3, 9 => (9, 0) ; "wall ahead resets counter")]
    fn next_local(ms: u64, counter: u16, now: u64) -> (u64, u16) {
        let next = ts(ms, counter)
            .next_local(WallTime::from_millis(now))
            .unwrap();
        (next.millis(), next.counter())
    }

    #[test_case(MILLIS_MAX + 1, 0, 0 ; "wall past max")]
    #[test_case(MILLIS_MAX, u16::MAX, 0 ; "bump past max")]
    fn overflow(ms: u64, counter: u16, now: u64) {
        let res = Timestamp::from_parts(WallTime::from_millis(ms), counter)
            .and_then(|t| t.next_local(WallTime::from_millis(now)));
        assert!(res.is_err());
    }

    #[proptest]
    fn parts_roundtrip_and_order(
        #[strategy(0..=MILLIS_MAX)] a: u64,
        ca: u16,
        #[strategy(0..=MILLIS_MAX)] b: u64,
        cb: u16,
    ) {
        let (ta, tb) = (ts(a, ca), ts(b, cb));
        prop_assert_eq!((ta.millis(), ta.counter()), (a, ca));
        prop_assert_eq!(ta.cmp(&tb), (a, ca).cmp(&(b, cb)));
        prop_assert_eq!(Timestamp::try_from(i64::from(ta)).unwrap(), ta);
    }

    #[proptest]
    fn rejects_out_of_range(
        #[strategy(i64::MAX as u64 + 1..)] big: u64,
        #[strategy(..0i64)] neg: i64,
    ) {
        prop_assert!(Timestamp::try_from(big).is_err());
        prop_assert!(Timestamp::try_from(neg).is_err());
    }

    #[derive(Debug, Arbitrary)]
    enum Step {
        Local(#[strategy(0..=MILLIS_MAX)] u64),
        Observe(Timestamp),
    }

    #[proptest]
    fn clock_strictly_monotonic(start: Timestamp, steps: Vec<Step>) {
        let mut state = start;
        for step in steps {
            state = match step {
                Step::Local(ms) => {
                    let Ok(next) = state.next_local(WallTime::from_millis(ms)) else {
                        continue;
                    };
                    prop_assert!(next > state && next.millis() >= ms);
                    next
                }
                Step::Observe(remote) => {
                    let Ok(next) = state.next_after(remote) else {
                        continue;
                    };
                    prop_assert!(next > remote && next >= state);
                    next
                }
            };
        }
    }
}
