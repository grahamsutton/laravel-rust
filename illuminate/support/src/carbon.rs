//! Carbon: a simple, expressive date and time type.
//!
//! ```
//! use illuminate_support::Carbon;
//!
//! let date = Carbon::parse("2024-03-12 15:30:00").unwrap();
//!
//! assert_eq!(date.format("l, F jS Y"), "Tuesday, March 12th 2024");
//! assert_eq!(date.add_days(1).to_date_string(), "2024-03-13");
//! ```

use std::cell::Cell;
use std::fmt;
use std::sync::{LazyLock, RwLock};

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Utc,
};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

static DEFAULT_TIMEZONE: LazyLock<RwLock<Tz>> = LazyLock::new(|| RwLock::new(Tz::UTC));
static TEST_NOW: LazyLock<RwLock<Option<Carbon>>> = LazyLock::new(|| RwLock::new(None));

thread_local! {
    static STORAGE_FORMAT: Cell<bool> = const { Cell::new(false) };
    static THREAD_TEST_NOW: Cell<Option<Carbon>> = const { Cell::new(None) };
}

mod period;
mod relative;

pub use chrono::Weekday;
pub use period::{CarbonPeriod, CarbonPeriodIter};

/// A date and time, with a timezone, and a delightful API.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Carbon {
    inner: DateTime<Tz>,
}

impl Carbon {
    // ------------------------------------------------------------------
    // Creation
    // ------------------------------------------------------------------

    /// Wrap a `chrono` date time.
    pub fn from_datetime(inner: DateTime<Tz>) -> Self {
        Self { inner }
    }

    /// Get the current date and time (respecting `set_test_now`).
    pub fn now() -> Self {
        if let Some(now) = THREAD_TEST_NOW.with(|cell| cell.get()) {
            return now;
        }
        if let Some(now) = *TEST_NOW.read().unwrap() {
            return now;
        }
        Self {
            inner: Utc::now().with_timezone(&Self::default_timezone()),
        }
    }

    /// Today at midnight.
    pub fn today() -> Self {
        Self::now().start_of_day()
    }

    /// Tomorrow at midnight.
    pub fn tomorrow() -> Self {
        Self::today().add_days(1)
    }

    /// Yesterday at midnight.
    pub fn yesterday() -> Self {
        Self::today().sub_days(1)
    }

    /// Create a date from its components in the default timezone.
    pub fn create(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Option<Self> {
        let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
        Self::from_naive(naive, Self::default_timezone())
    }

    /// Create a date (at midnight) from its components.
    pub fn create_from_date(year: i32, month: u32, day: u32) -> Option<Self> {
        Self::create(year, month, day, 0, 0, 0)
    }

    /// Create a date from a Unix timestamp.
    pub fn from_timestamp(timestamp: i64) -> Self {
        let utc = DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default();
        Self {
            inner: utc.with_timezone(&Self::default_timezone()),
        }
    }

    /// Create a date from a Unix timestamp in milliseconds.
    pub fn from_timestamp_millis(millis: i64) -> Self {
        let utc = DateTime::<Utc>::from_timestamp_millis(millis).unwrap_or_default();
        Self {
            inner: utc.with_timezone(&Self::default_timezone()),
        }
    }

    fn from_naive(naive: NaiveDateTime, tz: Tz) -> Option<Self> {
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(dt) => Some(Self { inner: dt }),
            LocalResult::Ambiguous(dt, _) => Some(Self { inner: dt }),
            LocalResult::None => None,
        }
    }

