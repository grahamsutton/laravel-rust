//! The PHP (and Laravel helper) functions available inside templates.
//!
//! Functions registered with `Blade::function()` take precedence over these
//! built-ins, which is how the framework wires up `route()`, `old()`,
//! `__()` and friends to the real services.

use std::sync::Arc;

use illuminate_support::{Carbon, Result, Str, ValueExt};

use crate::exception::{TypeError, error};
use crate::expr::eval::call_callable;
use crate::objects::{DateObject, OptionalObject};
use crate::php::{self, Num};
use crate::registry::Registry;
use crate::value::{ArrayKey, ViewArray, ViewValue};

pub(crate) static NULL: ViewValue = ViewValue::Null;

/// The argument at `index` (or `null`).
pub(crate) fn arg(args: &[ViewValue], index: usize) -> &ViewValue {
    args.get(index).unwrap_or(&NULL)
}

/// The argument at `index` as a string.
pub(crate) fn str_arg(args: &[ViewValue], index: usize) -> Result<String> {
    php::to_str(arg(args, index))
}

/// The argument at `index` as an optional string.
pub(crate) fn opt_str(args: &[ViewValue], index: usize) -> Result<Option<String>> {
    match args.get(index) {
        None | Some(ViewValue::Null) => Ok(None),
        Some(value) => php::to_str(value).map(Some),
    }
}

/// The argument at `index` as an integer.
pub(crate) fn int_arg(args: &[ViewValue], index: usize, default: i64) -> i64 {
    match args.get(index) {
        None | Some(ViewValue::Null) => default,
        Some(value) => value.as_i64().unwrap_or(default),
    }
}

/// The argument at `index` as a float.
pub(crate) fn float_arg(args: &[ViewValue], index: usize) -> f64 {
    arg(args, index).as_f64().unwrap_or(0.0)
}

/// The argument at `index` as a boolean.
pub(crate) fn bool_arg(args: &[ViewValue], index: usize, default: bool) -> bool {
    args.get(index).map(ViewValue::truthy).unwrap_or(default)
}

/// The argument at `index` as an array.
pub(crate) fn array_arg(
    args: &[ViewValue],
    index: usize,
    function: &str,
) -> Result<Arc<ViewArray>> {
    to_array(arg(args, index)).ok_or_else(|| {
        TypeError::new(format!(
            "{function}(): Argument #{} must be of type array, {} given",
            index + 1,
            arg(args, index).type_name()
        ))
        .into()
    })
}

/// View a value as an array, when possible.
pub(crate) fn to_array(value: &ViewValue) -> Option<Arc<ViewArray>> {
    match value {
        ViewValue::Array(array) => Some(match crate::methods::paginator_items(array) {
            Some(items) => items,
            None => array.clone(),
        }),
        ViewValue::Object(object) => object.iterate().map(|pairs| {
            Arc::new(
                pairs
                    .into_iter()
                    .map(|(k, v)| (ArrayKey::from_value(&k).unwrap_or(ArrayKey::Int(0)), v))
                    .collect(),
            )
        }),
        _ => None,
    }
}

macro_rules! builtins {
    ($($name:literal),* $(,)?) => {
        const BUILTINS: &[&str] = &[$($name),*];
    };
}

builtins!(
    "count",
    "sizeof",
    "strtoupper",
    "strtolower",
    "mb_strtoupper",
    "mb_strtolower",
    "ucfirst",
    "lcfirst",
    "ucwords",
    "trim",
    "ltrim",
    "rtrim",
    "chop",
    "strlen",
    "mb_strlen",
    "substr",
    "mb_substr",
    "str_repeat",
    "str_replace",
    "str_ireplace",
    "str_contains",
    "str_starts_with",
    "str_ends_with",
    "strpos",
    "stripos",
    "strrpos",
    "strrev",
    "str_pad",
    "str_split",
    "mb_str_split",
    "substr_count",
    "wordwrap",
    "nl2br",
    "e",
    "htmlspecialchars",
    "htmlentities",
    "htmlspecialchars_decode",
    "html_entity_decode",
    "strip_tags",
    "addslashes",
    "sprintf",
    "vsprintf",
    "number_format",
    "round",
    "floor",
    "ceil",
    "abs",
    "max",
    "min",
    "intval",
    "floatval",
    "doubleval",
    "strval",
    "boolval",
    "is_null",
    "is_array",
    "is_string",
    "is_numeric",
    "is_int",
    "is_integer",
    "is_long",
    "is_float",
    "is_double",
    "is_bool",
    "is_object",
    "is_callable",
    "is_iterable",
    "is_countable",
    "is_scalar",
    "gettype",
    "get_debug_type",
    "in_array",
    "array_key_exists",
    "key_exists",
    "array_keys",
    "array_values",
    "array_merge",
    "array_slice",
    "array_filter",
    "array_map",
    "array_reverse",
    "array_unique",
    "array_sum",
    "array_product",
    "array_search",
    "array_column",
    "array_combine",
    "array_flip",
    "array_chunk",
    "array_fill",
    "array_fill_keys",
    "array_key_first",
    "array_key_last",
    "array_is_list",
    "array_diff",
    "array_diff_key",
    "array_intersect",
    "array_intersect_key",
    "array_count_values",
    "array_pad",
    "iterator_to_array",
    "implode",
    "join",
    "explode",
    "json_encode",
    "json_decode",
    "range",
    "date",
    "time",
    "strtotime",
    "now",
    "today",
    "collect",
    "data_get",
    "value",
    "head",
    "last",
    "blank",
    "filled",
    "optional",
    "dump",
    "dd",
    "class_basename",
    "str",
    "method_field",
    "csrf_field",
    "config",
    "env",
    "__",
    "trans",
    "trans_choice",
    "old",
    "session",
    "auth_check",
    "gate_check",
    "app_environment",
    "vite",
    "vite_react_refresh",
    "fonts",
    "context",
    "context_has",
    "urlencode",
    "rawurlencode",
    "urldecode",
    "rawurldecode",
    "http_build_query",
    "print_r",
    "var_export",
    "ctype_digit",
    "ctype_alpha",
    "ctype_alnum",
    "ctype_upper",
    "ctype_lower",
    "ctype_space",
    "lcg_value",
    "pi",
    "sqrt",
    "pow",
    "intdiv",
    "fmod",
    "array_rand",
    "uniqid",
    "md5",
    "spl_object_id",
    "tap",
    "with",
    "throw_if",
    "throw_unless",
    "abort",
    "abort_if",
    "abort_unless",
    "to_route",
    "retry",
    "now_timestamp",
    "request",
    "app",
    "app_locale",
    "app_version",
);

/// Determine if a built-in function exists.
pub(crate) fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// Call a built-in function. Returns `None` when there is no such function.
pub(crate) fn call_builtin(
    name: &str,
    args: &[ViewValue],
    registry: &Arc<Registry>,
) -> Option<Result<ViewValue>> {
    if !is_builtin(name) {
        return None;
    }
    Some(builtin(name, args, registry))
}

fn s(value: impl Into<String>) -> ViewValue {
    ViewValue::from(value.into())
}

