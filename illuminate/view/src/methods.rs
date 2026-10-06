//! Methods on plain values.
//!
//! Arrays behave like Laravel collections (`$users->count()`,
//! `$posts->pluck('title')`), serialized models and paginators answer their
//! accessor methods (`$users->total()` reads `total`, `$post->createdAt()`
//! reads `created_at`), and strings respond to `Str` methods and — when they
//! hold a date — to Carbon's (`$post->published_at->diffForHumans()`).

use std::cmp::Ordering;
use std::sync::Arc;

use illuminate_support::{Result, Str};

use crate::exception::{BadMethodCallException, error};
use crate::expr::eval::call_callable;
use crate::functions::{self, arg, bool_arg, int_arg, str_arg, to_array};
use crate::objects;
use crate::php;
use crate::registry::Registry;
use crate::value::{ArrayKey, ViewArray, ViewValue};

/// Call a method on a value.
pub(crate) fn call_method(
    target: &ViewValue,
    name: &str,
    args: Vec<ViewValue>,
    registry: &Arc<Registry>,
) -> Result<ViewValue> {
    match target {
        ViewValue::Object(object) => {
            if let Some(result) = object.call(name, &args) {
                return result;
            }
            if let Some(ViewValue::Closure(closure)) = object.get(name) {
                return closure.call(&args);
            }
            Err(BadMethodCallException::new(format!(
                "Call to undefined method {}::{name}()",
                object.class_name()
            ))
            .into())
        }
        ViewValue::Array(array) => array_method(array, name, &args, registry),
        ViewValue::Str(s) => string_method(s, name, &args, registry),
        ViewValue::Html(html) => match name {
            "toHtml" | "__toString" => Ok(ViewValue::Html(html.clone())),
            "isEmpty" => Ok(ViewValue::Bool(html.is_empty())),
            "isNotEmpty" => Ok(ViewValue::Bool(!html.is_empty())),
            _ => Err(BadMethodCallException::new(format!(
                "Call to undefined method Illuminate\\Support\\HtmlString::{name}()"
            ))
            .into()),
        },
        ViewValue::Closure(closure) => match name {
            "call" | "__invoke" => closure.call(&args),
            _ => Err(BadMethodCallException::new(format!("Call to undefined method Closure::{name}()")).into()),
        },
        ViewValue::Null => Err(error(format!("Call to a member function {name}() on null"))),
        other => Err(error(format!("Call to a member function {name}() on {}", other.type_name()))),
    }
}

/// Read a date property from a string holding a date.
pub(crate) fn date_property(value: &str, property: &str) -> Option<ViewValue> {
    objects::date_string_property(value, property)
}

// ----------------------------------------------------------------------
// Paginators
// ----------------------------------------------------------------------

/// Determine if an array is a serialized paginator (`{data, per_page, path, ...}`).
pub(crate) fn is_paginator(array: &ViewArray) -> bool {
    array.len() >= 4
        && matches!(array.get_str("data"), Some(ViewValue::Array(_)))
        && array.contains_key("per_page")
        && array.contains_key("path")
}

/// The items of a serialized paginator.
pub(crate) fn paginator_items(array: &ViewArray) -> Option<Arc<ViewArray>> {
    if !is_paginator(array) {
        return None;
    }
    match array.get_str("data") {
        Some(ViewValue::Array(items)) => Some(items.clone()),
        _ => None,
    }
}

fn paginator_method(array: &ViewArray, name: &str, args: &[ViewValue]) -> Option<ViewValue> {
    let get = |key: &str| array.get_str(key).cloned().unwrap_or_default();
    let int = |key: &str| array.get_str(key).and_then(ViewValue::as_i64);
    Some(match name {
        "total" => get("total"),
        "currentPage" => get("current_page"),
        "lastPage" => get("last_page"),
        "perPage" => get("per_page"),
        "firstItem" => get("from"),
        "lastItem" => get("to"),
        "nextPageUrl" => get("next_page_url"),
        "previousPageUrl" => get("prev_page_url"),
        "path" => get("path"),
        "items" => get("data"),
        "nextCursor" => get("next_cursor"),
        "previousCursor" => get("prev_cursor"),
        "onFirstPage" => ViewValue::Bool(match int("current_page") {
            Some(page) => page <= 1,
            None => get("prev_page_url").is_null(),
        }),
        "hasMorePages" => ViewValue::Bool(match (int("current_page"), int("last_page")) {
            (Some(current), Some(last)) => current < last,
            _ => !get("next_page_url").is_null(),
        }),
        "onLastPage" => ViewValue::Bool(match (int("current_page"), int("last_page")) {
            (Some(current), Some(last)) => current >= last,
            _ => get("next_page_url").is_null(),
        }),
        "hasPages" => ViewValue::Bool(match int("last_page") {
            Some(last) => last > 1,
            None => int("current_page").is_some_and(|p| p > 1) || !get("next_page_url").is_null(),
        }),
        "url" => {
            let page = int_arg(args, 0, 1).max(1);
            let path = get("path").to_string_lossy();
            let separator = if path.contains('?') { '&' } else { '?' };
            ViewValue::from(format!("{path}{separator}page={page}"))
        }
        "links" | "render" => ViewValue::html(pagination_links(array)),
        _ => return None,
    })
}

