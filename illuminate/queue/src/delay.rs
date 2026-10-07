//! Delays: "how long until this job may run?"
//!
//! Anywhere the queue asks for a delay you may pass a number of seconds, a
//! [`Duration`], a [`CarbonInterval`], or the [`Carbon`] moment the job
//! should become available:
//!
//! ```
//! use std::time::Duration;
//! use illuminate_queue::IntoDelay;
//!
//! assert_eq!(10.into_delay(), Duration::from_secs(10));
//! assert_eq!(Duration::from_millis(250).into_delay(), Duration::from_millis(250));
//! assert_eq!((-5).into_delay(), Duration::ZERO);
//! ```

use std::time::Duration;

use illuminate_support::{Carbon, CarbonInterval};

/// Anything that can be turned into a delay.
pub trait IntoDelay {
    /// The delay, relative to now.
    fn into_delay(self) -> Duration;
}

macro_rules! delay_from_unsigned {
    ($($type:ty),*) => {
        $(
            impl IntoDelay for $type {
                fn into_delay(self) -> Duration {
                    Duration::from_secs(self as u64)
                }
            }
        )*
    };
}

macro_rules! delay_from_signed {
    ($($type:ty),*) => {
        $(
            impl IntoDelay for $type {
                fn into_delay(self) -> Duration {
                    Duration::from_secs(self.max(0) as u64)
                }
            }
        )*
    };
}

delay_from_unsigned!(u8, u16, u32, u64, usize);
delay_from_signed!(i8, i16, i32, i64, isize);

impl IntoDelay for f64 {
    fn into_delay(self) -> Duration {
        Duration::try_from_secs_f64(self.max(0.0)).unwrap_or(Duration::ZERO)
    }
}

impl IntoDelay for Duration {
    fn into_delay(self) -> Duration {
        self
    }
}

impl IntoDelay for CarbonInterval {
    fn into_delay(self) -> Duration {
        let now = Carbon::now();
        until(now, now.add(self))
    }
}

impl IntoDelay for Carbon {
    fn into_delay(self) -> Duration {
        until(Carbon::now(), self)
    }
}

impl<T: IntoDelay> IntoDelay for Option<T> {
    fn into_delay(self) -> Duration {
        self.map(IntoDelay::into_delay).unwrap_or(Duration::ZERO)
    }
}

fn until(now: Carbon, at: Carbon) -> Duration {
    let milliseconds = at.timestamp_millis() - now.timestamp_millis();
    Duration::from_millis(milliseconds.max(0) as u64)
}

/// Whole seconds, rounded up (`0.2s` is `1`), the way payloads record delays.
pub(crate) fn ceil_seconds(duration: Duration) -> u64 {
    let seconds = duration.as_secs();
    if duration.subsec_nanos() > 0 {
        seconds + 1
    } else {
        seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_convert() {
        assert_eq!(30u64.into_delay(), Duration::from_secs(30));
        assert_eq!(0.5f64.into_delay(), Duration::from_millis(500));
        assert_eq!(Some(3).into_delay(), Duration::from_secs(3));
        assert_eq!(None::<u64>.into_delay(), Duration::ZERO);
        assert_eq!(
            CarbonInterval::minutes(2).into_delay(),
            Duration::from_secs(120)
        );
        let soon = Carbon::now().add_seconds(60).into_delay();
        assert!(soon > Duration::from_secs(58) && soon <= Duration::from_secs(60));
        assert_eq!(Carbon::now().sub_seconds(60).into_delay(), Duration::ZERO);
    }

    #[test]
    fn seconds_round_up() {
        assert_eq!(ceil_seconds(Duration::from_millis(200)), 1);
        assert_eq!(ceil_seconds(Duration::from_secs(2)), 2);
        assert_eq!(ceil_seconds(Duration::ZERO), 0);
    }
}