fn builtin(name: &str, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    let a0 = arg(args, 0);
    Ok(match name {
        // ----------------------------------------------------------------
        // Counting & types
        // ----------------------------------------------------------------
        "count" | "sizeof" => ViewValue::from(count(a0, int_arg(args, 1, 0) == 1)?),
        "is_null" => ViewValue::Bool(a0.is_null()),
        "is_array" => ViewValue::Bool(matches!(a0, ViewValue::Array(_))),
        "is_string" => ViewValue::Bool(matches!(a0, ViewValue::Str(_))),
        "is_numeric" => ViewValue::Bool(php::is_numeric(a0)),
        "is_int" | "is_integer" | "is_long" => ViewValue::Bool(matches!(a0, ViewValue::Int(_))),
        "is_float" | "is_double" => ViewValue::Bool(matches!(a0, ViewValue::Float(_))),
        "is_bool" => ViewValue::Bool(matches!(a0, ViewValue::Bool(_))),
        "is_object" => ViewValue::Bool(matches!(
            a0,
            ViewValue::Object(_) | ViewValue::Closure(_) | ViewValue::Html(_)
        )),
        "is_callable" => ViewValue::Bool(match a0 {
            ViewValue::Closure(_) => true,
            ViewValue::Str(name) => crate::expr::eval::function_exists(name, registry),
            _ => false,
        }),
        "is_iterable" | "is_countable" => ViewValue::Bool(match a0 {
            ViewValue::Array(_) => true,
            ViewValue::Object(o) => o.iterate().is_some() || o.count().is_some(),
            _ => false,
        }),
        "is_scalar" => ViewValue::Bool(matches!(
            a0,
            ViewValue::Int(_) | ViewValue::Float(_) | ViewValue::Str(_) | ViewValue::Bool(_)
        )),
        "gettype" => s(match a0 {
            ViewValue::Null => "NULL",
            ViewValue::Bool(_) => "boolean",
            ViewValue::Int(_) => "integer",
            ViewValue::Float(_) => "double",
            ViewValue::Str(_) => "string",
            ViewValue::Array(_) => "array",
            _ => "object",
        }),
        "get_debug_type" => s(a0.type_name()),

        // ----------------------------------------------------------------
        // Strings
        // ----------------------------------------------------------------
        "strtoupper" => s(str_arg(args, 0)?.to_ascii_uppercase()),
        "strtolower" => s(str_arg(args, 0)?.to_ascii_lowercase()),
        "mb_strtoupper" => s(str_arg(args, 0)?.to_uppercase()),
        "mb_strtolower" => s(str_arg(args, 0)?.to_lowercase()),
        "ucfirst" => s(Str::ucfirst(&str_arg(args, 0)?)),
        "lcfirst" => s(Str::lcfirst(&str_arg(args, 0)?)),
        "ucwords" => {
            let value = str_arg(args, 0)?;
            let delimiters = opt_str(args, 1)?.unwrap_or_else(|| " \t\r\n\x0C\x0B".to_string());
            let mut out = String::with_capacity(value.len());
            let mut capitalize = true;
            for c in value.chars() {
                if capitalize {
                    out.extend(c.to_uppercase());
                } else {
                    out.push(c);
                }
                capitalize = delimiters.contains(c);
            }
            s(out)
        }
        "trim" | "ltrim" | "rtrim" | "chop" => {
            let value = str_arg(args, 0)?;
            let chars = trim_chars(opt_str(args, 1)?.as_deref());
            let matcher = |c: char| chars.contains(&c);
            s(match name {
                "trim" => value.trim_matches(matcher),
                "ltrim" => value.trim_start_matches(matcher),
                _ => value.trim_end_matches(matcher),
            })
        }
        "strlen" => ViewValue::from(str_arg(args, 0)?.len()),
        "mb_strlen" => ViewValue::from(str_arg(args, 0)?.chars().count()),
        "substr" | "mb_substr" => {
            let value = str_arg(args, 0)?;
            let length = match args.get(2) {
                None | Some(ViewValue::Null) => None,
                Some(v) => Some(v.as_i64().unwrap_or(0) as isize),
            };
            s(Str::substr(&value, int_arg(args, 1, 0) as isize, length))
        }
        "str_repeat" => s(str_arg(args, 0)?.repeat(int_arg(args, 1, 0).max(0) as usize)),
        "str_replace" | "str_ireplace" => str_replace(args, name == "str_ireplace")?,
        "str_contains" => ViewValue::Bool(str_arg(args, 0)?.contains(&str_arg(args, 1)?)),
        "str_starts_with" => ViewValue::Bool(str_arg(args, 0)?.starts_with(&str_arg(args, 1)?)),
        "str_ends_with" => ViewValue::Bool(str_arg(args, 0)?.ends_with(&str_arg(args, 1)?)),
        "strpos" | "stripos" | "strrpos" => {
            let (mut haystack, mut needle) = (str_arg(args, 0)?, str_arg(args, 1)?);
            if name == "stripos" {
                haystack = haystack.to_lowercase();
                needle = needle.to_lowercase();
            }
            let offset = int_arg(args, 2, 0).max(0) as usize;
            let found = if name == "strrpos" {
                haystack.rfind(&needle)
            } else {
                haystack
                    .get(offset..)
                    .and_then(|h| h.find(&needle))
                    .map(|i| i + offset)
            };
            found.map(ViewValue::from).unwrap_or(ViewValue::Bool(false))
        }
        "strrev" => s(str_arg(args, 0)?.chars().rev().collect::<String>()),
        "str_pad" => {
            let value = str_arg(args, 0)?;
            let length = int_arg(args, 1, 0).max(0) as usize;
            let pad = opt_str(args, 2)?.unwrap_or_else(|| " ".to_string());
            s(match int_arg(args, 3, 1) {
                0 => Str::pad_left(&value, length, &pad),
                2 => Str::pad_both(&value, length, &pad),
                _ => Str::pad_right(&value, length, &pad),
            })
        }
        "str_split" | "mb_str_split" => {
            let value = str_arg(args, 0)?;
            let size = int_arg(args, 1, 1).max(1) as usize;
            let chars: Vec<char> = value.chars().collect();
            ViewValue::list(chars.chunks(size).map(|c| s(c.iter().collect::<String>())))
        }
        "substr_count" => ViewValue::from(str_arg(args, 0)?.matches(&str_arg(args, 1)?).count()),
        "wordwrap" => s(wordwrap(
            &str_arg(args, 0)?,
            int_arg(args, 1, 75).max(1) as usize,
            &opt_str(args, 2)?.unwrap_or_else(|| "\n".into()),
            bool_arg(args, 3, false),
        )),
        "nl2br" => {
            let value = str_arg(args, 0)?;
            let mut out = String::with_capacity(value.len());
            let mut chars = value.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\r' if chars.peek() == Some(&'\n') => {
                        chars.next();
                        out.push_str("<br />\r\n");
                    }
                    '\n' | '\r' => {
                        out.push_str("<br />");
                        out.push(c);
                    }
                    other => out.push(other),
                }
            }
            s(out)
        }
        "e" => match a0 {
            ViewValue::Html(html) => s(html.to_string()),
            ViewValue::Object(o) if o.to_html().is_some() => s(o.to_html().unwrap_or_default()),
            other => s(php::escape(
                &php::to_str(other)?,
                bool_arg(args, 1, registry.double_encode),
            )),
        },
        "htmlspecialchars" | "htmlentities" => {
            s(php::escape(&str_arg(args, 0)?, bool_arg(args, 3, true)))
        }
        "htmlspecialchars_decode" | "html_entity_decode" => s(decode_entities(&str_arg(args, 0)?)),
        "strip_tags" => s(strip_tags(&str_arg(args, 0)?)),
        "addslashes" => {
            let value = str_arg(args, 0)?;
            let mut out = String::with_capacity(value.len());
            for c in value.chars() {
                if matches!(c, '\'' | '"' | '\\' | '\0') {
                    out.push('\\');
                }
                out.push(c);
            }
            s(out)
        }
        "sprintf" => s(sprintf(&str_arg(args, 0)?, &args[1.min(args.len())..])?),
        "vsprintf" => {
            let values: Vec<ViewValue> = array_arg(args, 1, name)?.values().cloned().collect();
            s(sprintf(&str_arg(args, 0)?, &values)?)
        }
        "implode" | "join" => implode(args)?,
        "explode" => explode(
            &str_arg(args, 0)?,
            &str_arg(args, 1)?,
            args.get(2).and_then(|v| v.as_i64()),
        )?,
        "class_basename" => s(Str::class_basename(&str_arg(args, 0)?)),
        "str" => s(match args.first() {
            Some(value) => php::to_str(value)?,
            None => String::new(),
        }),
        "urlencode" => s(url_encode(&str_arg(args, 0)?, true)),
        "rawurlencode" => s(url_encode(&str_arg(args, 0)?, false)),
        "urldecode" | "rawurldecode" => s(url_decode(&str_arg(args, 0)?, name == "urldecode")),
        "http_build_query" => s(http_build_query(&*array_arg(args, 0, name)?, None)),
        "ctype_digit" | "ctype_alpha" | "ctype_alnum" | "ctype_upper" | "ctype_lower"
        | "ctype_space" => {
            let ViewValue::Str(value) = a0 else {
                return Ok(ViewValue::Bool(false));
            };
            let check: fn(&char) -> bool = match name {
                "ctype_digit" => |c| c.is_ascii_digit(),
                "ctype_alpha" => |c| c.is_ascii_alphabetic(),
                "ctype_alnum" => |c| c.is_ascii_alphanumeric(),
                "ctype_upper" => |c| c.is_ascii_uppercase(),
                "ctype_lower" => |c| c.is_ascii_lowercase(),
                _ => |c| c.is_ascii_whitespace() || *c == '\x0B',
            };
            ViewValue::Bool(!value.is_empty() && value.chars().all(|c| check(&c)))
        }
        "md5" => s(format!("{:032x}", fnv_hash(&str_arg(args, 0)?))),
        "uniqid" => s(format!(
            "{}{:x}",
            opt_str(args, 0)?.unwrap_or_default(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros())
                .unwrap_or(0)
        )),

        // ----------------------------------------------------------------
        // Numbers
        // ----------------------------------------------------------------
        "number_format" => {
            let decimals = int_arg(args, 1, 0).max(0) as usize;
            let decimal_point = opt_str(args, 2)?.unwrap_or_else(|| ".".into());
            let separator = opt_str(args, 3)?.unwrap_or_else(|| ",".into());
            s(number_format(
                float_arg(args, 0),
                decimals,
                &decimal_point,
                &separator,
            ))
        }
        "round" => ViewValue::Float(php_round(float_arg(args, 0), int_arg(args, 1, 0) as i32)),
        "floor" => ViewValue::Float(float_arg(args, 0).floor()),
        "ceil" => ViewValue::Float(float_arg(args, 0).ceil()),
        "abs" => match php::to_number(a0, "abs", &NULL)? {
            Num::Int(i) => ViewValue::Int(i.wrapping_abs()),
            Num::Float(f) => ViewValue::Float(f.abs()),
        },
        "sqrt" => ViewValue::Float(float_arg(args, 0).sqrt()),
        "pow" => php::arithmetic(php::Arith::Pow, a0, arg(args, 1))?,
        "intdiv" => {
            let divisor = int_arg(args, 1, 0);
            if divisor == 0 {
                return Err(crate::exception::DivisionByZeroError::new("Division by zero").into());
            }
            let dividend = int_arg(args, 0, 0);
            match dividend.checked_div(divisor) {
                Some(quotient) => ViewValue::Int(quotient),
                None => {
                    return Err(crate::exception::ArithmeticError::new(
                        "Division of PHP_INT_MIN by -1 is not an integer",
                    )
                    .into());
                }
            }
        }
        "fmod" => ViewValue::Float(float_arg(args, 0) % float_arg(args, 1)),
        "pi" => ViewValue::Float(std::f64::consts::PI),
        "lcg_value" => ViewValue::Float(
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0) as f64)
                / 1e9,
        ),
        "max" | "min" => {
            let values: Vec<ViewValue> = if args.len() == 1 {
                array_arg(args, 0, name)?.values().cloned().collect()
            } else {
                args.to_vec()
            };
            let mut best: Option<ViewValue> = None;
            for value in values {
                best = Some(match best {
                    None => value,
                    Some(current) => {
                        let ordering = php::compare(&value, &current);
                        if (name == "max" && ordering.is_gt())
                            || (name == "min" && ordering.is_lt())
                        {
                            value
                        } else {
                            current
                        }
                    }
                });
            }
            best.ok_or_else(|| {
                error(format!(
                    "{name}(): Argument #1 ($value) must contain at least one element"
                ))
            })?
        }
        "intval" => match a0 {
            ViewValue::Str(value) if args.len() > 1 => {
                let base = int_arg(args, 1, 10) as u32;
                ViewValue::Int(i64::from_str_radix(value.trim(), base).unwrap_or(0))
            }
            ViewValue::Str(value) => ViewValue::Int(
                php::parse_numeric(value)
                    .map(Num::as_i64)
                    .unwrap_or_else(|| leading_number(value).as_i64()),
            ),
            ViewValue::Array(a) => ViewValue::Int(!a.is_empty() as i64),
            other => ViewValue::Int(other.as_i64().unwrap_or(0)),
        },
        "floatval" | "doubleval" => match a0 {
            ViewValue::Str(value) => ViewValue::Float(
                php::parse_numeric(value)
                    .map(Num::as_f64)
                    .unwrap_or_else(|| leading_number(value).as_f64()),
            ),
            other => ViewValue::Float(other.as_f64().unwrap_or(0.0)),
        },
        "strval" => s(php::to_str(a0)?),
        "boolval" => ViewValue::Bool(a0.truthy()),

        // ----------------------------------------------------------------
        // Arrays
        // ----------------------------------------------------------------
        "in_array" => {
            let haystack = array_arg(args, 1, name)?;
            let strict = bool_arg(args, 2, false);
            ViewValue::Bool(haystack.values().any(|v| {
                if strict {
                    php::strict_eq(v, a0)
                } else {
                    php::loose_eq(v, a0)
                }
            }))
        }
        "array_key_exists" | "key_exists" => {
            let key = ArrayKey::from_value(a0)?;
            match arg(args, 1) {
                ViewValue::Object(o) => ViewValue::Bool(o.get(&key.to_string()).is_some()),
                _ => ViewValue::Bool(array_arg(args, 1, name)?.get(&key).is_some()),
            }
        }
        "array_keys" => {
            let array = array_arg(args, 0, name)?;
            match args.get(1) {
                Some(search) => ViewValue::list(
                    array
                        .iter()
                        .filter(|(_, v)| php::loose_eq(v, search))
                        .map(|(k, _)| k.to_value()),
                ),
                None => ViewValue::list(array.keys().map(ArrayKey::to_value)),
            }
        }
        "array_values" => ViewValue::list(array_arg(args, 0, name)?.values().cloned()),
        "array_merge" => {
            let mut merged = ViewArray::new();
            for (index, _) in args.iter().enumerate() {
                merge_into(&mut merged, &*array_arg(args, index, name)?);
            }
            ViewValue::from(merged)
        }
        "array_slice" => {
            let array = array_arg(args, 0, name)?;
            let length = match args.get(2) {
                None | Some(ViewValue::Null) => None,
                Some(v) => v.as_i64(),
            };
            ViewValue::from(slice(
                &array,
                int_arg(args, 1, 0),
                length,
                bool_arg(args, 3, false),
            ))
        }
        "array_filter" => {
            let array = array_arg(args, 0, name)?;
            let mode = int_arg(args, 2, 0);
            let mut filtered = ViewArray::new();
            for (key, value) in array.iter() {
                let keep = match args.get(1) {
                    None | Some(ViewValue::Null) => value.truthy(),
                    Some(callback) => {
                        let callback_args = match mode {
                            2 => vec![key.to_value()],
                            1 => vec![value.clone(), key.to_value()],
                            _ => vec![value.clone()],
                        };
                        call_callable(callback, &callback_args, registry)?.truthy()
                    }
                };
                if keep {
                    filtered.insert(key.clone(), value.clone());
                }
            }
            ViewValue::from(filtered)
        }
        "array_map" => {
            if args.len() <= 2 {
                let array = array_arg(args, 1, name)?;
                let mut mapped = ViewArray::with_capacity(array.len());
                for (key, value) in array.iter() {
                    let result = match a0 {
                        ViewValue::Null => value.clone(),
                        callback => call_callable(callback, std::slice::from_ref(value), registry)?,
                    };
                    mapped.insert(key.clone(), result);
                }
                ViewValue::from(mapped)
            } else {
                let arrays: Vec<Vec<ViewValue>> = (1..args.len())
                    .map(|i| array_arg(args, i, name).map(|a| a.values().cloned().collect()))
                    .collect::<Result<_>>()?;
                let longest = arrays.iter().map(Vec::len).max().unwrap_or(0);
                let mut mapped = ViewArray::new();
                for index in 0..longest {
                    let row: Vec<ViewValue> = arrays
                        .iter()
                        .map(|a| a.get(index).cloned().unwrap_or_default())
                        .collect();
                    mapped.push(match a0 {
                        ViewValue::Null => ViewValue::list(row),
                        callback => call_callable(callback, &row, registry)?,
                    });
                }
                ViewValue::from(mapped)
            }
        }
        "array_reverse" => {
            let array = array_arg(args, 0, name)?;
            let preserve = bool_arg(args, 1, false);
            let mut reversed = ViewArray::with_capacity(array.len());
            for (key, value) in array.iter().rev() {
                match key {
                    ArrayKey::Int(_) if !preserve => reversed.push(value.clone()),
                    key => reversed.insert(key.clone(), value.clone()),
                }
            }
            ViewValue::from(reversed)
        }
        "array_unique" => {
            let array = array_arg(args, 0, name)?;
            let mut seen: Vec<String> = Vec::new();
            let mut unique = ViewArray::new();
            for (key, value) in array.iter() {
                let repr = value.to_string_lossy();
                if !seen.contains(&repr) {
                    seen.push(repr);
                    unique.insert(key.clone(), value.clone());
                }
            }
            ViewValue::from(unique)
        }
        "array_sum" | "array_product" => {
            let array = array_arg(args, 0, name)?;
            let mut total = if name == "array_sum" {
                ViewValue::Int(0)
            } else {
                ViewValue::Int(1)
            };
            let op = if name == "array_sum" {
                php::Arith::Add
            } else {
                php::Arith::Mul
            };
            for value in array.values() {
                total = php::arithmetic(op, &total, value)?;
            }
            total
        }
        "array_search" => {
            let haystack = array_arg(args, 1, name)?;
            let strict = bool_arg(args, 2, false);
            haystack
                .iter()
                .find(|(_, v)| {
                    if strict {
                        php::strict_eq(v, a0)
                    } else {
                        php::loose_eq(v, a0)
                    }
                })
                .map(|(k, _)| k.to_value())
                .unwrap_or(ViewValue::Bool(false))
        }
        "array_column" => {
            let array = array_arg(args, 0, name)?;
            let column = arg(args, 1);
            let index = arg(args, 2);
            let mut out = ViewArray::new();
            for row in array.values() {
                let value = if column.is_null() {
                    Some(row.clone())
                } else {
                    row.get(&php::to_str(column)?)
                };
                let Some(value) = value else { continue };
                match index {
                    ViewValue::Null => out.push(value),
                    key => match row.get(&php::to_str(key)?) {
                        Some(k) => out.insert(ArrayKey::from_value(&k)?, value),
                        None => out.push(value),
                    },
                }
            }
            ViewValue::from(out)
        }
        "array_combine" => {
            let keys = array_arg(args, 0, name)?;
            let values = array_arg(args, 1, name)?;
            if keys.len() != values.len() {
                return Err(error(
                    "array_combine(): Argument #1 ($keys) and argument #2 ($values) must have the same number of elements",
                ));
            }
            let mut out = ViewArray::new();
            for (key, value) in keys.values().zip(values.values()) {
                out.insert(ArrayKey::from_value(key)?, value.clone());
            }
            ViewValue::from(out)
        }
        "array_flip" => {
            let array = array_arg(args, 0, name)?;
            let mut out = ViewArray::new();
            for (key, value) in array.iter() {
                out.insert(ArrayKey::from_value(value)?, key.to_value());
            }
            ViewValue::from(out)
        }
        "array_chunk" => {
            let array = array_arg(args, 0, name)?;
            let size = int_arg(args, 1, 1).max(1) as usize;
            let preserve = bool_arg(args, 2, false);
            let mut chunks = ViewArray::new();
            let mut current = ViewArray::new();
            for (key, value) in array.iter() {
                if preserve {
                    current.insert(key.clone(), value.clone());
                } else {
                    current.push(value.clone());
                }
                if current.len() == size {
                    chunks.push(ViewValue::from(std::mem::take(&mut current)));
                }
            }
            if !current.is_empty() {
                chunks.push(ViewValue::from(current));
            }
            ViewValue::from(chunks)
        }
        "array_fill" => {
            let start = int_arg(args, 0, 0);
            let count = int_arg(args, 1, 0).max(0);
            let mut out = ViewArray::new();
            for i in 0..count {
                out.insert(ArrayKey::Int(start + i), arg(args, 2).clone());
            }
            ViewValue::from(out)
        }
        "array_fill_keys" => {
            let keys = array_arg(args, 0, name)?;
            let mut out = ViewArray::new();
            for key in keys.values() {
                out.insert(ArrayKey::from_value(key)?, arg(args, 1).clone());
            }
            ViewValue::from(out)
        }
        "array_pad" => {
            let array = array_arg(args, 0, name)?;
            let size = int_arg(args, 1, 0);
            let mut values: Vec<ViewValue> = array.values().cloned().collect();
            let missing = (size.unsigned_abs() as usize).saturating_sub(values.len());
            let pad = std::iter::repeat_n(arg(args, 2).clone(), missing);
            if size >= 0 {
                values.extend(pad);
            } else {
                values.splice(0..0, pad);
            }
            ViewValue::list(values)
        }
        "array_key_first" => array_arg(args, 0, name)?
            .keys()
            .next()
            .map(ArrayKey::to_value)
            .unwrap_or_default(),
        "array_key_last" => array_arg(args, 0, name)?
            .keys()
            .last()
            .map(ArrayKey::to_value)
            .unwrap_or_default(),
        "array_is_list" => ViewValue::Bool(array_arg(args, 0, name)?.is_list()),
        "array_diff" | "array_intersect" => {
            let first = array_arg(args, 0, name)?;
            let others: Vec<Arc<ViewArray>> = (1..args.len())
                .map(|i| array_arg(args, i, name))
                .collect::<Result<_>>()?;
            let mut out = ViewArray::new();
            for (key, value) in first.iter() {
                let repr = value.to_string_lossy();
                let present = |a: &Arc<ViewArray>| a.values().any(|v| v.to_string_lossy() == repr);
                let keep = if name == "array_diff" {
                    !others.iter().any(present)
                } else {
                    others.iter().all(present)
                };
                if keep {
                    out.insert(key.clone(), value.clone());
                }
            }
            ViewValue::from(out)
        }
        "array_diff_key" | "array_intersect_key" => {
            let first = array_arg(args, 0, name)?;
            let others: Vec<Arc<ViewArray>> = (1..args.len())
                .map(|i| array_arg(args, i, name))
                .collect::<Result<_>>()?;
            let mut out = ViewArray::new();
            for (key, value) in first.iter() {
                let present = |a: &Arc<ViewArray>| a.get(key).is_some();
                let keep = if name == "array_diff_key" {
                    !others.iter().any(present)
                } else {
                    others.iter().all(present)
                };
                if keep {
                    out.insert(key.clone(), value.clone());
                }
            }
            ViewValue::from(out)
        }
        "array_count_values" => {
            let array = array_arg(args, 0, name)?;
            let mut out = ViewArray::new();
            for value in array.values() {
                let key = ArrayKey::from_value(value)?;
                let current = out.get(&key).and_then(ViewValue::as_i64).unwrap_or(0);
                out.insert(key, ViewValue::Int(current + 1));
            }
            ViewValue::from(out)
        }
        "array_rand" => {
            let array = array_arg(args, 0, name)?;
            if array.is_empty() {
                return Err(error("array_rand(): Argument #1 ($array) cannot be empty"));
            }
            let index =
                (fnv_hash(&format!("{:?}", std::time::SystemTime::now())) as usize) % array.len();
            array
                .keys()
                .nth(index)
                .map(ArrayKey::to_value)
                .unwrap_or_default()
        }
        "iterator_to_array" => ViewValue::Array(array_arg(args, 0, name)?),
        "range" => range(a0, arg(args, 1), args.get(2))?,
        "json_encode" => s(php::json_encode(a0, int_arg(args, 1, 0))?),
        "json_decode" => php::json_decode(&str_arg(args, 0)?),
        "print_r" | "var_export" => {
            let dumped = dump_text(a0);
            if bool_arg(args, 1, false) {
                s(dumped)
            } else {
                ViewValue::html(dumped)
            }
        }

        // ----------------------------------------------------------------
        // Dates
        // ----------------------------------------------------------------
        "date" => {
            let format = str_arg(args, 0)?;
            let date = match args.get(1) {
                None | Some(ViewValue::Null) => Carbon::now(),
                Some(ts) => Carbon::from_timestamp(ts.as_i64().unwrap_or(0)),
            };
            s(date.format(&format))
        }
        "time" | "now_timestamp" => ViewValue::Int(Carbon::now().timestamp()),
        "strtotime" => match Carbon::parse(&str_arg(args, 0)?) {
            Ok(date) => ViewValue::Int(date.timestamp()),
            Err(_) => ViewValue::Bool(false),
        },
        "now" => ViewValue::object(DateObject(match opt_str(args, 0)? {
            Some(tz) => Carbon::now().tz(&tz)?,
            None => Carbon::now(),
        })),
        "today" => ViewValue::object(DateObject(Carbon::today())),

        // ----------------------------------------------------------------
        // Laravel helpers
        // ----------------------------------------------------------------
        "collect" => match a0 {
            ViewValue::Null => ViewValue::empty_array(),
            ViewValue::Array(_) => a0.clone(),
            other => to_array(other)
                .map(ViewValue::Array)
                .unwrap_or_else(|| ViewValue::list([other.clone()])),
        },
        "data_get" => {
            let found = data_get(a0, &str_arg(args, 1)?);
            match found {
                Some(value) if !value.is_null() => value,
                _ => value_of(arg(args, 2), &[], registry)?,
            }
        }
        "value" => value_of(a0, &args[1.min(args.len())..], registry)?,
        "head" => to_array(a0)
            .and_then(|a| a.first().cloned())
            .unwrap_or(ViewValue::Bool(false)),
        "last" => to_array(a0)
            .and_then(|a| a.last().cloned())
            .unwrap_or(ViewValue::Bool(false)),
        "blank" => ViewValue::Bool(is_blank(a0)),
        "filled" => ViewValue::Bool(!is_blank(a0)),
        "optional" => match a0 {
            ViewValue::Null => ViewValue::object(OptionalObject),
            other => match args.get(1) {
                Some(callback) => call_callable(callback, std::slice::from_ref(other), registry)?,
                None => other.clone(),
            },
        },
        "tap" => {
            if let Some(callback) = args.get(1) {
                call_callable(callback, std::slice::from_ref(a0), registry)?;
            }
            a0.clone()
        }
        "with" => match args.get(1) {
            Some(callback) => call_callable(callback, std::slice::from_ref(a0), registry)?,
            None => a0.clone(),
        },
        "retry" => {
            let times = int_arg(args, 0, 1).max(1);
            let mut last_error = None;
            for attempt in 1..=times {
                match call_callable(arg(args, 1), &[ViewValue::Int(attempt)], registry) {
                    Ok(value) => return Ok(value),
                    Err(e) => last_error = Some(e),
                }
            }
            return Err(last_error.unwrap_or_else(|| error("retry() failed")));
        }
        "throw_if" | "throw_unless" => {
            let condition = a0.truthy();
            if condition == (name == "throw_if") {
                let message = opt_str(args, 1)?.unwrap_or_else(|| "Exception".into());
                return Err(crate::exception::RuntimeException::new(message).into());
            }
            a0.clone()
        }
        "abort" | "abort_if" | "abort_unless" => {
            let (should_abort, offset) = match name {
                "abort" => (true, 0),
                "abort_if" => (a0.truthy(), 1),
                _ => (!a0.truthy(), 1),
            };
            if should_abort {
                let status = int_arg(args, offset, 500) as u16;
                return Err(match opt_str(args, offset + 1)? {
                    Some(message) => {
                        illuminate_http::HttpException::with_message(status, message).into()
                    }
                    None => illuminate_http::HttpException::new(status).into(),
                });
            }
            ViewValue::Null
        }
        "dump" => ViewValue::html(args.iter().map(dump_html).collect::<String>()),
        "dd" => {
            let html: String = args.iter().map(dump_html).collect();
            let response = illuminate_http::Response::new(html).with_status(500);
            return Err(illuminate_http::HttpResponseException::new(response).into());
        }
        "method_field" => ViewValue::html(format!(
            "<input type=\"hidden\" name=\"_method\" value=\"{}\">",
            str_arg(args, 0)?
        )),
        "csrf_field" => match registry.call("csrf_token", &[]) {
            Some(token) => ViewValue::html(format!(
                "<input type=\"hidden\" name=\"_token\" value=\"{}\" autocomplete=\"off\">",
                php::to_str(&token?)?
            )),
            None => {
                return Err(crate::exception::BadMethodCallException::new(
                    "Call to undefined function csrf_token(). Register it with Blade::function(\"csrf_token\", ...) to use @csrf.",
                )
                .into());
            }
        },
        "config" => {
            let key = str_arg(args, 0)?;
            let value = illuminate_container::try_app::<illuminate_config::Repository>()
                .map(|config| ViewValue::from(config.get(&key)))
                .unwrap_or_default();
            if value.is_null() {
                arg(args, 1).clone()
            } else {
                value
            }
        }
        "env" => {
            let value = illuminate_support::env(&str_arg(args, 0)?, arg(args, 1).to_json());
            ViewValue::from(value)
        }
        "__" | "trans" => {
            let key = str_arg(args, 0)?;
            s(make_replacements(&key, arg(args, 1))?)
        }
        "trans_choice" => {
            let key = str_arg(args, 0)?;
            let count = match arg(args, 1) {
                ViewValue::Array(a) => a.len() as f64,
                other => other.as_f64().unwrap_or(0.0),
            };
            let mut replace = to_array(arg(args, 2))
                .map(|a| (*a).clone())
                .unwrap_or_default();
            if replace.get_str("count").is_none() {
                replace.set("count", php::Num::Float(count).into_value_clean());
            }
            let line = choose_plural(&key, count);
            s(make_replacements(&line, &ViewValue::from(replace))?)
        }
        "old" => arg(args, 1).clone(),
        "session" => {
            if a0.is_null() {
                ViewValue::Null
            } else {
                arg(args, 1).clone()
            }
        }
        "auth_check" | "gate_check" => ViewValue::Bool(false),
        "app_environment" => {
            let configured = illuminate_container::try_app::<illuminate_config::Repository>()
                .map(|config| config.get("app.env"))
                .filter(|value| !value.is_null());
            match configured {
                Some(value) => s(value.to_string_lossy()),
                None => s("production"),
            }
        }
        "app_locale" => {
            let configured = illuminate_container::try_app::<illuminate_config::Repository>()
                .map(|config| config.get("app.locale"))
                .filter(|value| !value.is_null());
            match configured {
                Some(value) => s(value.to_string_lossy()),
                None => s("en"),
            }
        }
        "app_version" => s(env!("CARGO_PKG_VERSION")),
        "app" => {
            if !args.is_empty() {
                return Err(crate::exception::BadMethodCallException::new(format!(
                    "No service [{}] is available to views. Register Blade::function(\"app\", ...) to use app() and @inject.",
                    a0.to_string_lossy()
                ))
                .into());
            }
            let locale =
                crate::expr::eval::call_function("app_locale", &[], registry)?.to_string_lossy();
            let environment = crate::expr::eval::call_function("app_environment", &[], registry)?
                .to_string_lossy();
            let version =
                crate::expr::eval::call_function("app_version", &[], registry)?.to_string_lossy();
            ViewValue::object(crate::objects::AppObject {
                locale,
                environment,
                version,
            })
        }
        "vite" | "vite_react_refresh" | "fonts" => ViewValue::html(""),
        "context" => ViewValue::Null,
        "context_has" => ViewValue::Bool(false),
        "spl_object_id" => ViewValue::Int(match a0 {
            ViewValue::Object(o) => Arc::as_ptr(o) as *const () as usize as i64,
            _ => 0,
        }),
        "request" => {
            let request = illuminate_http::request();
            match opt_str(args, 0)? {
                Some(key) => {
                    let value = ViewValue::from(request.input(&key));
                    if value.is_null() {
                        arg(args, 1).clone()
                    } else {
                        value
                    }
                }
                None => ViewValue::object(crate::objects::RequestObject(request)),
            }
        }
        "to_route" => return Err(error("to_route() is not available in views")),
        _ => return Err(error(format!("Call to undefined function {name}()"))),
    })
}

