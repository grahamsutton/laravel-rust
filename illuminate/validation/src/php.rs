//! Small helpers that reproduce the PHP semantics Laravel's validator relies
//! on: `is_numeric`, `FILTER_VALIDATE_INT`, loose comparisons, `trim`,
//! `str_getcsv`, and exact decimal comparisons (Laravel uses `brick/math`).

use std::cmp::Ordering;

use illuminate_support::{Value, ValueExt};

use crate::data::is_file;

/// PHP's `trim()` character list: space, tab, newline, carriage return, NUL
/// and vertical tab.
pub(crate) fn trim(value: &str) -> &str {
    value.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'))
}

/// PHP's `is_numeric()` for strings.
pub(crate) fn is_numeric_str(value: &str) -> bool {
    // Leading and trailing whitespace is allowed (PHP 8).
    let s = value.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0B' | '\x0C'));
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - frac_start;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == bytes.len()
}

/// PHP's `is_numeric()` for any value.
pub(crate) fn is_numeric(value: &Value) -> bool {
    match value {
        Value::Number(_) => true,
        Value::String(s) => is_numeric_str(s),
        _ => false,
    }
}

/// PHP's `is_int()`.
pub(crate) fn is_int(value: &Value) -> bool {
    matches!(value, Value::Number(n) if n.is_i64() || n.is_u64())
}

/// `filter_var($value, FILTER_VALIDATE_INT) !== false`.
pub(crate) fn filter_int(value: &Value) -> bool {
    match value {
        Value::Number(n) => {
            if n.is_i64() {
                return true;
            }
            if n.is_u64() {
                return false;
            }
            match n.as_f64() {
                Some(f) => f.fract() == 0.0 && f.abs() < 9.2e18,
                None => false,
            }
        }
        Value::Bool(true) => true,
        Value::String(s) => filter_int_str(s),
        _ => false,
    }
}

fn filter_int_str(value: &str) -> bool {
    let s = value.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0B'));
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return false;
    }
    s.parse::<i64>().is_ok()
}

/// PHP's `gettype()`, which Laravel uses to compare the types of two fields.
pub(crate) fn gettype(value: Option<&Value>) -> &'static str {
    match value {
        None | Some(Value::Null) => "NULL",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(n)) if n.is_f64() => "double",
        Some(Value::Number(_)) => "integer",
        Some(Value::String(_)) => "string",
        Some(v) if is_file(v) => "object",
        Some(_) => "array",
    }
}

/// Convert a value to a string the way PHP's `(string)` cast would.
pub(crate) fn to_php_string(value: &Value) -> String {
    value.to_string_lossy()
}

/// PHP 8's loose comparison (`==`).
pub(crate) fn loose_eq(a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        (Null, Null) => true,
        (Bool(x), other) | (other, Bool(x)) => *x == other.truthy(),
        (Null, String(s)) | (String(s), Null) => s.is_empty(),
        (Null, Number(n)) | (Number(n), Null) => n.as_f64() == Some(0.0),
        (Null, Array(items)) | (Array(items), Null) => items.is_empty(),
        (Null, Object(map)) | (Object(map), Null) => map.is_empty(),
        (Number(x), Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Number(n), String(s)) | (String(s), Number(n)) => {
            if is_numeric_str(s) {
                trim(s).parse::<f64>().ok() == n.as_f64()
            } else {
                to_php_string(&Number(n.clone())) == *s
            }
        }
        (String(x), String(y)) => {
            if is_numeric_str(x) && is_numeric_str(y) {
                match (Dec::parse(x), Dec::parse(y)) {
                    (Some(x), Some(y)) => x.cmp(&y) == Ordering::Equal,
                    _ => x == y,
                }
            } else {
                x == y
            }
        }
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| loose_eq(a, b)),
        (Object(x), Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| loose_eq(v, w)))
        }
        (Array(items), Object(map)) | (Object(map), Array(items)) => {
            items.len() == map.len()
                && items
                    .iter()
                    .enumerate()
                    .all(|(i, v)| map.get(&i.to_string()).is_some_and(|w| loose_eq(v, w)))
        }
        _ => false,
    }
}

/// PHP's `in_array()`, strict or loose.
pub(crate) fn in_array(needle: &Value, haystack: &[Value], strict: bool) -> bool {
    haystack.iter().any(|item| {
        if strict {
            strict_eq(needle, item)
        } else {
            loose_eq(needle, item)
        }
    })
}

