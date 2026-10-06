//! How long an item should live in the cache.

use illuminate_support::{Carbon, CarbonInterval};

/// A cache lifetime: a number of seconds, an interval, an absolute moment,
/// or forever.
///
/// Anything Laravel accepts as a TTL converts into one:
///
/// ```
/// use std::time::Duration;
/// use illuminate_cache::Ttl;
/// use illuminate_support::{Carbon, CarbonInterval};
///
/// assert_eq!(Ttl::from(600).to_seconds(), Some(600));
/// assert_eq!(Ttl::from(Duration::from_secs(90)).to_seconds(), Some(90));
/// assert_eq!(Ttl::from(CarbonInterval::minutes(10)).to_seconds(), Some(600));
/// assert_eq!(Ttl::from(Carbon::now().sub_minutes(1)).to_seconds(), Some(0));
/// assert_eq!(Ttl::from(None::<i64>).to_seconds(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ttl {
    /// A number of seconds from now (zero or negative means "already expired").
    Seconds(i64),
    /// An interval from now.
    Interval(CarbonInterval),
    /// An absolute expiration time.
    At(Carbon),
    /// Never expire.
    Forever,
}

impl Ttl {
    /// The number of seconds the item should live, or `None` for forever.
    ///
    /// Like Laravel, expiration times in the past become `0` — and storing
    /// an item for zero seconds removes it from the cache.
    pub fn to_seconds(&self) -> Option<i64> {
        let seconds = match self {
            Ttl::Forever => return None,
            Ttl::Seconds(seconds) => *seconds,
            Ttl::Interval(interval) => {
                let now = Carbon::now();
                seconds_until(now, now.add(*interval))
            }
            Ttl::At(at) => seconds_until(Carbon::now(), *at),
        };
        Some(seconds.max(0))
    }
}

fn seconds_until(now: Carbon, at: Carbon) -> i64 {
    let milliseconds = at.timestamp_millis() - now.timestamp_millis();
    (milliseconds as f64 / 1000.0).ceil() as i64
}

macro_rules! ttl_from_integer {
    ($($type:ty),*) => {
        $(
            impl From<$type> for Ttl {
                fn from(seconds: $type) -> Self {
                    Ttl::Seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
                }
            }
        )*
    };
}

ttl_from_integer!(i32, i64, u32, u64, usize);

impl From<std::time::Duration> for Ttl {
    fn from(duration: std::time::Duration) -> Self {
        Ttl::Seconds(duration.as_secs_f64().ceil() as i64)
    }
}

impl From<CarbonInterval> for Ttl {
    fn from(interval: CarbonInterval) -> Self {
        Ttl::Interval(interval)
    }
}

impl From<Carbon> for Ttl {
    fn from(at: Carbon) -> Self {
        Ttl::At(at)
    }
}

impl<T: Into<Ttl>> From<Option<T>> for Ttl {
    fn from(ttl: Option<T>) -> Self {
        ttl.map(Into::into).unwrap_or(Ttl::Forever)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::freeze_time;

    #[test]
    fn ttls_convert_to_seconds() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        assert_eq!(Ttl::from(-5).to_seconds(), Some(0));
        assert_eq!(Ttl::from(0u64).to_seconds(), Some(0));
        assert_eq!(
            Ttl::from(std::time::Duration::from_millis(1500)).to_seconds(),
            Some(2)
        );
        assert_eq!(
            Ttl::from(Carbon::from_timestamp(1_700_000_060)).to_seconds(),
            Some(60)
        );
        assert_eq!(
            Ttl::from(Carbon::from_timestamp(1_600_000_000)).to_seconds(),
            Some(0)
        );
        assert_eq!(Ttl::from(CarbonInterval::hours(2)).to_seconds(), Some(7200));
        assert_eq!(Ttl::from(Some(30)).to_seconds(), Some(30));
        assert_eq!(Ttl::Forever.to_seconds(), None);
    }
}
