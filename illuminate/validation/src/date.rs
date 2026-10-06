//! Date handling for the date rules: a pragmatic `strtotime()` (absolute
//! dates plus relative expressions such as `+1 week` or `3 days ago`) and
//! PHP's `DateTime::createFromFormat()` / `format()` pair used by
//! `date_format`.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};

use illuminate_support::Carbon;

/// A parsed point in time, as microseconds since the Unix epoch.
pub(crate) type Instant = i128;

fn carbon_instant(date: &Carbon) -> Instant {
    date.timestamp() as i128 * 1_000_000 + date.micro() as i128
}

/// Parse a date the way Laravel's `Date::parse()` would: absolute dates,
/// `now` / `today` / `tomorrow` / `yesterday`, and relative expressions.
pub(crate) fn parse(value: &str) -> Option<Instant> {
    parse_carbon(value).map(|c| carbon_instant(&c))
}

/// Parse a date into a Carbon instance.
pub(crate) fn parse_carbon(value: &str) -> Option<Carbon> {
    let value = value.trim();
    if let Some(date) = parse_absolute(value) {
        return Some(date);
    }
    if let Some(stripped) = value.strip_prefix('@') {
        return stripped.parse::<i64>().ok().map(Carbon::from_timestamp);
    }
    parse_relative(value).or_else(|| {
        // Bare integers aren't dates (PHP's `strtotime` doesn't read them as timestamps).
        let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
        if unsigned.bytes().all(|b| b.is_ascii_digit()) {
            None
        } else {
            Carbon::parse(value).ok()
        }
    })
}

const RELATIVE_WORDS: [&str; 26] = [
    "now",
    "today",
    "tomorrow",
    "yesterday",
    "midnight",
    "noon",
    "ago",
    "next",
    "last",
    "previous",
    "this",
    "first",
    "of",
    "sec",
    "second",
    "min",
    "minute",
    "hour",
    "day",
    "week",
    "weekday",
    "fortnight",
    "month",
    "year",
    "weeks",
    "days",
];

/// Determine if a date string is (or contains) a relative expression.
fn looks_relative(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with(['+', '-']) {
        return true;
    }
    lower
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|word| word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '+' || c == '-'))
        .any(|word| {
            let singular = word.strip_suffix('s').unwrap_or(word);
            RELATIVE_WORDS.contains(&word) || RELATIVE_WORDS.contains(&singular)
        })
}

/// Parse an absolute date (no relative words, no bare integers).
fn parse_absolute(value: &str) -> Option<Carbon> {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    if value.is_empty() || unsigned.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !value.bytes().any(|b| b.is_ascii_digit()) || looks_relative(value) {
        return None;
    }
    Carbon::parse(value).ok()
}

/// Determine if the value is a valid, non-relative date (Laravel's `date`
/// rule: `strtotime()` succeeds and `date_parse()` yields a real calendar date).
pub(crate) fn is_valid_date(value: &str) -> bool {
    let value = value.trim();
    if parse_absolute(value).is_some() {
        return true;
    }
    // An absolute date followed by a relative modifier ("2024-01-01 +1 day").
    match split_relative_suffix(value) {
        Some((base, rest)) => {
            parse_absolute(base).is_some() && apply_relative(Carbon::now(), rest).is_some()
        }
        None => false,
    }
}

fn split_relative_suffix(value: &str) -> Option<(&str, &str)> {
    let bytes = value.as_bytes();
    for (i, window) in bytes.windows(2).enumerate() {
        if i > 0
            && bytes[i - 1] == b' '
            && (window[0] == b'+' || window[0] == b'-')
            && window[1].is_ascii_digit()
        {
            return Some((value[..i].trim(), &value[i..]));
        }
    }
    None
}

fn parse_relative(value: &str) -> Option<Carbon> {
    let lower = value.to_ascii_lowercase();
    let lower = lower.trim();
    if lower.is_empty() {
        return Some(Carbon::now());
    }
    if let Some((base, rest)) = split_relative_suffix(value)
        && let Some(base) = parse_absolute(base)
    {
        return apply_relative(base, rest);
    }
    let mut words = lower.split_whitespace().peekable();
    let base = match words.peek().copied() {
        Some("now") => {
            words.next();
            Carbon::now()
        }
        Some("today") | Some("midnight") => {
            words.next();
            Carbon::today()
        }
        Some("tomorrow") => {
            words.next();
            Carbon::tomorrow()
        }
        Some("yesterday") => {
            words.next();
            Carbon::yesterday()
        }
        Some("noon") => {
            words.next();
            Carbon::today().add_hours(12)
        }
        _ => Carbon::now(),
    };
    let rest: Vec<&str> = words.collect();
    apply_relative(base, &rest.join(" "))
}

