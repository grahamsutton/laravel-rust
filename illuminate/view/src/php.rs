//! PHP's value semantics: conversions, comparisons, arithmetic and JSON.
//!
//! Blade templates are written with PHP-flavored expressions, so they should
//! behave the way a Laravel developer expects: `"10" == 10`, `0.1 + 0.2`
//! echoes `0.3`, and `json_encode` escapes slashes.

use std::cmp::Ordering;
use std::sync::Arc;

use illuminate_support::Result;

use crate::exception::TypeError;
use crate::value::{ArrayKey, ViewArray, ViewValue};

/// A parsed PHP number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    pub(crate) fn as_i64(self) -> i64 {
        match self {
            Num::Int(i) => i,
            Num::Float(f) => float_to_int(f),
        }
    }

    pub(crate) fn into_value(self) -> ViewValue {
        match self {
            Num::Int(i) => ViewValue::Int(i),
            Num::Float(f) => ViewValue::Float(f),
        }
    }
}

/// PHP's float to int conversion (truncation, out of range becomes 0).
pub(crate) fn float_to_int(f: f64) -> i64 {
    if f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        f as i64
    } else {
        0
    }
}

fn is_php_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0B' | '\x0C')
}

/// Parse a PHP "numeric string" (leading and trailing whitespace allowed).
pub(crate) fn parse_numeric(s: &str) -> Option<Num> {
    let t = s.trim_matches(is_php_whitespace);
    if t.is_empty() {
        return None;
    }
    let bytes = t.as_bytes();
    let mut i = 0;
    if bytes[i] == b'+' || bytes[i] == b'-' {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - digits_start;
    let mut is_float = false;
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        is_float = true;
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - start;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            is_float = true;
            i = j;
        }
    }
    if i != bytes.len() {
        return None;
    }
    if !is_float {
        if let Ok(v) = t.parse::<i64>() {
            return Some(Num::Int(v));
        }
    }
    t.parse::<f64>().ok().map(Num::Float)
}

/// Determine if a value is numeric (`is_numeric`).
pub(crate) fn is_numeric(value: &ViewValue) -> bool {
    match value {
        ViewValue::Int(_) | ViewValue::Float(_) => true,
        ViewValue::Str(s) => parse_numeric(s).is_some(),
        _ => false,
    }
}

/// Format a float the way PHP's `echo` does (`precision = 14`).
pub fn float_to_string(f: f64) -> String {
    format_float_precision(f, 14)
}

/// Format a float like PHP's `%.{precision}G`, used by string casts.
pub(crate) fn format_float_precision(f: f64, precision: i32) -> String {
    if f.is_nan() {
        return "NAN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF".into() } else { "-INF".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let sci = format!("{:.*e}", (precision - 1) as usize, f);
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if exponent < -4 || exponent >= precision {
        let mut mantissa = trim_fraction(mantissa);
        if !mantissa.contains('.') {
            mantissa.push_str(".0");
        }
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}E{sign}{}", exponent.abs())
    } else {
        let decimals = (precision - 1 - exponent).max(0) as usize;
        trim_fraction(&format!("{:.*}", decimals, f))
    }
}