    /// Parse a date string in any common format.
    ///
    /// Understands ISO 8601 / RFC 3339, RFC 2822, `Y-m-d H:i:s`, `Y-m-d`,
    /// Unix timestamps, and PHP-style relative expressions: `now`, `today`,
    /// `tomorrow 10:00`, `+1 day`, `-2 weeks`, `3 days ago`, `next monday`,
    /// `last friday`, `first day of next month`, `last day of this month`,
    /// `midnight`, `noon`, `3pm`, ...
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let now = Carbon::parse("2024-03-12 15:30:00").unwrap(); // a Tuesday
    /// Carbon::with_test_now(now, || {
    ///     assert_eq!(Carbon::parse("+1 day").unwrap().to_date_time_string(), "2024-03-13 15:30:00");
    ///     assert_eq!(Carbon::parse("next monday").unwrap().to_date_time_string(), "2024-03-18 00:00:00");
    ///     assert_eq!(Carbon::parse("tomorrow 10:00").unwrap().to_date_time_string(), "2024-03-13 10:00:00");
    ///     assert_eq!(Carbon::parse("first day of next month").unwrap().to_date_string(), "2024-04-01");
    /// });
    /// ```
    pub fn parse(value: &str) -> crate::Result<Self> {
        let value = value.trim();
        let tz = Self::default_timezone();

        match value.to_ascii_lowercase().as_str() {
            "now" | "" => return Ok(Self::now()),
            "today" => return Ok(Self::today()),
            "tomorrow" => return Ok(Self::tomorrow()),
            "yesterday" => return Ok(Self::yesterday()),
            _ => {}
        }

        if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
            return Ok(Self {
                inner: dt.with_timezone(&tz),
            });
        }
        if let Ok(dt) = DateTime::parse_from_rfc2822(value) {
            return Ok(Self {
                inner: dt.with_timezone(&tz),
            });
        }
        for format in [
            "%Y-%m-%dT%H:%M:%S%.f%z",
            "%Y-%m-%d %H:%M:%S%.f%z",
            "%Y-%m-%d %H:%M:%S%z",
            "%Y-%m-%d %H:%M:%S %z",
        ] {
            if let Ok(dt) = DateTime::parse_from_str(value, format) {
                return Ok(Self {
                    inner: dt.with_timezone(&tz),
                });
            }
        }
        for format in [
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M",
            "%Y/%m/%d %H:%M:%S",
        ] {
            if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
                if let Some(c) = Self::from_naive(naive, tz) {
                    return Ok(c);
                }
            }
        }
        for format in ["%Y-%m-%d", "%Y/%m/%d", "%m/%d/%Y", "%d-%m-%Y", "%B %d, %Y", "%b %d, %Y", "%d %B %Y"] {
            if let Ok(date) = NaiveDate::parse_from_str(value, format) {
                if let Some(c) = Self::from_naive(date.and_time(NaiveTime::MIN), tz) {
                    return Ok(c);
                }
            }
        }
        if let Ok(ts) = value.parse::<i64>() {
            return Ok(Self::from_timestamp(ts));
        }
        if let Some(date) = relative::parse(value, Self::now()) {
            return Ok(date);
        }

        Err(InvalidFormatException {
            value: value.to_string(),
        }
        .into())
    }

    /// Create a date from a specific PHP-style format (`Y-m-d H:i:s`).
    pub fn create_from_format(format: &str, value: &str) -> crate::Result<Self> {
        let chrono_format = php_format_to_strftime(format);
        let tz = Self::default_timezone();
        if let Ok(dt) = DateTime::parse_from_str(value, &chrono_format) {
            return Ok(Self {
                inner: dt.with_timezone(&tz),
            });
        }
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, &chrono_format) {
            if let Some(c) = Self::from_naive(naive, tz) {
                return Ok(c);
            }
        }
        if let Ok(date) = NaiveDate::parse_from_str(value, &chrono_format) {
            if let Some(c) = Self::from_naive(date.and_time(NaiveTime::MIN), tz) {
                return Ok(c);
            }
        }
        Err(InvalidFormatException {
            value: value.to_string(),
        }
        .into())
    }

    // ------------------------------------------------------------------
    // Global configuration
    // ------------------------------------------------------------------

    /// Set the default timezone used when creating dates.
    pub fn set_default_timezone(name: &str) -> crate::Result<()> {
        let tz: Tz = name
            .parse()
            .map_err(|_| crate::error::InvalidArgumentException::new(format!("Unknown timezone [{name}].")))?;
        *DEFAULT_TIMEZONE.write().unwrap() = tz;
        Ok(())
    }

    /// Get the default timezone.
    pub fn default_timezone() -> Tz {
        *DEFAULT_TIMEZONE.read().unwrap()
    }

    /// Freeze "now" at the given moment, which is wonderful for testing.
    pub fn set_test_now(now: Option<Carbon>) {
        *TEST_NOW.write().unwrap() = now;
    }

    /// Determine if a test "now" is set.
    pub fn has_test_now() -> bool {
        TEST_NOW.read().unwrap().is_some()
    }

    /// Run the callback while serializing dates in storage format
    /// (`Y-m-d H:i:s`) rather than ISO-8601. Used when persisting models.
    pub fn with_storage_format<R>(callback: impl FnOnce() -> R) -> R {
        let previous = STORAGE_FORMAT.with(|f| f.replace(true));
        let result = callback();
        STORAGE_FORMAT.with(|f| f.set(previous));
        result
    }

    // ------------------------------------------------------------------
    // Getters
    // ------------------------------------------------------------------

    /// Get the underlying `chrono` date time.
    pub fn inner(&self) -> &DateTime<Tz> {
        &self.inner
    }

    pub fn year(&self) -> i32 {
        self.inner.year()
    }

    pub fn month(&self) -> u32 {
        self.inner.month()
    }

    pub fn day(&self) -> u32 {
        self.inner.day()
    }

    pub fn hour(&self) -> u32 {
        self.inner.hour()
    }

    pub fn minute(&self) -> u32 {
        self.inner.minute()
    }

    pub fn second(&self) -> u32 {
        self.inner.second()
    }

    pub fn micro(&self) -> u32 {
        self.inner.nanosecond() / 1000
    }

    /// Day of the week, 0 (Sunday) through 6 (Saturday).
    pub fn day_of_week(&self) -> u32 {
        self.inner.weekday().num_days_from_sunday()
    }

    /// Day of the year, 1-based.
    pub fn day_of_year(&self) -> u32 {
        self.inner.ordinal()
    }

    pub fn days_in_month(&self) -> u32 {
        days_in_month(self.year(), self.month())
    }

    /// The Unix timestamp.
    pub fn timestamp(&self) -> i64 {
        self.inner.timestamp()
    }

    pub fn timestamp_millis(&self) -> i64 {
        self.inner.timestamp_millis()
    }

    /// The name of the timezone.
    pub fn timezone_name(&self) -> String {
        self.inner.timezone().name().to_string()
    }

    // ------------------------------------------------------------------
    // Formatting
    // ------------------------------------------------------------------

    /// Format the date using PHP's `date()` format characters.
    pub fn format(&self, format: &str) -> String {
        format_php(&self.inner, format)
    }

    /// `2024-03-12 15:30:00`
    pub fn to_date_time_string(&self) -> String {
        self.format("Y-m-d H:i:s")
    }

    /// `2024-03-12`
    pub fn to_date_string(&self) -> String {
        self.format("Y-m-d")
    }

    /// `15:30:00`
    pub fn to_time_string(&self) -> String {
        self.format("H:i:s")
    }

    /// `Mar 12, 2024`
    pub fn to_formatted_date_string(&self) -> String {
        self.format("M j, Y")
    }

    /// `Tue, Mar 12, 2024 3:30 PM`
    pub fn to_day_date_time_string(&self) -> String {
        self.format("D, M j, Y g:i A")
    }

    /// `2024-03-12T15:30:00+00:00`
    pub fn to_iso8601_string(&self) -> String {
        self.format("c")
    }

    /// `2024-03-12T15:30:00.000000Z` — the format used when serializing to JSON.
    pub fn to_json(&self) -> String {
        self.inner
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string()
    }

    /// `Tue, 12 Mar 2024 15:30:00 +0000`
    pub fn to_rfc2822_string(&self) -> String {
        self.inner.to_rfc2822()
    }

    /// `Tue, 12 Mar 2024 15:30:00 GMT` (cookies and HTTP headers)
    pub fn to_cookie_string(&self) -> String {
        self.inner
            .with_timezone(&Utc)
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string()
    }

    // ------------------------------------------------------------------
    // Manipulation (always returns a new instance)
    // ------------------------------------------------------------------

    fn shift(self, duration: Duration) -> Self {
        Self {
            inner: self.inner + duration,
        }
    }

    pub fn add_seconds(self, n: i64) -> Self {
        self.shift(Duration::seconds(n))
    }

    pub fn sub_seconds(self, n: i64) -> Self {
        self.shift(Duration::seconds(-n))
    }

    pub fn add_second(self) -> Self {
        self.add_seconds(1)
    }

    pub fn add_minutes(self, n: i64) -> Self {
        self.shift(Duration::minutes(n))
    }

    pub fn sub_minutes(self, n: i64) -> Self {
        self.shift(Duration::minutes(-n))
    }

    pub fn add_minute(self) -> Self {
        self.add_minutes(1)
    }

    pub fn sub_minute(self) -> Self {
        self.sub_minutes(1)
    }

    pub fn add_hours(self, n: i64) -> Self {
        self.shift(Duration::hours(n))
    }

    pub fn sub_hours(self, n: i64) -> Self {
        self.shift(Duration::hours(-n))
    }

    pub fn add_hour(self) -> Self {
        self.add_hours(1)
    }

    pub fn sub_hour(self) -> Self {
        self.sub_hours(1)
    }

    pub fn add_days(self, n: i64) -> Self {
        self.shift(Duration::days(n))
    }

    pub fn sub_days(self, n: i64) -> Self {
        self.shift(Duration::days(-n))
    }

    pub fn add_day(self) -> Self {
        self.add_days(1)
    }

    pub fn sub_day(self) -> Self {
        self.sub_days(1)
    }

    pub fn add_weeks(self, n: i64) -> Self {
        self.shift(Duration::weeks(n))
    }

    pub fn sub_weeks(self, n: i64) -> Self {
        self.shift(Duration::weeks(-n))
    }

    pub fn add_week(self) -> Self {
        self.add_weeks(1)
    }

    pub fn sub_week(self) -> Self {
        self.sub_weeks(1)
    }

    /// Add months, without overflowing into the following month
    /// (Jan 31 + 1 month = Feb 29 in a leap year).
    pub fn add_months(self, n: i64) -> Self {
        let total = self.year() as i64 * 12 + (self.month() as i64 - 1) + n;
        let year = total.div_euclid(12) as i32;
        let month = (total.rem_euclid(12) + 1) as u32;
        let day = self.day().min(days_in_month(year, month));
        self.set_date(year, month, day)
    }

    pub fn sub_months(self, n: i64) -> Self {
        self.add_months(-n)
    }

    pub fn add_month(self) -> Self {
        self.add_months(1)
    }

    pub fn sub_month(self) -> Self {
        self.sub_months(1)
    }

    pub fn add_years(self, n: i64) -> Self {
        self.add_months(n * 12)
    }

    pub fn sub_years(self, n: i64) -> Self {
        self.add_months(-n * 12)
    }

    pub fn add_year(self) -> Self {
        self.add_years(1)
    }

    pub fn sub_year(self) -> Self {
        self.sub_years(1)
    }

    /// Add a [`CarbonInterval`] / duration.
    pub fn add(self, interval: impl Into<CarbonInterval>) -> Self {
        let interval = interval.into();
        self.add_months(interval.months).shift(interval.duration)
    }

    /// Subtract a [`CarbonInterval`] / duration.
    pub fn sub(self, interval: impl Into<CarbonInterval>) -> Self {
        let interval = interval.into();
        self.add_months(-interval.months).shift(-interval.duration)
    }

    /// Set the date portion, keeping the time.
    pub fn set_date(self, year: i32, month: u32, day: u32) -> Self {
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .map(|d| d.and_time(self.inner.time()))
            .unwrap_or_else(|| self.inner.naive_local());
        Self::from_naive(naive, self.inner.timezone()).unwrap_or(self)
    }

    /// Set the time portion, keeping the date.
    pub fn set_time(self, hour: u32, minute: u32, second: u32) -> Self {
        let naive = self
            .inner
            .date_naive()
            .and_hms_opt(hour, minute, second)
            .unwrap_or_else(|| self.inner.naive_local());
        Self::from_naive(naive, self.inner.timezone()).unwrap_or(self)
    }

    pub fn start_of_day(self) -> Self {
        self.set_time(0, 0, 0)
    }

    pub fn end_of_day(self) -> Self {
        let naive = self
            .inner
            .date_naive()
            .and_hms_micro_opt(23, 59, 59, 999_999)
            .unwrap();
        Self::from_naive(naive, self.inner.timezone()).unwrap_or(self)
    }

    pub fn start_of_month(self) -> Self {
        self.set_date(self.year(), self.month(), 1).start_of_day()
    }

    pub fn end_of_month(self) -> Self {
        self.set_date(self.year(), self.month(), self.days_in_month())
            .end_of_day()
    }

    pub fn start_of_year(self) -> Self {
        self.set_date(self.year(), 1, 1).start_of_day()
    }

    pub fn end_of_year(self) -> Self {
        self.set_date(self.year(), 12, 31).end_of_day()
    }

    /// Start of the week (Monday, like Carbon).
    pub fn start_of_week(self) -> Self {
        let offset = self.inner.weekday().num_days_from_monday() as i64;
        self.sub_days(offset).start_of_day()
    }

    /// End of the week (Sunday).
    pub fn end_of_week(self) -> Self {
        let offset = 6 - self.inner.weekday().num_days_from_monday() as i64;
        self.add_days(offset).end_of_day()
    }

    pub fn start_of_hour(self) -> Self {
        self.set_time(self.hour(), 0, 0)
    }

    pub fn start_of_minute(self) -> Self {
        self.set_time(self.hour(), self.minute(), 0)
    }

    /// Convert the date into the given timezone.
    pub fn tz(self, name: &str) -> crate::Result<Self> {
        let tz: Tz = name
            .parse()
            .map_err(|_| crate::error::InvalidArgumentException::new(format!("Unknown timezone [{name}].")))?;
        Ok(Self {
            inner: self.inner.with_timezone(&tz),
        })
    }

    /// Convert the date into UTC.
    pub fn utc(self) -> Self {
        Self {
            inner: self.inner.with_timezone(&Tz::UTC),
        }
    }

    // ------------------------------------------------------------------
    // Comparison
    // ------------------------------------------------------------------

    pub fn eq(&self, other: &Carbon) -> bool {
        self.inner == other.inner
    }

    pub fn gt(&self, other: &Carbon) -> bool {
        self.inner > other.inner
    }

    pub fn gte(&self, other: &Carbon) -> bool {
        self.inner >= other.inner
    }

    pub fn lt(&self, other: &Carbon) -> bool {
        self.inner < other.inner
    }

    pub fn lte(&self, other: &Carbon) -> bool {
        self.inner <= other.inner
    }

    pub fn is_after(&self, other: &Carbon) -> bool {
        self.gt(other)
    }

    pub fn is_before(&self, other: &Carbon) -> bool {
        self.lt(other)
    }

    pub fn between(&self, a: &Carbon, b: &Carbon) -> bool {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self >= lo && self <= hi
    }

    pub fn is_past(&self) -> bool {
        *self < Self::now()
    }

    pub fn is_future(&self) -> bool {
        *self > Self::now()
    }

    pub fn is_same_day(&self, other: &Carbon) -> bool {
        self.inner.date_naive() == other.inner.with_timezone(&self.inner.timezone()).date_naive()
    }

    pub fn is_today(&self) -> bool {
        self.is_same_day(&Self::now())
    }

    pub fn is_tomorrow(&self) -> bool {
        self.is_same_day(&Self::now().add_day())
    }

    pub fn is_yesterday(&self) -> bool {
        self.is_same_day(&Self::now().sub_day())
    }

    pub fn is_weekend(&self) -> bool {
        matches!(self.inner.weekday(), Weekday::Sat | Weekday::Sun)
    }

    pub fn is_weekday(&self) -> bool {
        !self.is_weekend()
    }

    pub fn is_leap_year(&self) -> bool {
        is_leap_year(self.year())
    }

    // ------------------------------------------------------------------
    // Differences
    // ------------------------------------------------------------------

    /// Signed number of seconds from `self` to `other` (positive when other is later).
    pub fn diff_in_seconds(&self, other: &Carbon) -> i64 {
        (other.inner - self.inner).num_seconds()
    }

    pub fn diff_in_minutes(&self, other: &Carbon) -> i64 {
        (other.inner - self.inner).num_minutes()
    }

    pub fn diff_in_hours(&self, other: &Carbon) -> i64 {
        (other.inner - self.inner).num_hours()
    }

    pub fn diff_in_days(&self, other: &Carbon) -> i64 {
        (other.inner - self.inner).num_days()
    }

    pub fn diff_in_weeks(&self, other: &Carbon) -> i64 {
        (other.inner - self.inner).num_weeks()
    }

    /// Whole months between the two dates.
    pub fn diff_in_months(&self, other: &Carbon) -> i64 {
        let (start, end, sign) = if self <= other {
            (self, other, 1)
        } else {
            (other, self, -1)
        };
        let mut months = (end.year() as i64 - start.year() as i64) * 12
            + (end.month() as i64 - start.month() as i64);
        if start.add_months(months) > *end {
            months -= 1;
        }
        months * sign
    }

    pub fn diff_in_years(&self, other: &Carbon) -> i64 {
        self.diff_in_months(other) / 12
    }

    /// A human readable difference from now, like "3 minutes ago" or "2 days from now".
    pub fn diff_for_humans(&self) -> String {
        self.humanized_difference(&Self::now(), true)
    }

    /// A human readable difference relative to another date ("before" / "after").
    pub fn diff_for_humans_from(&self, other: &Carbon) -> String {
        self.humanized_difference(other, false)
    }

    fn humanized_difference(&self, other: &Carbon, relative_to_now: bool) -> String {
        let seconds = other.diff_in_seconds(self);
        let phrase = humanize_seconds(seconds.abs(), self, other);
        match (seconds < 0, relative_to_now) {
            (true, true) => format!("{phrase} ago"),
            (false, true) => format!("{phrase} from now"),
            (true, false) => format!("{phrase} before"),
            (false, false) => format!("{phrase} after"),
        }
    }
}

fn humanize_seconds(seconds: i64, a: &Carbon, b: &Carbon) -> String {
    let unit = |n: i64, word: &str| {
        if n == 1 {
            format!("1 {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    if seconds < 60 {
        unit(seconds.max(1), "second")
    } else if seconds < 3600 {
        unit(seconds / 60, "minute")
    } else if seconds < 86_400 {
        unit(seconds / 3600, "hour")
    } else if seconds < 7 * 86_400 {
        unit(seconds / 86_400, "day")
    } else {
        let months = a.diff_in_months(b).abs();
        if months == 0 {
            unit(seconds / (7 * 86_400), "week")
        } else if months < 12 {
            unit(months, "month")
        } else {
            unit(months / 12, "year")
        }
    }
}

impl Default for Carbon {
    /// The default date is "now".
    fn default() -> Self {
        Self::now()
    }
}

impl fmt::Display for Carbon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_date_time_string())
    }
}

impl fmt::Debug for Carbon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Carbon({} {})", self.format("Y-m-d H:i:s.u"), self.timezone_name())
    }
}

impl std::str::FromStr for Carbon {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Carbon::parse(s)
    }
}

