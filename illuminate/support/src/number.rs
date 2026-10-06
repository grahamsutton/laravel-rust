//! Number formatting helpers, mirroring `Illuminate\Support\Number`.
//!
//! Formatting follows the output of PHP's `intl` extension for the `en`
//! locale (the default), including its rounding and "negative zero"
//! handling. A handful of other locales (`de`, `fr`, `es`, `it`, `nl`,
//! `pt`, `ru`, `sv`, ...) are supported for separators and currency
//! placement.
//!
//! ```
//! use illuminate_support::Number;
//!
//! assert_eq!(Number::format(100000.0, None), "100,000");
//! assert_eq!(Number::currency(1000.0, "EUR"), "€1,000.00");
//! assert_eq!(Number::abbreviate(1230000.0, 2), "1.23M");
//! assert_eq!(Number::for_humans(1500000.0, 1), "1.5 million");
//! assert_eq!(Number::spell(102.0), "one hundred two");
//! ```

use std::cell::RefCell;
use std::sync::{LazyLock, RwLock};

static LOCALE: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("en".to_string()));
static CURRENCY: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("USD".to_string()));

thread_local! {
    static LOCALE_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
    static CURRENCY_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Separator conventions for a locale.
struct LocaleFormat {
    decimal: char,
    group: &'static str,
    /// Only group when the integer part has at least this many digits.
    min_grouping: usize,
    /// Whether the currency symbol follows the number ("1,00 €").
    currency_after: bool,
    /// The separator used before a percent sign.
    percent_space: &'static str,
}

fn locale_format(locale: &str) -> LocaleFormat {
    let language = locale.split(['_', '-']).next().unwrap_or(locale).to_lowercase();
    let (decimal, group, min_grouping, currency_after, percent_space) = match language.as_str() {
        "de" | "it" | "nl" | "id" | "tr" | "da" | "el" => (',', ".", 4, true, "\u{A0}"),
        "es" => (',', ".", 5, true, "\u{A0}"),
        "pt" => (',', ".", 4, locale != "pt_BR" && locale != "pt-BR", "\u{A0}"),
        "fr" => (',', "\u{202F}", 4, true, "\u{202F}"),
        "ru" | "sv" | "uk" | "pl" | "cs" | "fi" | "nb" | "no" | "sk" | "hu" => {
            (',', "\u{A0}", 4, true, "\u{A0}")
        }
        _ => ('.', ",", 4, false, ""),
    };
    let currency_after = if language == "pt" && (locale == "pt_BR" || locale == "pt-BR") {
        false
    } else {
        currency_after
    };
    LocaleFormat {
        decimal,
        group,
        min_grouping,
        currency_after,
        percent_space,
    }
}

/// The number of fraction digits a currency uses.
fn currency_digits(code: &str) -> usize {
    match code {
        "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "UGX" | "XAF" | "XOF" | "XPF" | "PYG" | "KMF"
        | "GNF" | "RWF" | "VUV" | "BIF" | "DJF" => 0,
        "BHD" | "KWD" | "OMR" | "JOD" | "TND" | "LYD" | "IQD" => 3,
        _ => 2,
    }
}

/// The symbol a locale uses for a currency.
fn currency_symbol(code: &str, locale: &str) -> String {
    let language = locale.split(['_', '-']).next().unwrap_or(locale).to_lowercase();
    let symbol = match (language.as_str(), code) {
        (_, "EUR") => "€",
        ("fr", "USD") => "$US",
        ("fr", "GBP") => "£GB",
        ("fr", "CAD") => "$CA",
        ("pt", "BRL") | ("en", "BRL") => "R$",
        (_, "USD") => "$",
        (_, "GBP") => "£",
        (_, "JPY") => "¥",
        (_, "INR") => "₹",
        (_, "KRW") => "₩",
        (_, "ILS") => "₪",
        (_, "VND") => "₫",
        (_, "PHP") => "₱",
        (_, "NGN") => "₦",
        ("en", "CAD") => "CA$",
        ("en", "AUD") => "A$",
        ("en", "NZD") => "NZ$",
        ("en", "HKD") => "HK$",
        ("en", "MXN") => "MX$",
        ("en", "TWD") => "NT$",
        ("en", "CNY") => "CN¥",
        ("en", "XAF") => "FCFA",
        ("en", "XOF") => "F\u{202F}CFA",
        _ => return code.to_string(),
    };
    symbol.to_string()
}

/// Static number helpers, mirroring `Illuminate\Support\Number`.
pub struct Number;

impl Number {
    /// Format a number with grouped thousands. Without a precision, up to
    /// three fraction digits are shown (trailing zeros removed).
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::format(100000.0, None), "100,000");
    /// assert_eq!(Number::format(100000.5, Some(2)), "100,000.50");
    /// assert_eq!(Number::format(123.4567, None), "123.457");
    /// ```
    pub fn format(number: f64, precision: Option<usize>) -> String {
        Self::format_with(number, precision, None, None)
    }

    /// Format a number showing at most `max_precision` fraction digits.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::format_max_precision(100000.123, 2), "100,000.12");
    /// assert_eq!(Number::format_max_precision(100000.0, 2), "100,000");
    /// ```
    pub fn format_max_precision(number: f64, max_precision: usize) -> String {
        Self::format_with(number, None, Some(max_precision), None)
    }

    /// Format a number in the given locale.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::format_locale(123456789.0, None, "de"), "123.456.789");
    /// ```
    pub fn format_locale(number: f64, precision: Option<usize>, locale: &str) -> String {
        Self::format_with(number, precision, None, Some(locale))
    }

    /// Format a number with every option Laravel's `Number::format` accepts.
    pub fn format_with(
        number: f64,
        precision: Option<usize>,
        max_precision: Option<usize>,
        locale: Option<&str>,
    ) -> String {
        let locale = locale.map(String::from).unwrap_or_else(Self::default_locale);
        let format = |n: f64| raw_format(n, precision, max_precision, &locale);
        format(without_negative_zero(number, &format))
    }

    /// Convert the given number to its percentage equivalent.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::percentage(10.0, 0), "10%");
    /// assert_eq!(Number::percentage(10.0, 2), "10.00%");
    /// ```
    pub fn percentage(number: f64, precision: usize) -> String {
        Self::percentage_with(number, Some(precision), None, None)
    }

    /// Convert the number to a percentage with at most `max_precision` fraction digits.
    pub fn percentage_max_precision(number: f64, max_precision: usize) -> String {
        Self::percentage_with(number, None, Some(max_precision), None)
    }

    /// Convert the number to a percentage with every option.
    pub fn percentage_with(
        number: f64,
        precision: Option<usize>,
        max_precision: Option<usize>,
        locale: Option<&str>,
    ) -> String {
        let locale = locale.map(String::from).unwrap_or_else(Self::default_locale);
        let precision = if max_precision.is_some() { None } else { Some(precision.unwrap_or(0)) };
        if !number.is_finite() {
            return format!("{}%", raw_format(number, precision, max_precision, &locale));
        }
        let format = |n: f64| {
            let repr = shift_decimal(&format!("{}", (n / 100.0).abs()), 2);
            format_repr(n < 0.0, &repr, precision, max_precision, &locale)
        };
        let formatted = format(without_negative_zero(number, &format));
        format!("{formatted}{}%", locale_format(&locale).percent_space)
    }

    /// Convert the given number to its currency equivalent (an empty code
    /// uses the default currency).
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::currency(1000.0, ""), "$1,000.00");
    /// assert_eq!(Number::currency(1000.0, "EUR"), "€1,000.00");
    /// assert_eq!(Number::currency(1000.0, "JPY"), "¥1,000");
    /// ```
    pub fn currency(number: f64, currency: &str) -> String {
        Self::currency_with(number, currency, None, None)
    }

    /// Convert the number to a currency string with every option.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::currency_with(1000.0, "EUR", Some("de"), None), "1.000,00\u{a0}€");
    /// assert_eq!(Number::currency_with(1000.0, "EUR", Some("de"), Some(0)), "1.000\u{a0}€");
    /// ```
    pub fn currency_with(
        number: f64,
        currency: &str,
        locale: Option<&str>,
        precision: Option<usize>,
    ) -> String {
        let locale = locale.map(String::from).unwrap_or_else(Self::default_locale);
        let code = if currency.is_empty() {
            Self::default_currency()
        } else {
            currency.to_uppercase()
        };
        let digits = precision.unwrap_or_else(|| currency_digits(&code));
        let format = |n: f64| raw_format(n.abs(), Some(digits), None, &locale);
        let number = without_negative_zero(number, &|n| {
            if n < 0.0 { format!("-{}", format(n)) } else { format(n) }
        });
        let formatted = format(number);
        let sign = if number < 0.0 { "-" } else { "" };
        let symbol = currency_symbol(&code, &locale);
        if locale_format(&locale).currency_after {
            format!("{sign}{formatted}\u{A0}{symbol}")
        } else if symbol.chars().all(|c| c.is_ascii_uppercase()) {
            format!("{sign}{symbol}\u{A0}{formatted}")
        } else {
            format!("{sign}{symbol}{formatted}")
        }
    }

    /// Get the default currency.
    pub fn default_currency() -> String {
        CURRENCY_OVERRIDE
            .with(|c| c.borrow().clone())
            .unwrap_or_else(|| CURRENCY.read().unwrap().clone())
    }

    /// Set the default currency.
    pub fn use_currency(currency: impl Into<String>) {
        *CURRENCY.write().unwrap() = currency.into().to_uppercase();
    }

    /// Run the callback with the given default currency (on this thread).
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// let formatted = Number::with_currency("GBP", || Number::currency(10.0, ""));
    /// assert_eq!(formatted, "£10.00");
    /// ```
    pub fn with_currency<R>(currency: &str, callback: impl FnOnce() -> R) -> R {
        let previous = CURRENCY_OVERRIDE.with(|c| c.replace(Some(currency.to_uppercase())));
        let result = callback();
        CURRENCY_OVERRIDE.with(|c| *c.borrow_mut() = previous);
        result
    }

    /// Get the default locale.
    pub fn default_locale() -> String {
        LOCALE_OVERRIDE
            .with(|c| c.borrow().clone())
            .unwrap_or_else(|| LOCALE.read().unwrap().clone())
    }

    /// Set the default locale.
    pub fn use_locale(locale: impl Into<String>) {
        *LOCALE.write().unwrap() = locale.into();
    }

    /// Run the callback with the given default locale (on this thread).
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::with_locale("de", || Number::format(1500.0, None)), "1.500");
    /// ```
    pub fn with_locale<R>(locale: &str, callback: impl FnOnce() -> R) -> R {
        let previous = LOCALE_OVERRIDE.with(|c| c.replace(Some(locale.to_string())));
        let result = callback();
        LOCALE_OVERRIDE.with(|c| *c.borrow_mut() = previous);
        result
    }

    /// Convert the given number to its file size equivalent.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::file_size(1024.0, 0), "1 KB");
    /// assert_eq!(Number::file_size(1536.0, 1), "1.5 KB");
    /// assert_eq!(Number::file_size(1024.0 * 1024.0, 0), "1 MB");
    /// ```
    pub fn file_size(bytes: f64, precision: usize) -> String {
        Self::file_size_with(bytes, precision, None)
    }

    /// Convert the number to its file size equivalent with at most
    /// `max_precision` fraction digits.
    pub fn file_size_max_precision(bytes: f64, max_precision: usize) -> String {
        Self::file_size_with(bytes, 0, Some(max_precision))
    }

    fn file_size_with(bytes: f64, precision: usize, max_precision: Option<usize>) -> String {
        if !bytes.is_finite() {
            return format!("{} B", Self::format_with(bytes, Some(precision), max_precision, None));
        }
        let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
        let mut size = bytes;
        let mut unit = 0;
        while (size.abs() / 1024.0) > 0.9 && unit < units.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }
        format!(
            "{} {}",
            Self::format_with(size, Some(precision), max_precision, None),
            units[unit]
        )
    }

    /// Convert the number to a human readable abbreviation (1K, 1.2M).
    pub fn abbreviate(number: f64, precision: usize) -> String {
        summarize(number, precision, None, &ABBREVIATED_UNITS)
    }

    /// Abbreviate the number with at most `max_precision` fraction digits.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::abbreviate_max_precision(1230.0, 1), "1.2K");
    /// ```
    pub fn abbreviate_max_precision(number: f64, max_precision: usize) -> String {
        summarize(number, 0, Some(max_precision), &ABBREVIATED_UNITS)
    }

    /// Convert the number to a human readable string (1 thousand, 1.2 million).
    pub fn for_humans(number: f64, precision: usize) -> String {
        summarize(number, precision, None, &HUMAN_UNITS)
    }

    /// Convert the number to a human readable string with at most
    /// `max_precision` fraction digits.
    pub fn for_humans_max_precision(number: f64, max_precision: usize) -> String {
        summarize(number, 0, Some(max_precision), &HUMAN_UNITS)
    }

    /// Convert the number to its ordinal form (1st, 2nd, 3rd).
    pub fn ordinal(number: i64) -> String {
        let suffix = match (number.abs() % 10, number.abs() % 100) {
            (_, 11..=13) => "th",
            (1, _) => "st",
            (2, _) => "nd",
            (3, _) => "rd",
            _ => "th",
        };
        format!("{number}{suffix}")
    }

    /// Spell out the given number in English words.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::spell(10.0), "ten");
    /// assert_eq!(Number::spell(1.2), "one point two");
    /// assert_eq!(Number::spell(1234.0), "one thousand two hundred thirty-four");
    /// ```
    pub fn spell(number: f64) -> String {
        spell_number(number)
    }

    /// Spell out the number, unless it is at or below `after` or at or above
    /// `until`, in which case it is formatted with digits instead.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::spell_with(10.0, Some(10.0), None), "10");
    /// assert_eq!(Number::spell_with(11.0, Some(10.0), None), "eleven");
    /// assert_eq!(Number::spell_with(5.0, None, Some(10.0)), "five");
    /// assert_eq!(Number::spell_with(100000.0, None, Some(50000.0)), "100,000");
    /// ```
    pub fn spell_with(number: f64, after: Option<f64>, until: Option<f64>) -> String {
        if after.is_some_and(|after| number <= after) || until.is_some_and(|until| number >= until) {
            return Self::format(number, None);
        }
        spell_number(number)
    }

    /// Spell out the ordinal form of the given number.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::spell_ordinal(1), "first");
    /// assert_eq!(Number::spell_ordinal(21), "twenty-first");
    /// assert_eq!(Number::spell_ordinal(100), "one hundredth");
    /// ```
    pub fn spell_ordinal(number: i64) -> String {
        let cardinal = spell_number(number as f64);
        let (head, last) = match cardinal.rfind([' ', '-']) {
            Some(index) => cardinal.split_at(index + 1),
            None => ("", cardinal.as_str()),
        };
        let ordinal = match last {
            "one" => "first".to_string(),
            "two" => "second".to_string(),
            "three" => "third".to_string(),
            "five" => "fifth".to_string(),
            "eight" => "eighth".to_string(),
            "nine" => "ninth".to_string(),
            "twelve" => "twelfth".to_string(),
            word if word.ends_with('y') => format!("{}ieth", &word[..word.len() - 1]),
            word => format!("{word}th"),
        };
        format!("{head}{ordinal}")
    }

    /// Split the range `start..to` into chunks of `by`, like Laravel's `Number::pairs`
    /// with the default start (0) and offset (1).
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::pairs(25, 10).unwrap(), vec![(0, 9), (10, 19), (20, 25)]);
    /// assert_eq!(Number::pairs_with(25, 10, 0, 0).unwrap(), vec![(0, 10), (10, 20), (20, 25)]);
    /// ```
    pub fn pairs(to: i64, by: i64) -> crate::Result<Vec<(i64, i64)>> {
        Self::pairs_with(to, by, 0, 1)
    }

    /// Split a range into pairs, with a custom start and offset.
    pub fn pairs_with(to: i64, by: i64, start: i64, offset: i64) -> crate::Result<Vec<(i64, i64)>> {
        if by == 0 {
            return Err(crate::error::InvalidArgumentException::new("The $by argument must not be zero.").into());
        }
        let by = by.abs();
        let mut output = Vec::new();
        let mut lower = start;
        while lower < to {
            output.push((lower, (lower + by - offset).min(to)));
            lower += by;
        }
        Ok(output)
    }

    /// Split a floating point range into pairs.
    pub fn pairs_f64(to: f64, by: f64, start: f64, offset: f64) -> crate::Result<Vec<(f64, f64)>> {
        if by == 0.0 {
            return Err(crate::error::InvalidArgumentException::new("The $by argument must not be zero.").into());
        }
        let by = by.abs();
        let mut output = Vec::new();
        let mut lower = start;
        while lower < to {
            output.push((lower, (lower + by - offset).min(to)));
            lower += by;
        }
        Ok(output)
    }

    /// Remove any trailing zero digits after the decimal point.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::trim(12.0), "12");
    /// assert_eq!(Number::trim(12.30), "12.3");
    /// ```
    pub fn trim(number: f64) -> String {
        if number.is_nan() {
            return "NaN".to_string();
        }
        if number.is_infinite() {
            return if number > 0.0 { "INF" } else { "-INF" }.to_string();
        }
        crate::value::format_float(number)
    }

    /// Parse a localized number string ("1,234.56").
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::parse("1,234.56"), Some(1234.56));
    /// assert_eq!(Number::parse_locale("1.234,56", "de"), Some(1234.56));
    /// assert_eq!(Number::parse_int("10.123"), Some(10));
    /// ```
    pub fn parse(value: &str) -> Option<f64> {
        Self::parse_locale(value, &Self::default_locale())
    }

    /// Parse a number string using the given locale's separators.
    pub fn parse_locale(value: &str, locale: &str) -> Option<f64> {
        let format = locale_format(locale);
        let mut normalized = String::with_capacity(value.len());
        for c in value.trim().chars() {
            if format.group.contains(c) || c == '\u{A0}' || c == '\u{202F}' || (c == ' ' && format.decimal == ',') {
                continue;
            }
            normalized.push(if c == format.decimal { '.' } else { c });
        }
        let end = normalized
            .char_indices()
            .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
            .map(|(i, _)| i)
            .unwrap_or(normalized.len());
        normalized[..end].parse::<f64>().ok()
    }

    /// Parse a localized integer string (fractions are truncated).
    pub fn parse_int(value: &str) -> Option<i64> {
        Self::parse(value).map(|n| n.trunc() as i64)
    }

    /// Parse a localized integer string using the given locale.
    pub fn parse_int_locale(value: &str, locale: &str) -> Option<i64> {
        Self::parse_locale(value, locale).map(|n| n.trunc() as i64)
    }

    /// Parse a localized float string.
    pub fn parse_float(value: &str) -> Option<f64> {
        Self::parse(value)
    }

    /// Parse a localized float string using the given locale.
    pub fn parse_float_locale(value: &str, locale: &str) -> Option<f64> {
        Self::parse_locale(value, locale)
    }

    /// Clamp the number between a minimum and maximum.
    pub fn clamp(number: f64, min: f64, max: f64) -> f64 {
        number.max(min).min(max)
    }
}