/// A simple, accessible pagination navigation.
fn pagination_links(array: &ViewArray) -> String {
    let get = |key: &str| array.get_str(key).cloned().unwrap_or_default();
    let url = |page: i64| {
        let path = get("path").to_string_lossy();
        let separator = if path.contains('?') { '&' } else { '?' };
        format!("{path}{separator}page={page}")
    };
    let current = get("current_page").as_i64().unwrap_or(1);
    let last = get("last_page").as_i64();
    if last.is_some_and(|l| l <= 1) || (last.is_none() && current <= 1 && get("next_page_url").is_null()) {
        return String::new();
    }
    let mut html = String::from("<nav role=\"navigation\" aria-label=\"Pagination Navigation\">\n    <ul class=\"pagination\">\n");
    match get("prev_page_url") {
        ViewValue::Null => html.push_str(
            "        <li class=\"page-item disabled\" aria-disabled=\"true\"><span class=\"page-link\">&laquo; Previous</span></li>\n",
        ),
        prev => html.push_str(&format!(
            "        <li class=\"page-item\"><a class=\"page-link\" href=\"{}\" rel=\"prev\">&laquo; Previous</a></li>\n",
            illuminate_support::e(prev.to_string_lossy())
        )),
    }
    if let Some(last) = last {
        for page in 1..=last {
            if page == current {
                html.push_str(&format!(
                    "        <li class=\"page-item active\" aria-current=\"page\"><span class=\"page-link\">{page}</span></li>\n"
                ));
            } else {
                html.push_str(&format!(
                    "        <li class=\"page-item\"><a class=\"page-link\" href=\"{}\">{page}</a></li>\n",
                    illuminate_support::e(url(page))
                ));
            }
        }
    }
    match get("next_page_url") {
        ViewValue::Null => html.push_str(
            "        <li class=\"page-item disabled\" aria-disabled=\"true\"><span class=\"page-link\">Next &raquo;</span></li>\n",
        ),
        next => html.push_str(&format!(
            "        <li class=\"page-item\"><a class=\"page-link\" href=\"{}\" rel=\"next\">Next &raquo;</a></li>\n",
            illuminate_support::e(next.to_string_lossy())
        )),
    }
    html.push_str("    </ul>\n</nav>\n");
    html
}

// ----------------------------------------------------------------------
// Collections
// ----------------------------------------------------------------------

fn list(items: impl IntoIterator<Item = ViewValue>) -> ViewValue {
    ViewValue::list(items)
}

fn from_pairs(pairs: impl IntoIterator<Item = (ArrayKey, ViewValue)>) -> ViewValue {
    ViewValue::from(pairs.into_iter().collect::<ViewArray>())
}

/// Read `key` from an item using "dot" notation (for pluck, sortBy, where...).
fn item_value(item: &ViewValue, key: &ViewValue, registry: &Arc<Registry>) -> Result<ViewValue> {
    match key {
        ViewValue::Closure(_) => call_callable(key, std::slice::from_ref(item), registry),
        ViewValue::Null => Ok(item.clone()),
        other => Ok(functions::data_get(item, &php::to_str(other)?).unwrap_or_default()),
    }
}

/// Evaluate a `where` style comparison.
fn compare_op(left: &ViewValue, op: &str, right: &ViewValue) -> bool {
    match op {
        "=" | "==" => php::loose_eq(left, right),
        "!=" | "<>" => !php::loose_eq(left, right),
        "===" => php::strict_eq(left, right),
        "!==" => !php::strict_eq(left, right),
        "<" => php::compare(left, right) == Ordering::Less,
        "<=" => php::compare(left, right) != Ordering::Greater,
        ">" => php::compare(left, right) == Ordering::Greater,
        ">=" => php::compare(left, right) != Ordering::Less,
        _ => php::loose_eq(left, right),
    }
}

fn callback_args(value: &ViewValue, key: &ArrayKey) -> [ViewValue; 2] {
    [value.clone(), key.to_value()]
}

fn array_method(array: &Arc<ViewArray>, name: &str, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    if is_paginator(array) {
        if let Some(value) = paginator_method(array, name, args) {
            return Ok(value);
        }
        if let Some(items) = paginator_items(array) {
            if collection_method_exists(name) {
                return collection_method(&items, name, args, registry);
            }
        }
    }
    if collection_method_exists(name) {
        return collection_method(array, name, args, registry);
    }
    // Serialized models: `$user->fullName()` reads `full_name` (or `fullName`).
    let snake = Str::snake(name);
    let found = array.get_str(&snake).or_else(|| array.get_str(name)).or_else(|| {
        name.strip_prefix("get")
            .filter(|rest| rest.starts_with(|c: char| c.is_uppercase()))
            .and_then(|rest| array.get_str(&Str::snake(rest)))
    });
    match found {
        Some(ViewValue::Closure(closure)) => closure.call(args),
        Some(value) => Ok(value.clone()),
        None => Err(BadMethodCallException::new(format!("Call to undefined method {name}() on array")).into()),
    }
}

const COLLECTION_METHODS: &[&str] = &[
    "count", "isEmpty", "isNotEmpty", "first", "last", "keys", "values", "all", "toArray", "jsonSerialize",
    "toJson", "toPrettyJson", "contains", "doesntContain", "containsStrict", "has", "hasAny", "get", "pluck", "implode",
    "join", "sum", "avg", "average", "max", "min", "median", "take", "skip", "slice", "sortBy", "sortByDesc", "sort",
    "sortDesc", "sortKeys", "sortKeysDesc", "reverse", "where", "whereStrict", "whereIn", "whereNotIn", "whereNull",
    "whereNotNull", "whereBetween", "firstWhere", "filter", "reject", "map", "mapWithKeys", "flatMap", "each",
    "groupBy", "keyBy", "chunk", "unique", "merge", "only", "except", "search", "flip", "flatten", "collapse",
    "push", "prepend", "put", "pull", "forget", "every", "some", "isList", "random", "shuffle", "split", "pipe",
    "when", "unless", "whenEmpty", "whenNotEmpty", "zip", "combine", "diff", "intersect", "nth", "pad", "countBy",
    "sole", "firstOrFail", "collect", "toBase", "dump", "dd", "concat", "partition", "reduce", "takeWhile",
    "takeUntil", "skipWhile", "skipUntil", "dot", "undot", "mapInto", "tap", "transform",
];