impl Serialize for Carbon {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if STORAGE_FORMAT.with(|f| f.get()) {
            serializer.serialize_str(&self.to_date_time_string())
        } else {
            serializer.serialize_str(&self.to_json())
        }
    }
}

impl<'de> Deserialize<'de> for Carbon {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Carbon;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a date string or timestamp")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Carbon, E> {
                Carbon::parse(v).map_err(E::custom)
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Carbon, E> {
                Ok(Carbon::from_timestamp(v))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Carbon, E> {
                Ok(Carbon::from_timestamp(v as i64))
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Carbon, E> {
                Ok(Carbon::from_timestamp(v as i64))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl From<Carbon> for serde_json::Value {
    fn from(date: Carbon) -> Self {
        serde_json::Value::String(if STORAGE_FORMAT.with(|f| f.get()) {
            date.to_date_time_string()
        } else {
            date.to_json()
        })
    }
}

impl From<DateTime<Utc>> for Carbon {
    fn from(dt: DateTime<Utc>) -> Self {
        Self {
            inner: dt.with_timezone(&Carbon::default_timezone()),
        }
    }
}

impl From<Carbon> for DateTime<Utc> {
    fn from(c: Carbon) -> Self {
        c.inner.with_timezone(&Utc)
    }
}

impl std::ops::Add<CarbonInterval> for Carbon {
    type Output = Carbon;

    fn add(self, rhs: CarbonInterval) -> Carbon {
        Carbon::add(self, rhs)
    }
}

impl std::ops::Sub<CarbonInterval> for Carbon {
    type Output = Carbon;

    fn sub(self, rhs: CarbonInterval) -> Carbon {
        Carbon::sub(self, rhs)
    }
}

/// Thrown when a date string cannot be parsed.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Could not parse '{value}': Failed to parse time string.")]
pub struct InvalidFormatException {
    pub value: String,
}

/// A span of time: some months plus an exact duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CarbonInterval {
    pub months: i64,
    pub duration: Duration,
}

impl CarbonInterval {
    pub fn seconds(n: i64) -> Self {
        Self { months: 0, duration: Duration::seconds(n) }
    }

    pub fn minutes(n: i64) -> Self {
        Self { months: 0, duration: Duration::minutes(n) }
    }

    pub fn hours(n: i64) -> Self {
        Self { months: 0, duration: Duration::hours(n) }
    }

    pub fn days(n: i64) -> Self {
        Self { months: 0, duration: Duration::days(n) }
    }

    pub fn weeks(n: i64) -> Self {
        Self { months: 0, duration: Duration::weeks(n) }
    }

    pub fn months(n: i64) -> Self {
        Self { months: n, duration: Duration::zero() }
    }

    pub fn years(n: i64) -> Self {
        Self { months: n * 12, duration: Duration::zero() }
    }

    /// Approximate total seconds (months count as 30 days).
    pub fn total_seconds(&self) -> i64 {
        self.months * 30 * 86_400 + self.duration.num_seconds()
    }

    /// A human readable representation, like "2 hours 5 minutes".
    pub fn for_humans(&self) -> String {
        let mut parts = Vec::new();
        let mut push = |n: i64, word: &str| {
            if n != 0 {
                parts.push(if n.abs() == 1 { format!("{n} {word}") } else { format!("{n} {word}s") });
            }
        };
        push(self.months / 12, "year");
        push(self.months % 12, "month");
        let mut secs = self.duration.num_seconds();
        let weeks = secs / (7 * 86_400);
        secs -= weeks * 7 * 86_400;
        let days = secs / 86_400;
        secs -= days * 86_400;
        let hours = secs / 3600;
        secs -= hours * 3600;
        let minutes = secs / 60;
        secs -= minutes * 60;
        push(weeks, "week");
        push(days, "day");
        push(hours, "hour");
        push(minutes, "minute");
        push(secs, "second");
        if parts.is_empty() {
            "1 second".to_string()
        } else {
            parts.join(" ")
        }
    }
}

impl From<Duration> for CarbonInterval {
    fn from(duration: Duration) -> Self {
        Self { months: 0, duration }
    }
}

impl From<std::time::Duration> for CarbonInterval {
    fn from(duration: std::time::Duration) -> Self {
        Self {
            months: 0,
            duration: Duration::from_std(duration).unwrap_or_else(|_| Duration::zero()),
        }
    }
}

/// Options for [`Carbon::diff_for_humans_with`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffOptions {
    /// How the difference is phrased.
    pub syntax: DiffSyntax,
    /// Use short unit names ("3h" rather than "3 hours").
    pub short: bool,
    /// The number of units to display ("1 hour 30 minutes" is two parts).
    pub parts: usize,
    /// Join the final two parts with " and " instead of a space.
    pub join: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            syntax: DiffSyntax::Auto,
            short: false,
            parts: 1,
            join: false,
        }
    }
}

impl DiffOptions {
    /// The default options: automatic syntax, long units, one part.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use short unit names ("3h ago").
    pub fn short(mut self) -> Self {
        self.short = true;
        self
    }

    /// Display the given number of units.
    pub fn parts(mut self, parts: usize) -> Self {
        self.parts = parts.max(1);
        self
    }

    /// Join the final two parts with " and ".
    pub fn join(mut self) -> Self {
        self.join = true;
        self
    }

    /// Omit "ago" / "from now" / "before" / "after".
    pub fn absolute(mut self) -> Self {
        self.syntax = DiffSyntax::Absolute;
        self
    }

    /// Phrase the difference relative to now ("ago" / "from now").
    pub fn relative_to_now(mut self) -> Self {
        self.syntax = DiffSyntax::RelativeToNow;
        self
    }

    /// Phrase the difference relative to the other date ("before" / "after").
    pub fn relative_to_other(mut self) -> Self {
        self.syntax = DiffSyntax::RelativeToOther;
        self
    }
}

/// How a human readable difference is phrased.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffSyntax {
    /// "ago" / "from now" when comparing to now, "before" / "after" otherwise.
    #[default]
    Auto,
    /// Just the duration: "3 hours".
    Absolute,
    /// "3 hours ago" / "3 hours from now".
    RelativeToNow,
    /// "3 hours before" / "3 hours after".
    RelativeToOther,
}

impl Carbon {
    /// Monday, for use with [`Carbon::next`] and friends.
    pub const MONDAY: Weekday = Weekday::Mon;
    /// Tuesday.
    pub const TUESDAY: Weekday = Weekday::Tue;
    /// Wednesday.
    pub const WEDNESDAY: Weekday = Weekday::Wed;
    /// Thursday.
    pub const THURSDAY: Weekday = Weekday::Thu;
    /// Friday.
    pub const FRIDAY: Weekday = Weekday::Fri;
    /// Saturday.
    pub const SATURDAY: Weekday = Weekday::Sat;
    /// Sunday.
    pub const SUNDAY: Weekday = Weekday::Sun;

    // ------------------------------------------------------------------
    // More creation helpers
    // ------------------------------------------------------------------

    /// Get the current date and time in the given timezone.
    pub fn now_in(timezone: &str) -> crate::Result<Self> {
        Self::now().tz(timezone)
    }

    /// Create a date for today at the given time.
    pub fn create_from_time(hour: u32, minute: u32, second: u32) -> Option<Self> {
        let today = Self::today();
        Self::create(today.year(), today.month(), today.day(), hour, minute, second)
    }

    /// Alias of [`Carbon::from_timestamp`].
    pub fn create_from_timestamp(timestamp: i64) -> Self {
        Self::from_timestamp(timestamp)
    }

    /// Alias of [`Carbon::from_timestamp_millis`].
    pub fn create_from_timestamp_ms(millis: i64) -> Self {
        Self::from_timestamp_millis(millis)
    }

    /// Create a date from the time embedded in a ULID or a v1 / v6 / v7 UUID.
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let date = Carbon::create_from_id("01DXH9C4P0ED4AGJJP9CRKQ55C").unwrap();
    /// assert_eq!(date.utc().to_date_time_string(), "2020-01-01 19:30:00");
    /// ```
    pub fn create_from_id(id: &str) -> crate::Result<Self> {
        if crate::str::Str::is_ulid(id) {
            let ulid = ulid::Ulid::from_string(id)?;
            return Ok(Self::from_timestamp_millis(ulid.timestamp_ms() as i64));
        }
        let uuid = uuid::Uuid::parse_str(id)?;
        let timestamp = uuid.get_timestamp().ok_or_else(|| {
            crate::error::InvalidArgumentException::new(format!("The ID [{id}] does not contain a timestamp."))
        })?;
        let (seconds, nanos) = timestamp.to_unix();
        let utc = DateTime::<Utc>::from_timestamp(seconds as i64, nanos).unwrap_or_default();
        Ok(Self::from(utc))
    }

    /// Freeze "now" for the current thread only while the callback runs.
    /// Unlike [`Carbon::set_test_now`], this is safe in parallel tests.
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let frozen = Carbon::parse("2017-06-27 13:14:15").unwrap();
    /// Carbon::with_test_now(frozen, || {
    ///     assert_eq!(Carbon::now(), frozen);
    /// });
    /// ```
    pub fn with_test_now<R>(now: Carbon, callback: impl FnOnce() -> R) -> R {
        let previous = THREAD_TEST_NOW.with(|cell| cell.replace(Some(now)));
        let result = callback();
        THREAD_TEST_NOW.with(|cell| cell.set(previous));
        result
    }

    /// Freeze (or, with `None`, unfreeze) "now" for the current thread only.
    pub fn set_thread_test_now(now: Option<Carbon>) {
        THREAD_TEST_NOW.with(|cell| cell.set(now));
    }

    /// The "now" frozen for the current thread, if any.
    pub fn thread_test_now() -> Option<Carbon> {
        THREAD_TEST_NOW.with(|cell| cell.get())
    }

    /// Get a copy of the instance (Carbon values are immutable and `Copy`).
    pub fn copy(&self) -> Self {
        *self
    }

    // ------------------------------------------------------------------
    // More getters
    // ------------------------------------------------------------------

    /// The day of the week as a `chrono::Weekday`.
    pub fn weekday(&self) -> Weekday {
        self.inner.weekday()
    }

    /// ISO-8601 day of the week, 1 (Monday) through 7 (Sunday).
    pub fn day_of_week_iso(&self) -> u32 {
        self.inner.weekday().number_from_monday()
    }

    /// The quarter of the year (1 through 4).
    pub fn quarter(&self) -> u32 {
        (self.month() - 1) / 3 + 1
    }

    /// The ISO-8601 week of the year.
    pub fn week_of_year(&self) -> u32 {
        self.inner.iso_week().week()
    }

    /// The week of the month (1 through 5).
    pub fn week_of_month(&self) -> u32 {
        self.day().div_ceil(7)
    }

    /// The number of days in the year.
    pub fn days_in_year(&self) -> u32 {
        if self.is_leap_year() { 366 } else { 365 }
    }

    /// The milliseconds of the current second.
    pub fn millisecond(&self) -> u32 {
        self.inner.nanosecond() / 1_000_000
    }

    /// The timezone offset from UTC, in seconds.
    pub fn offset(&self) -> i32 {
        self.inner.offset().fix().local_minus_utc()
    }

    /// The timezone offset from UTC, in hours.
    pub fn offset_hours(&self) -> i32 {
        self.offset() / 3600
    }

    /// The name of the day ("Tuesday").
    pub fn day_name(&self) -> String {
        self.format("l")
    }

    /// The short name of the day ("Tue").
    pub fn short_day_name(&self) -> String {
        self.format("D")
    }

    /// The name of the month ("March").
    pub fn month_name(&self) -> String {
        self.format("F")
    }

    /// The short name of the month ("Mar").
    pub fn short_month_name(&self) -> String {
        self.format("M")
    }