const ABBREVIATED_UNITS: [(i32, &str); 5] = [(3, "K"), (6, "M"), (9, "B"), (12, "T"), (15, "Q")];
const HUMAN_UNITS: [(i32, &str); 5] = [
    (3, " thousand"),
    (6, " million"),
    (9, " billion"),
    (12, " trillion"),
    (15, " quadrillion"),
];

/// Laravel's `Number::summarize`.
fn summarize(number: f64, precision: usize, max_precision: Option<usize>, units: &[(i32, &str); 5]) -> String {
    let format = |n: f64| Number::format_with(n, Some(precision), max_precision, None);
    if !number.is_finite() {
        return format(number);
    }
    if number == 0.0 {
        return if precision > 0 { format(0.0) } else { "0".to_string() };
    }
    if number < 0.0 {
        let summary = summarize(number.abs(), precision, max_precision, units);
        return if summary == summarize(0.0, precision, max_precision, units) {
            summary
        } else {
            format!("-{summary}")
        };
    }
    if number >= 1e15 {
        return format!("{}{}", summarize(number / 1e15, precision, max_precision, units), units[4].1);
    }
    let unit_for = |exponent: i32| units.iter().find(|(e, _)| *e == exponent).map(|(_, u)| *u);
    let exponent = number.log10().floor() as i64;
    let mut display = (exponent - exponent % 3).max(0) as i32;
    let mut value = number / 10f64.powi(display);
    let mut formatted = format(value);
    if Number::parse(&formatted).is_some_and(|parsed| parsed >= 1000.0) && unit_for(display + 3).is_some() {
        value /= 1000.0;
        display += 3;
        formatted = format(value);
    }
    format!("{formatted}{}", unit_for(display).unwrap_or("")).trim().to_string()
}

