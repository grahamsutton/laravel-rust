//! Parsing of PHP-style relative date expressions (`+1 day`, `next monday`,
//! `first day of next month`, `tomorrow 10:00`, ...).

use chrono::{Datelike, NaiveDate, Weekday};

use super::Carbon;

const WEEKDAYS: &[(&str, Weekday)] = &[
    ("monday", Weekday::Mon),
    ("mon", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("tue", Weekday::Tue),
    ("tues", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("wed", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("thu", Weekday::Thu),
    ("thur", Weekday::Thu),
    ("thurs", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("fri", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sat", Weekday::Sat),
    ("sunday", Weekday::Sun),
    ("sun", Weekday::Sun),
];

const MONTHS: &[(&str, u32)] = &[
    ("january", 1),
    ("jan", 1),
    ("february", 2),
    ("feb", 2),
    ("march", 3),
    ("mar", 3),
    ("april", 4),
    ("apr", 4),
    ("may", 5),
    ("june", 6),
    ("jun", 6),
    ("july", 7),
    ("jul", 7),
    ("august", 8),
    ("aug", 8),
    ("september", 9),
    ("sep", 9),
    ("sept", 9),
    ("october", 10),
    ("oct", 10),
    ("november", 11),
    ("nov", 11),
    ("december", 12),
    ("dec", 12),
];

fn weekday(token: &str) -> Option<Weekday> {
    WEEKDAYS.iter().find(|(name, _)| *name == token).map(|(_, d)| *d)
}

fn month(token: &str) -> Option<u32> {
    MONTHS.iter().find(|(name, _)| *name == token).map(|(_, m)| *m)
}

/// Normalize a unit name (`days`, `hrs`, `min`, ...) to its canonical form.
pub(crate) fn unit(token: &str) -> Option<&'static str> {
    Some(match token {
        "usec" | "usecs" | "microsecond" | "microseconds" | "µs" => "microsecond",
        "ms" | "msec" | "msecs" | "millisecond" | "milliseconds" => "millisecond",
        "s" | "sec" | "secs" | "second" | "seconds" => "second",
        "m" | "min" | "mins" | "minute" | "minutes" => "minute",
        "h" | "hr" | "hrs" | "hour" | "hours" => "hour",
        "d" | "day" | "days" => "day",
        "weekday" | "weekdays" => "weekday",
        "w" | "week" | "weeks" => "week",
        "fortnight" | "fortnights" | "forthnight" | "forthnights" => "fortnight",
        "mo" | "month" | "months" => "month",
        "quarter" | "quarters" => "quarter",
        "y" | "yr" | "yrs" | "year" | "years" => "year",
        "decade" | "decades" => "decade",
        "century" | "centuries" => "century",
        _ => return None,
    })
}

/// Parse a time of day: `10:00`, `10:00:30`, `3pm`, `3:30pm`, `10am`.
fn time_of_day(token: &str) -> Option<(u32, u32, u32)> {
    let (body, meridiem) = if let Some(body) = token.strip_suffix("am") {
        (body, Some(false))
    } else if let Some(body) = token.strip_suffix("pm") {
        (body, Some(true))
    } else {
        (token, None)
    };
    let parts: Vec<&str> = body.split(':').collect();
    if parts.is_empty() || parts.len() > 3 || (parts.len() == 1 && meridiem.is_none()) {
        return None;
    }
    let mut numbers = Vec::with_capacity(3);
    for part in &parts {
        if part.is_empty() || part.len() > 2 || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        numbers.push(part.parse::<u32>().ok()?);
    }
    let (mut hour, minute, second) = (numbers[0], *numbers.get(1).unwrap_or(&0), *numbers.get(2).unwrap_or(&0));
    if let Some(pm) = meridiem {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = match (pm, hour) {
            (false, 12) => 0,
            (true, 12) => 12,
            (true, h) => h + 12,
            (false, h) => h,
        };
    }
    (hour < 24 && minute < 60 && second < 60).then_some((hour, minute, second))
}

/// Apply a relative expression to the base date. Returns `None` when the
/// expression contains something we don't understand.
pub(crate) fn parse(expression: &str, base: Carbon) -> Option<Carbon> {
    let lowered = expression.to_lowercase().replace(',', " ");
    let mut tokens: Vec<String> = Vec::new();
    for raw in lowered.split_whitespace() {
        // Split "+1day" / "-2weeks" into number and unit.
        let split = raw
            .char_indices()
            .find(|(i, c)| *i > 0 && c.is_ascii_alphabetic())
            .map(|(i, _)| i);
        match split {
            Some(i) if raw[..i].trim_start_matches(['+', '-']).chars().all(|c| c.is_ascii_digit())
                && !raw[..i].trim_start_matches(['+', '-']).is_empty()
                && unit(&raw[i..]).is_some() =>
            {
                tokens.push(raw[..i].to_string());
                tokens.push(raw[i..].to_string());
            }
            _ => tokens.push(raw.to_string()),
        }
    }
    if tokens.is_empty() {
        return None;
    }

    let mut date = base;
    let mut offsets: Vec<(&'static str, i64)> = Vec::new();
    let mut day_of: Option<bool> = None; // Some(true) = first day of, Some(false) = last day of
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i].as_str();
        let next = tokens.get(i + 1).map(String::as_str);
        match token {
            "now" | "at" | "of" | "and" => {}
            "today" | "midnight" => date = date.start_of_day(),
            "noon" => date = date.set_time(12, 0, 0),
            "tomorrow" => date = date.add_days(1).start_of_day(),
            "yesterday" => date = date.sub_days(1).start_of_day(),
            "ago" => {
                for (_, amount) in offsets.iter_mut() {
                    *amount = -*amount;
                }
            }
            "first" | "last" if next == Some("day") && tokens.get(i + 2).map(String::as_str) == Some("of") => {
                day_of = Some(token == "first");
                i += 3;
                continue;
            }
            "next" | "last" | "previous" | "this" => {
                let target = next?;
                let direction = match token {
                    "next" => 1,
                    "this" => 0,
                    _ => -1,
                };
                if let Some(day) = weekday(target) {
                    date = match direction {
                        1 => date.next(day),
                        -1 => date.previous(day),
                        _ => this_weekday(date, day),
                    };
                } else if let Some(unit) = unit(target) {
                    offsets.push((unit, direction));
                } else {
                    return None;
                }
                i += 2;
                continue;
            }
            _ => {
                if let Some(day) = weekday(token) {
                    date = this_weekday(date, day);
                } else if let Some(month) = month(token) {
                    // "march", "march 2025", "5 march" style month selection.
                    let mut year = date.year();
                    if let Some(y) = next.and_then(|n| n.parse::<i32>().ok()).filter(|y| *y > 31) {
                        year = y;
                        i += 1;
                    }
                    let day = date.day().min(super::days_in_month(year, month));
                    date = date.set_date(year, month, day);
                } else if let Some((h, m, s)) = time_of_day(token) {
                    date = date.set_time(h, m, s);
                } else if let Ok(parsed) = NaiveDate::parse_from_str(token, "%Y-%m-%d") {
                    date = date
                        .set_date(parsed.year(), parsed.month(), parsed.day())
                        .start_of_day();
                } else if let Ok(amount) = token.trim_start_matches('+').parse::<i64>() {
                    let target = next?;
                    if let Some(unit) = unit(target) {
                        offsets.push((unit, amount));
                        i += 2;
                        continue;
                    }
                    if let Some(month) = month(target) {
                        // "5 march"
                        let day = u32::try_from(amount).ok().filter(|d| (1..=31).contains(d))?;
                        date = date.set_date(date.year(), month, day.min(super::days_in_month(date.year(), month)));
                        i += 2;
                        continue;
                    }
                    return None;
                } else {
                    return None;
                }
            }
        }
        i += 1;
    }

    for (unit, amount) in offsets {
        date = date.add_unit(unit, amount).ok()?;
    }
    match day_of {
        Some(true) => date = date.set_date(date.year(), date.month(), 1),
        Some(false) => date = date.set_date(date.year(), date.month(), date.days_in_month()),
        None => {}
    }
    Some(date)
}

fn this_weekday(date: Carbon, day: Weekday) -> Carbon {
    if date.weekday() == day {
        date.start_of_day()
    } else {
        date.next(day)
    }
}