fn collection_method_exists(name: &str) -> bool {
    COLLECTION_METHODS.contains(&name)
}

fn sorted_by(
    array: &ViewArray,
    key: Option<&ViewValue>,
    descending: bool,
    registry: &Arc<Registry>,
) -> Result<ViewValue> {
    let mut entries: Vec<(ArrayKey, ViewValue, ViewValue)> = Vec::with_capacity(array.len());
    for (k, v) in array.iter() {
        let sort_key = match key {
            Some(key) => item_value(v, key, registry)?,
            None => v.clone(),
        };
        entries.push((k.clone(), v.clone(), sort_key));
    }
    entries.sort_by(|a, b| {
        let ordering = php::compare(&a.2, &b.2);
        if descending { ordering.reverse() } else { ordering }
    });
    Ok(from_pairs(entries.into_iter().map(|(k, v, _)| (k, v))))
}

fn collection_method(array: &Arc<ViewArray>, name: &str, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    let a0 = arg(args, 0);
    let call = |callback: &ViewValue, value: &ViewValue, key: &ArrayKey| {
        call_callable(callback, &callback_args(value, key), registry)
    };
    Ok(match name {
        "count" => ViewValue::from(array.len()),
        "isEmpty" => ViewValue::Bool(array.is_empty()),
        "isNotEmpty" => ViewValue::Bool(!array.is_empty()),
        "isList" => ViewValue::Bool(array.is_list()),
        "all" | "toArray" | "jsonSerialize" | "collect" | "toBase" => ViewValue::Array(array.clone()),
        "toJson" => ViewValue::from(php::json_encode(&ViewValue::Array(array.clone()), int_arg(args, 0, 0))?),
        "toPrettyJson" => ViewValue::from(php::json_encode(&ViewValue::Array(array.clone()), php::JSON_PRETTY_PRINT)?),
        "keys" => list(array.keys().map(ArrayKey::to_value)),
        "values" => list(array.values().cloned()),
        "first" | "firstOrFail" | "sole" => {
            let found = match args.first() {
                Some(callback @ ViewValue::Closure(_)) => {
                    let mut found = None;
                    for (k, v) in array.iter() {
                        if call(callback, v, k)?.truthy() {
                            found = Some(v.clone());
                            break;
                        }
                    }
                    found
                }
                _ => array.first().cloned(),
            };
            match found {
                Some(value) => value,
                None if name == "first" => functions::value_of(arg(args, 1), &[], registry)?,
                None => return Err(error("Item not found.")),
            }
        }
        "last" => match args.first() {
            Some(callback @ ViewValue::Closure(_)) => {
                let mut found = ViewValue::Null;
                for (k, v) in array.iter().rev() {
                    if call(callback, v, k)?.truthy() {
                        found = v.clone();
                        break;
                    }
                }
                found
            }
            _ => array.last().cloned().unwrap_or_default(),
        },
        "contains" | "containsStrict" | "doesntContain" | "some" => {
            let found = match args.len() {
                0 => !array.is_empty(),
                1 => match a0 {
                    callback @ ViewValue::Closure(_) => {
                        let mut found = false;
                        for (k, v) in array.iter() {
                            if call(callback, v, k)?.truthy() {
                                found = true;
                                break;
                            }
                        }
                        found
                    }
                    needle => array.values().any(|v| {
                        if name == "containsStrict" { php::strict_eq(v, needle) } else { php::loose_eq(v, needle) }
                    }),
                },
                _ => {
                    let (op, value) = if args.len() >= 3 { (str_arg(args, 1)?, arg(args, 2)) } else { ("=".into(), arg(args, 1)) };
                    let mut found = false;
                    for item in array.values() {
                        if compare_op(&item_value(item, a0, registry)?, &op, value) {
                            found = true;
                            break;
                        }
                    }
                    found
                }
            };
            ViewValue::Bool(if name == "doesntContain" { !found } else { found })
        }
        "has" => {
            let keys: Vec<ViewValue> = if args.len() > 1 { args.to_vec() } else { key_values(a0) };
            ViewValue::Bool(keys.iter().all(|k| array.get_value(k).is_some()))
        }
        "hasAny" => {
            let keys: Vec<ViewValue> = if args.len() > 1 { args.to_vec() } else { key_values(a0) };
            ViewValue::Bool(keys.iter().any(|k| array.get_value(k).is_some()))
        }
        "get" => match array.get_value(a0) {
            Some(value) => value.clone(),
            None => functions::value_of(arg(args, 1), &[], registry)?,
        },
        "pluck" => {
            let mut out = ViewArray::new();
            for item in array.values() {
                let value = item_value(item, a0, registry)?;
                match args.get(1) {
                    Some(key) if !key.is_null() => {
                        let key = item_value(item, key, registry)?;
                        out.insert(ArrayKey::from_value(&key)?, value);
                    }
                    _ => out.push(value),
                }
            }
            ViewValue::from(out)
        }
        "implode" | "join" => {
            let (values, glue): (Vec<ViewValue>, String) = match (a0, args.get(1)) {
                (key @ ViewValue::Str(_), Some(glue)) if name == "implode" => {
                    let values = array.values().map(|item| item_value(item, key, registry)).collect::<Result<_>>()?;
                    (values, php::to_str(glue)?)
                }
                (callback @ ViewValue::Closure(_), glue) => {
                    let mut values = Vec::new();
                    for (k, v) in array.iter() {
                        values.push(call(callback, v, k)?);
                    }
                    (values, glue.map(php::to_str).transpose()?.unwrap_or_default())
                }
                (glue, final_glue) if name == "join" && final_glue.is_some() => {
                    let glue = php::to_str(glue)?;
                    let final_glue = php::to_str(final_glue.unwrap_or(&functions::NULL))?;
                    let parts: Vec<String> = array.values().map(php::to_str).collect::<Result<_>>()?;
                    let joined = match parts.len() {
                        0 => String::new(),
                        1 => parts[0].clone(),
                        n => format!("{}{}{}", parts[..n - 1].join(&glue), final_glue, parts[n - 1]),
                    };
                    return Ok(ViewValue::from(joined));
                }
                (glue, _) => (array.values().cloned().collect(), php::to_str(glue)?),
            };
            let parts: Vec<String> = values.iter().map(php::to_str).collect::<Result<_>>()?;
            ViewValue::from(parts.join(&glue))
        }
        "sum" | "avg" | "average" | "max" | "min" | "median" => {
            let mut values = Vec::with_capacity(array.len());
            for item in array.values() {
                let value = match args.first() {
                    Some(key) => item_value(item, key, registry)?,
                    None => item.clone(),
                };
                if !value.is_null() || name == "sum" {
                    values.push(value);
                }
            }
            match name {
                "sum" => {
                    let mut total = ViewValue::Int(0);
                    for value in &values {
                        total = php::arithmetic(php::Arith::Add, &total, value)?;
                    }
                    total
                }
                "avg" | "average" => {
                    if values.is_empty() {
                        ViewValue::Null
                    } else {
                        let mut total = ViewValue::Int(0);
                        for value in &values {
                            total = php::arithmetic(php::Arith::Add, &total, value)?;
                        }
                        php::arithmetic(php::Arith::Div, &total, &ViewValue::from(values.len()))?
                    }
                }
                "median" => {
                    if values.is_empty() {
                        ViewValue::Null
                    } else {
                        values.sort_by(php::compare);
                        let middle = values.len() / 2;
                        if values.len() % 2 == 1 {
                            values[middle].clone()
                        } else {
                            let total = php::arithmetic(php::Arith::Add, &values[middle - 1], &values[middle])?;
                            php::arithmetic(php::Arith::Div, &total, &ViewValue::Int(2))?
                        }
                    }
                }
                _ => {
                    let mut best: Option<ViewValue> = None;
                    for value in values {
                        best = Some(match best {
                            None => value,
                            Some(current) => {
                                let ordering = php::compare(&value, &current);
                                if (name == "max" && ordering.is_gt()) || (name == "min" && ordering.is_lt()) {
                                    value
                                } else {
                                    current
                                }
                            }
                        });
                    }
                    best.unwrap_or_default()
                }
            }
        }
        "take" => {
            let n = int_arg(args, 0, 0);
            if n < 0 {
                ViewValue::from(functions::slice(array, n, None, true))
            } else {
                ViewValue::from(functions::slice(array, 0, Some(n), true))
            }
        }
        "skip" => ViewValue::from(functions::slice(array, int_arg(args, 0, 0), None, true)),
        "slice" => {
            let length = match args.get(1) {
                None | Some(ViewValue::Null) => None,
                Some(v) => v.as_i64(),
            };
            ViewValue::from(functions::slice(array, int_arg(args, 0, 0), length, true))
        }
        "nth" => {
            let step = int_arg(args, 0, 1).max(1) as usize;
            let offset = int_arg(args, 1, 0).max(0) as usize;
            list(array.values().skip(offset).step_by(step).cloned())
        }
        "sortBy" => sorted_by(array, args.first(), bool_arg(args, 2, false), registry)?,
        "sortByDesc" => sorted_by(array, args.first(), true, registry)?,
        "sort" => match args.first() {
            Some(callback @ ViewValue::Closure(_)) => {
                let mut entries: Vec<(ArrayKey, ViewValue)> = array.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let mut failure = None;
                entries.sort_by(|a, b| match call_callable(callback, &[a.1.clone(), b.1.clone()], registry) {
                    Ok(result) => result.as_i64().unwrap_or(0).cmp(&0),
                    Err(e) => {
                        failure.get_or_insert(e);
                        Ordering::Equal
                    }
                });
                if let Some(e) = failure {
                    return Err(e);
                }
                from_pairs(entries)
            }
            _ => sorted_by(array, None, false, registry)?,
        },
        "sortDesc" => sorted_by(array, None, true, registry)?,
        "sortKeys" | "sortKeysDesc" => {
            let mut entries: Vec<(ArrayKey, ViewValue)> = array.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            entries.sort_by(|a, b| php::compare(&a.0.to_value(), &b.0.to_value()));
            if name == "sortKeysDesc" {
                entries.reverse();
            }
            from_pairs(entries)
        }
        "reverse" => from_pairs(array.iter().rev().map(|(k, v)| (k.clone(), v.clone()))),
        "where" | "whereStrict" => {
            let (op, value) = match args.len() {
                1 => ("=".to_string(), ViewValue::Bool(true)),
                2 => (if name == "whereStrict" { "===".into() } else { "=".into() }, arg(args, 1).clone()),
                _ => (str_arg(args, 1)?, arg(args, 2).clone()),
            };
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let actual = item_value(item, a0, registry)?;
                let keep = if args.len() == 1 { actual.truthy() } else { compare_op(&actual, &op, &value) };
                if keep {
                    out.insert(k.clone(), item.clone());
                }
            }
            ViewValue::from(out)
        }
        "firstWhere" => {
            let (op, value) = match args.len() {
                1 => ("=".to_string(), ViewValue::Bool(true)),
                2 => ("=".to_string(), arg(args, 1).clone()),
                _ => (str_arg(args, 1)?, arg(args, 2).clone()),
            };
            let mut found = ViewValue::Null;
            for item in array.values() {
                let actual = item_value(item, a0, registry)?;
                let matches = if args.len() == 1 { actual.truthy() } else { compare_op(&actual, &op, &value) };
                if matches {
                    found = item.clone();
                    break;
                }
            }
            found
        }
        "whereIn" | "whereNotIn" => {
            let candidates: Vec<ViewValue> = to_array(arg(args, 1)).map(|a| a.values().cloned().collect()).unwrap_or_default();
            let strict = bool_arg(args, 2, false);
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let actual = item_value(item, a0, registry)?;
                let present = candidates.iter().any(|c| if strict { php::strict_eq(c, &actual) } else { php::loose_eq(c, &actual) });
                if present == (name == "whereIn") {
                    out.insert(k.clone(), item.clone());
                }
            }
            ViewValue::from(out)
        }
        "whereNull" | "whereNotNull" => {
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let actual = match args.first() {
                    Some(key) => item_value(item, key, registry)?,
                    None => item.clone(),
                };
                if actual.is_null() == (name == "whereNull") {
                    out.insert(k.clone(), item.clone());
                }
            }
            ViewValue::from(out)
        }
        "whereBetween" => {
            let bounds = to_array(arg(args, 1)).unwrap_or_default();
            let (low, high) = (bounds.first().cloned().unwrap_or_default(), bounds.last().cloned().unwrap_or_default());
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let actual = item_value(item, a0, registry)?;
                if php::compare(&actual, &low).is_ge() && php::compare(&actual, &high).is_le() {
                    out.insert(k.clone(), item.clone());
                }
            }
            ViewValue::from(out)
        }
        "filter" | "reject" => {
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let truthy = match args.first() {
                    Some(callback) if !callback.is_null() => call(callback, item, k)?.truthy(),
                    _ => item.truthy(),
                };
                if truthy == (name == "filter") {
                    out.insert(k.clone(), item.clone());
                }
            }
            ViewValue::from(out)
        }
        "map" | "transform" => {
            let mut out = ViewArray::with_capacity(array.len());
            for (k, item) in array.iter() {
                out.insert(k.clone(), call(a0, item, k)?);
            }
            ViewValue::from(out)
        }
        "mapWithKeys" => {
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                if let ViewValue::Array(pairs) = call(a0, item, k)? {
                    for (key, value) in pairs.iter() {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            ViewValue::from(out)
        }
        "flatMap" => {
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                match call(a0, item, k)? {
                    ViewValue::Array(inner) => functions::merge_into(&mut out, &inner),
                    other => out.push(other),
                }
            }
            ViewValue::from(out)
        }
        "each" => {
            for (k, item) in array.iter() {
                if matches!(call(a0, item, k)?, ViewValue::Bool(false)) {
                    break;
                }
            }
            ViewValue::Array(array.clone())
        }
        "every" => {
            let mut all = true;
            for (k, item) in array.iter() {
                let ok = match a0 {
                    ViewValue::Closure(_) => call(a0, item, k)?.truthy(),
                    key if args.len() >= 2 => {
                        let (op, value) = if args.len() >= 3 { (str_arg(args, 1)?, arg(args, 2)) } else { ("=".into(), arg(args, 1)) };
                        compare_op(&item_value(item, key, registry)?, &op, value)
                    }
                    key => item_value(item, key, registry)?.truthy(),
                };
                if !ok {
                    all = false;
                    break;
                }
            }
            ViewValue::Bool(all)
        }
        "groupBy" | "keyBy" | "countBy" => {
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let group = match args.first() {
                    Some(key @ ViewValue::Closure(_)) => call(key, item, k)?,
                    Some(key) => item_value(item, key, registry)?,
                    None => item.clone(),
                };
                let group_key = ArrayKey::from_value(&match group {
                    ViewValue::Bool(b) => ViewValue::Int(b as i64),
                    other => other,
                })?;
                match name {
                    "keyBy" => out.insert(group_key, item.clone()),
                    "countBy" => {
                        let current = out.get(&group_key).and_then(ViewValue::as_i64).unwrap_or(0);
                        out.insert(group_key, ViewValue::Int(current + 1));
                    }
                    _ => {
                        let mut bucket = match out.get(&group_key) {
                            Some(ViewValue::Array(existing)) => (**existing).clone(),
                            _ => ViewArray::new(),
                        };
                        if bool_arg(args, 1, false) {
                            bucket.insert(k.clone(), item.clone());
                        } else {
                            bucket.push(item.clone());
                        }
                        out.insert(group_key, ViewValue::from(bucket));
                    }
                }
            }
            ViewValue::from(out)
        }
        "chunk" | "split" => {
            let items: Vec<(ArrayKey, ViewValue)> = array.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let n = int_arg(args, 0, 1).max(1) as usize;
            let size = if name == "split" { items.len().div_ceil(n).max(1) } else { n };
            list(items.chunks(size).map(|chunk| {
                if name == "split" {
                    list(chunk.iter().map(|(_, v)| v.clone()))
                } else {
                    from_pairs(chunk.iter().cloned())
                }
            }))
        }
        "unique" => {
            let mut seen: Vec<ViewValue> = Vec::new();
            let mut out = ViewArray::new();
            for (k, item) in array.iter() {
                let id = match args.first() {
                    Some(key) => item_value(item, key, registry)?,
                    None => item.clone(),
                };
                if !seen.iter().any(|s| php::loose_eq(s, &id)) {
                    seen.push(id);
                    out.insert(k.clone(), item.clone());
                }
            }
            ViewValue::from(out)
        }
        "merge" | "concat" => {
            let mut out = (**array).clone();
            if let Some(other) = to_array(a0) {
                if name == "concat" {
                    for value in other.values() {
                        out.push(value.clone());
                    }
                } else {
                    functions::merge_into(&mut out, &other);
                }
            }
            ViewValue::from(out)
        }
        "only" | "except" => {
            let keys: Vec<ViewValue> = if args.len() > 1 { args.to_vec() } else { key_values(a0) };
            let keys: Vec<ArrayKey> = keys.iter().map(ArrayKey::from_value).collect::<Result<_>>()?;
            from_pairs(
                array
                    .iter()
                    .filter(|(k, _)| keys.contains(k) == (name == "only"))
                    .map(|(k, v)| (k.clone(), v.clone())),
            )
        }
        "search" => {
            let mut found = ViewValue::Bool(false);
            for (k, item) in array.iter() {
                let matches = match a0 {
                    ViewValue::Closure(_) => call(a0, item, k)?.truthy(),
                    needle => {
                        if bool_arg(args, 1, false) { php::strict_eq(item, needle) } else { php::loose_eq(item, needle) }
                    }
                };
                if matches {
                    found = k.to_value();
                    break;
                }
            }
            found
        }
        "flip" => {
            let mut out = ViewArray::new();
            for (k, v) in array.iter() {
                out.insert(ArrayKey::from_value(v)?, k.to_value());
            }
            ViewValue::from(out)
        }
        "flatten" => {
            let depth = match args.first() {
                None | Some(ViewValue::Null) => usize::MAX,
                Some(v) => v.as_i64().unwrap_or(0).max(0) as usize,
            };
            let mut out = Vec::new();
            flatten_into(array, depth, &mut out);
            list(out)
        }
        "collapse" => {
            let mut out = ViewArray::new();
            for item in array.values() {
                if let Some(inner) = to_array(item) {
                    functions::merge_into(&mut out, &inner);
                }
            }
            ViewValue::from(out)
        }
        "push" => {
            let mut out = (**array).clone();
            for value in args {
                out.push(value.clone());
            }
            ViewValue::from(out)
        }
        "prepend" => {
            let mut out = ViewArray::new();
            match args.get(1) {
                Some(key) => out.insert(ArrayKey::from_value(key)?, a0.clone()),
                None => out.push(a0.clone()),
            }
            for (k, v) in array.iter() {
                match k {
                    ArrayKey::Int(_) if args.get(1).is_none() => out.push(v.clone()),
                    k => out.insert(k.clone(), v.clone()),
                }
            }
            ViewValue::from(out)
        }
        "put" => {
            let mut out = (**array).clone();
            out.insert(ArrayKey::from_value(a0)?, arg(args, 1).clone());
            ViewValue::from(out)
        }
        "forget" | "pull" => {
            let mut out = (**array).clone();
            let removed = out.remove(&ArrayKey::from_value(a0)?);
            if name == "pull" { removed.unwrap_or_default() } else { ViewValue::from(out) }
        }
        "random" => {
            if array.is_empty() {
                return Err(error("You requested 1 items, but there are only 0 items available."));
            }
            let index = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0) as usize)
                % array.len();
            array.values().nth(index).cloned().unwrap_or_default()
        }
        "shuffle" => {
            let mut values: Vec<ViewValue> = array.values().cloned().collect();
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(7);
            let len = values.len();
            for i in (1..len).rev() {
                let j = (seed.wrapping_mul(31).wrapping_add(i * 17)) % (i + 1);
                values.swap(i, j);
            }
            list(values)
        }
        "pipe" => call_callable(a0, &[ViewValue::Array(array.clone())], registry)?,
        "tap" => {
            call_callable(a0, &[ViewValue::Array(array.clone())], registry)?;
            ViewValue::Array(array.clone())
        }
        "when" | "unless" => {
            let condition = match a0 {
                ViewValue::Closure(_) => call_callable(a0, &[ViewValue::Array(array.clone())], registry)?.truthy(),
                other => other.truthy(),
            };
            let callback = if condition == (name == "when") { args.get(1) } else { args.get(2) };
            match callback {
                Some(callback) => {
                    let result = call_callable(callback, &[ViewValue::Array(array.clone()), a0.clone()], registry)?;
                    if result.is_null() { ViewValue::Array(array.clone()) } else { result }
                }
                None => ViewValue::Array(array.clone()),
            }
        }
        "whenEmpty" | "whenNotEmpty" => {
            if array.is_empty() == (name == "whenEmpty") {
                let result = call_callable(a0, &[ViewValue::Array(array.clone())], registry)?;
                if result.is_null() { ViewValue::Array(array.clone()) } else { result }
            } else {
                ViewValue::Array(array.clone())
            }
        }
        "zip" => {
            let others: Vec<Vec<ViewValue>> =
                args.iter().map(|a| to_array(a).map(|a| a.values().cloned().collect()).unwrap_or_default()).collect();
            list(array.values().enumerate().map(|(i, v)| {
                let mut row = vec![v.clone()];
                row.extend(others.iter().map(|o| o.get(i).cloned().unwrap_or_default()));
                list(row)
            }))
        }
        "combine" => {
            let values: Vec<ViewValue> = to_array(a0).map(|a| a.values().cloned().collect()).unwrap_or_default();
            let mut out = ViewArray::new();
            for (key, value) in array.values().zip(values) {
                out.insert(ArrayKey::from_value(key)?, value);
            }
            ViewValue::from(out)
        }
        "diff" | "intersect" => {
            let other: Vec<ViewValue> = to_array(a0).map(|a| a.values().cloned().collect()).unwrap_or_default();
            from_pairs(
                array
                    .iter()
                    .filter(|(_, v)| other.iter().any(|o| php::loose_eq(o, v)) == (name == "intersect"))
                    .map(|(k, v)| (k.clone(), v.clone())),
            )
        }
        "pad" => {
            let size = int_arg(args, 0, 0);
            let mut values: Vec<ViewValue> = array.values().cloned().collect();
            let missing = (size.unsigned_abs() as usize).saturating_sub(values.len());
            let padding = std::iter::repeat_n(arg(args, 1).clone(), missing);
            if size >= 0 {
                values.extend(padding);
            } else {
                values.splice(0..0, padding);
            }
            list(values)
        }
        "partition" => {
            let mut pass = ViewArray::new();
            let mut fail = ViewArray::new();
            for (k, item) in array.iter() {
                if call(a0, item, k)?.truthy() {
                    pass.insert(k.clone(), item.clone());
                } else {
                    fail.insert(k.clone(), item.clone());
                }
            }
            list([ViewValue::from(pass), ViewValue::from(fail)])
        }
        "reduce" => {
            let mut carry = arg(args, 1).clone();
            for (k, item) in array.iter() {
                carry = call_callable(a0, &[carry, item.clone(), k.to_value()], registry)?;
            }
            carry
        }
        "takeWhile" | "takeUntil" | "skipWhile" | "skipUntil" => {
            let mut out = ViewArray::new();
            let mut switched = false;
            for (k, item) in array.iter() {
                if !switched {
                    let result = match a0 {
                        ViewValue::Closure(_) => call(a0, item, k)?.truthy(),
                        value => php::loose_eq(item, value),
                    };
                    let stop = match name {
                        "takeWhile" | "skipWhile" => !result,
                        _ => result,
                    };
                    if stop {
                        switched = true;
                    }
                }
                let take = match name {
                    "takeWhile" | "takeUntil" => !switched,
                    _ => switched,
                };
                if take {
                    out.insert(k.clone(), item.clone());
                } else if name.starts_with("take") {
                    break;
                }
            }
            ViewValue::from(out)
        }
        "dot" => {
            let mut out = ViewArray::new();
            dot_into(array, "", &mut out);
            ViewValue::from(out)
        }
        "undot" => {
            let mut root = ViewValue::empty_array();
            for (k, v) in array.iter() {
                set_dotted(&mut root, &k.to_string(), v.clone());
            }
            root
        }
        "mapInto" => return Err(error("mapInto() is not supported in Blade templates")),
        "dump" => ViewValue::html(functions::dump_html(&ViewValue::Array(array.clone()))),
        "dd" => {
            let response = illuminate_http::Response::new(functions::dump_html(&ViewValue::Array(array.clone()))).with_status(500);
            return Err(illuminate_http::HttpResponseException::new(response).into());
        }
        _ => return Err(BadMethodCallException::new(format!("Method Collection::{name} does not exist.")).into()),
    })
}