/// Laravel's `withoutNegativeZero`: values that format like zero become zero.
fn without_negative_zero(number: f64, format: &dyn Fn(f64) -> String) -> f64 {
    if number == 0.0 {
        return 0.0;
    }
    if number < 0.0 && format(number.abs()) == format(0.0) {
        0.0
    } else {
        number
    }
}

/// Format a number with the locale's separators (no sign handling for -0).
fn raw_format(number: f64, precision: Option<usize>, max_precision: Option<usize>, locale: &str) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number.is_infinite() {
        return if number > 0.0 { "∞" } else { "-∞" }.to_string();
    }
    format_repr(number < 0.0, &format!("{}", number.abs()), precision, max_precision, locale)
}

/// Format the decimal representation of a number. Like ICU, rounding is
/// performed half-even on the shortest decimal representation of the value.
fn format_repr(
    negative: bool,
    repr: &str,
    precision: Option<usize>,
    max_precision: Option<usize>,
    locale: &str,
) -> String {
    let locale = locale_format(locale);
    let digits = max_precision.or(precision).unwrap_or(3);
    let (int_part, frac_part) = round_decimal(repr, digits);
    let frac_part = if max_precision.is_some() || precision.is_none() {
        frac_part.trim_end_matches('0').to_string()
    } else {
        frac_part
    };
    let mut grouped = String::new();
    let should_group = int_part.len() >= locale.min_grouping;
    for (i, c) in int_part.chars().enumerate() {
        if should_group && i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push_str(locale.group);
        }
        grouped.push(c);
    }
    let sign = if negative { "-" } else { "" };
    if frac_part.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}{}{frac_part}", locale.decimal)
    }
}

