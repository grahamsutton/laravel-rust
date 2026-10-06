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
    Timelike, Utc, Weekday,
};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

static DEFAULT_TIMEZONE: LazyLock<RwLock<Tz>> = LazyLock::new(|| RwLock::new(Tz::UTC));
static TEST_NOW: LazyLock<RwLock<Option<Carbon>>> = LazyLock::new(|| RwLock::new(None));

thread_local! {
    static STORAGE_FORMAT: Cell<bool> = const { Cell::new(false) };
}

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
    /// Unix timestamps, and the relative words `now`, `today`, `tomorrow`
    /// and `yesterday`.
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
}