impl Num {
    /// Convert to a value, collapsing whole floats into integers.
    pub(crate) fn into_value_clean(self) -> ViewValue {
        match self {
            Num::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => ViewValue::Int(f as i64),
            other => other.into_value(),
        }
    }
}

fn trim_chars(chars: Option<&str>) -> Vec<char> {
    match chars {
        None => vec![' ', '\t', '\n', '\r', '\0', '\x0B'],
        Some(spec) => {
            let spec: Vec<char> = spec.chars().collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i < spec.len() {
                if i + 3 < spec.len() && spec[i + 1] == '.' && spec[i + 2] == '.' {
                    let (from, to) = (spec[i] as u32, spec[i + 3] as u32);
                    for code in from..=to {
                        if let Some(c) = char::from_u32(code) {
                            out.push(c);
                        }
                    }
                    i += 4;
                } else {
                    out.push(spec[i]);
                    i += 1;
                }
            }
            out
        }
    }
}

fn count(value: &ViewValue, recursive: bool) -> Result<usize> {
    Ok(match value {
        ViewValue::Array(array) => {
            let items = crate::methods::paginator_items(array).unwrap_or_else(|| array.clone());
            if recursive {
                items.len()
                    + items
                        .values()
                        .map(|v| {
                            if matches!(v, ViewValue::Array(_)) {
                                count(v, true).unwrap_or(0)
                            } else {
                                0
                            }
                        })
                        .sum::<usize>()
            } else {
                items.len()
            }
        }
        ViewValue::Null => 0,
        ViewValue::Object(object) => object.count().ok_or_else(|| {
            TypeError::new(format!(
                "count(): Argument #1 ($value) must be of type Countable|array, {} given",
                object.class_name()
            ))
        })?,
        other => {
            return Err(TypeError::new(format!(
                "count(): Argument #1 ($value) must be of type Countable|array, {} given",
                other.type_name()
            ))
            .into());
        }
    })
}