/// Round a plain decimal string ("123.4565") half-even to `digits` places,
/// returning its integer and fraction parts.
fn round_decimal(repr: &str, digits: usize) -> (String, String) {
    let (int_part, frac_part) = repr.split_once('.').unwrap_or((repr, ""));
    if frac_part.len() <= digits {
        return (int_part.to_string(), format!("{frac_part:0<digits$}"));
    }
    let dropped = &frac_part.as_bytes()[digits..];
    let first = dropped[0] - b'0';
    let rest_nonzero = dropped[1..].iter().any(|b| *b != b'0');
    let mut all: Vec<u8> = int_part.bytes().chain(frac_part.bytes().take(digits)).collect();
    let last = all.last().map(|b| b - b'0').unwrap_or(0);
    if first > 5 || (first == 5 && (rest_nonzero || last % 2 == 1)) {
        let mut index = all.len();
        loop {
            if index == 0 {
                all.insert(0, b'1');
                break;
            }
            index -= 1;
            if all[index] == b'9' {
                all[index] = b'0';
            } else {
                all[index] += 1;
                break;
            }
        }
    }
    let split = all.len() - digits;
    let int_part = String::from_utf8(all[..split].to_vec()).unwrap_or_default();
    let frac_part = String::from_utf8(all[split..].to_vec()).unwrap_or_default();
    (if int_part.is_empty() { "0".to_string() } else { int_part }, frac_part)
}