fn trim_fraction(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// Format a float for JSON (`serialize_precision = -1`).
pub(crate) fn float_to_json(f: f64) -> String {
    if !f.is_finite() {
        return "0".into();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{:.1}", f);
    }
    let abs = f.abs();
    if (1e-5..1e15).contains(&abs) {
        format!("{f}")
    } else {
        let s = format!("{f:e}");
        let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
        let mut mantissa = mantissa.to_string();
        if !mantissa.contains('.') {
            mantissa.push_str(".0");
        }
        let exponent: i32 = exponent.parse().unwrap_or(0);
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{}", exponent.abs())
    }
}

/// Convert a value to a string for concatenation and string functions.
///
/// Arrays can't be converted, just like PHP (where it is an error once
/// Laravel turns warnings into exceptions).
pub(crate) fn to_str(value: &ViewValue) -> Result<String> {
    match value {
        ViewValue::Array(_) => Err(TypeError::new("Array to string conversion").into()),
        ViewValue::Closure(_) => Err(TypeError::new("Object of class Closure could not be converted to string").into()),
        ViewValue::Object(o) => o.to_string_value().ok_or_else(|| {
            TypeError::new(format!("Object of class {} could not be converted to string", o.class_name())).into()
        }),
        other => Ok(other.to_string_lossy()),
    }
}

/// Convert a value to a number for arithmetic.
pub(crate) fn to_number(value: &ViewValue, op: &str, other: &ViewValue) -> Result<Num> {
    match value {
        ViewValue::Null => Ok(Num::Int(0)),
        ViewValue::Bool(b) => Ok(Num::Int(*b as i64)),
        ViewValue::Int(i) => Ok(Num::Int(*i)),
        ViewValue::Float(f) => Ok(Num::Float(*f)),
        ViewValue::Str(s) | ViewValue::Html(s) => parse_numeric(s)
            .or_else(|| leading_numeric(s))
            .ok_or_else(|| unsupported(value, op, other)),
        _ => Err(unsupported(value, op, other)),
    }
}

/// PHP 8 accepts "leading numeric" strings like `"5 apples"` (with a warning).
fn leading_numeric(s: &str) -> Option<Num> {
    let t = s.trim_start_matches(is_php_whitespace);
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || ((*c == '-' || *c == '+') && *i == 0)))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    if end == 0 {
        return None;
    }
    parse_numeric(&t[..end])
}

fn unsupported(left: &ViewValue, op: &str, right: &ViewValue) -> illuminate_support::Error {
    TypeError::new(format!(
        "Unsupported operand types: {} {op} {}",
        left.type_name(),
        right.type_name()
    ))
    .into()
}

/// The arithmetic operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

impl Arith {
    fn symbol(self) -> &'static str {
        match self {
            Arith::Add => "+",
            Arith::Sub => "-",
            Arith::Mul => "*",
            Arith::Div => "/",
            Arith::Mod => "%",
            Arith::Pow => "**",
        }
    }
}

/// Perform arithmetic with PHP's semantics.
pub(crate) fn arithmetic(op: Arith, left: &ViewValue, right: &ViewValue) -> Result<ViewValue> {
    if op == Arith::Add {
        if let (ViewValue::Array(a), ViewValue::Array(b)) = (left, right) {
            let mut union = (**a).clone();
            for (key, value) in b.iter() {
                if union.get(key).is_none() {
                    union.insert(key.clone(), value.clone());
                }
            }
            return Ok(ViewValue::Array(Arc::new(union)));
        }
    }
    let symbol = op.symbol();
    let l = to_number(left, symbol, right)?;
    let r = to_number(right, symbol, left)?;
    Ok(match op {
        Arith::Add => match (l, r) {
            (Num::Int(a), Num::Int(b)) => a.checked_add(b).map(Num::Int).unwrap_or(Num::Float(a as f64 + b as f64)),
            _ => Num::Float(l.as_f64() + r.as_f64()),
        },
        Arith::Sub => match (l, r) {
            (Num::Int(a), Num::Int(b)) => a.checked_sub(b).map(Num::Int).unwrap_or(Num::Float(a as f64 - b as f64)),
            _ => Num::Float(l.as_f64() - r.as_f64()),
        },
        Arith::Mul => match (l, r) {
            (Num::Int(a), Num::Int(b)) => a.checked_mul(b).map(Num::Int).unwrap_or(Num::Float(a as f64 * b as f64)),
            _ => Num::Float(l.as_f64() * r.as_f64()),
        },
        Arith::Div => {
            if r.as_f64() == 0.0 {
                return Err(crate::exception::DivisionByZeroError::new("Division by zero").into());
            }
            match (l, r) {
                (Num::Int(a), Num::Int(b)) if a % b == 0 => Num::Int(a / b),
                _ => Num::Float(l.as_f64() / r.as_f64()),
            }
        }
        Arith::Mod => {
            let (a, b) = (l.as_i64(), r.as_i64());
            if b == 0 {
                return Err(crate::exception::DivisionByZeroError::new("Modulo by zero").into());
            }
            Num::Int(a.wrapping_rem(b))
        }
        Arith::Pow => match (l, r) {
            (Num::Int(a), Num::Int(b)) if b >= 0 => u32::try_from(b)
                .ok()
                .and_then(|b| a.checked_pow(b))
                .map(Num::Int)
                .unwrap_or(Num::Float((a as f64).powf(b as f64))),
            _ => Num::Float(l.as_f64().powf(r.as_f64())),
        },
    }
    .into_value())
}