fn unit_seconds(unit: &str) -> Option<(&'static str, i64)> {
    let unit = unit.trim_end_matches('s');
    Some(match unit {
        "sec" | "second" => ("seconds", 1),
        "min" | "minute" => ("seconds", 60),
        "hour" => ("seconds", 3600),
        "day" => ("days", 1),
        "week" => ("days", 7),
        "fortnight" => ("days", 14),
        "month" => ("months", 1),
        "year" => ("months", 12),
        _ => return None,
    })
}

fn shift(date: Carbon, amount: i64, unit: &str) -> Option<Carbon> {
    let (kind, factor) = unit_seconds(unit)?;
    let amount = amount.checked_mul(factor)?;
    Some(match kind {
        "seconds" => date.add_seconds(amount),
        "days" => date.add_days(amount),
        _ => date.add_months(amount),
    })
}

/// Apply relative modifiers ("+1 week 2 days", "3 days ago", "next month").
fn apply_relative(mut date: Carbon, expression: &str) -> Option<Carbon> {
    let lower = expression.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    if tokens.is_empty() {
        return Some(date);
    }
    let mut i = 0;
    let mut consumed_any = false;
    while i < tokens.len() {
        let token = tokens[i];
        if token == "next" || token == "last" || token == "previous" || token == "this" {
            let unit = tokens.get(i + 1)?;
            let amount = match token {
                "next" => 1,
                "this" => 0,
                _ => -1,
            };
            date = shift(date, amount, unit)?;
            i += 2;
            consumed_any = true;
            continue;
        }
        // "+1 week", "-2 days", "3 days", "+1week"
        let (number, unit, used) = match token.find(|c: char| c.is_ascii_alphabetic()) {
            Some(pos) if pos > 0 => (&token[..pos], &token[pos..], 1),
            _ => (token, *tokens.get(i + 1)?, 2),
        };
        let number = number.strip_prefix('+').unwrap_or(number);
        let mut amount: i64 = match number {
            "a" | "an" => 1,
            n => n.parse().ok()?,
        };
        i += used;
        if tokens.get(i) == Some(&"ago") {
            amount = -amount;
            i += 1;
        }
        date = shift(date, amount, unit)?;
        consumed_any = true;
    }
    consumed_any.then_some(date)
}

// ----------------------------------------------------------------------
// createFromFormat / format
// ----------------------------------------------------------------------

/// The pieces of a date parsed with a PHP format.
#[derive(Clone, Debug)]
pub(crate) struct Parsed {
    pub(crate) datetime: NaiveDateTime,
    /// The UTC offset in seconds, when the format contained a timezone.
    pub(crate) offset: Option<i32>,
    /// The timezone text exactly as given (for `e` / `T`).
    tz_text: Option<String>,
}

impl Parsed {
    /// The instant this date represents. Dates without a timezone are
    /// interpreted in the application's default timezone.
    pub(crate) fn instant(&self) -> Option<Instant> {
        let utc = match self.offset {
            Some(offset) => self.datetime - Duration::seconds(offset as i64),
            None => {
                let tz = Carbon::default_timezone();
                tz.from_local_datetime(&self.datetime)
                    .earliest()?
                    .naive_utc()
            }
        };
        let utc = utc.and_utc();
        Some(utc.timestamp() as i128 * 1_000_000 + utc.timestamp_subsec_micros() as i128)
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

struct Cursor<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn digits(&mut self, min: usize, max: usize) -> Option<i64> {
        let rest = self.rest();
        let len = rest
            .bytes()
            .take(max)
            .take_while(u8::is_ascii_digit)
            .count();
        if len < min {
            return None;
        }
        let value = rest[..len].parse().ok()?;
        self.pos += len;
        Some(value)
    }

    fn signed_digits(&mut self, max: usize) -> Option<i64> {
        let negative = if self.rest().starts_with('-') {
            self.pos += 1;
            true
        } else {
            if self.rest().starts_with('+') {
                self.pos += 1;
            }
            false
        };
        let value = self.digits(1, max)?;
        Some(if negative { -value } else { value })
    }

    fn letters(&mut self) -> &'a str {
        let rest = self.rest();
        let len = rest
            .char_indices()
            .find(|(_, c)| !c.is_ascii_alphabetic())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        self.pos += len;
        &rest[..len]
    }