/// Multiply a plain decimal string by 10^places, by moving the decimal point.
fn shift_decimal(repr: &str, places: usize) -> String {
    let (int_part, frac_part) = repr.split_once('.').unwrap_or((repr, ""));
    let padded = format!("{frac_part:0<places$}");
    let (moved, rest) = padded.split_at(places);
    let int_part = format!("{int_part}{moved}");
    let int_part = int_part.trim_start_matches('0');
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    if rest.is_empty() {
        int_part.to_string()
    } else {
        format!("{int_part}.{rest}")
    }
}

const ONES: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [&str; 7] = ["", "thousand", "million", "billion", "trillion", "quadrillion", "quintillion"];

fn spell_below_thousand(n: u64) -> String {
    let mut words = Vec::new();
    let hundreds = n / 100;
    let rest = n % 100;
    if hundreds > 0 {
        words.push(format!("{} hundred", ONES[hundreds as usize]));
    }
    if rest > 0 {
        if rest < 20 {
            words.push(ONES[rest as usize].to_string());
        } else if rest.is_multiple_of(10) {
            words.push(TENS[(rest / 10) as usize].to_string());
        } else {
            words.push(format!("{}-{}", TENS[(rest / 10) as usize], ONES[(rest % 10) as usize]));
        }
    }
    words.join(" ")
}

fn spell_integer(n: u64) -> String {
    if n == 0 {
        return "zero".to_string();
    }
    let mut groups = Vec::new();
    let mut remaining = n;
    while remaining > 0 {
        groups.push(remaining % 1000);
        remaining /= 1000;
    }
    let mut words = Vec::new();
    for (scale, group) in groups.iter().enumerate().rev() {
        if *group == 0 {
            continue;
        }
        let chunk = spell_below_thousand(*group);
        if scale == 0 {
            words.push(chunk);
        } else {
            words.push(format!("{chunk} {}", SCALES[scale]));
        }
    }
    words.join(" ")
}