fn key_values(value: &ViewValue) -> Vec<ViewValue> {
    match value {
        ViewValue::Array(keys) => keys.values().cloned().collect(),
        other => vec![other.clone()],
    }
}

fn flatten_into(array: &ViewArray, depth: usize, out: &mut Vec<ViewValue>) {
    for value in array.values() {
        match value {
            ViewValue::Array(inner) if depth > 0 => {
                if depth == 1 {
                    out.extend(inner.values().cloned());
                } else {
                    flatten_into(inner, depth - 1, out);
                }
            }
            other => out.push(other.clone()),
        }
    }
}

fn dot_into(array: &ViewArray, prefix: &str, out: &mut ViewArray) {
    for (key, value) in array.iter() {
        let name = format!("{prefix}{key}");
        match value {
            ViewValue::Array(inner) if !inner.is_empty() => dot_into(inner, &format!("{name}."), out),
            other => out.set(&name, other.clone()),
        }
    }
}

fn set_dotted(root: &mut ViewValue, key: &str, value: ViewValue) {
    let mut slot = root;
    let segments: Vec<&str> = key.split('.').collect();
    for (index, segment) in segments.iter().enumerate() {
        if !matches!(slot, ViewValue::Array(_)) {
            *slot = ViewValue::empty_array();
        }
        let ViewValue::Array(array) = slot else { return };
        let array = Arc::make_mut(array);
        let key = ArrayKey::new(segment);
        if index == segments.len() - 1 {
            array.insert(key, value);
            return;
        }
        if array.get(&key).is_none() {
            array.insert(key.clone(), ViewValue::empty_array());
        }
        slot = array.get_mut(&key).expect("the key was just inserted");
    }
}