/// PHP's strict comparison (`===`).
pub(crate) fn strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.is_f64() != y.is_f64() {
                return false;
            }
            match (x.as_i64(), y.as_i64()) {
                (Some(x), Some(y)) => x == y,
                _ => match (x.as_u64(), y.as_u64()) {
                    (Some(x), Some(y)) => x == y,
                    _ => x.as_f64() == y.as_f64(),
                },
            }
        }
        _ => a == b,
    }
}

/// PHP's `str_getcsv($value, ',', '"', '\\')`, used to split rule parameters.
pub(crate) fn str_getcsv(input: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut at_field_start = true;
    let mut in_quotes = false;

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        current.push('"');
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                }
                '\\' => {
                    // The escape character is kept verbatim, but protects the next quote.
                    current.push('\\');
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                other => current.push(other),
            }
            continue;
        }
        match c {
            ',' => {
                fields.push(std::mem::take(&mut current));
                at_field_start = true;
                continue;
            }
            '"' if at_field_start => in_quotes = true,
            other => current.push(other),
        }
        at_field_start = false;
    }
    fields.push(current);
    fields
}

/// An exact decimal number, standing in for `brick/math`'s `BigNumber`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dec {
    negative: bool,
    /// Integer digits without leading zeros.
    int: String,
    /// Fraction digits without trailing zeros.
    frac: String,
}

impl Dec {
    /// Parse a numeric string (`" -12.50 "`, `"1e3"`). Exponents beyond
    /// ±1000 are rejected, like Laravel's `ensureExponentWithinAllowedRange`.
    pub(crate) fn parse(value: &str) -> Option<Dec> {
        let s = trim(value);
        if !is_numeric_str(s) {
            return None;
        }
        let s = s.trim();
        let (negative, rest) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let (mantissa, exponent) = match rest.find(['e', 'E']) {
            Some(pos) => (&rest[..pos], rest[pos + 1..].parse::<i64>().ok()?),
            None => (rest, 0),
        };
        if exponent.abs() > 1000 {
            return None;
        }
        let (int, frac) = match mantissa.find('.') {
            Some(pos) => (&mantissa[..pos], &mantissa[pos + 1..]),
            None => (mantissa, ""),
        };
        let mut digits: String = format!("{int}{frac}");
        let mut point = int.len() as i64 + exponent;
        if point < 0 {
            digits = "0".repeat((-point) as usize) + &digits;
            point = 0;
        }
        let point = point as usize;
        if point > digits.len() {
            digits.push_str(&"0".repeat(point - digits.len()));
        }
        let int = digits[..point].trim_start_matches('0').to_string();
        let frac = digits[point..].trim_end_matches('0').to_string();
        let negative = negative && !(int.is_empty() && frac.is_empty());
        Some(Dec { negative, int, frac })
    }

    /// Build a decimal from an integer.
    pub(crate) fn from_usize(value: usize) -> Dec {
        Dec::parse(&value.to_string()).expect("integers are numeric")
    }

    /// Build a decimal from a float.
    pub(crate) fn from_f64(value: f64) -> Option<Dec> {
        if !value.is_finite() {
            return None;
        }
        Dec::parse(&format!("{value}"))
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.int.is_empty() && self.frac.is_empty()
    }

    fn cmp_abs(&self, other: &Dec) -> Ordering {
        self.int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.cmp(&other.int))
            .then_with(|| {
                let width = self.frac.len().max(other.frac.len());
                let a = format!("{:0<width$}", self.frac);
                let b = format!("{:0<width$}", other.frac);
                a.cmp(&b)
            })
    }

    /// The number of fraction digits.
    pub(crate) fn scale(&self) -> usize {
        self.frac.len()
    }

    /// The value scaled by `10^scale` as an integer, when it fits in an `i128`.
    pub(crate) fn scaled(&self, scale: usize) -> Option<i128> {
        let digits = format!("{}{:0<scale$}", self.int, self.frac);
        let digits = if digits.is_empty() { "0".to_string() } else { digits };
        let magnitude: i128 = digits.parse().ok()?;
        Some(if self.negative { -magnitude } else { magnitude })
    }

    /// An approximate float representation.
    pub(crate) fn to_f64(&self) -> f64 {
        let s = format!(
            "{}{}.{}",
            if self.negative { "-" } else { "" },
            if self.int.is_empty() { "0" } else { &self.int },
            if self.frac.is_empty() { "0" } else { &self.frac }
        );
        s.parse().unwrap_or(0.0)
    }
}

