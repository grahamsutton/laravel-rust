//! Cron expressions: parsing, `is_due` and next / previous run dates.
//!
//! Supports the five standard fields with ranges (`1-5`), steps (`*/15`,
//! `1-23/2`, `5/10`), lists (`1,13`), month and weekday names (`JAN`,
//! `MON-FRI`), `?`, the `L` and `W` day-of-month modifiers, the `L` and
//! `#` day-of-week modifiers, and the `@daily`-style macros.
//!
//! ```
//! use illuminate_console::scheduling::CronExpression;
//! use illuminate_support::Carbon;
//!
//! let cron = CronExpression::parse("0 13 * * 1-5").unwrap();
//!
//! let monday = Carbon::parse("2024-03-11 13:00:00").unwrap();
//! assert!(cron.is_due(&monday));
//!
//! let next = cron.next_run_date(&Carbon::parse("2024-03-15 14:00:00").unwrap()).unwrap();
//! assert_eq!(next.to_date_time_string(), "2024-03-18 13:00:00");
//! ```

use chrono::{
    Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike,
    Weekday,
};
use chrono_tz::Tz;
use illuminate_support::Carbon;
use illuminate_support::error::InvalidArgumentException;

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Field {
    any: bool,
    values: Vec<bool>,
}

impl Field {
    fn contains(&self, value: u32) -> bool {
        self.values.get(value as usize).copied().unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DayOfMonth {
    field: Field,
    last: bool,
    last_weekday: bool,
    nearest_weekdays: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DayOfWeek {
    field: Field,
    nth: Vec<(u32, u32)>,
    last: Vec<u32>,
}

/// A parsed cron expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronExpression {
    expression: String,
    minutes: Field,
    hours: Field,
    days_of_month: DayOfMonth,
    months: Field,
    days_of_week: DayOfWeek,
}

fn invalid_value(value: &str, position: usize) -> InvalidArgumentException {
    InvalidArgumentException::new(format!(
        "Invalid CRON field value {value} at position {position}"
    ))
}

fn parse_number(
    value: &str,
    names: &[&str],
    offset: u32,
    position: usize,
) -> Result<u32, InvalidArgumentException> {
    if let Ok(number) = value.parse::<u32>() {
        return Ok(number);
    }

    names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(value))
        .map(|index| index as u32 + offset)
        .ok_or_else(|| invalid_value(value, position))
}

/// Parse a list of ranges and steps into the set of matching values.
fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    names: &[&str],
    name_offset: u32,
    position: usize,
) -> Result<Field, InvalidArgumentException> {
    let mut values = vec![false; max as usize + 1];
    let any = field == "*" || field == "?";

    for part in field.split(',') {
        if part.is_empty() {
            return Err(invalid_value(field, position));
        }

        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().map_err(|_| invalid_value(part, position))?;
                if step == 0 {
                    return Err(invalid_value(part, position));
                }
                (range, Some(step))
            }
            None => (part, None),
        };

        let (start, end) = if range == "*" || range == "?" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (
                parse_number(start, names, name_offset, position)?,
                parse_number(end, names, name_offset, position)?,
            )
        } else {
            let start = parse_number(range, names, name_offset, position)?;
            (start, if step.is_some() { max } else { start })
        };

        if start < min || end > max || start > end {
            return Err(invalid_value(part, position));
        }

        let step = step.unwrap_or(1) as usize;
        for value in (start..=end).step_by(step) {
            values[value as usize] = true;
        }
    }

    Ok(Field { any, values })
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|date| date.pred_opt())
        .map(|date| date.day())
        .unwrap_or(31)
}

fn is_weekday(date: NaiveDate) -> bool {
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
}

/// The weekday nearest to the given day of the month (`15W`).
fn nearest_weekday(year: i32, month: u32, day: u32) -> Option<u32> {
    let last = days_in_month(year, month);
    if day > last {
        return None;
    }

    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    Some(match date.weekday() {
        Weekday::Sat if day == 1 => 3,
        Weekday::Sat => day - 1,
        Weekday::Sun if day == last => day - 2,
        Weekday::Sun => day + 1,
        _ => day,
    })
}