/// PHP's loose equality (`==`), with PHP 8 string-number semantics.
pub fn loose_eq(a: &ViewValue, b: &ViewValue) -> bool {
    use ViewValue::*;
    match (a, b) {
        (Null, Null) => true,
        (Bool(x), other) | (other, Bool(x)) => *x == other.truthy(),
        (Null, Str(s)) | (Str(s), Null) | (Null, Html(s)) | (Html(s), Null) => s.is_empty(),
        (Null, other) | (other, Null) => !other.truthy(),
        (Int(x), Int(y)) => x == y,
        (Int(_) | Float(_), Int(_) | Float(_)) => a.as_f64() == b.as_f64(),
        (Int(_) | Float(_), Str(s) | Html(s)) | (Str(s) | Html(s), Int(_) | Float(_)) => {
            let number = if matches!(a, Int(_) | Float(_)) { a } else { b };
            match parse_numeric(s) {
                Some(n) => n.as_f64() == number.as_f64().unwrap_or(f64::NAN),
                None => number.to_string_lossy() == **s,
            }
        }
        (Str(x) | Html(x), Str(y) | Html(y)) => {
            if x == y {
                return true;
            }
            match (parse_numeric(x), parse_numeric(y)) {
                (Some(m), Some(n)) => m.as_f64() == n.as_f64(),
                _ => false,
            }
        }
        (Array(x), Array(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, value)| y.get(key).is_some_and(|other| loose_eq(value, other)))
        }
        (Object(x), Object(y)) => Arc::ptr_eq(x, y),
        (Object(o), Str(s)) | (Str(s), Object(o)) => o.to_string_value().is_some_and(|v| v == **s),
        (Closure(x), Closure(y)) => x.ptr_eq(y),
        _ => false,
    }
}

/// PHP's strict equality (`===`).
pub fn strict_eq(a: &ViewValue, b: &ViewValue) -> bool {
    use ViewValue::*;
    match (a, b) {
        (Null, Null) => true,
        (Bool(x), Bool(y)) => x == y,
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        (Html(x), Html(y)) => x == y,
        (Array(x), Array(y)) => {
            Arc::ptr_eq(x, y)
                || (x.len() == y.len()
                    && x.iter()
                        .zip(y.iter())
                        .all(|((k1, v1), (k2, v2))| k1 == k2 && strict_eq(v1, v2)))
        }
        (Object(x), Object(y)) => Arc::ptr_eq(x, y),
        (Closure(x), Closure(y)) => x.ptr_eq(y),
        _ => false,
    }
}