fn str_replace(args: &[ViewValue], insensitive: bool) -> Result<ViewValue> {
    let search = arg(args, 0);
    let replace = arg(args, 1);
    let pairs: Vec<(String, String)> = match search {
        ViewValue::Array(searches) => {
            let replacements: Option<Vec<String>> = match replace {
                ViewValue::Array(r) => Some(r.values().map(|v| v.to_string_lossy()).collect()),
                _ => None,
            };
            searches
                .values()
                .enumerate()
                .map(|(i, s)| {
                    let r = match &replacements {
                        Some(list) => list.get(i).cloned().unwrap_or_default(),
                        None => replace.to_string_lossy(),
                    };
                    (s.to_string_lossy(), r)
                })
                .collect()
        }
        other => vec![(php::to_str(other)?, php::to_str(replace)?)],
    };
    let apply = |subject: &str| -> String {
        let mut result = subject.to_string();
        for (from, to) in &pairs {
            if from.is_empty() {
                continue;
            }
            result = if insensitive {
                replace_insensitive(&result, from, to)
            } else {
                result.replace(from, to)
            };
        }
        result
    };
    Ok(match arg(args, 2) {
        ViewValue::Array(subjects) => ViewValue::from(
            subjects
                .iter()
                .map(|(k, v)| (k.clone(), ViewValue::from(apply(&v.to_string_lossy()))))
                .collect::<ViewArray>(),
        ),
        subject => ViewValue::from(apply(&php::to_str(subject)?)),
    })
}