    /// The age, in whole years, from this date until now.
    pub fn age(&self) -> i64 {
        self.diff_in_years(&Self::now()).abs()
    }

    // ------------------------------------------------------------------
    // Setters (overflowing like PHP's DateTime)
    // ------------------------------------------------------------------

    /// Set the year (Feb 29 overflows into March in non-leap years).
    pub fn set_year(self, year: i32) -> Self {
        self.set_date_overflowing(year as i64, self.month() as i64, self.day() as i64)
    }

    /// Set the month (overflowing into the next year past 12).
    pub fn set_month(self, month: u32) -> Self {
        self.set_date_overflowing(self.year() as i64, month as i64, self.day() as i64)
    }

    /// Set the day of the month (overflowing into the next month).
    pub fn set_day(self, day: u32) -> Self {
        self.set_date_overflowing(self.year() as i64, self.month() as i64, day as i64)
    }

    /// Set the hour (overflowing into the next day past 23).
    pub fn set_hour(self, hour: u32) -> Self {
        self.set_time_overflowing(hour as i64, self.minute() as i64, self.second() as i64)
    }

    /// Set the minute (overflowing into the next hour past 59).
    pub fn set_minute(self, minute: u32) -> Self {
        self.set_time_overflowing(self.hour() as i64, minute as i64, self.second() as i64)
    }

    /// Set the second (overflowing into the next minute past 59).
    pub fn set_second(self, second: u32) -> Self {
        self.set_time_overflowing(self.hour() as i64, self.minute() as i64, second as i64)
    }

    /// Set the microseconds of the current second.
    pub fn set_microsecond(self, microsecond: u32) -> Self {
        let naive = self.inner.naive_local().with_nanosecond(microsecond.min(999_999) * 1000);
        naive
            .and_then(|n| Self::from_naive(n, self.inner.timezone()))
            .unwrap_or(self)
    }

    /// Set the date and time at once.
    pub fn set_date_time(self, year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        self.set_date(year, month, day).set_time(hour, minute, second)
    }

    /// Convert the instance into the given timezone (alias of `tz`).
    pub fn set_timezone(self, name: &str) -> crate::Result<Self> {
        self.tz(name)
    }

    /// Keep the wall-clock time but change the timezone.
    pub fn shift_timezone(self, name: &str) -> crate::Result<Self> {
        let tz: Tz = name.parse().map_err(|_| {
            crate::error::InvalidArgumentException::new(format!("Unknown timezone [{name}]."))
        })?;
        Ok(Self::from_naive(self.inner.naive_local(), tz).unwrap_or(self))
    }

    fn set_date_overflowing(self, year: i64, month: i64, day: i64) -> Self {
        let total_months = year * 12 + (month - 1);
        let (y, m) = (total_months.div_euclid(12) as i32, (total_months.rem_euclid(12) + 1) as u32);
        let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else {
            return self;
        };
        let date = first + Duration::days(day - 1);
        Self::from_naive(date.and_time(self.inner.time()), self.inner.timezone()).unwrap_or(self)
    }

    fn set_time_overflowing(self, hour: i64, minute: i64, second: i64) -> Self {
        let midnight = self.inner.date_naive().and_time(NaiveTime::MIN);
        let naive = midnight
            + Duration::hours(hour)
            + Duration::minutes(minute)
            + Duration::seconds(second)
            + Duration::nanoseconds(self.inner.nanosecond() as i64);
        Self::from_naive(naive, self.inner.timezone()).unwrap_or(self)
    }

    // ------------------------------------------------------------------
    // More manipulation
    // ------------------------------------------------------------------

    /// Add a [`CarbonInterval`] (alias of `add`, mirroring Laravel's `plus`).
    pub fn plus(self, interval: impl Into<CarbonInterval>) -> Self {
        self.add(interval)
    }

    /// Subtract a [`CarbonInterval`] (alias of `sub`, mirroring Laravel's `minus`).
    pub fn minus(self, interval: impl Into<CarbonInterval>) -> Self {
        self.sub(interval)
    }

    /// Add the given amount of a unit (`"day"`, `"weeks"`, `"month"`, ...).
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let date = Carbon::parse("2024-01-01 10:00:00").unwrap();
    /// assert_eq!(date.add_unit("minutes", 14).unwrap().to_time_string(), "10:14:00");
    /// assert!(date.add_unit("fortnights", 1).is_ok());
    /// assert!(date.add_unit("lightyears", 1).is_err());
    /// ```
    pub fn add_unit(self, unit: &str, value: i64) -> crate::Result<Self> {
        let normalized = relative::unit(&unit.to_lowercase()).ok_or_else(|| {
            crate::error::InvalidArgumentException::new(format!("Unknown unit '{unit}'."))
        })?;
        Ok(match normalized {
            "microsecond" => self.shift(Duration::microseconds(value)),
            "millisecond" => self.shift(Duration::milliseconds(value)),
            "second" => self.add_seconds(value),
            "minute" => self.add_minutes(value),
            "hour" => self.add_hours(value),
            "day" => self.add_days(value),
            "weekday" => self.add_weekdays(value),
            "week" => self.add_weeks(value),
            "fortnight" => self.add_weeks(value * 2),
            "month" => self.add_months(value),
            "quarter" => self.add_months(value * 3),
            "year" => self.add_years(value),
            "decade" => self.add_years(value * 10),
            _ => self.add_years(value * 100),
        })
    }

    /// Subtract the given amount of a unit.
    pub fn sub_unit(self, unit: &str, value: i64) -> crate::Result<Self> {
        self.add_unit(unit, -value)
    }

    /// Add milliseconds.
    pub fn add_milliseconds(self, n: i64) -> Self {
        self.shift(Duration::milliseconds(n))
    }

    /// Subtract milliseconds.
    pub fn sub_milliseconds(self, n: i64) -> Self {
        self.shift(Duration::milliseconds(-n))
    }

    /// Add microseconds.
    pub fn add_microseconds(self, n: i64) -> Self {
        self.shift(Duration::microseconds(n))
    }

    /// Subtract microseconds.
    pub fn sub_microseconds(self, n: i64) -> Self {
        self.shift(Duration::microseconds(-n))
    }

    /// Add quarters (three months each).
    pub fn add_quarters(self, n: i64) -> Self {
        self.add_months(n * 3)
    }

    /// Subtract quarters.
    pub fn sub_quarters(self, n: i64) -> Self {
        self.add_months(-n * 3)
    }

    /// Add decades.
    pub fn add_decades(self, n: i64) -> Self {
        self.add_years(n * 10)
    }

    /// Subtract decades.
    pub fn sub_decades(self, n: i64) -> Self {
        self.add_years(-n * 10)
    }

    /// Add months, overflowing into the following month like PHP
    /// (Jan 31 + 1 month = Mar 2/3).
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let date = Carbon::parse("2026-01-31").unwrap();
    /// assert_eq!(date.add_months_with_overflow(1).to_date_string(), "2026-03-03");
    /// assert_eq!(date.add_months(1).to_date_string(), "2026-02-28");
    /// ```
    pub fn add_months_with_overflow(self, n: i64) -> Self {
        self.set_date_overflowing(self.year() as i64, self.month() as i64 + n, self.day() as i64)
    }

    /// Subtract months, overflowing like PHP.
    pub fn sub_months_with_overflow(self, n: i64) -> Self {
        self.add_months_with_overflow(-n)
    }

    /// Add the given number of weekdays (skipping weekends).
    pub fn add_weekdays(self, n: i64) -> Self {
        let step = if n >= 0 { 1 } else { -1 };
        let mut date = self;
        let mut remaining = n.abs();
        while remaining > 0 {
            date = date.add_days(step);
            if date.is_weekday() {
                remaining -= 1;
            }
        }
        date
    }

    /// Subtract the given number of weekdays.
    pub fn sub_weekdays(self, n: i64) -> Self {
        self.add_weekdays(-n)
    }

    /// Move to the next weekday (skipping weekends), keeping the time.
    pub fn next_weekday(self) -> Self {
        self.add_weekdays(1)
    }

    /// Move to the previous weekday, keeping the time.
    pub fn previous_weekday(self) -> Self {
        self.sub_weekdays(1)
    }

    /// Move to the start of the next occurrence of the given day of the week.
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let tuesday = Carbon::parse("2024-03-12 15:30:00").unwrap();
    /// assert_eq!(tuesday.next(Carbon::FRIDAY).to_date_time_string(), "2024-03-15 00:00:00");
    /// assert_eq!(tuesday.next(Carbon::TUESDAY).to_date_string(), "2024-03-19");
    /// assert_eq!(tuesday.previous(Carbon::MONDAY).to_date_string(), "2024-03-11");
    /// ```
    pub fn next(self, day: Weekday) -> Self {
        let current = self.inner.weekday().num_days_from_monday() as i64;
        let target = day.num_days_from_monday() as i64;
        let mut delta = (target - current).rem_euclid(7);
        if delta == 0 {
            delta = 7;
        }
        self.add_days(delta).start_of_day()
    }

    /// Move to the start of the previous occurrence of the given day of the week.
    pub fn previous(self, day: Weekday) -> Self {
        let current = self.inner.weekday().num_days_from_monday() as i64;
        let target = day.num_days_from_monday() as i64;
        let mut delta = (current - target).rem_euclid(7);
        if delta == 0 {
            delta = 7;
        }
        self.sub_days(delta).start_of_day()
    }

    /// The end of the hour (`HH:59:59.999999`).
    pub fn end_of_hour(self) -> Self {
        self.start_of_hour().add_hours(1).shift(-Duration::microseconds(1))
    }

    /// The end of the minute (`HH:MM:59.999999`).
    pub fn end_of_minute(self) -> Self {
        self.start_of_minute().add_minutes(1).shift(-Duration::microseconds(1))
    }

    /// The start of the second.
    pub fn start_of_second(self) -> Self {
        self.set_microsecond(0)
    }

    /// The start of the quarter.
    pub fn start_of_quarter(self) -> Self {
        let month = (self.quarter() - 1) * 3 + 1;
        self.set_date(self.year(), month, 1).start_of_day()
    }

    /// The end of the quarter.
    pub fn end_of_quarter(self) -> Self {
        let month = self.quarter() * 3;
        self.set_date(self.year(), month, days_in_month(self.year(), month))
            .end_of_day()
    }

    /// The start of the decade (`2020-01-01` for 2024).
    pub fn start_of_decade(self) -> Self {
        self.set_date(self.year() - self.year().rem_euclid(10), 1, 1).start_of_day()
    }

    /// The end of the decade (`2029-12-31` for 2024).
    pub fn end_of_decade(self) -> Self {
        self.set_date(self.year() - self.year().rem_euclid(10) + 9, 12, 31).end_of_day()
    }

    /// The start of the century (`2001-01-01` for 2024).
    pub fn start_of_century(self) -> Self {
        let year = self.year() - (self.year() - 1).rem_euclid(100);
        self.set_date(year, 1, 1).start_of_day()
    }

    /// The end of the century (`2100-12-31` for 2024).
    pub fn end_of_century(self) -> Self {
        let year = self.year() - (self.year() - 1).rem_euclid(100) + 99;
        self.set_date(year, 12, 31).end_of_day()
    }

    /// Round down to the start of the second.
    pub fn floor_second(self) -> Self {
        self.floor_unit("second", 1)
    }

    /// Round down to the start of the minute.
    pub fn floor_minute(self) -> Self {
        self.floor_unit("minute", 1)
    }

    /// Round down to the start of the hour.
    pub fn floor_hour(self) -> Self {
        self.floor_unit("hour", 1)
    }

    /// Round down to the start of the day.
    pub fn floor_day(self) -> Self {
        self.start_of_day()
    }