impl DayOfMonth {
    fn parse(field: &str) -> Result<Self, InvalidArgumentException> {
        let mut regular = Vec::new();
        let mut result = DayOfMonth {
            field: Field {
                any: field == "*" || field == "?",
                values: vec![false; 32],
            },
            last: false,
            last_weekday: false,
            nearest_weekdays: Vec::new(),
        };

        for part in field.split(',') {
            let upper = part.to_ascii_uppercase();
            if upper == "L" {
                result.last = true;
            } else if upper == "LW" {
                result.last_weekday = true;
            } else if let Some(day) = upper.strip_suffix('W') {
                let day: u32 = day.parse().map_err(|_| invalid_value(part, 2))?;
                if !(1..=31).contains(&day) {
                    return Err(invalid_value(part, 2));
                }
                result.nearest_weekdays.push(day);
            } else {
                regular.push(part);
            }
        }

        if !regular.is_empty() {
            let parsed = parse_field(&regular.join(","), 1, 31, &[], 0, 2)?;
            result.field.values = parsed.values;
        }

        Ok(result)
    }

    fn matches(&self, date: NaiveDate) -> bool {
        let day = date.day();
        let last = days_in_month(date.year(), date.month());

        if self.field.contains(day) || (self.last && day == last) {
            return true;
        }

        if self.last_weekday {
            let mut candidate = last;
            while let Some(d) = NaiveDate::from_ymd_opt(date.year(), date.month(), candidate) {
                if is_weekday(d) {
                    break;
                }
                candidate -= 1;
            }
            if day == candidate {
                return true;
            }
        }

        self.nearest_weekdays
            .iter()
            .any(|target| nearest_weekday(date.year(), date.month(), *target) == Some(day))
    }
}

impl DayOfWeek {
    fn parse(field: &str) -> Result<Self, InvalidArgumentException> {
        let mut regular = Vec::new();
        let mut result = DayOfWeek {
            field: Field {
                any: field == "*" || field == "?",
                values: vec![false; 8],
            },
            nth: Vec::new(),
            last: Vec::new(),
        };

        let weekday = |value: &str| -> Result<u32, InvalidArgumentException> {
            let day = parse_number(value, &WEEKDAYS, 0, 4)?;
            if day > 7 {
                return Err(invalid_value(value, 4));
            }
            Ok(day % 7)
        };

        for part in field.split(',') {
            if let Some((day, nth)) = part.split_once('#') {
                let nth: u32 = nth.parse().map_err(|_| invalid_value(part, 4))?;
                if !(1..=5).contains(&nth) {
                    return Err(invalid_value(part, 4));
                }
                result.nth.push((weekday(day)?, nth));
            } else if part.len() > 1 && part.to_ascii_uppercase().ends_with('L') {
                result.last.push(weekday(&part[..part.len() - 1])?);
            } else {
                regular.push(part);
            }
        }

        if !regular.is_empty() {
            let parsed = parse_field(&regular.join(","), 0, 7, &WEEKDAYS, 0, 4)?;
            let mut values = parsed.values;
            if values[7] {
                values[0] = true;
            }
            values.truncate(7);
            result.field.values = values;
        }

        Ok(result)
    }

    fn matches(&self, date: NaiveDate) -> bool {
        let weekday = date.weekday().num_days_from_sunday();
        let day = date.day();

        if self.field.contains(weekday) {
            return true;
        }

        if self
            .nth
            .iter()
            .any(|(w, n)| *w == weekday && (day - 1) / 7 + 1 == *n)
        {
            return true;
        }

        let last = days_in_month(date.year(), date.month());
        self.last.iter().any(|w| *w == weekday && day + 7 > last)
    }
}

impl CronExpression {
    /// Parse a cron expression.
    pub fn parse(expression: &str) -> Result<Self, InvalidArgumentException> {
        let normalized = match expression.trim().to_ascii_lowercase().as_str() {
            "@yearly" | "@annually" => "0 0 1 1 *".to_string(),
            "@monthly" => "0 0 1 * *".to_string(),
            "@weekly" => "0 0 * * 0".to_string(),
            "@daily" | "@midnight" => "0 0 * * *".to_string(),
            "@hourly" => "0 * * * *".to_string(),
            _ => expression.trim().to_string(),
        };

        let fields: Vec<&str> = normalized.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(InvalidArgumentException::new(format!(
                "{expression} is not a valid CRON expression"
            )));
        }