fn spell_number(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number.is_infinite() {
        return if number > 0.0 { "infinity" } else { "minus infinity" }.to_string();
    }
    let negative = number < 0.0;
    let abs = number.abs();
    let integer = abs.trunc() as u64;
    let mut words = spell_integer(integer);
    let repr = crate::value::format_float(abs);
    if let Some((_, fraction)) = repr.split_once('.') {
        let digits: Vec<&str> = fraction
            .chars()
            .filter_map(|c| c.to_digit(10))
            .map(|d| ONES[d as usize])
            .collect();
        if !digits.is_empty() {
            words = format!("{words} point {}", digits.join(" "));
        }
    }
    if negative { format!("minus {words}") } else { words }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_formats_like_intl() {
        assert_eq!(Number::format(0.0, None), "0");
        assert_eq!(Number::format(1.0, None), "1");
        assert_eq!(Number::format(100000.0, None), "100,000");
        assert_eq!(Number::format(100000.0, Some(2)), "100,000.00");
        assert_eq!(Number::format(100000.123, Some(2)), "100,000.12");
        assert_eq!(Number::format_max_precision(100000.1234, 3), "100,000.123");
        assert_eq!(Number::format_max_precision(100000.1236, 3), "100,000.124");
        assert_eq!(Number::format(123456789.0, None), "123,456,789");
        assert_eq!(Number::format(-1.0, None), "-1");
        assert_eq!(Number::format(0.2, None), "0.2");
        assert_eq!(Number::format(0.2, Some(2)), "0.20");
        assert_eq!(Number::format_max_precision(0.1234, 3), "0.123");
        assert_eq!(Number::format(1.23, None), "1.23");
        assert_eq!(Number::format(-1.23, None), "-1.23");
        assert_eq!(Number::format(123.456, None), "123.456");
        assert_eq!(Number::format(f64::INFINITY, None), "∞");
        assert_eq!(Number::format(f64::NAN, None), "NaN");
        assert_eq!(Number::format(-0.0, None), "0");
        assert_eq!(Number::format(-0.4, Some(0)), "0");
        assert_eq!(Number::format(-0.04, Some(1)), "0.0");
        assert_eq!(Number::format(-0.06, Some(1)), "-0.1");
    }

    #[test]
    fn it_rounds_half_even_on_the_shortest_representation() {
        assert_eq!(round_decimal("5.325", 2), ("5".to_string(), "32".to_string()));
        assert_eq!(round_decimal("5.335", 2), ("5".to_string(), "34".to_string()));
        assert_eq!(round_decimal("999.5", 0), ("1000".to_string(), String::new()));
        assert_eq!(round_decimal("0.06", 1), ("0".to_string(), "1".to_string()));
        assert_eq!(round_decimal("1.2", 3), ("1".to_string(), "200".to_string()));
        assert_eq!(shift_decimal("0.0012345000000000001", 2), "0.12345000000000001");
        assert_eq!(shift_decimal("10", 2), "1000");
    }

    #[test]
    fn it_formats_in_other_locales() {
        assert_eq!(Number::format_locale(123456789.0, None, "en"), "123,456,789");
        assert_eq!(Number::format_locale(123456789.0, None, "de"), "123.456.789");
        assert_eq!(Number::format_locale(123456789.0, None, "fr"), "123\u{202F}456\u{202F}789");
        assert_eq!(Number::format_locale(123456789.0, None, "ru"), "123\u{A0}456\u{A0}789");
        assert_eq!(Number::format_locale(123456789.0, None, "sv"), "123\u{A0}456\u{A0}789");
        assert_eq!(Number::format_locale(1234.5, Some(2), "es"), "1234,50");
        assert_eq!(Number::with_locale("de", || Number::format(123456789.0, None)), "123.456.789");
        assert_eq!(Number::default_locale(), "en");
    }

    #[test]
    fn it_formats_percentages() {
        assert_eq!(Number::percentage(0.0, 0), "0%");
        assert_eq!(Number::percentage(1.0, 0), "1%");
        assert_eq!(Number::percentage(10.0, 2), "10.00%");
        assert_eq!(Number::percentage(100.0, 0), "100%");
        assert_eq!(Number::percentage_max_precision(100.1234, 3), "100.123%");
        assert_eq!(Number::percentage(1000.0, 0), "1,000%");
        assert_eq!(Number::percentage(1.75, 0), "2%");
        assert_eq!(Number::percentage(1.75, 2), "1.75%");
        assert_eq!(Number::percentage(1.75, 3), "1.750%");
        assert_eq!(Number::percentage(0.12345, 0), "0%");
        assert_eq!(Number::percentage(0.12345, 2), "0.12%");
        assert_eq!(Number::percentage(0.12345, 4), "0.1235%");
        assert_eq!(Number::percentage(-0.4, 0), "0%");
        assert_eq!(Number::percentage(-0.04, 1), "0.0%");
        assert_eq!(Number::percentage(-0.4, 1), "-0.4%");
        assert_eq!(Number::percentage(-5.0, 0), "-5%");
        assert_eq!(Number::percentage_with(10.0, Some(2), None, Some("de")), "10,00\u{A0}%");
    }

    #[test]
    fn it_formats_currencies() {
        assert_eq!(Number::currency(0.0, ""), "$0.00");
        assert_eq!(Number::currency(1.0, ""), "$1.00");
        assert_eq!(Number::currency(10.0, "EUR"), "€10.00");
        assert_eq!(Number::currency(-5.0, ""), "-$5.00");
        assert_eq!(Number::currency(5.325, ""), "$5.32");
        assert_eq!(Number::currency_with(0.0, "", None, Some(0)), "$0");
        assert_eq!(Number::currency_with(10.252, "", None, Some(0)), "$10");
        assert_eq!(Number::currency(-0.001, ""), "$0.00");
        assert_eq!(Number::currency(0.1 + 0.2 - 0.3 - 0.0000000001, ""), "$0.00");
        assert_eq!(Number::currency_with(-0.4, "", None, Some(0)), "$0");
        assert_eq!(Number::currency(-0.006, ""), "-$0.01");
        assert_eq!(Number::currency_with(1.0, "EUR", Some("de"), None), "1,00\u{A0}€");
        assert_eq!(Number::currency_with(1.0, "USD", Some("de"), None), "1,00\u{A0}$");
        assert_eq!(Number::currency_with(1.0, "GBP", Some("de"), None), "1,00\u{A0}£");
        assert_eq!(Number::currency_with(123456789.12345, "USD", Some("de"), None), "123.456.789,12\u{A0}$");
        assert_eq!(Number::currency_with(1234.56, "USD", Some("fr"), None), "1\u{202F}234,56\u{A0}$US");
        assert_eq!(Number::currency(1000.0, "JPY"), "¥1,000");
        assert_eq!(Number::currency(1000.0, "CHF"), "CHF\u{A0}1,000.00");
        assert_eq!(Number::currency(1000.0, "cad"), "CA$1,000.00");
        assert_eq!(Number::default_currency(), "USD");
        assert_eq!(Number::with_currency("GBP", Number::default_currency), "GBP");
    }

    #[test]
    fn it_converts_bytes_to_file_sizes() {
        assert_eq!(Number::file_size(0.0, 0), "0 B");
        assert_eq!(Number::file_size(0.0, 2), "0.00 B");
        assert_eq!(Number::file_size(1.0, 0), "1 B");
        assert_eq!(Number::file_size(1024.0, 0), "1 KB");
        assert_eq!(Number::file_size(2048.0, 2), "2.00 KB");
        assert_eq!(Number::file_size(1264.0, 2), "1.23 KB");
        assert_eq!(Number::file_size_max_precision(1264.12345, 3), "1.234 KB");
        assert_eq!(Number::file_size(1264.0, 3), "1.234 KB");
        assert_eq!(Number::file_size(1024.0 * 1024.0 * 1024.0 * 5.0, 0), "5 GB");
        assert_eq!(Number::file_size(1024f64.powi(4) * 10.0, 0), "10 TB");
        assert_eq!(Number::file_size(1024f64.powi(8), 0), "1 YB");
        assert_eq!(Number::file_size(1024f64.powi(9), 0), "1,024 YB");
        assert_eq!(Number::file_size(-2048.0, 0), "-2 KB");
        assert_eq!(Number::file_size(-1264.0, 2), "-1.23 KB");
        assert_eq!(Number::file_size(f64::INFINITY, 0), "∞ B");
        assert_eq!(Number::file_size(f64::NEG_INFINITY, 0), "-∞ B");
        assert_eq!(Number::file_size(f64::NAN, 0), "NaN B");
        assert_eq!(Number::file_size(1000.0, 0), "1 KB");
    }

    #[test]
    fn it_summarizes_for_humans() {
        assert_eq!(Number::for_humans(1.0, 0), "1");
        assert_eq!(Number::for_humans(1.0, 2), "1.00");
        assert_eq!(Number::for_humans(1000.0, 0), "1 thousand");
        assert_eq!(Number::for_humans(1000.0, 2), "1.00 thousand");
        assert_eq!(Number::for_humans_max_precision(1000.0, 2), "1 thousand");
        assert_eq!(Number::for_humans(1230.0, 0), "1 thousand");
        assert_eq!(Number::for_humans_max_precision(1230.0, 1), "1.2 thousand");
        assert_eq!(Number::for_humans(1_000_000.0, 0), "1 million");
        assert_eq!(Number::for_humans(1e15, 0), "1 quadrillion");
        assert_eq!(Number::for_humans(1e18, 0), "1 thousand quadrillion");
        assert_eq!(Number::for_humans(1234.0, 2), "1.23 thousand");
        assert_eq!(Number::for_humans(12345.0, 0), "12 thousand");
        assert_eq!(Number::for_humans(489939.0, 0), "490 thousand");
        assert_eq!(Number::for_humans(489939.0, 4), "489.9390 thousand");
        assert_eq!(Number::for_humans(500000000.0, 5), "500.00000 million");
        assert_eq!(Number::for_humans(1e21, 0), "1 million quadrillion");
        assert_eq!(Number::for_humans(0.0, 0), "0");
        assert_eq!(Number::for_humans(0.0, 2), "0.00");
        assert_eq!(Number::for_humans(-1000.0, 0), "-1 thousand");
        assert_eq!(Number::for_humans(-1234.0, 2), "-1.23 thousand");
        assert_eq!(Number::for_humans_max_precision(-1100000000000.0, 1), "-1.1 trillion");
        assert_eq!(Number::for_humans(-0.4, 0), "0");
        assert_eq!(Number::for_humans(-0.4, 2), "-0.40");
        assert_eq!(Number::for_humans(0.005, 0), "0");
        assert_eq!(Number::for_humans(0.005, 3), "0.005");
        assert_eq!(Number::for_humans(-0.005, 3), "-0.005");
        assert_eq!(Number::for_humans(999499.0, 0), "999 thousand");
        assert_eq!(Number::for_humans(999500.0, 0), "1 million");
        assert_eq!(Number::for_humans(f64::INFINITY, 0), "∞");
        assert_eq!(Number::for_humans(1500000.0, 1), "1.5 million");
    }

    #[test]
    fn it_abbreviates() {
        assert_eq!(Number::abbreviate(1.0, 0), "1");
        assert_eq!(Number::abbreviate(1.0, 2), "1.00");
        assert_eq!(Number::abbreviate(1000.0, 0), "1K");
        assert_eq!(Number::abbreviate(1000.0, 2), "1.00K");
        assert_eq!(Number::abbreviate_max_precision(1000.0, 2), "1K");
        assert_eq!(Number::abbreviate(489939.0, 0), "490K");
        assert_eq!(Number::abbreviate(1230000.0, 2), "1.23M");
        assert_eq!(Number::abbreviate(1e18, 0), "1KQ");
        assert_eq!(Number::abbreviate(1234567890123456789.0, 2), "1.23KQ");
        assert_eq!(Number::abbreviate(1e33, 0), "1KQQ");
        assert_eq!(Number::abbreviate(-1234.0, 2), "-1.23K");
        assert_eq!(Number::abbreviate(-0.05, 0), "0");
        assert_eq!(Number::abbreviate(999999999.0, 0), "1B");
        assert_eq!(Number::with_locale("de", || Number::abbreviate(999500.0, 0)), "1M");
        assert_eq!(Number::with_locale("fr", || Number::abbreviate(999500.0, 0)), "1M");
    }

    #[test]
    fn it_spells_numbers() {
        assert_eq!(Number::spell(0.0), "zero");
        assert_eq!(Number::spell(10.0), "ten");
        assert_eq!(Number::spell(21.0), "twenty-one");
        assert_eq!(Number::spell(102.0), "one hundred two");
        assert_eq!(Number::spell(10000.0), "ten thousand");
        assert_eq!(Number::spell(1_000_000.0), "one million");
        assert_eq!(Number::spell(-5.0), "minus five");
        assert_eq!(Number::spell(1.2), "one point two");
        assert_eq!(Number::spell_with(9.0, Some(10.0), None), "9");
        assert_eq!(Number::spell_with(9.0, None, Some(10.0)), "nine");
        assert_eq!(Number::spell_with(10000.0, None, Some(50000.0)), "ten thousand");
        assert_eq!(Number::spell_ordinal(2), "second");
        assert_eq!(Number::spell_ordinal(3), "third");
        assert_eq!(Number::spell_ordinal(12), "twelfth");
        assert_eq!(Number::spell_ordinal(20), "twentieth");
        assert_eq!(Number::spell_ordinal(42), "forty-second");
        assert_eq!(Number::spell_ordinal(1000), "one thousandth");
        assert_eq!(Number::ordinal(1), "1st");
        assert_eq!(Number::ordinal(2), "2nd");
        assert_eq!(Number::ordinal(3), "3rd");
        assert_eq!(Number::ordinal(11), "11th");
        assert_eq!(Number::ordinal(21), "21st");
    }

    #[test]
    fn it_builds_pairs() {
        assert_eq!(Number::pairs_with(25, 10, 0, 0).unwrap(), vec![(0, 10), (10, 20), (20, 25)]);
        assert_eq!(Number::pairs_with(25, 10, 0, 1).unwrap(), vec![(0, 9), (10, 19), (20, 25)]);
        assert_eq!(Number::pairs_with(25, 10, 1, 0).unwrap(), vec![(1, 11), (11, 21), (21, 25)]);
        assert_eq!(Number::pairs_with(25, 10, 1, 1).unwrap(), vec![(1, 10), (11, 20), (21, 25)]);
        assert_eq!(Number::pairs_with(2500, 1000, 0, 1).unwrap(), vec![(0, 999), (1000, 1999), (2000, 2500)]);
        assert_eq!(
            Number::pairs_f64(10.0, 2.5, 0.0, 0.5).unwrap(),
            vec![(0.0, 2.0), (2.5, 4.5), (5.0, 7.0), (7.5, 9.5)]
        );
        assert!(Number::pairs(100, 0).is_err());
        assert_eq!(Number::pairs(100, -10).unwrap(), Number::pairs(100, 10).unwrap());
    }

    #[test]
    fn it_trims_parses_and_clamps() {
        assert_eq!(Number::trim(12.0), "12");
        assert_eq!(Number::trim(120.0), "120");
        assert_eq!(Number::trim(12.3), "12.3");
        assert_eq!(Number::trim(12.3456789), "12.3456789");
        assert_eq!(Number::parse("1,234"), Some(1234.0));
        assert_eq!(Number::parse("1,234.5"), Some(1234.5));
        assert_eq!(Number::parse("-1,234.56"), Some(-1234.56));
        assert_eq!(Number::parse_locale("1.234,56", "de"), Some(1234.56));
        assert_eq!(Number::parse_locale("1 234,56", "fr"), Some(1234.56));
        assert_eq!(Number::parse_int("1,234.5"), Some(1234));
        assert_eq!(Number::parse_int("-1,234.56"), Some(-1234));
        assert_eq!(Number::parse_int_locale("1.234", "de"), Some(1234));
        assert_eq!(Number::parse_int("3,000,000,000"), Some(3_000_000_000));
        assert_eq!(Number::parse_float("10"), Some(10.0));
        assert_eq!(Number::parse_float_locale("10", "fr"), Some(10.0));
        assert_eq!(Number::parse("abc"), None);
        assert_eq!(Number::clamp(105.0, 10.0, 100.0), 100.0);
        assert_eq!(Number::clamp(5.0, 10.0, 100.0), 10.0);
        assert_eq!(Number::clamp(20.0, 10.0, 100.0), 20.0);
    }
}