    /// Round up to the next second (unless already exact).
    pub fn ceil_second(self) -> Self {
        self.ceil_unit("second", 1)
    }

    /// Round up to the next minute (unless already exact).
    pub fn ceil_minute(self) -> Self {
        self.ceil_unit("minute", 1)
    }

    /// Round up to the next hour (unless already exact).
    pub fn ceil_hour(self) -> Self {
        self.ceil_unit("hour", 1)
    }

    /// Round up to the next day (unless already midnight).
    pub fn ceil_day(self) -> Self {
        let start = self.start_of_day();
        if start == self { self } else { start.add_day() }
    }

    /// Round to the nearest second.
    pub fn round_second(self) -> Self {
        self.round_unit("second", 1)
    }

    /// Round to the nearest minute.
    ///
    /// ```
    /// use illuminate_support::Carbon;
    ///
    /// let date = Carbon::parse("2024-03-12 15:30:31").unwrap();
    /// assert_eq!(date.round_minute().to_time_string(), "15:31:00");
    /// assert_eq!(date.floor_hour().to_time_string(), "15:00:00");
    /// assert_eq!(date.ceil_hour().to_time_string(), "16:00:00");
    /// assert_eq!(date.round_unit("minute", 15).to_time_string(), "15:30:00");
    /// ```
    pub fn round_minute(self) -> Self {
        self.round_unit("minute", 1)
    }

    /// Round to the nearest hour.
    pub fn round_hour(self) -> Self {
        self.round_unit("hour", 1)
    }

    /// Round to the nearest day.
    pub fn round_day(self) -> Self {
        let start = self.start_of_day();
        if self.hour() >= 12 { start.add_day() } else { start }
    }

    /// Round down to a multiple of `precision` units (`second`, `minute` or `hour`).
    pub fn floor_unit(self, unit: &str, precision: u32) -> Self {
        self.round_with(unit, precision, |value| value.floor())
    }

    /// Round up to a multiple of `precision` units (`second`, `minute` or `hour`).
    pub fn ceil_unit(self, unit: &str, precision: u32) -> Self {
        self.round_with(unit, precision, |value| value.ceil())
    }

    /// Round to the nearest multiple of `precision` units (`second`, `minute` or `hour`).
    pub fn round_unit(self, unit: &str, precision: u32) -> Self {
        self.round_with(unit, precision, |value| value.round())
    }

    fn round_with(self, unit: &str, precision: u32, round: impl Fn(f64) -> f64) -> Self {
        let unit_seconds = match relative::unit(&unit.to_lowercase()) {
            Some("second") => 1.0,
            Some("minute") => 60.0,
            Some("hour") => 3600.0,
            Some("day") => 86_400.0,
            _ => return self,
        } * precision.max(1) as f64;
        let midnight = self.inner.date_naive().and_time(NaiveTime::MIN);
        let elapsed = (self.inner.naive_local() - midnight).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
        let rounded = round(elapsed / unit_seconds) * unit_seconds;
        let naive = midnight + Duration::microseconds((rounded * 1_000_000.0).round() as i64);
        Self::from_naive(naive, self.inner.timezone()).unwrap_or(self)
    }

    // ------------------------------------------------------------------
    // More comparison
    // ------------------------------------------------------------------

    /// Determine if this date is the given day of the week.
    pub fn is_day_of_week(&self, day: Weekday) -> bool {
        self.inner.weekday() == day
    }

    /// Determine if this date is a Monday.
    pub fn is_monday(&self) -> bool {
        self.is_day_of_week(Weekday::Mon)
    }

    /// Determine if this date is a Tuesday.
    pub fn is_tuesday(&self) -> bool {
        self.is_day_of_week(Weekday::Tue)
    }

    /// Determine if this date is a Wednesday.
    pub fn is_wednesday(&self) -> bool {
        self.is_day_of_week(Weekday::Wed)
    }

    /// Determine if this date is a Thursday.
    pub fn is_thursday(&self) -> bool {
        self.is_day_of_week(Weekday::Thu)
    }

    /// Determine if this date is a Friday.
    pub fn is_friday(&self) -> bool {
        self.is_day_of_week(Weekday::Fri)
    }

    /// Determine if this date is a Saturday.
    pub fn is_saturday(&self) -> bool {
        self.is_day_of_week(Weekday::Sat)
    }

    /// Determine if this date is a Sunday.
    pub fn is_sunday(&self) -> bool {
        self.is_day_of_week(Weekday::Sun)
    }

    fn in_same_zone(&self, other: &Carbon) -> Carbon {
        Carbon {
            inner: other.inner.with_timezone(&self.inner.timezone()),
        }
    }

    /// Determine if both dates fall in the same year.
    pub fn is_same_year(&self, other: &Carbon) -> bool {
        self.year() == self.in_same_zone(other).year()
    }

    /// Determine if both dates fall in the same month of the same year.
    pub fn is_same_month(&self, other: &Carbon) -> bool {
        let other = self.in_same_zone(other);
        self.year() == other.year() && self.month() == other.month()
    }

    /// Determine if both dates fall in the same quarter of the same year.
    pub fn is_same_quarter(&self, other: &Carbon) -> bool {
        let other = self.in_same_zone(other);
        self.year() == other.year() && self.quarter() == other.quarter()
    }

    /// Determine if both dates fall in the same ISO week.
    pub fn is_same_week(&self, other: &Carbon) -> bool {
        self.inner.iso_week() == self.in_same_zone(other).inner.iso_week()
    }

    /// Determine if both dates fall in the same hour of the same day.
    pub fn is_same_hour(&self, other: &Carbon) -> bool {
        self.is_same_day(other) && self.hour() == self.in_same_zone(other).hour()
    }

    /// Determine if both dates fall in the same minute.
    pub fn is_same_minute(&self, other: &Carbon) -> bool {
        self.is_same_hour(other) && self.minute() == self.in_same_zone(other).minute()
    }

    /// Determine if both dates format identically with the given PHP format.
    pub fn is_same_as(&self, format: &str, other: &Carbon) -> bool {
        self.format(format) == self.in_same_zone(other).format(format)
    }

    /// Determine if the date is in the current year.
    pub fn is_current_year(&self) -> bool {
        self.is_same_year(&Self::now())
    }

    /// Determine if the date is in the current month.
    pub fn is_current_month(&self) -> bool {
        self.is_same_month(&Self::now())
    }

    /// Determine if the date is in the current quarter.
    pub fn is_current_quarter(&self) -> bool {
        self.is_same_quarter(&Self::now())
    }

    /// Determine if the date is in the current week.
    pub fn is_current_week(&self) -> bool {
        self.is_same_week(&Self::now())
    }

    /// Determine if the date is today (alias of `is_today`).
    pub fn is_current_day(&self) -> bool {
        self.is_today()
    }

    /// Determine if the date is in the current hour.
    pub fn is_current_hour(&self) -> bool {
        self.is_same_hour(&Self::now())
    }

    /// Determine if the date is in the current minute.
    pub fn is_current_minute(&self) -> bool {
        self.is_same_minute(&Self::now())
    }

    /// Determine if the date is in next year.
    pub fn is_next_year(&self) -> bool {
        self.is_same_year(&Self::now().add_year())
    }

    /// Determine if the date is in last year.
    pub fn is_last_year(&self) -> bool {
        self.is_same_year(&Self::now().sub_year())
    }

    /// Determine if the date is in next month.
    pub fn is_next_month(&self) -> bool {
        self.is_same_month(&Self::now().start_of_month().add_month())
    }

    /// Determine if the date is in last month.
    pub fn is_last_month(&self) -> bool {
        self.is_same_month(&Self::now().start_of_month().sub_month())
    }

    /// Determine if the date is in next week.
    pub fn is_next_week(&self) -> bool {
        self.is_same_week(&Self::now().add_week())
    }

    /// Determine if the date is in last week.
    pub fn is_last_week(&self) -> bool {
        self.is_same_week(&Self::now().sub_week())
    }

    /// Determine if today is this date's birthday (same month and day).
    pub fn is_birthday(&self) -> bool {
        self.is_birthday_of(&Self::now())
    }

    /// Determine if the given date shares this date's month and day.
    pub fn is_birthday_of(&self, other: &Carbon) -> bool {
        let other = self.in_same_zone(other);
        self.month() == other.month() && self.day() == other.day()
    }

    /// Determine if the date is the first day of its month.
    pub fn is_first_day_of_month(&self) -> bool {
        self.day() == 1
    }

    /// Determine if the date is the last day of its month.
    pub fn is_last_day_of_month(&self) -> bool {
        self.day() == self.days_in_month()
    }

    /// Determine if the time is exactly midnight (`00:00:00`).
    pub fn is_start_of_day(&self) -> bool {
        self.hour() == 0 && self.minute() == 0 && self.second() == 0
    }

    /// Determine if the time is the last second of the day (`23:59:59`).
    pub fn is_end_of_day(&self) -> bool {
        self.hour() == 23 && self.minute() == 59 && self.second() == 59
    }

    /// Determine if the time is midnight (alias of `is_start_of_day`).
    pub fn is_midnight(&self) -> bool {
        self.is_start_of_day()
    }

    /// Determine if the time is midday (`12:00:00`).
    pub fn is_midday(&self) -> bool {
        self.hour() == 12 && self.minute() == 0 && self.second() == 0
    }

    /// Get whichever of the two dates is closest to this one.
    pub fn closest(&self, a: &Carbon, b: &Carbon) -> Carbon {
        if self.diff_in_seconds_abs(a) <= self.diff_in_seconds_abs(b) { *a } else { *b }
    }

    /// Get whichever of the two dates is farthest from this one.
    pub fn farthest(&self, a: &Carbon, b: &Carbon) -> Carbon {
        if self.diff_in_seconds_abs(a) >= self.diff_in_seconds_abs(b) { *a } else { *b }
    }

    // ------------------------------------------------------------------
    // Absolute differences
    // ------------------------------------------------------------------