/// PHP's comparison (`<=>`), with PHP 8 semantics.
pub fn compare(a: &ViewValue, b: &ViewValue) -> Ordering {
    use ViewValue::*;
    match (a, b) {
        (Bool(_), _) | (_, Bool(_)) | (Null, _) | (_, Null) => {
            if let (Null, Str(s)) | (Null, Html(s)) = (a, b) {
                return "".cmp(&**s);
            }
            if let (Str(s), Null) | (Html(s), Null) = (a, b) {
                return (**s).cmp("");
            }
            a.truthy().cmp(&b.truthy())
        }
        (Int(x), Int(y)) => x.cmp(y),
        (Int(_) | Float(_), Int(_) | Float(_)) => cmp_f64(a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0)),
        (Int(_) | Float(_), Str(s) | Html(s)) => match parse_numeric(s) {
            Some(n) => cmp_f64(a.as_f64().unwrap_or(0.0), n.as_f64()),
            None => a.to_string_lossy().as_str().cmp(&**s),
        },
        (Str(s) | Html(s), Int(_) | Float(_)) => match parse_numeric(s) {
            Some(n) => cmp_f64(n.as_f64(), b.as_f64().unwrap_or(0.0)),
            None => (**s).cmp(b.to_string_lossy().as_str()),
        },
        (Str(x) | Html(x), Str(y) | Html(y)) => match (parse_numeric(x), parse_numeric(y)) {
            (Some(m), Some(n)) => cmp_f64(m.as_f64(), n.as_f64()),
            _ => x.cmp(y),
        },
        (Array(x), Array(y)) => {
            if x.len() != y.len() {
                return x.len().cmp(&y.len());
            }
            for (key, value) in x.iter() {
                match y.get(key) {
                    Some(other) => match compare(value, other) {
                        Ordering::Equal => continue,
                        unequal => return unequal,
                    },
                    None => return Ordering::Greater,
                }
            }
            Ordering::Equal
        }
        (Array(_), _) => Ordering::Greater,
        (_, Array(_)) => Ordering::Less,
        _ => a.to_string_lossy().cmp(&b.to_string_lossy()),
    }
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

// ----------------------------------------------------------------------
// HTML escaping
// ----------------------------------------------------------------------

/// Escape HTML special characters. When `double_encode` is false, existing
/// entities (`&amp;`, `&#039;`, `&eacute;`...) are left untouched.
pub fn escape(value: &str, double_encode: bool) -> String {
    if double_encode {
        return illuminate_support::e(value);
    }
    let mut out = String::with_capacity(value.len() + value.len() / 8);
    let bytes = value.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        let replacement = match bytes[i] {
            b'&' => {
                if entity_length(&value[i..]).is_some() {
                    None
                } else {
                    Some("&amp;")
                }
            }
            b'<' => Some("&lt;"),
            b'>' => Some("&gt;"),
            b'"' => Some("&quot;"),
            b'\'' => Some("&#039;"),
            _ => None,
        };
        if let Some(replacement) = replacement {
            out.push_str(&value[last..i]);
            out.push_str(replacement);
            last = i + 1;
        }
        i += 1;
    }
    out.push_str(&value[last..]);
    out
}