fn replace_insensitive(subject: &str, from: &str, to: &str) -> String {
    let lower_subject = subject.to_lowercase();
    let lower_from = from.to_lowercase();
    if lower_subject.len() != subject.len() {
        return subject.replace(from, to);
    }
    let mut out = String::with_capacity(subject.len());
    let mut last = 0;
    for (index, _) in lower_subject.match_indices(&lower_from) {
        out.push_str(&subject[last..index]);
        out.push_str(to);
        last = index + from.len();
    }
    out.push_str(&subject[last..]);
    out
}

fn implode(args: &[ViewValue]) -> Result<ViewValue> {
    let (glue, pieces) = match (arg(args, 0), arg(args, 1)) {
        (ViewValue::Array(pieces), ViewValue::Null) => (String::new(), pieces.clone()),
        (ViewValue::Array(pieces), glue) => (php::to_str(glue)?, pieces.clone()),
        (glue, _) => (php::to_str(glue)?, array_arg(args, 1, "implode")?),
    };
    let parts: Vec<String> = pieces.values().map(php::to_str).collect::<Result<_>>()?;
    Ok(ViewValue::from(parts.join(&glue)))
}

fn explode(delimiter: &str, value: &str, limit: Option<i64>) -> Result<ViewValue> {
    if delimiter.is_empty() {
        return Err(crate::exception::InvalidArgumentException::new(
            "explode(): Argument #1 ($separator) cannot be empty",
        )
        .into());
    }
    let parts: Vec<&str> = value.split(delimiter).collect();
    let parts: Vec<String> = match limit {
        Some(limit) if limit > 0 && (limit as usize) < parts.len() => {
            let limit = limit as usize;
            let mut out: Vec<String> = parts[..limit - 1].iter().map(|s| s.to_string()).collect();
            out.push(parts[limit - 1..].join(delimiter));
            out
        }
        Some(0) => vec![parts.join(delimiter)],
        Some(limit) if limit < 0 => {
            let keep = parts.len().saturating_sub(limit.unsigned_abs() as usize);
            parts[..keep].iter().map(|s| s.to_string()).collect()
        }
        _ => parts.iter().map(|s| s.to_string()).collect(),
    };
    Ok(ViewValue::list(parts.into_iter().map(ViewValue::from)))
}