    /// Absolute number of seconds between the two dates.
    pub fn diff_in_seconds_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_seconds(other).abs()
    }

    /// Absolute number of minutes between the two dates.
    pub fn diff_in_minutes_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_minutes(other).abs()
    }

    /// Absolute number of hours between the two dates.
    pub fn diff_in_hours_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_hours(other).abs()
    }

    /// Absolute number of days between the two dates.
    pub fn diff_in_days_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_days(other).abs()
    }

    /// Absolute number of weeks between the two dates.
    pub fn diff_in_weeks_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_weeks(other).abs()
    }

    /// Absolute number of months between the two dates.
    pub fn diff_in_months_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_months(other).abs()
    }

    /// Absolute number of years between the two dates.
    pub fn diff_in_years_abs(&self, other: &Carbon) -> i64 {
        self.diff_in_years(other).abs()
    }

    /// Signed number of weekdays from `self` to `other`.
    pub fn diff_in_weekdays(&self, other: &Carbon) -> i64 {
        let (start, end, sign) = if self <= other { (*self, *other, 1) } else { (*other, *self, -1) };
        let mut count = 0;
        let mut date = start.start_of_day();
        let end = end.start_of_day();
        while date < end {
            if date.is_weekday() {
                count += 1;
            }
            date = date.add_day();
        }
        count * sign
    }

    /// The difference between the two dates as an interval.
    pub fn diff(&self, other: &Carbon) -> CarbonInterval {
        let months = self.diff_in_months(other);
        let anchored = self.add_months(months);
        CarbonInterval {
            months,
            duration: other.inner - anchored.inner,
        }
    }

    // ------------------------------------------------------------------
    // Human readable differences, with options
    // ------------------------------------------------------------------

    /// A short human readable difference from now, like "3h ago".
    pub fn diff_for_humans_short(&self) -> String {
        self.diff_for_humans_with(None, DiffOptions::new().short())
    }

    /// A human readable difference with full control over the output.
    ///
    /// ```
    /// use illuminate_support::Carbon;
    /// use illuminate_support::carbon::DiffOptions;
    ///
    /// let now = Carbon::parse("2024-03-12 12:00:00").unwrap();
    /// let earlier = now.sub_minutes(90);
    ///
    /// assert_eq!(earlier.diff_for_humans_with(Some(&now), DiffOptions::new()), "1 hour before");
    /// assert_eq!(earlier.diff_for_humans_with(Some(&now), DiffOptions::new().parts(2)), "1 hour 30 minutes before");
    /// assert_eq!(earlier.diff_for_humans_with(Some(&now), DiffOptions::new().short().parts(2)), "1h 30m before");
    /// assert_eq!(earlier.diff_for_humans_with(Some(&now), DiffOptions::new().absolute()), "1 hour");
    /// assert_eq!(earlier.diff_for_humans_with(Some(&now), DiffOptions::new().relative_to_now()), "1 hour ago");
    /// ```
    pub fn diff_for_humans_with(&self, other: Option<&Carbon>, options: DiffOptions) -> String {
        let reference = other.copied().unwrap_or_else(Self::now);
        let is_past = *self < reference;
        let (start, end) = if is_past { (*self, reference) } else { (reference, *self) };
        let interval = start.diff(&end);
        let phrase = interval.humanize(options.short, options.parts, options.join);
        let syntax = match options.syntax {
            DiffSyntax::Auto if other.is_none() => DiffSyntax::RelativeToNow,
            DiffSyntax::Auto => DiffSyntax::RelativeToOther,
            syntax => syntax,
        };
        match (syntax, is_past) {
            (DiffSyntax::RelativeToNow, true) => format!("{phrase} ago"),
            (DiffSyntax::RelativeToNow, false) => format!("{phrase} from now"),
            (DiffSyntax::RelativeToOther, true) => format!("{phrase} before"),
            (DiffSyntax::RelativeToOther, false) => format!("{phrase} after"),
            _ => phrase,
        }
    }

    // ------------------------------------------------------------------
    // More formatting
    // ------------------------------------------------------------------

    /// `2024-03-12T15:30:00+00:00`
    pub fn to_atom_string(&self) -> String {
        self.format("Y-m-d\\TH:i:sP")
    }

    /// `2024-03-12T15:30:00+00:00`
    pub fn to_rfc3339_string(&self) -> String {
        self.to_atom_string()
    }

    /// `2024-03-12T15:30:00+00:00`
    pub fn to_w3c_string(&self) -> String {
        self.to_atom_string()
    }

    /// `2024-03-12T15:30` (for `<input type="datetime-local">`)
    pub fn to_date_time_local_string(&self) -> String {
        self.format("Y-m-d\\TH:i")
    }

    /// `2024-03-12 15:30:00.000000` — the date time string with microseconds.
    pub fn to_date_time_string_with_micros(&self) -> String {
        self.format("Y-m-d H:i:s.u")
    }

    /// A daily [`CarbonPeriod`] from this date until the given one.
    pub fn days_until(&self, end: Carbon) -> CarbonPeriod {
        CarbonPeriod::create(*self, end)
    }

    /// A [`CarbonPeriod`] from this date until the given one, stepping by the interval.
    pub fn range(&self, end: Carbon, interval: impl Into<CarbonInterval>) -> CarbonPeriod {
        CarbonPeriod::create(*self, end).every(interval)
    }
}

impl crate::traits::Conditionable for Carbon {}

impl CarbonInterval {
    /// Create an interval from each of its components.
    pub fn create(years: i64, months: i64, weeks: i64, days: i64, hours: i64, minutes: i64, seconds: i64) -> Self {
        Self {
            months: years * 12 + months,
            duration: Duration::weeks(weeks)
                + Duration::days(days)
                + Duration::hours(hours)
                + Duration::minutes(minutes)
                + Duration::seconds(seconds),
        }
    }

    /// An interval of the given number of milliseconds.
    pub fn milliseconds(n: i64) -> Self {
        Self { months: 0, duration: Duration::milliseconds(n) }
    }

    /// An interval of the given number of microseconds.
    pub fn microseconds(n: i64) -> Self {
        Self { months: 0, duration: Duration::microseconds(n) }
    }

    /// Parse a human readable interval: `"2 hours 30 minutes"`, `"1h 30m"`,
    /// `"3 days"`, `"1.5 hours"` or ISO-8601 (`"P1DT2H"`).
    ///
    /// ```
    /// use illuminate_support::CarbonInterval;
    ///
    /// let interval = CarbonInterval::parse("2 hours 30 minutes").unwrap();
    /// assert_eq!(interval.total_minutes(), 150.0);
    /// assert_eq!(interval.for_humans_short(), "2h 30m");
    /// assert_eq!(CarbonInterval::parse("PT1H30M").unwrap(), CarbonInterval::minutes(90));
    /// ```
    pub fn parse(value: &str) -> crate::Result<Self> {
        let invalid = || -> crate::Error {
            crate::error::InvalidArgumentException::new(format!("Invalid duration [{value}].")).into()
        };
        let trimmed = value.trim();
        if let Some(iso) = trimmed.strip_prefix('P').or_else(|| trimmed.strip_prefix('p')) {
            return parse_iso_duration(iso).ok_or_else(invalid);
        }
        let lowered = trimmed.to_lowercase().replace(',', " ").replace(" and ", " ");
        let mut interval = Self::default();
        let mut tokens: Vec<String> = Vec::new();
        for raw in lowered.split_whitespace() {
            let split = raw.find(|c: char| c.is_alphabetic() || c == 'µ');
            match split {
                Some(index) if index > 0 => {
                    tokens.push(raw[..index].to_string());
                    tokens.push(raw[index..].to_string());
                }
                _ => tokens.push(raw.to_string()),
            }
        }
        if tokens.is_empty() || tokens.len() % 2 != 0 {
            return Err(invalid());
        }
        for pair in tokens.chunks(2) {
            let amount: f64 = pair[0].parse().map_err(|_| invalid())?;
            let unit = relative::unit(&pair[1]).ok_or_else(invalid)?;
            interval = interval + Self::of_unit(unit, amount).ok_or_else(invalid)?;
        }
        Ok(interval)
    }

    fn of_unit(unit: &str, amount: f64) -> Option<Self> {
        let micros = |factor: f64| Some(Self::microseconds((amount * factor).round() as i64));
        match unit {
            "microsecond" => micros(1.0),
            "millisecond" => micros(1_000.0),
            "second" => micros(1_000_000.0),
            "minute" => micros(60_000_000.0),
            "hour" => micros(3_600_000_000.0),
            "day" | "weekday" => micros(86_400_000_000.0),
            "week" => micros(7.0 * 86_400_000_000.0),
            "fortnight" => micros(14.0 * 86_400_000_000.0),
            "month" if amount.fract() == 0.0 => Some(Self::months(amount as i64)),
            "quarter" if amount.fract() == 0.0 => Some(Self::months(amount as i64 * 3)),
            "year" if (amount * 12.0).fract() == 0.0 => Some(Self::months((amount * 12.0) as i64)),
            "decade" if amount.fract() == 0.0 => Some(Self::years(amount as i64 * 10)),
            "century" if amount.fract() == 0.0 => Some(Self::years(amount as i64 * 100)),
            _ => None,
        }
    }

    /// Total length in minutes (months count as 30 days).
    pub fn total_minutes(&self) -> f64 {
        self.total_microseconds() as f64 / 60_000_000.0
    }

    /// Total length in hours (months count as 30 days).
    pub fn total_hours(&self) -> f64 {
        self.total_microseconds() as f64 / 3_600_000_000.0
    }

    /// Total length in days (months count as 30 days).
    pub fn total_days(&self) -> f64 {
        self.total_microseconds() as f64 / 86_400_000_000.0
    }

    /// Total length in weeks (months count as 30 days).
    pub fn total_weeks(&self) -> f64 {
        self.total_days() / 7.0
    }

    /// Total length in milliseconds (months count as 30 days).
    pub fn total_milliseconds(&self) -> i64 {
        self.total_microseconds() / 1000
    }

    /// Total length in microseconds (months count as 30 days).
    pub fn total_microseconds(&self) -> i64 {
        self.months * 30 * 86_400_000_000 + self.duration.num_microseconds().unwrap_or(i64::MAX)
    }

    /// Normalize the interval: whole years of months stay as months, and the
    /// exact duration is kept as-is. Provided for parity with Carbon.
    pub fn cascade(self) -> Self {
        self
    }

    /// Determine if the interval is empty.
    pub fn is_empty(&self) -> bool {
        self.months == 0 && self.duration.is_zero()
    }

    /// Convert to a `std::time::Duration` (negative intervals become zero).
    pub fn to_std(&self) -> std::time::Duration {
        std::time::Duration::from_micros(self.total_microseconds().max(0) as u64)
    }

    /// A short human readable representation, like "2h 30m".
    pub fn for_humans_short(&self) -> String {
        self.humanize(true, usize::MAX, false)
    }

    /// A human readable representation using at most `parts` units.
    pub fn for_humans_parts(&self, parts: usize) -> String {
        self.humanize(false, parts.max(1), false)
    }

    /// The (years, months, weeks, days, hours, minutes, seconds) components.
    pub fn components(&self) -> [i64; 7] {
        let mut secs = self.duration.num_seconds();
        let weeks = secs / (7 * 86_400);
        secs -= weeks * 7 * 86_400;
        let days = secs / 86_400;
        secs -= days * 86_400;
        let hours = secs / 3600;
        secs -= hours * 3600;
        let minutes = secs / 60;
        secs -= minutes * 60;
        [self.months / 12, self.months % 12, weeks, days, hours, minutes, secs]
    }

    pub(crate) fn humanize(&self, short: bool, parts: usize, join: bool) -> String {
        const LONG: [&str; 7] = ["year", "month", "week", "day", "hour", "minute", "second"];
        const SHORT: [(&str, &str); 7] =
            [("yr", "yrs"), ("mo", "mos"), ("w", "w"), ("d", "d"), ("h", "h"), ("m", "m"), ("s", "s")];
        let pieces: Vec<String> = self
            .components()
            .iter()
            .enumerate()
            .filter(|(_, n)| **n != 0)
            .take(parts)
            .map(|(i, n)| {
                if short {
                    let (one, many) = SHORT[i];
                    format!("{n}{}", if n.abs() == 1 { one } else { many })
                } else if n.abs() == 1 {
                    format!("{n} {}", LONG[i])
                } else {
                    format!("{n} {}s", LONG[i])
                }
            })
            .collect();
        match pieces.len() {
            0 if short => "1s".to_string(),
            0 => "1 second".to_string(),
            n if join && n > 1 => format!("{} and {}", pieces[..n - 1].join(" "), pieces[n - 1]),
            _ => pieces.join(" "),
        }
    }
}

fn parse_iso_duration(iso: &str) -> Option<CarbonInterval> {
    let mut interval = CarbonInterval::default();
    let mut in_time = false;
    let mut number = String::new();
    for c in iso.chars() {
        match c {
            'T' | 't' => in_time = true,
            '0'..='9' | '.' => number.push(c),
            unit => {
                let amount: f64 = number.parse().ok()?;
                number.clear();
                let unit = match (unit.to_ascii_uppercase(), in_time) {
                    ('Y', false) => "year",
                    ('M', false) => "month",
                    ('W', false) => "week",
                    ('D', false) => "day",
                    ('H', true) => "hour",
                    ('M', true) => "minute",
                    ('S', true) => "second",
                    _ => return None,
                };
                interval = interval + CarbonInterval::of_unit(unit, amount)?;
            }
        }
    }
    number.is_empty().then_some(interval)
}