        Ok(Self {
            expression: expression.trim().to_string(),
            minutes: parse_field(fields[0], 0, 59, &[], 0, 0)?,
            hours: parse_field(fields[1], 0, 23, &[], 0, 1)?,
            days_of_month: DayOfMonth::parse(fields[2])?,
            months: parse_field(fields[3], 1, 12, &MONTHS, 1, 3)?,
            days_of_week: DayOfWeek::parse(fields[4])?,
        })
    }

    /// Determine if the given expression is valid.
    pub fn is_valid(expression: &str) -> bool {
        Self::parse(expression).is_ok()
    }

    /// The original expression.
    pub fn expression(&self) -> &str {
        &self.expression
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom = &self.days_of_month;
        let dow = &self.days_of_week;

        match (dom.field.any, dow.field.any) {
            (true, true) => true,
            (false, true) => dom.matches(date),
            (true, false) => dow.matches(date),
            (false, false) => dom.matches(date) || dow.matches(date),
        }
    }

    /// Determine if the expression matches the given wall-clock time.
    pub fn matches(&self, time: &NaiveDateTime) -> bool {
        self.minutes.contains(time.minute())
            && self.hours.contains(time.hour())
            && self.months.contains(time.month())
            && self.day_matches(time.date())
    }

    fn zone(date: &Carbon, timezone: Option<&str>) -> Tz {
        timezone
            .and_then(|name| name.parse::<Tz>().ok())
            .unwrap_or_else(|| date.inner().timezone())
    }

    fn truncate(time: NaiveDateTime) -> NaiveDateTime {
        time.date()
            .and_hms_opt(time.hour(), time.minute(), 0)
            .unwrap_or(time)
    }

    /// Determine if the expression is due at the given date (in its own timezone).
    pub fn is_due(&self, date: &Carbon) -> bool {
        self.is_due_in(date, None)
    }

    /// Determine if the expression is due at the given date, evaluated in
    /// the given timezone.
    pub fn is_due_in(&self, date: &Carbon, timezone: Option<&str>) -> bool {
        let tz = Self::zone(date, timezone);
        let local = Self::truncate(date.inner().with_timezone(&tz).naive_local());
        self.matches(&local)
    }

    /// The next date the expression is due, after the given date.
    pub fn next_run_date(&self, from: &Carbon) -> Result<Carbon, InvalidArgumentException> {
        self.run_date(from, 0, false, None, true)
    }

    /// The previous date the expression was due, before the given date.
    pub fn previous_run_date(&self, from: &Carbon) -> Result<Carbon, InvalidArgumentException> {
        self.run_date(from, 0, false, None, false)
    }

    /// Find a run date: the `nth` match after (or before) the given date,
    /// optionally counting the current minute, evaluated in a timezone.
    pub fn run_date(
        &self,
        from: &Carbon,
        nth: usize,
        allow_current: bool,
        timezone: Option<&str>,
        forward: bool,
    ) -> Result<Carbon, InvalidArgumentException> {
        let tz = Self::zone(from, timezone);
        let start = Self::truncate(from.inner().with_timezone(&tz).naive_local());
        let mut time = start;
        let mut skip = nth;

        if !allow_current {
            time = if forward {
                time + Duration::minutes(1)
            } else {
                time - Duration::minutes(1)
            };
        }

        for _ in 0..200_000 {
            if !self.months.contains(time.month()) {
                time = if forward {
                    first_of_next_month(time)
                } else {
                    last_minute_of_previous_month(time)
                };
                continue;
            }

            if !self.day_matches(time.date()) {
                time = if forward {
                    (time.date() + Duration::days(1)).and_time(NaiveTime::MIN)
                } else {
                    (time.date() - Duration::days(1))
                        .and_hms_opt(23, 59, 0)
                        .unwrap_or(time)
                };
                continue;
            }

            if !self.hours.contains(time.hour()) {
                time = if forward {
                    time.date().and_hms_opt(time.hour(), 0, 0).unwrap_or(time) + Duration::hours(1)
                } else {
                    time.date().and_hms_opt(time.hour(), 59, 0).unwrap_or(time) - Duration::hours(1)
                };
                continue;
            }

            if !self.minutes.contains(time.minute()) {
                time = if forward {
                    time + Duration::minutes(1)
                } else {
                    time - Duration::minutes(1)
                };
                continue;
            }

            let candidate = match tz.from_local_datetime(&time) {
                LocalResult::Single(date) => Some(date),
                LocalResult::Ambiguous(earliest, _) => Some(earliest),
                LocalResult::None => None,
            };

            match candidate {
                Some(date) if skip == 0 => return Ok(Carbon::from_datetime(date)),
                Some(_) => skip -= 1,
                None => {}
            }

            time = if forward {
                time + Duration::minutes(1)
            } else {
                time - Duration::minutes(1)
            };
        }

        Err(InvalidArgumentException::new(format!(
            "Impossible CRON expression [{}]",
            self.expression
        )))
    }

    /// The next `count` run dates after the given date.
    pub fn multiple_run_dates(
        &self,
        count: usize,
        from: &Carbon,
    ) -> Result<Vec<Carbon>, InvalidArgumentException> {
        (0..count)
            .map(|nth| self.run_date(from, nth, false, None, true))
            .collect()
    }
}