fn wordwrap(value: &str, width: usize, line_break: &str, cut: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in value.split(line_break) {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let mut word = word.to_string();
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            while cut && word.chars().count() > width {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                let head: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                lines.push(head);
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines.join(line_break)
}

/// Merge `source` into `target` with `array_merge` semantics.
pub(crate) fn merge_into(target: &mut ViewArray, source: &ViewArray) {
    for (key, value) in source.iter() {
        match key {
            ArrayKey::Int(_) => target.push(value.clone()),
            key => target.insert(key.clone(), value.clone()),
        }
    }
}

/// `array_slice` semantics.
pub(crate) fn slice(
    array: &ViewArray,
    offset: i64,
    length: Option<i64>,
    preserve_keys: bool,
) -> ViewArray {
    let len = array.len() as i64;
    let start = if offset < 0 {
        (len + offset).max(0)
    } else {
        offset.min(len)
    };
    let end = match length {
        None => len,
        Some(l) if l < 0 => (len + l).max(start),
        Some(l) => start.saturating_add(l).min(len),
    };
    let mut out = ViewArray::new();
    for (key, value) in array
        .iter()
        .skip(start as usize)
        .take((end - start).max(0) as usize)
    {
        match key {
            ArrayKey::Int(_) if !preserve_keys => out.push(value.clone()),
            key => out.insert(key.clone(), value.clone()),
        }
    }
    out
}

fn range(start: &ViewValue, end: &ViewValue, step: Option<&ViewValue>) -> Result<ViewValue> {
    if let (ViewValue::Str(a), ViewValue::Str(b)) = (start, end)
        && a.chars().count() == 1
        && b.chars().count() == 1
        && php::parse_numeric(a).is_none()
    {
        let (from, to) = (
            a.chars().next().unwrap_or('a') as u32,
            b.chars().next().unwrap_or('a') as u32,
        );
        let step = step
            .and_then(ViewValue::as_i64)
            .unwrap_or(1)
            .unsigned_abs()
            .max(1) as usize;
        let codes: Vec<u32> = if from <= to {
            (from..=to).step_by(step).collect()
        } else {
            (to..=from).rev().step_by(step).collect()
        };
        return Ok(ViewValue::list(
            codes
                .into_iter()
                .filter_map(char::from_u32)
                .map(|c| ViewValue::from(c.to_string())),
        ));
    }
    let is_float = matches!(start, ViewValue::Float(_))
        || matches!(end, ViewValue::Float(_))
        || matches!(step, Some(ViewValue::Float(_)));
    let from = start.as_f64().unwrap_or(0.0);
    let to = end.as_f64().unwrap_or(0.0);
    let step = step.and_then(ViewValue::as_f64).unwrap_or(1.0).abs();
    if step == 0.0 {
        return Err(error("range(): Argument #3 ($step) cannot be 0"));
    }
    let mut values = Vec::new();
    let count = ((to - from).abs() / step).floor() as usize;
    if count > 1_000_000 {
        return Err(error("range(): The range is too large"));
    }
    for i in 0..=count {
        let value = if from <= to {
            from + step * i as f64
        } else {
            from - step * i as f64
        };
        values.push(if is_float {
            ViewValue::Float(value)
        } else {
            ViewValue::Int(value as i64)
        });
    }
    Ok(ViewValue::list(values))
}

fn leading_number(s: &str) -> Num {
    let t = s.trim_start();
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) || (c == '.' && !seen_dot) {
            seen_dot |= c == '.';
            end = i + 1;
        } else {
            break;
        }
    }
    php::parse_numeric(&t[..end]).unwrap_or(Num::Int(0))
}

/// PHP's `round()`, which rounds half away from zero after "pre-rounding".
pub(crate) fn php_round(value: f64, precision: i32) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let factor = 10f64.powi(precision.abs());
    let scaled = if precision >= 0 {
        value * factor
    } else {
        value / factor
    };
    let pre_rounded: f64 = format!("{:.14e}", scaled).parse().unwrap_or(scaled);
    let rounded = pre_rounded.round();
    let result = if precision >= 0 {
        rounded / factor
    } else {
        rounded * factor
    };
    if result.is_finite() { result } else { value }
}

/// PHP's `number_format()`.
pub(crate) fn number_format(
    value: f64,
    decimals: usize,
    decimal_point: &str,
    separator: &str,
) -> String {
    let rounded = php_round(value, decimals as i32);
    let formatted = format!("{:.*}", decimals, rounded.abs());
    let (int_part, frac_part) = match formatted.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (formatted, None),
    };
    let mut grouped = String::new();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push_str(separator);
        }
        grouped.push(c);
    }
    let negative = rounded < 0.0 && rounded != 0.0;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&grouped);
    if let Some(frac) = frac_part {
        out.push_str(decimal_point);
        out.push_str(&frac);
    }
    out
}