/// The length of an HTML entity at the start of the string, if any.
fn entity_length(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'&') {
        return None;
    }
    let mut i = 1;
    if bytes.get(i) == Some(&b'#') {
        i += 1;
        let hex = matches!(bytes.get(i), Some(b'x') | Some(b'X'));
        if hex {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && (if hex { bytes[i].is_ascii_hexdigit() } else { bytes[i].is_ascii_digit() })
        {
            i += 1;
        }
        if i == start {
            return None;
        }
    } else {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    (bytes.get(i) == Some(&b';')).then_some(i + 1)
}

// ----------------------------------------------------------------------
// JSON
// ----------------------------------------------------------------------

/// `JSON_HEX_TAG`
pub const JSON_HEX_TAG: i64 = 1;
/// `JSON_HEX_AMP`
pub const JSON_HEX_AMP: i64 = 2;
/// `JSON_HEX_APOS`
pub const JSON_HEX_APOS: i64 = 4;
/// `JSON_HEX_QUOT`
pub const JSON_HEX_QUOT: i64 = 8;
/// `JSON_FORCE_OBJECT`
pub const JSON_FORCE_OBJECT: i64 = 16;
/// `JSON_NUMERIC_CHECK`
pub const JSON_NUMERIC_CHECK: i64 = 32;
/// `JSON_UNESCAPED_SLASHES`
pub const JSON_UNESCAPED_SLASHES: i64 = 64;
/// `JSON_PRETTY_PRINT`
pub const JSON_PRETTY_PRINT: i64 = 128;
/// `JSON_UNESCAPED_UNICODE`
pub const JSON_UNESCAPED_UNICODE: i64 = 256;
/// `JSON_PRESERVE_ZERO_FRACTION`
pub const JSON_PRESERVE_ZERO_FRACTION: i64 = 1024;
/// `JSON_THROW_ON_ERROR`
pub const JSON_THROW_ON_ERROR: i64 = 4_194_304;

/// The flags `@json` uses by default.
pub const BLADE_JSON_FLAGS: i64 = JSON_HEX_TAG | JSON_HEX_APOS | JSON_HEX_AMP | JSON_HEX_QUOT;

/// Encode a value as JSON exactly like PHP's `json_encode`.
pub fn json_encode(value: &ViewValue, flags: i64) -> Result<String> {
    let mut out = String::new();
    encode_value(value, flags, 0, &mut out)?;
    Ok(out)
}

fn encode_value(value: &ViewValue, flags: i64, depth: usize, out: &mut String) -> Result<()> {
    if depth > 512 {
        return Err(crate::exception::RuntimeException::new("Maximum stack depth exceeded").into());
    }
    match value {
        ViewValue::Null => out.push_str("null"),
        ViewValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        ViewValue::Int(i) => out.push_str(&i.to_string()),
        ViewValue::Float(f) => {
            if flags & JSON_PRESERVE_ZERO_FRACTION == 0 && f.fract() == 0.0 && f.abs() < 1e15 {
                out.push_str(&format!("{}", *f as i64));
            } else {
                out.push_str(&float_to_json(*f));
            }
        }
        ViewValue::Str(s) | ViewValue::Html(s) => {
            if flags & JSON_NUMERIC_CHECK != 0 {
                if let Some(n) = parse_numeric(s) {
                    return encode_value(&n.into_value(), flags, depth, out);
                }
            }
            encode_string(s, flags, out)
        }
        ViewValue::Array(array) => encode_array(array, flags, depth, out)?,
        ViewValue::Object(object) => {
            let json = ViewValue::from(object.to_json());
            if let ViewValue::Array(array) = &json {
                if array.is_empty() && !object.to_json().is_array() {
                    out.push_str("{}");
                    return Ok(());
                }
            }
            encode_value(&json, flags, depth + 1, out)?
        }
        ViewValue::Closure(_) => out.push_str("{}"),
    }
    Ok(())
}

fn encode_array(array: &ViewArray, flags: i64, depth: usize, out: &mut String) -> Result<()> {
    let pretty = flags & JSON_PRETTY_PRINT != 0;
    let as_list = flags & JSON_FORCE_OBJECT == 0 && array.is_list();
    if array.is_empty() {
        out.push_str(if as_list { "[]" } else { "{}" });
        return Ok(());
    }
    out.push(if as_list { '[' } else { '{' });
    for (index, (key, value)) in array.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        if pretty {
            out.push('\n');
            out.push_str(&"    ".repeat(depth + 1));
        }
        if !as_list {
            encode_string(&key.to_string(), flags, out);
            out.push(':');
            if pretty {
                out.push(' ');
            }
        }
        encode_value(value, flags, depth + 1, out)?;
    }
    if pretty {
        out.push('\n');
        out.push_str(&"    ".repeat(depth));
    }
    out.push(if as_list { ']' } else { '}' });
    Ok(())
}