impl fmt::Display for CarbonInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.for_humans())
    }
}

impl std::ops::Add for CarbonInterval {
    type Output = CarbonInterval;

    fn add(self, rhs: CarbonInterval) -> CarbonInterval {
        CarbonInterval {
            months: self.months + rhs.months,
            duration: self.duration + rhs.duration,
        }
    }
}

impl std::ops::Sub for CarbonInterval {
    type Output = CarbonInterval;

    fn sub(self, rhs: CarbonInterval) -> CarbonInterval {
        CarbonInterval {
            months: self.months - rhs.months,
            duration: self.duration - rhs.duration,
        }
    }
}

impl std::ops::Neg for CarbonInterval {
    type Output = CarbonInterval;

    fn neg(self) -> CarbonInterval {
        CarbonInterval {
            months: -self.months,
            duration: -self.duration,
        }
    }
}

impl std::str::FromStr for CarbonInterval {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CarbonInterval::parse(s)
    }
}

/// `CarbonImmutable` — every [`Carbon`] is already immutable.
pub type CarbonImmutable = Carbon;

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        _ => 28,
    }
}

fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// Format a date using PHP's `date()` format characters.
pub fn format_php(dt: &DateTime<Tz>, format: &str) -> String {
    let mut out = String::with_capacity(format.len() * 2);
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            'd' => out.push_str(&format!("{:02}", dt.day())),
            'D' => out.push_str(&dt.format("%a").to_string()),
            'j' => out.push_str(&dt.day().to_string()),
            'l' => out.push_str(&dt.format("%A").to_string()),
            'N' => out.push_str(&dt.weekday().number_from_monday().to_string()),
            'S' => out.push_str(ordinal_suffix(dt.day())),
            'w' => out.push_str(&dt.weekday().num_days_from_sunday().to_string()),
            'z' => out.push_str(&(dt.ordinal() - 1).to_string()),
            'W' => out.push_str(&format!("{:02}", dt.iso_week().week())),
            'F' => out.push_str(&dt.format("%B").to_string()),
            'm' => out.push_str(&format!("{:02}", dt.month())),
            'M' => out.push_str(&dt.format("%b").to_string()),
            'n' => out.push_str(&dt.month().to_string()),
            't' => out.push_str(&days_in_month(dt.year(), dt.month()).to_string()),
            'L' => out.push_str(if is_leap_year(dt.year()) { "1" } else { "0" }),
            'o' => out.push_str(&dt.iso_week().year().to_string()),
            'Y' => out.push_str(&dt.year().to_string()),
            'y' => out.push_str(&format!("{:02}", dt.year() % 100)),
            'a' => out.push_str(if dt.hour() < 12 { "am" } else { "pm" }),
            'A' => out.push_str(if dt.hour() < 12 { "AM" } else { "PM" }),
            'g' => out.push_str(&(((dt.hour() + 11) % 12) + 1).to_string()),
            'G' => out.push_str(&dt.hour().to_string()),
            'h' => out.push_str(&format!("{:02}", ((dt.hour() + 11) % 12) + 1)),
            'H' => out.push_str(&format!("{:02}", dt.hour())),
            'i' => out.push_str(&format!("{:02}", dt.minute())),
            's' => out.push_str(&format!("{:02}", dt.second())),
            'u' => out.push_str(&format!("{:06}", dt.nanosecond() / 1000)),
            'v' => out.push_str(&format!("{:03}", dt.nanosecond() / 1_000_000)),
            'e' => out.push_str(dt.timezone().name()),
            'T' => out.push_str(&dt.format("%Z").to_string()),
            'P' => out.push_str(&dt.format("%:z").to_string()),
            'p' => {
                let offset = dt.format("%:z").to_string();
                out.push_str(if offset == "+00:00" { "Z" } else { &offset });
            }
            'O' => out.push_str(&dt.format("%z").to_string()),
            'Z' => out.push_str(&dt.offset().fix().local_minus_utc().to_string()),
            'c' => out.push_str(&dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()),
            'r' => out.push_str(&dt.to_rfc2822()),
            'U' => out.push_str(&dt.timestamp().to_string()),
            other => out.push(other),
        }
    }
    out
}

use chrono::Offset;