/// A small `sprintf` supporting the common conversions.
pub(crate) fn sprintf(format: &str, args: &[ViewValue]) -> Result<String> {
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    let mut next_arg = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '%' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if chars.get(i) == Some(&'%') {
            out.push('%');
            i += 1;
            continue;
        }
        // Argument number: %1$s
        let mut arg_index = None;
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j > i && chars.get(j) == Some(&'$') {
            let n: usize = chars[i..j].iter().collect::<String>().parse().unwrap_or(1);
            arg_index = Some(n.saturating_sub(1));
            i = j + 1;
        }
        // Flags
        let mut left = false;
        let mut plus = false;
        let mut pad = ' ';
        loop {
            match chars.get(i) {
                Some('-') => left = true,
                Some('+') => plus = true,
                Some('0') => pad = '0',
                Some(' ') => pad = ' ',
                Some('\'') => {
                    i += 1;
                    pad = chars.get(i).copied().unwrap_or(' ');
                }
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
            width = width * 10 + d as usize;
            i += 1;
        }
        let mut precision: Option<usize> = None;
        if chars.get(i) == Some(&'.') {
            i += 1;
            let mut p = 0usize;
            while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
                p = p * 10 + d as usize;
                i += 1;
            }
            precision = Some(p);
        }
        let Some(&conversion) = chars.get(i) else {
            return Err(crate::exception::InvalidArgumentException::new(
                "Missing format specifier at end of string",
            )
            .into());
        };
        i += 1;
        let index = arg_index.unwrap_or_else(|| {
            let index = next_arg;
            next_arg += 1;
            index
        });
        let Some(value) = args.get(index) else {
            return Err(crate::exception::InvalidArgumentException::new(format!(
                "{} arguments are required, {} given",
                index + 2,
                args.len() + 1
            ))
            .into());
        };
        let number = |v: &ViewValue| v.as_f64().unwrap_or(0.0);
        let integer = |v: &ViewValue| v.as_i64().unwrap_or(0);
        let mut text = match conversion {
            's' => {
                let s = php::to_str(value)?;
                match precision {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                }
            }
            'd' | 'i' => {
                let n = integer(value);
                if plus && n >= 0 {
                    format!("+{n}")
                } else {
                    n.to_string()
                }
            }
            'u' => (integer(value) as u64).to_string(),
            'f' | 'F' => {
                let n = number(value);
                let s = format!(
                    "{:.*}",
                    precision.unwrap_or(6),
                    php_round(n, precision.unwrap_or(6) as i32)
                );
                if plus && n >= 0.0 { format!("+{s}") } else { s }
            }
            'e' | 'E' => {
                let s = format!("{:.*e}", precision.unwrap_or(6), number(value));
                let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
                let exponent: i32 = exponent.parse().unwrap_or(0);
                let s = format!(
                    "{mantissa}e{}{}",
                    if exponent < 0 { '-' } else { '+' },
                    exponent.abs()
                );
                if conversion == 'E' {
                    s.to_uppercase()
                } else {
                    s
                }
            }
            'g' | 'G' => {
                php::format_float_precision(number(value), precision.unwrap_or(6).max(1) as i32)
            }
            'x' => format!("{:x}", integer(value)),
            'X' => format!("{:X}", integer(value)),
            'o' => format!("{:o}", integer(value)),
            'b' => format!("{:b}", integer(value)),
            'c' => char::from_u32(integer(value) as u32)
                .map(String::from)
                .unwrap_or_default(),
            other => {
                return Err(crate::exception::InvalidArgumentException::new(format!(
                    "Unknown format specifier \"{other}\""
                ))
                .into());
            }
        };
        let length = text.chars().count();
        if length < width {
            let padding: String = std::iter::repeat_n(pad, width - length).collect();
            if left {
                text.push_str(&padding.replace('0', " "));
            } else if pad == '0' && (text.starts_with('-') || text.starts_with('+')) {
                let sign = text.remove(0);
                text = format!("{sign}{}{text}", padding);
            } else {
                text = format!("{padding}{text}");
            }
        }
        out.push_str(&text);
    }
    Ok(out)
}

/// Laravel's `data_get()` for view values.
pub(crate) fn data_get(target: &ViewValue, key: &str) -> Option<ViewValue> {
    if key.is_empty() {
        return Some(target.clone());
    }
    if let ViewValue::Array(array) = target
        && let Some(value) = array.get_str(key)
    {
        return Some(value.clone());
    }
    let (segment, rest) = match key.split_once('.') {
        Some((segment, rest)) => (segment, Some(rest)),
        None => (key, None),
    };
    if segment == "*" {
        let items = to_array(target)?;
        let mut out = Vec::new();
        for item in items.values() {
            match rest {
                Some(rest) => {
                    if let Some(found) = data_get(item, rest) {
                        if rest.contains('*')
                            && let ViewValue::Array(nested) = &found
                        {
                            out.extend(nested.values().cloned());
                            continue;
                        }
                        out.push(found);
                    } else {
                        out.push(ViewValue::Null);
                    }
                }
                None => out.push(item.clone()),
            }
        }
        return Some(ViewValue::list(out));
    }
    let next = match target {
        ViewValue::Array(array) => array.get_str(segment).cloned(),
        ViewValue::Object(object) => object
            .get(segment)
            .or_else(|| object.offset_get(&ViewValue::from(segment))),
        _ => None,
    }?;
    match rest {
        Some(rest) => data_get(&next, rest),
        None => Some(next),
    }
}

/// Laravel's `value()`: call closures, return anything else.
pub(crate) fn value_of(
    value: &ViewValue,
    args: &[ViewValue],
    registry: &Arc<Registry>,
) -> Result<ViewValue> {
    match value {
        ViewValue::Closure(closure) => closure.call(args),
        _ => {
            let _ = registry;
            Ok(value.clone())
        }
    }
}

/// Laravel's `blank()`.
pub(crate) fn is_blank(value: &ViewValue) -> bool {
    match value {
        ViewValue::Null => true,
        ViewValue::Str(s) | ViewValue::Html(s) => s.trim().is_empty(),
        ViewValue::Array(a) => a.is_empty(),
        ViewValue::Object(o) => o.count() == Some(0),
        ViewValue::Bool(_) | ViewValue::Int(_) | ViewValue::Float(_) | ViewValue::Closure(_) => {
            false
        }
    }
}

/// Apply Laravel's `:placeholder` replacements to a translation line.
pub(crate) fn make_replacements(line: &str, replace: &ViewValue) -> Result<String> {
    let Some(replacements) = to_array(replace) else {
        return Ok(line.to_string());
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (key, value) in replacements.iter() {
        pairs.push((key.to_string(), php::to_str(value)?));
    }
    pairs.sort_by_key(|pair| std::cmp::Reverse(pair.0.len()));
    let mut out = line.to_string();
    for (key, value) in pairs {
        out = out
            .replace(&format!(":{}", Str::ucfirst(&key)), &Str::ucfirst(&value))
            .replace(&format!(":{}", key.to_uppercase()), &value.to_uppercase())
            .replace(&format!(":{key}"), &value);
    }
    Ok(out)
}

/// Choose the right plural form of a translation line (`trans_choice`).
pub(crate) fn choose_plural(line: &str, count: f64) -> String {
    let segments: Vec<&str> = line.split('|').collect();
    for segment in &segments {
        if let Some(text) = match_interval(segment, count) {
            return text;
        }
    }
    let stripped: Vec<String> = segments.iter().map(|s| strip_interval(s)).collect();
    if stripped.len() == 1 {
        return stripped[0].clone();
    }
    let index = if count == 1.0 { 0 } else { 1 };
    stripped
        .get(index)
        .cloned()
        .unwrap_or_else(|| stripped[0].clone())
}

fn match_interval(segment: &str, count: f64) -> Option<String> {
    let segment = segment.trim_start();
    if let Some(rest) = segment.strip_prefix('{') {
        let close = rest.find('}')?;
        let matches = rest[..close]
            .split(',')
            .any(|v| v.trim().parse::<f64>().ok() == Some(count));
        return matches.then(|| rest[close + 1..].trim_start().to_string());
    }
    let first = segment.chars().next()?;
    if first != '[' && first != ']' {
        return None;
    }
    let close = segment[1..].find([']', '['])? + 1;
    let (from, to) = segment[1..close].split_once(',')?;
    let left_inclusive = first == '[';
    let right_inclusive = segment.as_bytes()[close] == b']';
    let lower_ok = match from.trim() {
        "*" => true,
        f => f.parse::<f64>().is_ok_and(|f| {
            if left_inclusive {
                count >= f
            } else {
                count > f
            }
        }),
    };
    let upper_ok = match to.trim() {
        "*" => true,
        t => t.parse::<f64>().is_ok_and(|t| {
            if right_inclusive {
                count <= t
            } else {
                count < t
            }
        }),
    };
    (lower_ok && upper_ok).then(|| segment[close + 1..].trim_start().to_string())
}

fn strip_interval(segment: &str) -> String {
    let trimmed = segment.trim_start();
    if trimmed.starts_with('{')
        && let Some(close) = trimmed.find('}')
    {
        return trimmed[close + 1..].trim_start().to_string();
    }
    if (trimmed.starts_with('[') || trimmed.starts_with(']'))
        && let Some(close) = trimmed[1..].find([']', '['])
    {
        return trimmed[close + 2..].trim_start().to_string();
    }
    segment.to_string()
}

fn strip_tags(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_tag = false;
    let mut quote: Option<char> = None;
    for c in value.chars() {
        if in_tag {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '>') => in_tag = false,
                _ => {}
            }
        } else if c == '<' {
            in_tag = true;
        } else {
            out.push(c);
        }
    }
    out
}