fn encode_string(s: &str, flags: i64, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' if flags & JSON_HEX_QUOT != 0 => out.push_str("\\u0022"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' if flags & JSON_UNESCAPED_SLASHES == 0 => out.push_str("\\/"),
            '<' if flags & JSON_HEX_TAG != 0 => out.push_str("\\u003C"),
            '>' if flags & JSON_HEX_TAG != 0 => out.push_str("\\u003E"),
            '&' if flags & JSON_HEX_AMP != 0 => out.push_str("\\u0026"),
            '\'' if flags & JSON_HEX_APOS != 0 => out.push_str("\\u0027"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) > 0x7F && flags & JSON_UNESCAPED_UNICODE == 0 => {
                let mut buffer = [0u16; 2];
                for unit in c.encode_utf16(&mut buffer) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Decode JSON into a view value (objects become associative arrays).
pub fn json_decode(source: &str) -> ViewValue {
    serde_json::from_str::<serde_json::Value>(source)
        .map(ViewValue::from)
        .unwrap_or(ViewValue::Null)
}

/// Implement `Js::from()`: a JavaScript expression for the given value.
pub fn js_from(value: &ViewValue, flags: i64) -> Result<String> {
    const REQUIRED: i64 = JSON_HEX_TAG | JSON_HEX_APOS | JSON_HEX_AMP | JSON_HEX_QUOT | JSON_UNESCAPED_UNICODE;
    let value = match value {
        ViewValue::Html(s) => ViewValue::Str(s.clone()),
        ViewValue::Object(o) => match o.to_html() {
            Some(html) => ViewValue::from(html),
            None => value.clone(),
        },
        other => other.clone(),
    };
    let json = json_encode(&value, flags | REQUIRED)?;
    if let ViewValue::Str(_) = value {
        return Ok(format!("'{}'", &json[1..json.len() - 1]));
    }
    if json == "[]" || json == "{}" {
        return Ok(json);
    }
    if json.starts_with(['"', '{', '[']) {
        let mut quoted = String::new();
        encode_string(&json, flags | REQUIRED, &mut quoted);
        return Ok(format!("JSON.parse('{}')", &quoted[1..quoted.len() - 1]));
    }
    Ok(json)
}

/// `Arr::toCssClasses()`.
pub fn css_classes(value: &ViewValue) -> String {
    conditional_list(value, |s| s.to_string())
}

/// `Arr::toCssStyles()`.
pub fn css_styles(value: &ViewValue) -> String {
    conditional_list(value, |s| {
        if s.ends_with(';') { s.to_string() } else { format!("{s};") }
    })
}

fn conditional_list(value: &ViewValue, map: impl Fn(&str) -> String) -> String {
    let mut out: Vec<String> = Vec::new();
    match value {
        ViewValue::Array(array) => {
            for (key, constraint) in array.iter() {
                match key {
                    ArrayKey::Int(_) => out.push(map(&constraint.to_string_lossy())),
                    ArrayKey::Str(name) => {
                        if constraint.truthy() {
                            out.push(map(name));
                        }
                    }
                }
            }
        }
        ViewValue::Null => {}
        other => out.push(map(&other.to_string_lossy())),
    }
    out.join(" ")
}

/// PHP's `trim()` characters.
pub(crate) fn php_trim(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn floats_print_like_php() {
        assert_eq!(float_to_string(0.1 + 0.2), "0.3");
        assert_eq!(float_to_string(2.0), "2");
        assert_eq!(float_to_string(2.5), "2.5");
        assert_eq!(float_to_string(1.0 / 3.0), "0.33333333333333");
        assert_eq!(float_to_string(1e25), "1.0E+25");
        assert_eq!(float_to_string(-1.5e-7), "-1.5E-7");
    }

    #[test]
    fn numeric_strings_are_detected() {
        assert_eq!(parse_numeric(" 42 "), Some(Num::Int(42)));
        assert_eq!(parse_numeric("1.5"), Some(Num::Float(1.5)));
        assert_eq!(parse_numeric("1e3"), Some(Num::Float(1000.0)));
        assert_eq!(parse_numeric(".5"), Some(Num::Float(0.5)));
        assert_eq!(parse_numeric("abc"), None);
        assert_eq!(parse_numeric("12abc"), None);
        assert_eq!(parse_numeric(""), None);
    }

    #[test]
    fn loose_comparison_follows_php_8() {
        assert!(loose_eq(&"10".into(), &10.into()));
        assert!(loose_eq(&"1e1".into(), &"10".into()));
        assert!(!loose_eq(&"abc".into(), &0.into()));
        assert!(loose_eq(&ViewValue::Null, &false.into()));
        assert!(loose_eq(&ViewValue::Null, &"".into()));
        assert!(loose_eq(&ViewValue::Null, &0.into()));
        assert!(!loose_eq(&ViewValue::Null, &"a".into()));
        assert!(loose_eq(&1.into(), &1.0.into()));
        assert!(!strict_eq(&1.into(), &1.0.into()));
    }

    #[test]
    fn arithmetic_follows_php() {
        assert_eq!(arithmetic(Arith::Div, &10.into(), &4.into()).unwrap(), ViewValue::Float(2.5));
        assert_eq!(arithmetic(Arith::Div, &10.into(), &5.into()).unwrap(), ViewValue::Int(2));
        assert_eq!(arithmetic(Arith::Add, &"5".into(), &3.into()).unwrap(), ViewValue::Int(8));
        assert!(arithmetic(Arith::Div, &1.into(), &0.into()).is_err());
        assert!(arithmetic(Arith::Add, &"abc".into(), &1.into()).is_err());
        assert_eq!(arithmetic(Arith::Pow, &2.into(), &10.into()).unwrap(), ViewValue::Int(1024));
    }

    #[test]
    fn json_encoding_matches_php() {
        let value = ViewValue::from(json!({"url": "http://laravel.com", "name": "Café", "tags": [], "meta": {}}));
        assert_eq!(
            json_encode(&value, 0).unwrap(),
            r#"{"url":"http:\/\/laravel.com","name":"Café","tags":[],"meta":[]}"#
        );
        assert_eq!(
            json_encode(&ViewValue::from("<a href='x'>&</a>"), BLADE_JSON_FLAGS).unwrap(),
            r#""<a href='x'>&<\/a>""#
        );
        assert_eq!(
            json_encode(&ViewValue::from(json!({"a": [1, 2]})), JSON_PRETTY_PRINT).unwrap(),
            "{\n    \"a\": [\n        1,\n        2\n    ]\n}"
        );
    }

    #[test]
    fn js_from_matches_laravel() {
        assert_eq!(js_from(&ViewValue::from("it's"), 0).unwrap(), "'it\\u0027s'");
        assert_eq!(js_from(&ViewValue::from(json!(["a"])), 0).unwrap(), "JSON.parse('[\\u0022a\\u0022]')");
        assert_eq!(js_from(&ViewValue::from(1), 0).unwrap(), "1");
        assert_eq!(js_from(&ViewValue::empty_array(), 0).unwrap(), "[]");
    }

    #[test]
    fn escaping_can_skip_double_encoding() {
        assert_eq!(escape("&amp; & <b>", true), "&amp;amp; &amp; &lt;b&gt;");
        assert_eq!(escape("&amp; & <b> &#039; &#x27;", false), "&amp; &amp; &lt;b&gt; &#039; &#x27;");
    }

    #[test]
    fn css_helpers_compile_conditional_lists() {
        let classes = ViewValue::from(json!({"0": "p-4", "font-bold": false, "bg-red": true}));
        assert_eq!(css_classes(&classes), "p-4 bg-red");
        let styles = ViewValue::from(json!({"0": "color: red", "font-weight: bold": true}));
        assert_eq!(css_styles(&styles), "color: red; font-weight: bold;");
    }
}