fn first_of_next_month(time: NaiveDateTime) -> NaiveDateTime {
    let (year, month) = if time.month() == 12 {
        (time.year() + 1, 1)
    } else {
        (time.year(), time.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1)
        .map(|date| date.and_time(NaiveTime::MIN))
        .unwrap_or(time)
}

fn last_minute_of_previous_month(time: NaiveDateTime) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(time.year(), time.month(), 1)
        .and_then(|date| date.pred_opt())
        .and_then(|date| date.and_hms_opt(23, 59, 0))
        .unwrap_or(time)
}

impl std::fmt::Display for CronExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.expression)
    }
}

impl std::str::FromStr for CronExpression {
    type Err = InvalidArgumentException;

    fn from_str(expression: &str) -> Result<Self, Self::Err> {
        Self::parse(expression)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> Carbon {
        let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").unwrap();
        Carbon::from_datetime(Tz::UTC.from_utc_datetime(&naive))
    }

    fn due(expression: &str, date: &str) -> bool {
        CronExpression::parse(expression).unwrap().is_due(&at(date))
    }

    fn next(expression: &str, from: &str) -> String {
        CronExpression::parse(expression)
            .unwrap()
            .next_run_date(&at(from))
            .unwrap()
            .to_date_time_string()
    }

    fn previous(expression: &str, from: &str) -> String {
        CronExpression::parse(expression)
            .unwrap()
            .previous_run_date(&at(from))
            .unwrap()
            .to_date_time_string()
    }

    #[test]
    fn every_minute_is_always_due() {
        assert!(due("* * * * *", "2024-01-01 00:00:00"));
        assert!(due("* * * * *", "2024-07-15 13:37:42"));
    }

    #[test]
    fn it_checks_fixed_times() {
        assert!(due("0 13 * * *", "2024-03-12 13:00:00"));
        assert!(due("0 13 * * *", "2024-03-12 13:00:59"));
        assert!(!due("0 13 * * *", "2024-03-12 13:01:00"));
        assert!(!due("0 13 * * *", "2024-03-12 12:00:00"));
    }

    #[test]
    fn it_supports_steps_ranges_and_lists() {
        assert!(due("*/15 * * * *", "2024-01-01 10:45:00"));
        assert!(!due("*/15 * * * *", "2024-01-01 10:46:00"));
        assert!(due("0 1-23/2 * * *", "2024-01-01 03:00:00"));
        assert!(!due("0 1-23/2 * * *", "2024-01-01 04:00:00"));
        assert!(due("0 1,13 * * *", "2024-01-01 13:00:00"));
        assert!(due("5/20 * * * *", "2024-01-01 13:45:00"));
        assert!(!due("5/20 * * * *", "2024-01-01 13:40:00"));
        assert!(due("0 0 1 1-12/3 *", "2024-04-01 00:00:00"));
        assert!(!due("0 0 1 1-12/3 *", "2024-05-01 00:00:00"));
    }

    #[test]
    fn it_supports_names() {
        assert!(due("0 0 * JAN MON", "2024-01-01 00:00:00"));
        assert!(due("0 0 * * mon-fri", "2024-01-05 00:00:00"));
        assert!(!due("0 0 * * MON-FRI", "2024-01-06 00:00:00"));
        assert!(due("0 0 1 jan-mar *", "2024-02-01 00:00:00"));
    }

    #[test]
    fn sunday_may_be_zero_or_seven() {
        assert!(due("0 0 * * 0", "2024-01-07 00:00:00"));
        assert!(due("0 0 * * 7", "2024-01-07 00:00:00"));
        assert!(due("0 0 * * 5-7", "2024-01-07 00:00:00"));
        assert!(due("0 0 * * 6,0", "2024-01-06 00:00:00"));
    }

    #[test]
    fn day_of_month_and_week_are_ored() {
        // The 15th, or any Monday...
        assert!(due("0 0 15 * 1", "2024-01-15 00:00:00"));
        assert!(due("0 0 15 * 1", "2024-01-08 00:00:00"));
        assert!(!due("0 0 15 * 1", "2024-01-09 00:00:00"));
        assert!(due("0 0 ? * 1", "2024-01-08 00:00:00"));
        assert!(!due("0 0 ? * 1", "2024-01-09 00:00:00"));
    }

    #[test]
    fn it_supports_last_day_modifiers() {
        assert!(due("0 0 L * *", "2024-02-29 00:00:00"));
        assert!(!due("0 0 L * *", "2024-02-28 00:00:00"));
        assert!(due("0 0 L * *", "2023-02-28 00:00:00"));
        // The last weekday of March 2024 is Friday the 29th...
        assert!(due("0 0 LW * *", "2024-03-29 00:00:00"));
        assert!(!due("0 0 LW * *", "2024-03-31 00:00:00"));
        // The last Friday of the month...
        assert!(due("0 0 * * 5L", "2024-03-29 00:00:00"));
        assert!(!due("0 0 * * 5L", "2024-03-22 00:00:00"));
        // The second Monday of the month...
        assert!(due("0 0 * * 1#2", "2024-03-11 00:00:00"));
        assert!(!due("0 0 * * 1#2", "2024-03-04 00:00:00"));
        // The weekday nearest to the 16th (Saturday) is Friday the 15th...
        assert!(due("0 0 16W * *", "2024-03-15 00:00:00"));
        assert!(!due("0 0 16W * *", "2024-03-16 00:00:00"));
    }

    #[test]
    fn it_supports_macros() {
        assert!(due("@daily", "2024-01-02 00:00:00"));
        assert!(due("@hourly", "2024-01-02 05:00:00"));
        assert!(due("@yearly", "2024-01-01 00:00:00"));
        assert!(!due("@yearly", "2024-01-02 00:00:00"));
        assert!(due("@weekly", "2024-01-07 00:00:00"));
        assert!(due("@monthly", "2024-02-01 00:00:00"));
    }

    #[test]
    fn it_calculates_next_run_dates() {
        assert_eq!(
            next("* * * * *", "2024-01-01 10:00:30"),
            "2024-01-01 10:01:00"
        );
        assert_eq!(
            next("0 * * * *", "2024-01-01 10:00:00"),
            "2024-01-01 11:00:00"
        );
        assert_eq!(
            next("0 13 * * *", "2024-01-01 14:00:00"),
            "2024-01-02 13:00:00"
        );
        assert_eq!(
            next("0 0 * * 1", "2024-01-03 12:00:00"),
            "2024-01-08 00:00:00"
        );
        assert_eq!(
            next("0 0 1 * *", "2024-01-15 00:00:00"),
            "2024-02-01 00:00:00"
        );
        assert_eq!(
            next("0 0 1 1 *", "2024-06-01 00:00:00"),
            "2025-01-01 00:00:00"
        );
        assert_eq!(
            next("0 0 29 2 *", "2024-03-01 00:00:00"),
            "2028-02-29 00:00:00"
        );
        assert_eq!(
            next("*/5 * * * *", "2024-01-01 23:58:00"),
            "2024-01-02 00:00:00"
        );
        assert_eq!(
            next("0 0 L * *", "2024-01-31 00:00:00"),
            "2024-02-29 00:00:00"
        );
        assert_eq!(
            next("30 8 * * 1-5", "2024-01-05 09:00:00"),
            "2024-01-08 08:30:00"
        );
        assert_eq!(
            next("0 0 1 1-12/3 *", "2024-01-02 00:00:00"),
            "2024-04-01 00:00:00"
        );
    }

    #[test]
    fn it_calculates_previous_run_dates() {
        assert_eq!(
            previous("0 * * * *", "2024-01-01 10:30:00"),
            "2024-01-01 10:00:00"
        );
        assert_eq!(
            previous("0 13 * * *", "2024-01-02 12:00:00"),
            "2024-01-01 13:00:00"
        );
        assert_eq!(
            previous("0 0 1 * *", "2024-03-15 00:00:00"),
            "2024-03-01 00:00:00"
        );
        assert_eq!(
            previous("0 0 1 * *", "2024-03-01 00:00:00"),
            "2024-02-01 00:00:00"
        );
    }

    #[test]
    fn it_supports_nth_and_current_dates() {
        let cron = CronExpression::parse("0 * * * *").unwrap();
        let from = at("2024-01-01 10:00:00");
        assert_eq!(
            cron.run_date(&from, 0, true, None, true)
                .unwrap()
                .to_date_time_string(),
            "2024-01-01 10:00:00"
        );
        assert_eq!(
            cron.run_date(&from, 2, false, None, true)
                .unwrap()
                .to_date_time_string(),
            "2024-01-01 13:00:00"
        );
        let dates = cron.multiple_run_dates(3, &from).unwrap();
        assert_eq!(dates.len(), 3);
        assert_eq!(dates[2].to_date_time_string(), "2024-01-01 13:00:00");
    }

    #[test]
    fn it_evaluates_in_timezones() {
        let cron = CronExpression::parse("0 9 * * *").unwrap();
        // 14:00 UTC is 09:00 in New York (EST)...
        let date = at("2024-01-15 14:00:00");
        assert!(cron.is_due_in(&date, Some("America/New_York")));
        assert!(!cron.is_due(&date));

        let next = cron
            .run_date(
                &at("2024-01-15 15:00:00"),
                0,
                false,
                Some("America/New_York"),
                true,
            )
            .unwrap();
        assert_eq!(next.to_date_time_string(), "2024-01-16 09:00:00");
        assert_eq!(next.utc().to_date_time_string(), "2024-01-16 14:00:00");
    }

    #[test]
    fn it_skips_missing_local_times() {
        // 02:30 does not exist on March 10th 2024 in New York (DST).
        let cron = CronExpression::parse("30 2 * * *").unwrap();
        let next = cron
            .run_date(
                &at("2024-03-10 05:00:00"),
                0,
                false,
                Some("America/New_York"),
                true,
            )
            .unwrap();
        assert_eq!(next.to_date_time_string(), "2024-03-11 02:30:00");
    }

    #[test]
    fn it_rejects_invalid_expressions() {
        assert_eq!(
            CronExpression::parse("* * * *").unwrap_err().message,
            "* * * * is not a valid CRON expression"
        );
        assert_eq!(
            CronExpression::parse("60 * * * *").unwrap_err().message,
            "Invalid CRON field value 60 at position 0"
        );
        assert!(CronExpression::parse("* 24 * * *").is_err());
        assert!(CronExpression::parse("* * 0 * *").is_err());
        assert!(CronExpression::parse("* * * 13 *").is_err());
        assert!(CronExpression::parse("* * * * 8").is_err());
        assert!(CronExpression::parse("*/0 * * * *").is_err());
        assert!(CronExpression::parse("5-1 * * * *").is_err());
        assert!(CronExpression::parse("a * * * *").is_err());
        assert!(!CronExpression::is_valid("1,,2 * * * *"));
        assert!(CronExpression::is_valid("0 0 * * MON#2"));
    }

    #[test]
    fn impossible_expressions_error() {
        let cron = CronExpression::parse("0 0 31 2 *").unwrap();
        assert!(cron.next_run_date(&at("2024-01-01 00:00:00")).is_err());
    }
}
