//! Date periods: iterate over the dates between two moments.

use super::{Carbon, CarbonInterval};

/// A period of time, iterated in steps of an interval (one day by default).
///
/// ```
/// use illuminate_support::{Carbon, CarbonInterval, CarbonPeriod};
///
/// let period = CarbonPeriod::create(
///     Carbon::parse("2024-01-01").unwrap(),
///     Carbon::parse("2024-01-03").unwrap(),
/// );
///
/// let dates: Vec<String> = period.iter().map(|date| date.to_date_string()).collect();
/// assert_eq!(dates, vec!["2024-01-01", "2024-01-02", "2024-01-03"]);
///
/// let hours = CarbonPeriod::create(Carbon::parse("2024-01-01 09:00").unwrap(), Carbon::parse("2024-01-01 12:00").unwrap())
///     .every(CarbonInterval::hours(1))
///     .exclude_end_date();
/// assert_eq!(hours.count(), 3);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarbonPeriod {
    start: Carbon,
    end: Option<Carbon>,
    interval: CarbonInterval,
    recurrences: Option<usize>,
    exclude_start: bool,
    exclude_end: bool,
}

impl CarbonPeriod {
    /// Create a daily period from `start` to `end` (inclusive).
    pub fn create(start: Carbon, end: Carbon) -> Self {
        Self {
            start,
            end: Some(end),
            interval: CarbonInterval::days(1),
            recurrences: None,
            exclude_start: false,
            exclude_end: false,
        }
    }

    /// Create a period that starts at `start` and repeats `recurrences` times.
    pub fn starting(start: Carbon, recurrences: usize) -> Self {
        Self {
            start,
            end: None,
            interval: CarbonInterval::days(1),
            recurrences: Some(recurrences),
            exclude_start: false,
            exclude_end: false,
        }
    }

    /// Step through the period using the given interval.
    pub fn every(mut self, interval: impl Into<CarbonInterval>) -> Self {
        self.interval = interval.into();
        self
    }

    /// Alias of `every`.
    pub fn interval(self, interval: impl Into<CarbonInterval>) -> Self {
        self.every(interval)
    }

    /// Limit the number of dates the period yields.
    pub fn recurrences(mut self, recurrences: usize) -> Self {
        self.recurrences = Some(recurrences);
        self
    }

    /// Change the end of the period.
    pub fn until(mut self, end: Carbon) -> Self {
        self.end = Some(end);
        self
    }

    /// Don't include the start date.
    pub fn exclude_start_date(mut self) -> Self {
        self.exclude_start = true;
        self
    }

    /// Don't include the end date.
    pub fn exclude_end_date(mut self) -> Self {
        self.exclude_end = true;
        self
    }

    /// The start of the period.
    pub fn start_date(&self) -> Carbon {
        self.start
    }

    /// The end of the period, if it has one.
    pub fn end_date(&self) -> Option<Carbon> {
        self.end
    }

    /// The interval between dates.
    pub fn get_interval(&self) -> CarbonInterval {
        self.interval
    }

    /// Iterate over the dates of the period.
    pub fn iter(&self) -> CarbonPeriodIter {
        CarbonPeriodIter {
            period: *self,
            index: 0,
            yielded: 0,
        }
    }

    /// Get every date of the period.
    pub fn to_array(&self) -> Vec<Carbon> {
        self.iter().collect()
    }

    /// Count the dates in the period.
    pub fn count(&self) -> usize {
        self.iter().count()
    }

    /// The first date of the period.
    pub fn first(&self) -> Option<Carbon> {
        self.iter().next()
    }

    /// The last date of the period.
    pub fn last(&self) -> Option<Carbon> {
        self.iter().last()
    }

    /// Determine if the given date falls within the period's bounds.
    pub fn contains(&self, date: &Carbon) -> bool {
        let after_start = if self.exclude_start { *date > self.start } else { *date >= self.start };
        let before_end = match self.end {
            Some(end) if self.exclude_end => *date < end,
            Some(end) => *date <= end,
            None => true,
        };
        after_start && before_end
    }

    fn date_at(&self, index: i64) -> Carbon {
        let interval = self.interval;
        self.start
            .add_months(interval.months * index)
            .add(CarbonInterval::from(interval.duration * index as i32))
    }
}

impl IntoIterator for CarbonPeriod {
    type Item = Carbon;
    type IntoIter = CarbonPeriodIter;

    fn into_iter(self) -> CarbonPeriodIter {
        self.iter()
    }
}

impl IntoIterator for &CarbonPeriod {
    type Item = Carbon;
    type IntoIter = CarbonPeriodIter;

    fn into_iter(self) -> CarbonPeriodIter {
        self.iter()
    }
}

/// An iterator over the dates of a [`CarbonPeriod`].
#[derive(Clone, Debug)]
pub struct CarbonPeriodIter {
    period: CarbonPeriod,
    index: i64,
    yielded: usize,
}

impl Iterator for CarbonPeriodIter {
    type Item = Carbon;

    fn next(&mut self) -> Option<Carbon> {
        let period = &self.period;
        if period.interval == CarbonInterval::default() {
            return None;
        }
        loop {
            if period.recurrences.is_some_and(|r| self.yielded >= r) {
                return None;
            }
            let date = period.date_at(self.index);
            self.index += 1;
            if let Some(end) = period.end {
                let past_end = if period.exclude_end { date >= end } else { date > end };
                if past_end {
                    return None;
                }
            }
            if self.index == 1 && period.exclude_start {
                continue;
            }
            self.yielded += 1;
            return Some(date);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> Carbon {
        Carbon::parse(value).unwrap()
    }

    #[test]
    fn it_iterates_daily_by_default() {
        let period = CarbonPeriod::create(date("2024-02-27"), date("2024-03-01"));
        let days: Vec<String> = period.into_iter().map(|d| d.to_date_string()).collect();
        assert_eq!(days, vec!["2024-02-27", "2024-02-28", "2024-02-29", "2024-03-01"]);
        assert_eq!(period.count(), 4);
        assert_eq!(period.first(), Some(date("2024-02-27")));
        assert_eq!(period.last(), Some(date("2024-03-01")));
    }

    #[test]
    fn it_supports_intervals_and_exclusions() {
        let period = CarbonPeriod::create(date("2024-01-31"), date("2024-05-31"))
            .every(CarbonInterval::months(1))
            .exclude_start_date();
        let months: Vec<String> = period.iter().map(|d| d.to_date_string()).collect();
        assert_eq!(months, vec!["2024-02-29", "2024-03-31", "2024-04-30", "2024-05-31"]);
        assert_eq!(period.exclude_end_date().count(), 3);
        assert_eq!(CarbonPeriod::starting(date("2024-01-01"), 3).every(CarbonInterval::weeks(1)).last(), Some(date("2024-01-15")));
        assert!(period.contains(&date("2024-03-15")));
        assert!(!period.contains(&date("2024-01-31")));
    }
}