/// Translate a PHP date format into a `strftime`-style format for parsing.
pub fn php_format_to_strftime(format: &str) -> String {
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        let piece = match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
                continue;
            }
            'd' => "%d",
            'j' => "%e",
            'D' => "%a",
            'l' => "%A",
            'm' => "%m",
            'n' => "%m",
            'M' => "%b",
            'F' => "%B",
            'Y' => "%Y",
            'y' => "%y",
            'H' => "%H",
            'G' => "%H",
            'h' => "%I",
            'g' => "%I",
            'i' => "%M",
            's' => "%S",
            'u' => "%6f",
            'v' => "%3f",
            'a' | 'A' => "%p",
            'P' => "%:z",
            'O' => "%z",
            'U' => "%s",
            '%' => "%%",
            other => {
                out.push(other);
                continue;
            }
        };
        out.push_str(piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_formats_like_php() {
        let date = Carbon::parse("2024-03-12 15:04:05").unwrap();
        assert_eq!(date.format("Y-m-d H:i:s"), "2024-03-12 15:04:05");
        assert_eq!(date.format("D, d M Y"), "Tue, 12 Mar 2024");
        assert_eq!(date.format("g:i A"), "3:04 PM");
        assert_eq!(date.format("jS F"), "12th March");
        assert_eq!(date.format("\\Y\\e\\a\\r: Y"), "Year: 2024");
    }

    #[test]
    fn it_adds_months_without_overflow() {
        let date = Carbon::parse("2024-01-31").unwrap();
        assert_eq!(date.add_month().to_date_string(), "2024-02-29");
        assert_eq!(date.add_years(1).to_date_string(), "2025-01-31");
        assert_eq!(date.sub_months(2).to_date_string(), "2023-11-30");
    }

    #[test]
    fn it_serializes_to_json_and_storage_formats() {
        let date = Carbon::parse("2024-03-12 15:04:05").unwrap();
        assert_eq!(serde_json::to_string(&date).unwrap(), "\"2024-03-12T15:04:05.000000Z\"");
        let stored = Carbon::with_storage_format(|| serde_json::to_string(&date).unwrap());
        assert_eq!(stored, "\"2024-03-12 15:04:05\"");
        let back: Carbon = serde_json::from_str("\"2024-03-12T15:04:05.000000Z\"").unwrap();
        assert_eq!(back, date);
    }

    #[test]
    fn it_diffs_for_humans() {
        let now = Carbon::parse("2024-03-12 12:00:00").unwrap();
        assert_eq!(now.sub_minutes(5).diff_for_humans_from(&now), "5 minutes before");
        assert_eq!(now.add_days(2).diff_for_humans_from(&now), "2 days after");
        assert_eq!(now.sub_months(3).diff_for_humans_from(&now), "3 months before");
    }

    #[test]
    fn it_creates_from_format() {
        let date = Carbon::create_from_format("d/m/Y H:i", "12/03/2024 15:30").unwrap();
        assert_eq!(date.to_date_time_string(), "2024-03-12 15:30:00");
    }

    fn at(value: &str) -> Carbon {
        Carbon::parse(value).unwrap()
    }

    #[test]
    fn it_sets_components_with_overflow() {
        let date = at("2024-02-29 10:20:30");
        assert_eq!(date.set_year(2023).to_date_string(), "2023-03-01");
        assert_eq!(date.set_month(13).to_date_string(), "2025-01-29");
        assert_eq!(at("2024-04-15").set_day(31).to_date_string(), "2024-05-01");
        assert_eq!(date.set_hour(25).to_date_time_string(), "2024-03-01 01:20:30");
        assert_eq!(date.set_minute(5).to_time_string(), "10:05:30");
        assert_eq!(date.set_second(0).to_time_string(), "10:20:00");
        assert_eq!(date.set_microsecond(123).micro(), 123);
        assert_eq!(date.set_date_time(2020, 1, 2, 3, 4, 5).to_date_time_string(), "2020-01-02 03:04:05");
        let tokyo = date.set_timezone("Asia/Tokyo").unwrap();
        assert_eq!(tokyo.to_date_time_string(), "2024-02-29 19:20:30");
        assert_eq!(tokyo, date);
        let shifted = date.shift_timezone("Asia/Tokyo").unwrap();
        assert_eq!(shifted.to_date_time_string(), "2024-02-29 10:20:30");
        assert_ne!(shifted, date);
    }

    #[test]
    fn it_exposes_calendar_getters() {
        let date = at("2024-03-12 15:30:00");
        assert_eq!(date.quarter(), 1);
        assert_eq!(date.week_of_year(), 11);
        assert_eq!(date.week_of_month(), 2);
        assert_eq!(date.day_name(), "Tuesday");
        assert_eq!(date.short_day_name(), "Tue");
        assert_eq!(date.month_name(), "March");
        assert_eq!(date.short_month_name(), "Mar");
        assert_eq!(date.day_of_week_iso(), 2);
        assert_eq!(date.days_in_year(), 366);
        assert!(date.is_tuesday() && !date.is_monday() && !date.is_sunday());
        assert!(at("2024-03-16").is_saturday());
        Carbon::with_test_now(at("2024-03-12 12:00:00"), || {
            assert_eq!(at("1990-03-13").age(), 33);
            assert_eq!(at("1990-03-12").age(), 34);
            assert!(at("1990-03-12").is_birthday());
            assert!(at("2024-03-01").is_current_month());
            assert!(at("2024-12-31").is_current_year());
            assert!(at("2024-02-10").is_current_quarter());
            assert!(at("2024-04-10").is_next_month());
            assert!(at("2024-02-10").is_last_month());
            assert!(at("2025-01-01").is_next_year());
            assert!(at("2023-01-01").is_last_year());
            assert!(at("2024-03-19").is_next_week());
            assert!(at("2024-03-05").is_last_week());
            assert!(at("2024-03-12 12:30:00").is_current_hour());
            assert!(at("2024-03-12").is_current_day());
        });
    }

    #[test]
    fn it_compares_calendar_units() {
        let date = at("2024-03-12 15:30:00");
        assert!(date.is_same_month(&at("2024-03-31")));
        assert!(!date.is_same_month(&at("2023-03-12")));
        assert!(date.is_same_year(&at("2024-12-31")));
        assert!(date.is_same_hour(&at("2024-03-12 15:59:59")));
        assert!(!date.is_same_hour(&at("2024-03-13 15:30:00")));
        assert!(date.is_same_minute(&at("2024-03-12 15:30:59")));
        assert!(date.is_same_quarter(&at("2024-01-01")));
        assert!(date.is_same_week(&at("2024-03-17")));
        assert!(date.is_same_as("Y-m", &at("2024-03-01")));
        assert!(at("2024-02-29").is_last_day_of_month());
        assert!(at("2024-02-01").is_first_day_of_month());
        assert!(at("2024-02-01").is_start_of_day());
        assert!(at("2024-02-01 23:59:59").is_end_of_day());
        assert!(at("2024-02-01 12:00:00").is_midday());
        assert_eq!(date.closest(&at("2024-03-10"), &at("2024-03-13")), at("2024-03-13"));
        assert_eq!(date.farthest(&at("2024-03-10"), &at("2024-03-13")), at("2024-03-10"));
        assert_eq!(date.min(at("2024-01-01")), at("2024-01-01"));
        assert_eq!(date.max(at("2024-01-01")), date);
    }

    #[test]
    fn it_navigates_weekdays() {
        let friday = at("2024-03-15 10:00:00");
        assert_eq!(friday.next_weekday().to_date_time_string(), "2024-03-18 10:00:00");
        assert_eq!(at("2024-03-18 10:00:00").previous_weekday().to_date_string(), "2024-03-15");
        assert_eq!(friday.add_weekdays(3).to_date_string(), "2024-03-20");
        assert_eq!(friday.sub_weekdays(5).to_date_string(), "2024-03-08");
        assert_eq!(friday.next(Carbon::MONDAY).to_date_time_string(), "2024-03-18 00:00:00");
        assert_eq!(friday.previous(Carbon::FRIDAY).to_date_string(), "2024-03-08");
        assert_eq!(at("2024-03-11").diff_in_weekdays(&at("2024-03-18")), 5);
    }

    #[test]
    fn it_finds_period_boundaries() {
        let date = at("2024-05-15 10:20:30");
        assert_eq!(date.start_of_quarter().to_date_time_string(), "2024-04-01 00:00:00");
        assert_eq!(date.end_of_quarter().to_date_time_string(), "2024-06-30 23:59:59");
        assert_eq!(date.start_of_decade().to_date_string(), "2020-01-01");
        assert_eq!(date.end_of_decade().to_date_string(), "2029-12-31");
        assert_eq!(date.start_of_century().to_date_string(), "2001-01-01");
        assert_eq!(date.end_of_century().to_date_string(), "2100-12-31");
        assert_eq!(date.end_of_hour().to_date_time_string_with_micros(), "2024-05-15 10:59:59.999999");
        assert_eq!(date.end_of_minute().to_time_string(), "10:20:59");
    }

    #[test]
    fn it_rounds_to_units() {
        let date = at("2024-03-12 15:30:31");
        assert_eq!(date.floor_minute().to_time_string(), "15:30:00");
        assert_eq!(date.ceil_minute().to_time_string(), "15:31:00");
        assert_eq!(date.round_minute().to_time_string(), "15:31:00");
        assert_eq!(date.round_hour().to_time_string(), "16:00:00");
        assert_eq!(date.floor_day().to_date_time_string(), "2024-03-12 00:00:00");
        assert_eq!(date.ceil_day().to_date_time_string(), "2024-03-13 00:00:00");
        assert_eq!(date.round_day().to_date_string(), "2024-03-13");
        assert_eq!(at("2024-03-12 15:00:00").ceil_hour().to_time_string(), "15:00:00");
        assert_eq!(at("2024-03-12 23:59:59").ceil_minute().to_date_time_string(), "2024-03-13 00:00:00");
        assert_eq!(date.floor_unit("minute", 15).to_time_string(), "15:30:00");
        assert_eq!(date.ceil_unit("minute", 15).to_time_string(), "15:45:00");
    }

    #[test]
    fn it_adds_units_and_overflowing_months() {
        let date = at("2026-01-31");
        assert_eq!(date.add_months_with_overflow(1).to_date_string(), "2026-03-03");
        assert_eq!(at("2026-05-31").sub_months_with_overflow(1).to_date_string(), "2026-05-01");
        assert_eq!(at("2026-05-31").sub_months(1).to_date_string(), "2026-04-30");
        assert_eq!(date.add_unit("days", 2).unwrap().to_date_string(), "2026-02-02");
        assert_eq!(date.sub_unit("year", 1).unwrap().to_date_string(), "2025-01-31");
        assert_eq!(date.add_quarters(1).to_date_string(), "2026-04-30");
        assert_eq!(date.add_decades(1).year(), 2036);
        assert_eq!(date.plus(CarbonInterval::weeks(1)).to_date_string(), "2026-02-07");
        assert_eq!(date.minus(CarbonInterval::days(1)).to_date_string(), "2026-01-30");
        assert_eq!(date.add_milliseconds(1500).to_time_string(), "00:00:01");
    }

    #[test]
    fn it_parses_relative_expressions() {
        Carbon::with_test_now(at("2024-03-12 15:30:00"), || {
            let parse = |value: &str| Carbon::parse(value).unwrap().to_date_time_string();
            assert_eq!(parse("+1 day"), "2024-03-13 15:30:00");
            assert_eq!(parse("-2 weeks"), "2024-02-27 15:30:00");
            assert_eq!(parse("3 days ago"), "2024-03-09 15:30:00");
            assert_eq!(parse("+1 week 2 days"), "2024-03-21 15:30:00");
            assert_eq!(parse("next monday"), "2024-03-18 00:00:00");
            assert_eq!(parse("last friday"), "2024-03-08 00:00:00");
            assert_eq!(parse("this tuesday"), "2024-03-12 00:00:00");
            assert_eq!(parse("friday"), "2024-03-15 00:00:00");
            assert_eq!(parse("tomorrow 10:00"), "2024-03-13 10:00:00");
            assert_eq!(parse("yesterday noon"), "2024-03-11 12:00:00");
            assert_eq!(parse("today 3pm"), "2024-03-12 15:00:00");
            assert_eq!(parse("midnight"), "2024-03-12 00:00:00");
            assert_eq!(parse("noon"), "2024-03-12 12:00:00");
            assert_eq!(parse("first day of next month"), "2024-04-01 15:30:00");
            assert_eq!(parse("last day of this month"), "2024-03-31 15:30:00");
            assert_eq!(parse("first day of last month midnight"), "2024-02-01 00:00:00");
            assert_eq!(parse("next month"), "2024-04-12 15:30:00");
            assert_eq!(parse("last year"), "2023-03-12 15:30:00");
            assert_eq!(parse("2024-01-01 +1 day"), "2024-01-02 00:00:00");
            assert_eq!(parse("+1day"), "2024-03-13 15:30:00");
            assert_eq!(parse("next week"), "2024-03-19 15:30:00");
            assert_eq!(parse("first day of january"), "2024-01-01 15:30:00");
            assert!(Carbon::parse("not a date at all").is_err());
            assert!(Carbon::parse("next blursday").is_err());
        });
    }

    #[test]
    fn it_diffs_for_humans_with_options() {
        let now = at("2024-03-12 12:00:00");
        Carbon::with_test_now(now, || {
            assert_eq!(now.sub_hours(3).diff_for_humans_short(), "3h ago");
            assert_eq!(now.add_days(2).diff_for_humans_short(), "2d from now");
            assert_eq!(now.sub_days(10).diff_for_humans_with(None, DiffOptions::new().parts(2)), "1 week 3 days ago");
            assert_eq!(now.sub_days(10).diff_for_humans_with(None, DiffOptions::new().parts(2).join()), "1 week and 3 days ago");
            assert_eq!(now.sub_months(14).diff_for_humans_with(None, DiffOptions::new().parts(2).short()), "1yr 2mos ago");
            assert_eq!(now.add_minutes(5).diff_for_humans_with(None, DiffOptions::new().absolute()), "5 minutes");
            assert_eq!(now.diff_for_humans_with(Some(&now.add_hours(1)), DiffOptions::new()), "1 hour before");
        });
        assert_eq!(at("2024-01-01").diff_in_days_abs(&at("2023-12-25")), 7);
        assert_eq!(at("2024-01-01").diff_in_months_abs(&at("2023-10-01")), 3);
        assert_eq!(at("2024-01-01").diff(&at("2024-02-02 01:00:00")).for_humans(), "1 month 1 day 1 hour");
    }

    #[test]
    fn it_freezes_time_per_thread() {
        let frozen = at("2017-06-27 13:14:15");
        Carbon::set_thread_test_now(Some(frozen));
        assert_eq!(Carbon::now(), frozen);
        assert_eq!(Carbon::thread_test_now(), Some(frozen));
        Carbon::set_thread_test_now(None);
        assert_ne!(Carbon::now(), frozen);
        assert_eq!(Carbon::with_test_now(frozen, || Carbon::now().to_date_time_string()), "2017-06-27 13:14:15");
        use crate::traits::Conditionable;
        assert!(Carbon::now().when(true, |c| c.add_day()).is_tomorrow());
    }

    #[test]
    fn it_creates_from_ids_and_times() {
        assert_eq!(
            Carbon::create_from_id("01DXH9C4P0ED4AGJJP9CRKQ55C").unwrap().utc().to_date_time_string(),
            "2020-01-01 19:30:00"
        );
        assert_eq!(
            Carbon::create_from_id("01880dfa-2825-72e4-acbb-b1e4981cf8af").unwrap().utc().to_date_time_string_with_micros(),
            "2023-05-12 03:21:18.117000"
        );
        assert_eq!(
            Carbon::create_from_id("71513cb4-f071-11ed-a0cf-325096b39f47").unwrap().utc().to_date_time_string_with_micros(),
            "2023-05-12 03:02:34.147346"
        );
        assert!(Carbon::create_from_id("a0a2a2d2-0b87-4a18-83f2-2529882be2de").is_err());
        assert_eq!(Carbon::create_from_time(9, 30, 0).unwrap().to_time_string(), "09:30:00");
        assert_eq!(Carbon::create_from_timestamp(0).utc().to_date_string(), "1970-01-01");
        assert_eq!(at("2024-03-12 15:30:00").to_atom_string(), "2024-03-12T15:30:00+00:00");
        assert_eq!(at("2024-03-12 15:30:00").to_date_time_local_string(), "2024-03-12T15:30");
        let immutable: CarbonImmutable = at("2024-01-01");
        assert_eq!(immutable.copy(), immutable);
    }

    #[test]
    fn it_works_with_intervals() {
        let interval = CarbonInterval::parse("2 hours 30 minutes").unwrap();
        assert_eq!(interval.total_minutes(), 150.0);
        assert_eq!(interval.total_hours(), 2.5);
        assert_eq!(interval.for_humans(), "2 hours 30 minutes");
        assert_eq!(interval.for_humans_short(), "2h 30m");
        assert_eq!(interval.to_string(), "2 hours 30 minutes");
        assert_eq!(CarbonInterval::parse("1h 30m").unwrap(), CarbonInterval::minutes(90));
        assert_eq!(CarbonInterval::parse("1.5 hours").unwrap(), CarbonInterval::minutes(90));
        assert_eq!(CarbonInterval::parse("P1Y2M3DT4H5M6S").unwrap(), CarbonInterval::create(1, 2, 0, 3, 4, 5, 6));
        assert_eq!(CarbonInterval::parse("3 days and 2 hours").unwrap().total_hours(), 74.0);
        assert!(CarbonInterval::parse("forever").is_err());
        assert_eq!(CarbonInterval::days(2).total_days(), 2.0);
        assert_eq!(CarbonInterval::weeks(1).total_weeks(), 1.0);
        assert_eq!((CarbonInterval::hours(1) + CarbonInterval::minutes(30)).total_minutes(), 90.0);
        assert_eq!((CarbonInterval::hours(1) - CarbonInterval::minutes(30)).total_minutes(), 30.0);
        assert_eq!((-CarbonInterval::days(1)).total_days(), -1.0);
        assert_eq!(CarbonInterval::milliseconds(1500).to_std(), std::time::Duration::from_millis(1500));
        assert_eq!(CarbonInterval::create(0, 0, 1, 1, 0, 0, 0).for_humans_parts(1), "1 week");
        assert!(CarbonInterval::default().is_empty());
        assert_eq!("45 seconds".parse::<CarbonInterval>().unwrap(), CarbonInterval::seconds(45));
    }

    #[test]
    fn it_creates_periods_from_dates() {
        let period = at("2024-01-01").days_until(at("2024-01-05"));
        assert_eq!(period.count(), 5);
        let hours = at("2024-01-01 09:00").range(at("2024-01-01 12:00"), CarbonInterval::hours(1));
        assert_eq!(hours.to_array().len(), 4);
    }
}