    fn literal(&mut self, c: char) -> Option<()> {
        if self.rest().starts_with(c) {
            self.pos += c.len_utf8();
            Some(())
        } else {
            None
        }
    }
}

fn month_from_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|m| {
            let m = m.to_ascii_lowercase();
            lower == m || (lower.len() == 3 && m.starts_with(&lower))
        })
        .map(|i| i as u32 + 1)
}

fn parse_offset(text: &str) -> Option<i32> {
    if text.eq_ignore_ascii_case("z")
        || text.eq_ignore_ascii_case("utc")
        || text.eq_ignore_ascii_case("gmt")
    {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    let (hours, minutes) = match digits.len() {
        1 | 2 => (digits.parse::<i32>().ok()?, 0),
        3 => (
            digits[..1].parse::<i32>().ok()?,
            digits[1..].parse::<i32>().ok()?,
        ),
        4 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    Some(sign * (hours * 3600 + minutes * 60))
}

/// PHP's `DateTime::createFromFormat('!' . $format, $value)`.
pub(crate) fn create_from_format(format: &str, value: &str) -> Option<Parsed> {
    let mut cursor = Cursor {
        input: value,
        pos: 0,
    };
    let (mut year, mut month, mut day) = (1970i64, 1i64, 1i64);
    let (mut hour, mut minute, mut second, mut micro) = (0i64, 0i64, 0i64, 0i64);
    let mut meridiem: Option<bool> = None;
    let mut offset: Option<i32> = None;
    let mut tz_text: Option<String> = None;
    let mut timestamp: Option<i64> = None;
    let mut day_of_year: Option<i64> = None;

    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        match c {
            'd' | 'j' => day = cursor.digits(1, 2)?,
            'D' | 'l' => {
                let name = cursor.letters().to_ascii_lowercase();
                let known = DAYS.iter().any(|d| {
                    let d = d.to_ascii_lowercase();
                    name == d || (name.len() == 3 && d.starts_with(&name))
                });
                if !known {
                    return None;
                }
            }
            'S' => {
                let suffix = cursor.letters();
                if !matches!(suffix, "st" | "nd" | "rd" | "th") {
                    return None;
                }
            }
            'z' => day_of_year = Some(cursor.digits(1, 3)?),
            'F' | 'M' => month = month_from_name(cursor.letters())? as i64,
            'm' | 'n' => month = cursor.digits(1, 2)?,
            'Y' => year = cursor.signed_digits(4)?,
            'y' => {
                let short = cursor.digits(2, 2)?;
                year = if short < 70 {
                    2000 + short
                } else {
                    1900 + short
                };
            }
            'a' | 'A' => {
                let text = cursor.rest().get(..2)?.to_ascii_lowercase();
                meridiem = match text.as_str() {
                    "am" => Some(false),
                    "pm" => Some(true),
                    _ => return None,
                };
                cursor.pos += 2;
            }
            'g' | 'h' | 'G' | 'H' => hour = cursor.digits(1, 2)?,
            'i' => minute = cursor.digits(2, 2)?,
            's' => second = cursor.digits(2, 2)?,
            'v' => micro = cursor.digits(3, 3)? * 1000,
            'u' => {
                let rest = cursor.rest();
                let len = rest.bytes().take(9).take_while(u8::is_ascii_digit).count();
                if len == 0 {
                    return None;
                }
                let digits = &rest[..len.min(6)];
                micro = format!("{digits:0<6}").parse().ok()?;
                cursor.pos += len;
            }
            'U' => timestamp = Some(cursor.signed_digits(20)?),
            'e' | 'T' | 'O' | 'P' | 'p' => {
                let rest = cursor.rest();
                let len = rest
                    .char_indices()
                    .find(|(_, ch)| {
                        !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | ':' | '/' | '_'))
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());
                let text = &rest[..len];
                if text.is_empty() {
                    return None;
                }
                let parsed_offset = match parse_offset(text) {
                    Some(o) => o,
                    None => {
                        let tz: chrono_tz::Tz = text.parse().ok()?;
                        let probe =
                            NaiveDate::from_ymd_opt(year as i32, month.clamp(1, 12) as u32, 1)?
                                .and_time(NaiveTime::MIN);
                        use chrono::Offset;
                        tz.offset_from_utc_datetime(&probe).fix().local_minus_utc()
                    }
                };
                offset = Some(parsed_offset);
                tz_text = Some(text.to_string());
                cursor.pos += len;
            }
            '\\' => {
                let next = chars.next()?;
                cursor.literal(next)?;
            }
            '!' | '|' => {
                (year, month, day, hour, minute, second, micro) = (1970, 1, 1, 0, 0, 0, 0);
            }
            '?' => {
                let ch = cursor.rest().chars().next()?;
                cursor.pos += ch.len_utf8();
            }
            '*' => {
                let rest = cursor.rest();
                let len = rest
                    .char_indices()
                    .find(|(_, ch)| {
                        matches!(ch, ' ' | ',' | ';' | ':' | '/' | '.' | '-' | '(' | ')')
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());
                cursor.pos += len;
            }
            '+' => cursor.pos = value.len(),
            '#' => {
                let ch = cursor.rest().chars().next()?;
                if !matches!(ch, ';' | ':' | '/' | '.' | ',' | '-' | '(' | ')') {
                    return None;
                }
                cursor.pos += 1;
            }
            other => cursor.literal(other)?,
        }
    }
    if cursor.pos != value.len() {
        return None;
    }

    if let Some(ts) = timestamp {
        let datetime = chrono::DateTime::from_timestamp(ts, 0)?.naive_utc();
        return Some(Parsed {
            datetime,
            offset: Some(0),
            tz_text,
        });
    }
    if let Some(is_pm) = meridiem {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = match (hour, is_pm) {
            (12, false) => 0,
            (12, true) => 12,
            (h, true) => h + 12,
            (h, false) => h,
        };
    }
    let date = match day_of_year {
        Some(doy) => NaiveDate::from_yo_opt(year as i32, (doy + 1) as u32)?,
        None => NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)?,
    };
    let time =
        NaiveTime::from_hms_micro_opt(hour as u32, minute as u32, second as u32, micro as u32)?;
    Some(Parsed {
        datetime: date.and_time(time),
        offset,
        tz_text,
    })
}

