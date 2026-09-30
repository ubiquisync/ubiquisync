use thiserror::Error;

mod clock;
mod service;
mod timestamp;

// pub use clock::{Hlc, MAX_SKEW_MS, SkewError, wall_ms};
// pub use service::{HlcError, HlcService, HlcStorage};
// pub use timestamp::Timestamp;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct HlcTimestamp {
    #[strategy(0..=i64::MAX as u64)]
    value: u64,
}

#[derive(Error, Debug)]
#[error("timestamp overflowed (either did not fit into i64 or was negative)")]
pub struct HlcTimestampOverflow;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct Timestamp {
    millis: u64,
}

impl HlcTimestamp {
    pub fn from_parts(timestamp: &Timestamp, counter: u16) -> Result<Self, HlcTimestampOverflow> {
        Self::from_millis_and_counter(timestamp.millis, counter)
    }

    fn from_millis_and_counter(millis: u64, counter: u16) -> Result<Self, HlcTimestampOverflow> {
        if millis > MILLIS_MAX {
            return Err(HlcTimestampOverflow);
        }
        let mut value = millis << COUNTER_BITS;
        value |= counter as u64;
        Ok(Self { value })
    }

    pub fn next_local(&self, now: &Timestamp) -> Result<Self, HlcTimestampOverflow> {
        let new = Self::from_parts(now, 0)?;
        if &new > self { Ok(new) } else { self.bump() }
    }

    pub fn next_after(
        &self,
        other: &HlcTimestamp,
        now: &Timestamp,
    ) -> Result<Self, HlcTimestampOverflow> {
        let next_local = self.next_local(now)?;
        if other > &next_local {
            other.bump()
        } else {
            Ok(next_local)
        }
    }

    pub fn millis(&self) -> u64 {
        self.value >> COUNTER_BITS
    }

    pub fn timestamp(&self) -> Timestamp {
        Timestamp {
            millis: self.millis(),
        }
    }

    fn counter(&self) -> u16 {
        self.value as u16
    }

    fn bump(&self) -> Result<Self, HlcTimestampOverflow> {
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

impl From<HlcTimestamp> for u64 {
    fn from(value: HlcTimestamp) -> Self {
        value.value
    }
}

impl From<HlcTimestamp> for i64 {
    fn from(value: HlcTimestamp) -> Self {
        // the constructors for Timestamp ensures it is convertible to i64
        value.value as i64
    }
}

impl TryFrom<u64> for HlcTimestamp {
    type Error = HlcTimestampOverflow;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value > i64::MAX as u64 {
            return Err(HlcTimestampOverflow);
        }
        Ok(Self { value })
    }
}

impl TryFrom<i64> for HlcTimestamp {
    type Error = HlcTimestampOverflow;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            // in this case we use the same error for negative
            return Err(HlcTimestampOverflow);
        }
        Ok(Self {
            value: value as u64,
        })
    }
}

impl Timestamp {
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        Timestamp {
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