impl Ord for Dec {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.cmp_abs(other),
            (true, true) => other.cmp_abs(self),
        }
    }
}

impl PartialOrd for Dec {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Dec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(if self.int.is_empty() { "0" } else { &self.int })?;
        if !self.frac.is_empty() {
            write!(f, ".{}", self.frac)?;
        }
        Ok(())
    }
}

/// Determine if `numerator` is a multiple of `denominator`, exactly.
pub(crate) fn is_multiple_of(numerator: &Dec, denominator: &Dec) -> bool {
    if numerator.is_zero() && denominator.is_zero() {
        return false;
    }
    if numerator.is_zero() {
        return true;
    }
    if denominator.is_zero() {
        return false;
    }
    let scale = numerator.scale().max(denominator.scale());
    match (numerator.scaled(scale), denominator.scaled(scale)) {
        (Some(n), Some(d)) => n % d == 0,
        _ => {
            let n = numerator.to_f64();
            let d = denominator.to_f64();
            (n % d).abs() < f64::EPSILON
        }
    }
}

/// PHP's comparison operators between two optional timestamps (where a
/// missing timestamp behaves like PHP's `null`).
pub(crate) fn compare_optional(first: Option<i128>, second: Option<i128>, operator: &str) -> bool {
    match (first, second) {
        (Some(a), Some(b)) => match operator {
            "<" => a < b,
            ">" => a > b,
            "<=" => a <= b,
            ">=" => a >= b,
            _ => a == b,
        },
        (None, None) => matches!(operator, "<=" | ">=" | "="),
        // `null` compared with a number converts both sides to booleans.
        (a, b) => {
            let a = a.is_some_and(|v| v != 0);
            let b = b.is_some_and(|v| v != 0);
            match operator {
                "<" => !a & b,
                ">" => a & !b,
                "<=" => a <= b,
                ">=" => a >= b,
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_detects_numeric_strings() {
        for ok in ["1", " 1", "1 ", "-1.5", ".5", "1.", "1e3", "+2", "0"] {
            assert!(is_numeric_str(ok), "{ok}");
        }
        for bad in ["", ".", "abc", "1e", "0x1A", "1 2", "--1"] {
            assert!(!is_numeric_str(bad), "{bad}");
        }
    }

    #[test]
    fn it_filters_integers_like_php() {
        assert!(filter_int(&json!(5)));
        assert!(filter_int(&json!("5")));
        assert!(filter_int(&json!(" -5 ")));
        assert!(filter_int(&json!(5.0)));
        assert!(!filter_int(&json!("05")));
        assert!(!filter_int(&json!("5.5")));
        assert!(!filter_int(&json!(5.5)));
        assert!(!filter_int(&json!(null)));
    }

    #[test]
    fn it_compares_loosely() {
        assert!(loose_eq(&json!("1"), &json!(1)));
        assert!(loose_eq(&json!("1.0"), &json!("1")));
        assert!(loose_eq(&json!(true), &json!("yes")));
        assert!(!loose_eq(&json!("abc"), &json!(0)));
        assert!(loose_eq(&json!(null), &json!("")));
    }

    #[test]
    fn it_splits_csv_parameters() {
        assert_eq!(str_getcsv("a,b,c"), vec!["a", "b", "c"]);
        assert_eq!(str_getcsv(r#""a,b","c""d""#), vec!["a,b", "c\"d"]);
        assert_eq!(str_getcsv(""), vec![""]);
    }

    #[test]
    fn decimals_compare_exactly() {
        let a = Dec::parse("10.50").unwrap();
        let b = Dec::parse("10.5").unwrap();
        assert_eq!(a, b);
        assert!(Dec::parse("99999999999999999999999").unwrap() > Dec::parse("99999999999999999999998").unwrap());
        assert!(Dec::parse("-2").unwrap() < Dec::parse("-1.5").unwrap());
        assert_eq!(Dec::parse("1e3").unwrap(), Dec::parse("1000").unwrap());
        assert!(Dec::parse("1e2000").is_none());
        assert!(is_multiple_of(&Dec::parse("10.5").unwrap(), &Dec::parse("3.5").unwrap()));
        assert!(!is_multiple_of(&Dec::parse("10").unwrap(), &Dec::parse("3").unwrap()));
    }
}