fn format_offset(offset: i32, colon: bool) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    let (hours, minutes) = (offset / 3600, (offset % 3600) / 60);
    if colon {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}{minutes:02}")
    }
}

/// PHP's `DateTime::format()` for a parsed date.
pub(crate) fn format(parsed: &Parsed, format: &str) -> String {
    let dt = parsed.datetime;
    let offset = parsed.offset.unwrap_or(0);
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        match c {
            'd' => out.push_str(&format!("{:02}", dt.day())),
            'j' => out.push_str(&dt.day().to_string()),
            'D' => out.push_str(&DAYS[dt.weekday().num_days_from_monday() as usize][..3]),
            'l' => out.push_str(DAYS[dt.weekday().num_days_from_monday() as usize]),
            'N' => out.push_str(&dt.weekday().number_from_monday().to_string()),
            'w' => out.push_str(&dt.weekday().num_days_from_sunday().to_string()),
            'S' => out.push_str(match (dt.day() % 10, dt.day() % 100) {
                (_, 11..=13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            }),
            'z' => out.push_str(&dt.ordinal0().to_string()),
            'F' => out.push_str(MONTHS[dt.month0() as usize]),
            'M' => out.push_str(&MONTHS[dt.month0() as usize][..3]),
            'm' => out.push_str(&format!("{:02}", dt.month())),
            'n' => out.push_str(&dt.month().to_string()),
            't' => {
                let next = if dt.month() == 12 {
                    NaiveDate::from_ymd_opt(dt.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(dt.year(), dt.month() + 1, 1)
                };
                let days = next
                    .map(|n| n.pred_opt().map(|d| d.day()).unwrap_or(30))
                    .unwrap_or(30);
                out.push_str(&days.to_string());
            }
            'L' => out.push_str(if NaiveDate::from_ymd_opt(dt.year(), 2, 29).is_some() {
                "1"
            } else {
                "0"
            }),
            'Y' => {
                let year = dt.year();
                if year < 0 {
                    out.push_str(&format!("-{:04}", -year));
                } else {
                    out.push_str(&format!("{year:04}"));
                }
            }
            'y' => out.push_str(&format!("{:02}", dt.year().rem_euclid(100))),
            'a' => out.push_str(if dt.hour() < 12 { "am" } else { "pm" }),
            'A' => out.push_str(if dt.hour() < 12 { "AM" } else { "PM" }),
            'g' => out.push_str(&(((dt.hour() + 11) % 12) + 1).to_string()),
            'h' => out.push_str(&format!("{:02}", ((dt.hour() + 11) % 12) + 1)),
            'G' => out.push_str(&dt.hour().to_string()),
            'H' => out.push_str(&format!("{:02}", dt.hour())),
            'i' => out.push_str(&format!("{:02}", dt.minute())),
            's' => out.push_str(&format!("{:02}", dt.second())),
            'u' => out.push_str(&format!("{:06}", dt.nanosecond() / 1000)),
            'v' => out.push_str(&format!("{:03}", dt.nanosecond() / 1_000_000)),
            'U' => out.push_str(
                &(dt - Duration::seconds(offset as i64))
                    .and_utc()
                    .timestamp()
                    .to_string(),
            ),
            'e' | 'T' => match &parsed.tz_text {
                Some(text) => out.push_str(text),
                None => out.push_str(if c == 'e' { "UTC" } else { "GMT" }),
            },
            'O' => out.push_str(&format_offset(offset, false)),
            'P' => out.push_str(&format_offset(offset, true)),
            'p' => {
                if offset == 0 {
                    out.push('Z');
                } else {
                    out.push_str(&format_offset(offset, true));
                }
            }
            'Z' => out.push_str(&offset.to_string()),
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '!' | '|' | '+' => {}
            other => out.push(other),
        }
    }
    out
}

/// Laravel's `date_format` check: the value parses with the format and
/// formats back to exactly the same string.
pub(crate) fn matches_format(format_string: &str, value: &str) -> bool {
    match create_from_format(format_string, value) {
        Some(parsed) => format(&parsed, format_string) == value,
        None => false,
    }
}

/// `getDateTimeWithOptionalFormat`: the format first, then a free parse.
pub(crate) fn parse_with_optional_format(format_string: &str, value: &str) -> Option<Instant> {
    create_from_format(format_string, value)
        .and_then(|p| p.instant())
        .or_else(|| parse(value))
}

/// Format a Carbon instance for a date rule parameter.
pub(crate) fn format_carbon(date: &Carbon, format_string: &str) -> String {
    date.format(format_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_validates_absolute_dates() {
        assert!(is_valid_date("2024-01-31"));
        assert!(is_valid_date("2024-01-31 10:00:00"));
        assert!(is_valid_date("January 5, 2024"));
        assert!(!is_valid_date("2024-02-30"));
        assert!(!is_valid_date("tomorrow"));
        assert!(!is_valid_date("2024"));
        assert!(!is_valid_date("not a date"));
        assert!(is_valid_date("2024-01-01 +1 day"));
        assert!(!is_valid_date("+1 week"));
        assert!(!is_valid_date("next monday"));
        assert!(!is_valid_date("3 days ago"));
        assert!(is_valid_date("10 September 2000"));
        assert!(is_valid_date("Tue, 12 Mar 2024 10:00:00 +0000"));
    }

    #[test]
    fn it_parses_relative_dates() {
        let now = parse("now").unwrap();
        let week = parse("+1 week").unwrap();
        assert!((week - now - 7 * 86_400_000_000).abs() < 5_000_000);
        let ago = parse("3 days ago").unwrap();
        assert!((now - ago - 3 * 86_400_000_000).abs() < 5_000_000);
        assert!(parse("tomorrow").unwrap() > now);
        assert!(parse("next month").unwrap() > now);
        assert!(parse("garbage words").is_none());
    }

    #[test]
    fn it_round_trips_formats() {
        assert!(matches_format("Y-m-d", "2024-03-12"));
        assert!(!matches_format("Y-m-d", "2024-3-12"));
        assert!(!matches_format("Y-m-d", "2024-02-30"));
        assert!(matches_format("d/m/Y H:i", "12/03/2024 15:30"));
        assert!(matches_format("H:i", "09:05"));
        assert!(matches_format(
            "Y-m-d\\TH:i:sP",
            "2024-03-12T15:30:00+02:00"
        ));
        assert!(matches_format("D, d M Y", "Tue, 12 Mar 2024"));
        assert!(!matches_format("D, d M Y", "Wed, 12 Mar 2024"));
        assert!(matches_format("g:i A", "3:04 PM"));
        assert!(matches_format("U", "1700000000"));
        assert!(!matches_format("Y-m-d", "2024-03-12 extra"));
    }

    #[test]
    fn offsets_shift_instants() {
        let a = create_from_format("Y-m-d H:i P", "2024-01-01 12:00 +02:00").unwrap();
        let b = create_from_format("Y-m-d H:i P", "2024-01-01 10:00 +00:00").unwrap();
        assert_eq!(a.instant(), b.instant());
    }
}