fn decode_entities(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';').filter(|e| *e < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn url_encode(value: &str, plus_for_space: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(byte as char),
            b'~' if !plus_for_space => out.push('~'),
            b' ' if plus_for_space => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn url_decode(value: &str, plus_as_space: bool) -> String {
    let bytes = value.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' if plus_as_space => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high * 16 + low);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn http_build_query(array: &ViewArray, prefix: Option<&str>) -> String {
    let mut parts = Vec::new();
    for (key, value) in array.iter() {
        let name = match prefix {
            Some(prefix) => format!("{prefix}[{key}]"),
            None => key.to_string(),
        };
        match value {
            ViewValue::Array(nested) => {
                let nested = http_build_query(nested, Some(&name));
                if !nested.is_empty() {
                    parts.push(nested);
                }
            }
            ViewValue::Null => {}
            ViewValue::Bool(b) => parts.push(format!("{}={}", url_encode(&name, true), *b as i32)),
            other => parts.push(format!(
                "{}={}",
                url_encode(&name, true),
                url_encode(&other.to_string_lossy(), true)
            )),
        }
    }
    parts.join("&")
}

fn fnv_hash(value: &str) -> u128 {
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    for byte in value.bytes() {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(0x0000000001000000000000000000013B);
    }
    hash
}

/// A plain-text dump of a value.
pub(crate) fn dump_text(value: &ViewValue) -> String {
    match value {
        ViewValue::Str(s) => format!("\"{s}\""),
        ViewValue::Html(s) => format!("Illuminate\\Support\\HtmlString {{\"{s}\"}}"),
        ViewValue::Null => "null".into(),
        ViewValue::Bool(b) => b.to_string(),
        ViewValue::Int(_) | ViewValue::Float(_) => value.to_string_lossy(),
        ViewValue::Array(_) => php::json_encode(
            value,
            php::JSON_PRETTY_PRINT | php::JSON_UNESCAPED_SLASHES | php::JSON_UNESCAPED_UNICODE,
        )
        .unwrap_or_default(),
        ViewValue::Object(object) => format!(
            "{} {}",
            object.class_name(),
            serde_json::to_string_pretty(&object.to_json()).unwrap_or_default()
        ),
        ViewValue::Closure(_) => "Closure()".into(),
    }
}

/// The HTML `dump()` produces.
pub(crate) fn dump_html(value: &ViewValue) -> String {
    format!(
        "<pre class=\"sf-dump\">{}</pre>\n",
        illuminate_support::e(dump_text(value))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn call(name: &str, args: Vec<ViewValue>) -> ViewValue {
        let registry = Arc::new(Registry::default());
        call_builtin(name, &args, &registry).unwrap().unwrap()
    }

    #[test]
    fn string_functions_behave_like_php() {
        assert_eq!(
            call("ucwords", vec!["hello world".into()]),
            ViewValue::from("Hello World")
        );
        assert_eq!(call("trim", vec!["  x  ".into()]), ViewValue::from("x"));
        assert_eq!(
            call("trim", vec!["--x--".into(), "-".into()]),
            ViewValue::from("x")
        );
        assert_eq!(
            call("substr", vec!["Laravel".into(), (-3).into()]),
            ViewValue::from("vel")
        );
        assert_eq!(
            call("str_replace", vec!["a".into(), "o".into(), "banana".into()]),
            ViewValue::from("bonono")
        );
        assert_eq!(
            call("nl2br", vec!["a\nb".into()]),
            ViewValue::from("a<br />\nb")
        );
        assert_eq!(
            call("strip_tags", vec!["<b>bold</b> text".into()]),
            ViewValue::from("bold text")
        );
        assert_eq!(
            call("strpos", vec!["abc".into(), "z".into()]),
            ViewValue::Bool(false)
        );
        assert_eq!(
            call("str_pad", vec!["5".into(), 3.into(), "0".into(), 0.into()]),
            ViewValue::from("005")
        );
    }

    #[test]
    fn sprintf_supports_common_formats() {
        let out = sprintf(
            "%s has %d items costing %.2f (%05.1f) %'*6s %-4s| %x %%",
            &[
                "Cart".into(),
                3.into(),
                9.5.into(),
                2.25.into(),
                "ab".into(),
                "l".into(),
                255.into(),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            "Cart has 3 items costing 9.50 (002.3) ****ab l   | ff %"
        );
        assert_eq!(
            sprintf("%2$s %1$s", &["a".into(), "b".into()]).unwrap(),
            "b a"
        );
    }

    #[test]
    fn number_formatting_rounds_like_php() {
        assert_eq!(number_format(1234.5, 0, ".", ","), "1,235");
        assert_eq!(number_format(1234.567, 2, ".", ","), "1,234.57");
        assert_eq!(number_format(1.005, 2, ".", ","), "1.01");
        assert_eq!(number_format(-1234.5, 1, ",", "."), "-1.234,5");
        assert_eq!(php_round(2.5, 0), 3.0);
        assert_eq!(php_round(-2.5, 0), -3.0);
        assert_eq!(php_round(1234.0, -2), 1200.0);
    }

    #[test]
    fn array_functions_preserve_php_semantics() {
        let arr = ViewValue::from(json!({"a": 1, "b": 0, "c": 3}));
        assert_eq!(
            call("array_filter", vec![arr.clone()]).to_json(),
            json!({"a": 1, "c": 3})
        );
        assert_eq!(
            call("array_keys", vec![arr.clone()]).to_json(),
            json!(["a", "b", "c"])
        );
        assert_eq!(call("array_sum", vec![arr.clone()]), ViewValue::Int(4));
        assert_eq!(
            call(
                "array_merge",
                vec![
                    ViewValue::from(json!([1, 2])),
                    ViewValue::from(json!({"x": 1, "0": 9}))
                ]
            )
            .to_json(),
            json!({"0": 1, "1": 2, "x": 1, "2": 9})
        );
        assert_eq!(
            call(
                "implode",
                vec![", ".into(), ViewValue::from(json!(["a", "b"]))]
            ),
            ViewValue::from("a, b")
        );
        assert_eq!(
            call("explode", vec![",".into(), "a,b,c".into(), 2.into()]).to_json(),
            json!(["a", "b,c"])
        );
        assert_eq!(
            call("range", vec![1.into(), 3.into()]).to_json(),
            json!([1, 2, 3])
        );
        assert_eq!(
            call("range", vec!["a".into(), "c".into()]).to_json(),
            json!(["a", "b", "c"])
        );
        assert_eq!(
            call("max", vec![1.into(), 5.into(), 3.into()]),
            ViewValue::Int(5)
        );
        assert_eq!(
            call("in_array", vec!["2".into(), ViewValue::from(json!([1, 2]))]),
            ViewValue::Bool(true)
        );
    }

    #[test]
    fn translation_helpers_have_sensible_defaults() {
        assert_eq!(
            call(
                "__",
                vec![
                    "Welcome, :name!".into(),
                    ViewValue::from(json!({"name": "taylor"}))
                ]
            ),
            ViewValue::from("Welcome, taylor!")
        );
        assert_eq!(
            call(
                "__",
                vec![
                    "Welcome, :Name!".into(),
                    ViewValue::from(json!({"name": "taylor"}))
                ]
            ),
            ViewValue::from("Welcome, Taylor!")
        );
        assert_eq!(
            call("trans_choice", vec!["apple|apples".into(), 1.into()]),
            ViewValue::from("apple")
        );
        assert_eq!(
            call("trans_choice", vec!["apple|apples".into(), 5.into()]),
            ViewValue::from("apples")
        );
        assert_eq!(
            call(
                "trans_choice",
                vec!["{0} none|[1,19] some|[20,*] many :count".into(), 25.into()]
            ),
            ViewValue::from("many 25")
        );
        assert_eq!(
            call(
                "trans_choice",
                vec!["{0} none|[1,19] some|[20,*] many".into(), 0.into()]
            ),
            ViewValue::from("none")
        );
    }

    #[test]
    fn data_get_supports_wildcards() {
        let data = ViewValue::from(json!({"users": [{"name": "a"}, {"name": "b"}]}));
        assert_eq!(
            data_get(&data, "users.*.name").unwrap().to_json(),
            json!(["a", "b"])
        );
        assert_eq!(
            data_get(&data, "users.1.name").unwrap(),
            ViewValue::from("b")
        );
        assert!(data_get(&data, "users.5.name").is_none());
    }
}