// ----------------------------------------------------------------------
// Strings
// ----------------------------------------------------------------------

fn string_method(value: &Arc<str>, name: &str, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    if let Some(result) = objects::date_string_method(value, name, args) {
        return result;
    }
    let mut full_args = Vec::with_capacity(args.len() + 1);
    full_args.push(ViewValue::Str(value.clone()));
    full_args.extend_from_slice(args);
    match name {
        "toString" | "value" | "__toString" => Ok(ViewValue::Str(value.clone())),
        "toHtmlString" => Ok(ViewValue::html(value.to_string())),
        "isEmpty" => Ok(ViewValue::Bool(value.is_empty())),
        "isNotEmpty" => Ok(ViewValue::Bool(!value.is_empty())),
        "explode" | "split" => {
            let parts: Vec<ViewValue> = value.split(&*str_arg(args, 0)?).map(ViewValue::from).collect();
            Ok(ViewValue::list(parts))
        }
        "exactly" => Ok(ViewValue::Bool(**value == *str_arg(args, 0)?)),
        "test" => Ok(ViewValue::Bool(value.contains(&*str_arg(args, 0)?))),
        "toInteger" => Ok(ViewValue::Int(value.trim().parse().unwrap_or(0))),
        "toFloat" => Ok(ViewValue::Float(value.trim().parse().unwrap_or(0.0))),
        "toBoolean" => Ok(ViewValue::Bool(matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ))),
        "append" => {
            let mut out = value.to_string();
            for a in args {
                out.push_str(&php::to_str(a)?);
            }
            Ok(ViewValue::from(out))
        }
        "prepend" => {
            let mut out = String::new();
            for a in args {
                out.push_str(&php::to_str(a)?);
            }
            out.push_str(value);
            Ok(ViewValue::from(out))
        }
        _ => match crate::statics::str_method(name, &full_args, registry) {
            Some(result) => result,
            None => Err(error(format!("Call to a member function {name}() on string"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn call(target: ViewValue, name: &str, args: Vec<ViewValue>) -> ViewValue {
        let registry = Arc::new(Registry::default());
        call_method(&target, name, args, &registry).unwrap()
    }

    fn users() -> ViewValue {
        ViewValue::from(json!([
            {"name": "Taylor", "age": 40, "team": "core"},
            {"name": "Abigail", "age": 30, "team": "docs"},
            {"name": "James", "age": 35, "team": "core"},
        ]))
    }

    #[test]
    fn arrays_behave_like_collections() {
        assert_eq!(call(users(), "count", vec![]), ViewValue::Int(3));
        assert_eq!(call(users(), "pluck", vec!["name".into()]).to_json(), json!(["Taylor", "Abigail", "James"]));
        assert_eq!(call(users(), "sum", vec!["age".into()]), ViewValue::Int(105));
        assert_eq!(call(users(), "avg", vec!["age".into()]), ViewValue::Int(35));
        let sorted = call(users(), "sortBy", vec!["age".into()]);
        assert_eq!(call(sorted.clone(), "keys", vec![]).to_json(), json!([1, 2, 0]));
        assert_eq!(call(sorted, "pluck", vec!["name".into()]).to_json(), json!(["Abigail", "James", "Taylor"]));
        assert_eq!(call(users(), "where", vec!["team".into(), "core".into()]).count(), 2);
        assert_eq!(call(users(), "firstWhere", vec!["age".into(), ">".into(), 36.into()]).get("name"), Some("Taylor".into()));
        assert_eq!(call(users(), "groupBy", vec!["team".into()]).get("core").unwrap().count(), 2);
        assert_eq!(call(users(), "implode", vec!["name".into(), ", ".into()]), ViewValue::from("Taylor, Abigail, James"));
        assert_eq!(call(ViewValue::from(json!([1, 2, 3])), "take", vec![2.into()]).to_json(), json!([1, 2]));
        assert_eq!(call(ViewValue::from(json!(["a", "b", "c"])), "join", vec![", ".into(), " and ".into()]), ViewValue::from("a, b and c"));
        assert_eq!(call(ViewValue::from(json!([1, 2, 3])), "contains", vec![2.into()]), ViewValue::Bool(true));
        assert_eq!(call(ViewValue::from(json!({"a": 1})), "has", vec!["a".into()]), ViewValue::Bool(true));
    }

    #[test]
    fn serialized_models_answer_accessor_methods() {
        let paginator = ViewValue::from(json!({
            "current_page": 2, "data": [{"id": 1}], "from": 16, "last_page": 4,
            "per_page": 15, "path": "http://localhost/users", "to": 30, "total": 50,
            "next_page_url": "http://localhost/users?page=3", "prev_page_url": "http://localhost/users?page=1",
        }));
        assert_eq!(call(paginator.clone(), "total", vec![]), ViewValue::Int(50));
        assert_eq!(call(paginator.clone(), "currentPage", vec![]), ViewValue::Int(2));
        assert_eq!(call(paginator.clone(), "count", vec![]), ViewValue::Int(1));
        assert_eq!(call(paginator.clone(), "hasMorePages", vec![]), ViewValue::Bool(true));
        assert_eq!(call(paginator.clone(), "url", vec![3.into()]), ViewValue::from("http://localhost/users?page=3"));
        assert!(call(paginator, "links", vec![]).to_string_lossy().contains("rel=\"next\""));

        let user = ViewValue::from(json!({"first_name": "Taylor"}));
        assert_eq!(call(user, "firstName", vec![]), ViewValue::from("Taylor"));
    }

    #[test]
    fn strings_respond_to_str_and_date_methods() {
        assert_eq!(call("hello world".into(), "title", vec![]), ViewValue::from("Hello World"));
        assert_eq!(call("2024-03-12 10:00:00".into(), "format", vec!["M j".into()]), ViewValue::from("Mar 12"));
        assert_eq!(call("2024-03-12".into(), "year", vec![]), ViewValue::Int(2024));
    }
}
