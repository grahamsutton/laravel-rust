//! Static calls: `Str::limit($title, 20)`, `Number::currency($total)`,
//! `Js::from($data)`, `Carbon::parse($date)`...
//!
//! A static call first looks for a function registered under the name
//! `"Class::method"` (so the framework can provide `Route::has`,
//! `Vite::asset`, `Auth::user` and friends), then falls back to the support
//! classes built in here.

use std::sync::Arc;

use illuminate_support::{Carbon, Number, Result, Str};

use crate::exception::{BadMethodCallException, error};
use crate::expr::eval::{call_callable, call_function};
use crate::functions::{self, arg, bool_arg, int_arg, opt_str, str_arg, to_array};
use crate::objects::{DateObject, to_carbon};
use crate::php;
use crate::registry::Registry;
use crate::value::{ArrayKey, ViewArray, ViewValue};

/// Call a static method.
pub(crate) fn call_static(
    class: &str,
    method: &str,
    args: &[ViewValue],
    registry: &Arc<Registry>,
) -> Result<ViewValue> {
    let key = format!("{class}::{method}");
    if let Some(function) = registry.functions.get(&key) {
        return function(args);
    }
    let result = match class {
        "Str" | "Stringable" => str_static(method, args, registry),
        "Number" => number_static(method, args),
        "Arr" => arr_static(method, args, registry),
        "Js" => match method {
            "from" => Some(php::js_from(arg(args, 0), int_arg(args, 1, 0)).map(ViewValue::html)),
            "encode" => {
                Some(php::json_encode(arg(args, 0), int_arg(args, 1, 0)).map(ViewValue::from))
            }
            _ => None,
        },
        "Carbon" | "Date" | "CarbonImmutable" => carbon_static(method, args),
        "Collection" | "LazyCollection" => match method {
            "make" | "wrap" => Some(call_function("collect", args, registry)),
            "times" => {
                let count = int_arg(args, 0, 0).max(0);
                Some((|| {
                    let mut out = ViewArray::new();
                    for i in 1..=count {
                        out.push(match args.get(1) {
                            Some(callback) => {
                                call_callable(callback, &[ViewValue::Int(i)], registry)?
                            }
                            None => ViewValue::Int(i),
                        });
                    }
                    Ok(ViewValue::from(out))
                })())
            }
            "range" => Some(Ok(ViewValue::list(
                (int_arg(args, 0, 0)..=int_arg(args, 1, 0)).map(ViewValue::Int),
            ))),
            _ => None,
        },
        "Auth" => match method {
            "check" => Some(call_function("auth_check", args, registry)),
            "guest" => Some(
                call_function("auth_check", args, registry).map(|v| ViewValue::Bool(!v.truthy())),
            ),
            "user" | "id" => Some(match registry.functions.get("auth") {
                Some(auth) => auth(&[]).and_then(|guard| {
                    crate::methods::call_method(&guard, method, args.to_vec(), registry)
                }),
                None => Ok(ViewValue::Null),
            }),
            _ => None,
        },
        "Gate" => match method {
            "allows" | "check" => Some(call_function("gate_check", args, registry)),
            "denies" => Some(
                call_function("gate_check", args, registry).map(|v| ViewValue::Bool(!v.truthy())),
            ),
            "any" => Some(gate_any(args, registry)),
            _ => None,
        },
        "Config" => match method {
            "get" => Some(call_function("config", args, registry)),
            "has" => Some(
                call_function("config", &args[..1.min(args.len())], registry)
                    .map(|v| ViewValue::Bool(!v.is_null())),
            ),
            _ => None,
        },
        "Session" => match method {
            "get" => Some(call_function("session", args, registry)),
            "has" => Some(
                call_function("session", &args[..1.min(args.len())], registry)
                    .map(|v| ViewValue::Bool(!v.is_null())),
            ),
            _ => None,
        },
        "Lang" => match method {
            "get" => Some(call_function("__", args, registry)),
            "choice" => Some(call_function("trans_choice", args, registry)),
            _ => None,
        },
        "URL" => match method {
            "to" => Some(call_function("url", args, registry)),
            "route" => Some(call_function("route", args, registry)),
            "asset" => Some(call_function("asset", args, registry)),
            _ => None,
        },
        "App" => match method {
            "environment" => Some(environment(args, registry)),
            "isProduction" => Some(environment(&["production".into()], registry)),
            "isLocal" => Some(environment(&["local".into()], registry)),
            "getLocale" | "currentLocale" => Some(
                call_function("app_locale", args, registry).or_else(|_| Ok(ViewValue::from("en"))),
            ),
            _ => None,
        },
        _ => None,
    };
    match result {
        Some(result) => result,
        None if is_known_class(class) => Err(BadMethodCallException::new(format!(
            "Call to undefined method {class}::{method}()"
        ))
        .into()),
        None => Err(error(format!("Class \"{class}\" not found"))),
    }
}

fn is_known_class(class: &str) -> bool {
    matches!(
        class,
        "Str"
            | "Stringable"
            | "Number"
            | "Arr"
            | "Js"
            | "Carbon"
            | "Date"
            | "CarbonImmutable"
            | "Collection"
            | "LazyCollection"
            | "Auth"
            | "Gate"
            | "Config"
            | "Session"
            | "Lang"
            | "URL"
            | "App"
    )
}

/// Determine if the application is in one of the given environments.
pub(crate) fn environment(patterns: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    let current = php::to_str(&call_function("app_environment", &[], registry)?)?;
    if patterns.is_empty() {
        return Ok(ViewValue::from(current));
    }
    let mut candidates = Vec::new();
    for pattern in patterns {
        match pattern {
            ViewValue::Array(list) => {
                for item in list.values() {
                    candidates.push(php::to_str(item)?);
                }
            }
            other => candidates.push(php::to_str(other)?),
        }
    }
    Ok(ViewValue::Bool(
        candidates.iter().any(|p| Str::is(p, &current)),
    ))
}

/// `@canany` / `Gate::any`: check any of the abilities.
pub(crate) fn gate_any(args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    let abilities: Vec<ViewValue> = match arg(args, 0) {
        ViewValue::Array(list) => list.values().cloned().collect(),
        other => vec![other.clone()],
    };
    for ability in abilities {
        let mut call_args = vec![ability];
        call_args.extend_from_slice(&args[1.min(args.len())..]);
        if call_function("gate_check", &call_args, registry)?.truthy() {
            return Ok(ViewValue::Bool(true));
        }
    }
    Ok(ViewValue::Bool(false))
}

fn str_static(
    method: &str,
    args: &[ViewValue],
    registry: &Arc<Registry>,
) -> Option<Result<ViewValue>> {
    let s = |value: String| Some(Ok(ViewValue::from(value)));
    match method {
        "random" => return s(Str::random(int_arg(args, 0, 16).max(0) as usize)),
        "password" => return s(Str::password(int_arg(args, 0, 32).max(1) as usize)),
        "uuid" | "orderedUuid" => return s(Str::uuid().to_string()),
        "uuid7" => return s(Str::uuid7().to_string()),
        "ulid" => return s(Str::ulid().to_string()),
        "of" => return Some(Ok(ViewValue::from(str_arg(args, 0).unwrap_or_default()))),
        _ => {}
    }
    // Normalize Laravel's "subject last" signatures into "subject first".
    let reordered: Vec<ViewValue> = match method {
        "is" | "isMatch" | "remove" => {
            let mut v: Vec<ViewValue> = args.to_vec();
            if v.len() >= 2 {
                v.swap(0, 1);
            }
            v
        }
        "replace" | "replaceFirst" | "replaceLast" | "replaceArray" | "replaceStart"
        | "replaceEnd" => {
            let mut v: Vec<ViewValue> = args.to_vec();
            if v.len() >= 3 {
                let subject = v.remove(2);
                v.insert(0, subject);
            }
            v
        }
        _ => args.to_vec(),
    };
    str_method(method, &reordered, registry)
}

/// Stringable-style string methods: the subject is the first argument.
pub(crate) fn str_method(
    method: &str,
    args: &[ViewValue],
    registry: &Arc<Registry>,
) -> Option<Result<ViewValue>> {
    let result = (|| -> Result<Option<ViewValue>> {
        let subject = str_arg(args, 0)?;
        let a = |i: usize| str_arg(args, i);
        let s = |value: String| Ok(Some(ViewValue::from(value)));
        let b = |value: bool| Ok(Some(ViewValue::Bool(value)));
        let needles = |i: usize| -> Result<Vec<String>> {
            match arg(args, i) {
                ViewValue::Array(list) => list.values().map(php::to_str).collect(),
                other => Ok(vec![php::to_str(other)?]),
            }
        };
        match method {
            "after" => s(Str::after(&subject, &a(1)?)),
            "afterLast" => s(Str::after_last(&subject, &a(1)?)),
            "before" => s(Str::before(&subject, &a(1)?)),
            "beforeLast" => s(Str::before_last(&subject, &a(1)?)),
            "between" => s(Str::between(&subject, &a(1)?, &a(2)?)),
            "betweenFirst" => s(Str::between_first(&subject, &a(1)?, &a(2)?)),
            "camel" => s(Str::camel(&subject)),
            "studly" => s(Str::studly(&subject)),
            "pascal" => s(Str::pascal(&subject)),
            "snake" => s(match opt_str(args, 1)? {
                Some(delimiter) => Str::snake_with(&subject, &delimiter),
                None => Str::snake(&subject),
            }),
            "kebab" => s(Str::kebab(&subject)),
            "title" => s(Str::title(&subject)),
            "headline" => s(Str::headline(&subject)),
            "ucfirst" => s(Str::ucfirst(&subject)),
            "lcfirst" => s(Str::lcfirst(&subject)),
            "ucwords" => s(Str::ucwords(&subject)),
            "ucsplit" => Ok(Some(ViewValue::list(
                Str::ucsplit(&subject).into_iter().map(ViewValue::from),
            ))),
            "lower" => s(Str::lower(&subject)),
            "upper" => s(Str::upper(&subject)),
            "plural" => s(match args.get(1) {
                Some(count) if !count.is_null() => {
                    let count = match count {
                        ViewValue::Array(list) => list.len() as i64,
                        other => other.as_i64().unwrap_or(2),
                    };
                    Str::plural_count(&subject, count)
                }
                _ => Str::plural(&subject),
            }),
            "pluralStudly" => s(Str::plural_studly(&subject)),
            "singular" => s(Str::singular(&subject)),
            "slug" => s(match opt_str(args, 1)? {
                Some(separator) => Str::slug_with(&subject, &separator),
                None => Str::slug(&subject),
            }),
            "ascii" | "transliterate" => s(Str::ascii(&subject)),
            "contains" => {
                let ignore_case = bool_arg(args, 2, false);
                let needles = needles(1)?;
                b(needles.iter().any(|n| {
                    !n.is_empty()
                        && if ignore_case {
                            subject.to_lowercase().contains(&n.to_lowercase())
                        } else {
                            subject.contains(n.as_str())
                        }
                }))
            }
            "containsAll" => {
                let ignore_case = bool_arg(args, 2, false);
                b(needles(1)?.iter().all(|n| {
                    if ignore_case {
                        subject.to_lowercase().contains(&n.to_lowercase())
                    } else {
                        subject.contains(n.as_str())
                    }
                }))
            }
            "doesntContain" => b(!needles(1)?
                .iter()
                .any(|n| !n.is_empty() && subject.contains(n.as_str()))),
            "startsWith" => b(needles(1)?
                .iter()
                .any(|n| !n.is_empty() && subject.starts_with(n.as_str()))),
            "doesntStartWith" => b(!needles(1)?
                .iter()
                .any(|n| !n.is_empty() && subject.starts_with(n.as_str()))),
            "endsWith" => b(needles(1)?
                .iter()
                .any(|n| !n.is_empty() && subject.ends_with(n.as_str()))),
            "doesntEndWith" => b(!needles(1)?
                .iter()
                .any(|n| !n.is_empty() && subject.ends_with(n.as_str()))),
            "is" | "isMatch" => b(needles(1)?.iter().any(|pattern| Str::is(pattern, &subject))),
            "isJson" => b(Str::is_json(&subject)),
            "isUuid" => b(Str::is_uuid(&subject)),
            "isUlid" => b(Str::is_ulid(&subject)),
            "isAscii" => b(Str::is_ascii(&subject)),
            "isUrl" => b(Str::is_url(&subject)),
            "length" => Ok(Some(ViewValue::from(Str::length(&subject)))),
            "wordCount" => Ok(Some(ViewValue::from(Str::word_count(&subject)))),
            "limit" => s(match args.get(2) {
                Some(end) => Str::limit_with(
                    &subject,
                    int_arg(args, 1, 100).max(0) as usize,
                    &php::to_str(end)?,
                ),
                None => Str::limit(&subject, int_arg(args, 1, 100).max(0) as usize),
            }),
            "words" => s(match args.get(2) {
                Some(end) => Str::words_with(
                    &subject,
                    int_arg(args, 1, 100).max(0) as usize,
                    &php::to_str(end)?,
                ),
                None => Str::words(&subject, int_arg(args, 1, 100).max(0) as usize),
            }),
            "replace" => {
                let searches = needles(1)?;
                let replacements = needles(2)?;
                let mut out = subject.clone();
                for (index, search) in searches.iter().enumerate() {
                    if search.is_empty() {
                        continue;
                    }
                    let replacement = if replacements.len() == 1 {
                        replacements[0].clone()
                    } else {
                        replacements.get(index).cloned().unwrap_or_default()
                    };
                    out = out.replace(search.as_str(), &replacement);
                }
                s(out)
            }
            "replaceFirst" => s(Str::replace_first(&a(1)?, &a(2)?, &subject)),
            "replaceLast" => s(Str::replace_last(&a(1)?, &a(2)?, &subject)),
            "replaceStart" => {
                let search = a(1)?;
                s(if !search.is_empty() && subject.starts_with(&search) {
                    Str::replace_first(&search, &a(2)?, &subject)
                } else {
                    subject.clone()
                })
            }
            "replaceEnd" => {
                let search = a(1)?;
                s(if !search.is_empty() && subject.ends_with(&search) {
                    Str::replace_last(&search, &a(2)?, &subject)
                } else {
                    subject.clone()
                })
            }
            "replaceArray" => {
                let replacements = needles(2)?;
                let replacements: Vec<&str> = replacements.iter().map(String::as_str).collect();
                s(Str::replace_array(&a(1)?, &replacements, &subject))
            }
            "remove" => {
                let mut out = subject.clone();
                for search in needles(1)? {
                    out = Str::remove(&search, &out);
                }
                s(out)
            }
            "start" => s(Str::start(&subject, &a(1)?)),
            "finish" => s(Str::finish(&subject, &a(1)?)),
            "wrap" => {
                let before = a(1)?;
                let after = opt_str(args, 2)?.unwrap_or_else(|| before.clone());
                s(Str::wrap(&subject, &before, &after))
            }
            "unwrap" => {
                let before = a(1)?;
                let after = opt_str(args, 2)?.unwrap_or_else(|| before.clone());
                let mut out = subject.as_str();
                if out.starts_with(&before) {
                    out = &out[before.len()..];
                }
                if out.ends_with(&after) {
                    out = &out[..out.len() - after.len()];
                }
                s(out.to_string())
            }
            "squish" => s(Str::squish(&subject)),
            "trim" | "ltrim" | "rtrim" => {
                let chars = opt_str(args, 1)?;
                let matcher = |c: char| match &chars {
                    Some(chars) => chars.contains(c),
                    None => c.is_whitespace(),
                };
                s(match method {
                    "trim" => subject.trim_matches(matcher).to_string(),
                    "ltrim" => subject.trim_start_matches(matcher).to_string(),
                    _ => subject.trim_end_matches(matcher).to_string(),
                })
            }
            "substr" => {
                let length = match args.get(2) {
                    None | Some(ViewValue::Null) => None,
                    Some(v) => v.as_i64().map(|l| l as isize),
                };
                s(Str::substr(&subject, int_arg(args, 1, 0) as isize, length))
            }
            "take" => {
                let n = int_arg(args, 1, 0);
                s(if n < 0 {
                    Str::substr(&subject, n as isize, None)
                } else {
                    Str::substr(&subject, 0, Some(n as isize))
                })
            }
            "mask" => {
                let character = a(1)?.chars().next().unwrap_or('*');
                let length = match args.get(3) {
                    None | Some(ViewValue::Null) => None,
                    Some(v) => v.as_i64().map(|l| l.max(0) as usize),
                };
                s(Str::mask(
                    &subject,
                    character,
                    int_arg(args, 2, 0) as isize,
                    length,
                ))
            }
            "padBoth" | "padLeft" | "padRight" => {
                let length = int_arg(args, 1, 0).max(0) as usize;
                let pad = opt_str(args, 2)?.unwrap_or_else(|| " ".into());
                s(match method {
                    "padBoth" => Str::pad_both(&subject, length, &pad),
                    "padLeft" => Str::pad_left(&subject, length, &pad),
                    _ => Str::pad_right(&subject, length, &pad),
                })
            }
            "repeat" => s(Str::repeat(&subject, int_arg(args, 1, 1).max(0) as usize)),
            "reverse" => s(Str::reverse(&subject)),
            "position" => Ok(Some(
                subject
                    .find(&a(1)?)
                    .map(|byte| ViewValue::from(subject[..byte].chars().count()))
                    .unwrap_or(ViewValue::Bool(false)),
            )),
            "excerpt" => {
                let phrase = opt_str(args, 1)?.unwrap_or_default();
                let radius = 100usize;
                let lower = subject.to_lowercase();
                let Some(byte) = lower.find(&phrase.to_lowercase()) else {
                    return Ok(Some(ViewValue::Null));
                };
                let start_char = subject[..byte].chars().count();
                let chars: Vec<char> = subject.chars().collect();
                let from = start_char.saturating_sub(radius);
                let to = (start_char + phrase.chars().count() + radius).min(chars.len());
                let mut out: String = chars[from..to].iter().collect();
                if from > 0 {
                    out = format!("...{}", out.trim_start());
                }
                if to < chars.len() {
                    out = format!("{}...", out.trim_end());
                }
                s(out)
            }
            "apa" => s(Str::title(&subject)),
            "toHtmlString" => Ok(Some(ViewValue::html(subject))),
            "inlineMarkdown" | "markdown" => Ok(Some(ViewValue::html(simple_markdown(
                &subject,
                method == "inlineMarkdown",
            )))),
            "e" | "escape" => s(registry.escape(&subject)),
            _ => Ok(None),
        }
    })();
    result.transpose()
}

/// A tiny Markdown renderer covering emphasis, code and links.
fn simple_markdown(source: &str, inline: bool) -> String {
    let escaped = illuminate_support::e(source);
    let mut out = String::new();
    let mut rest = escaped.as_str();
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix("**")
            && let Some(end) = inner.find("**")
        {
            out.push_str(&format!("<strong>{}</strong>", &inner[..end]));
            rest = &inner[end + 2..];
            continue;
        }
        if let Some(inner) = rest.strip_prefix('*').or_else(|| rest.strip_prefix('_')) {
            let marker = &rest[..1];
            if let Some(end) = inner.find(marker) {
                out.push_str(&format!("<em>{}</em>", &inner[..end]));
                rest = &inner[end + 1..];
                continue;
            }
        }
        if let Some(inner) = rest.strip_prefix('`')
            && let Some(end) = inner.find('`')
        {
            out.push_str(&format!("<code>{}</code>", &inner[..end]));
            rest = &inner[end + 1..];
            continue;
        }
        if let Some(inner) = rest.strip_prefix('[')
            && let Some(close) = inner.find("](")
            && let Some(end) = inner[close + 2..].find(')')
        {
            let text = &inner[..close];
            let href = &inner[close + 2..close + 2 + end];
            out.push_str(&format!("<a href=\"{href}\">{text}</a>"));
            rest = &inner[close + 3 + end..];
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if inline {
        out
    } else {
        out.split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .map(|p| format!("<p>{}</p>\n", p.trim()))
            .collect()
    }
}

fn number_static(method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
    let number = functions::float_arg(args, 0);
    let precision =
        |index: usize, default: usize| int_arg(args, index, default as i64).max(0) as usize;
    let s = |value: String| Some(Ok(ViewValue::from(value)));
    match method {
        "format" => s(match args.get(1) {
            None | Some(ViewValue::Null) => match args.get(2) {
                Some(max) if !max.is_null() => {
                    let max = max.as_i64().unwrap_or(0).max(0) as usize;
                    let formatted = Number::format(number, Some(max));
                    if formatted.contains('.') {
                        formatted
                            .trim_end_matches('0')
                            .trim_end_matches('.')
                            .to_string()
                    } else {
                        formatted
                    }
                }
                _ => Number::format(number, None),
            },
            Some(p) => Number::format(number, Some(p.as_i64().unwrap_or(0).max(0) as usize)),
        }),
        "percentage" => s(Number::percentage(number, precision(1, 0))),
        "currency" => s(Number::currency(
            number,
            &opt_str(args, 1)
                .ok()
                .flatten()
                .unwrap_or_else(|| "USD".into()),
        )),
        "fileSize" => s(Number::file_size(number, precision(1, 0))),
        "abbreviate" => s(Number::abbreviate(number, precision(1, 0))),
        "forHumans" => s(Number::for_humans(number, precision(1, 0))),
        "ordinal" => s(Number::ordinal(number as i64)),
        "clamp" => Some(Ok(ViewValue::Float(Number::clamp(
            number,
            functions::float_arg(args, 1),
            functions::float_arg(args, 2),
        )))),
        _ => None,
    }
}

fn arr_static(
    method: &str,
    args: &[ViewValue],
    registry: &Arc<Registry>,
) -> Option<Result<ViewValue>> {
    let a0 = arg(args, 0);
    let result = (|| -> Result<Option<ViewValue>> {
        Ok(Some(match method {
            "toCssClasses" => ViewValue::from(php::css_classes(a0)),
            "toCssStyles" => ViewValue::from(php::css_styles(a0)),
            "wrap" => match a0 {
                ViewValue::Null => ViewValue::empty_array(),
                ViewValue::Array(_) => a0.clone(),
                other => ViewValue::list([other.clone()]),
            },
            "accessible" => ViewValue::Bool(matches!(a0, ViewValue::Array(_))),
            "isAssoc" => ViewValue::Bool(to_array(a0).is_some_and(|a| !a.is_list())),
            "isList" => ViewValue::Bool(to_array(a0).is_some_and(|a| a.is_list())),
            "get" => match functions::data_get(a0, &str_arg(args, 1)?) {
                Some(value) if !value.is_null() => value,
                _ => functions::value_of(arg(args, 2), &[], registry)?,
            },
            "has" | "exists" => {
                let keys = match arg(args, 1) {
                    ViewValue::Array(list) => {
                        list.values().map(php::to_str).collect::<Result<Vec<_>>>()?
                    }
                    other => vec![php::to_str(other)?],
                };
                ViewValue::Bool(
                    !keys.is_empty() && keys.iter().all(|k| functions::data_get(a0, k).is_some()),
                )
            }
            "first" | "last" => {
                let array = to_array(a0).unwrap_or_default();
                let found = match args.get(1) {
                    Some(callback @ ViewValue::Closure(_)) => {
                        let mut found = None;
                        let items: Vec<(&ArrayKey, &ViewValue)> = if method == "first" {
                            array.iter().collect()
                        } else {
                            array.iter().rev().collect()
                        };
                        for (key, value) in items {
                            if call_callable(callback, &[value.clone(), key.to_value()], registry)?
                                .truthy()
                            {
                                found = Some(value.clone());
                                break;
                            }
                        }
                        found
                    }
                    _ => {
                        if method == "first" {
                            array.first().cloned()
                        } else {
                            array.last().cloned()
                        }
                    }
                };
                match found {
                    Some(value) => value,
                    None => functions::value_of(arg(args, 2), &[], registry)?,
                }
            }
            "only" | "except" => {
                let array = to_array(a0).unwrap_or_default();
                let keys: Vec<ArrayKey> = match arg(args, 1) {
                    ViewValue::Array(list) => list
                        .values()
                        .map(ArrayKey::from_value)
                        .collect::<Result<_>>()?,
                    other => vec![ArrayKey::from_value(other)?],
                };
                ViewValue::from(
                    array
                        .iter()
                        .filter(|(k, _)| keys.contains(k) == (method == "only"))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<ViewArray>(),
                )
            }
            "pluck" => {
                let array = to_array(a0).unwrap_or_default();
                let key = str_arg(args, 1)?;
                let mut out = ViewArray::new();
                for item in array.values() {
                    let value = functions::data_get(item, &key).unwrap_or_default();
                    match opt_str(args, 2)? {
                        Some(index) => {
                            let index = functions::data_get(item, &index).unwrap_or_default();
                            out.insert(ArrayKey::from_value(&index)?, value);
                        }
                        None => out.push(value),
                    }
                }
                ViewValue::from(out)
            }
            "join" => {
                let array = to_array(a0).unwrap_or_default();
                let parts: Vec<String> = array.values().map(php::to_str).collect::<Result<_>>()?;
                let glue = str_arg(args, 1)?;
                match opt_str(args, 2)? {
                    Some(final_glue) if parts.len() > 1 => ViewValue::from(format!(
                        "{}{}{}",
                        parts[..parts.len() - 1].join(&glue),
                        final_glue,
                        parts[parts.len() - 1]
                    )),
                    _ => ViewValue::from(parts.join(&glue)),
                }
            }
            "query" => call_function("http_build_query", args, registry)?,
            "flatten" => crate::methods::call_method(
                a0,
                "flatten",
                args[1.min(args.len())..].to_vec(),
                registry,
            )?,
            "collapse" => crate::methods::call_method(a0, "collapse", Vec::new(), registry)?,
            "where" => crate::methods::call_method(
                a0,
                "filter",
                args[1.min(args.len())..].to_vec(),
                registry,
            )?,
            "whereNotNull" => {
                crate::methods::call_method(a0, "whereNotNull", Vec::new(), registry)?
            }
            "map" => crate::methods::call_method(
                a0,
                "map",
                args[1.min(args.len())..].to_vec(),
                registry,
            )?,
            "sort" => crate::methods::call_method(
                a0,
                "sort",
                args[1.min(args.len())..].to_vec(),
                registry,
            )?,
            "sortDesc" => crate::methods::call_method(a0, "sortDesc", Vec::new(), registry)?,
            "dot" => crate::methods::call_method(a0, "dot", Vec::new(), registry)?,
            "undot" => crate::methods::call_method(a0, "undot", Vec::new(), registry)?,
            "keyBy" => crate::methods::call_method(
                a0,
                "keyBy",
                args[1.min(args.len())..].to_vec(),
                registry,
            )?,
            _ => return Ok(None),
        }))
    })();
    result.transpose()
}

fn carbon_static(method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
    let date = |carbon: Carbon| Some(Ok(ViewValue::object(DateObject(carbon))));
    match method {
        "now" => date(match opt_str(args, 0) {
            Ok(Some(tz)) => match Carbon::now().tz(&tz) {
                Ok(d) => d,
                Err(e) => return Some(Err(e)),
            },
            _ => Carbon::now(),
        }),
        "today" => date(Carbon::today()),
        "tomorrow" => date(Carbon::tomorrow()),
        "yesterday" => date(Carbon::yesterday()),
        "parse" | "make" => match to_carbon(arg(args, 0)) {
            Some(carbon) => date(carbon),
            None => Some(Err(error(format!(
                "Could not parse '{}': Failed to parse time string",
                arg(args, 0).to_string_lossy()
            )))),
        },
        "createFromTimestamp" => date(Carbon::from_timestamp(int_arg(args, 0, 0))),
        "createFromFormat" => Some((|| {
            let carbon = Carbon::create_from_format(&str_arg(args, 0)?, &str_arg(args, 1)?)?;
            Ok(ViewValue::object(DateObject(carbon)))
        })()),
        "create" => {
            let carbon = Carbon::create(
                int_arg(args, 0, 1970) as i32,
                int_arg(args, 1, 1) as u32,
                int_arg(args, 2, 1) as u32,
                int_arg(args, 3, 0) as u32,
                int_arg(args, 4, 0) as u32,
                int_arg(args, 5, 0) as u32,
            );
            match carbon {
                Some(carbon) => date(carbon),
                None => Some(Err(error("Invalid date"))),
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn call(class: &str, method: &str, args: Vec<ViewValue>) -> ViewValue {
        let registry = Arc::new(Registry::default());
        call_static(class, method, &args, &registry).unwrap()
    }

    #[test]
    fn str_helpers_are_available() {
        assert_eq!(
            call("Str", "limit", vec!["The quick brown fox".into(), 9.into()]),
            ViewValue::from("The quick...")
        );
        assert_eq!(
            call("Str", "title", vec!["hello world".into()]),
            ViewValue::from("Hello World")
        );
        assert_eq!(
            call("Str", "plural", vec!["post".into()]),
            ViewValue::from("posts")
        );
        assert_eq!(
            call("Str", "plural", vec!["post".into(), 1.into()]),
            ViewValue::from("post")
        );
        assert_eq!(
            call("Str", "is", vec!["admin/*".into(), "admin/users".into()]),
            ViewValue::Bool(true)
        );
        assert_eq!(
            call(
                "Str",
                "replace",
                vec!["a".into(), "o".into(), "banana".into()]
            ),
            ViewValue::from("bonono")
        );
        assert_eq!(
            call("Str", "slug", vec!["Laravel Rocks".into()]),
            ViewValue::from("laravel-rocks")
        );
    }

    #[test]
    fn number_and_arr_helpers_are_available() {
        assert_eq!(
            call("Number", "format", vec![1234567.into()]),
            ViewValue::from("1,234,567")
        );
        assert_eq!(
            call("Number", "currency", vec![12.5.into()]),
            ViewValue::from("$12.50")
        );
        assert_eq!(
            call("Number", "ordinal", vec![3.into()]),
            ViewValue::from("3rd")
        );
        assert_eq!(
            call(
                "Arr",
                "toCssClasses",
                vec![ViewValue::from(
                    json!({"0": "p-4", "active": true, "hidden": false})
                )]
            ),
            ViewValue::from("p-4 active")
        );
        assert_eq!(
            call(
                "Arr",
                "get",
                vec![ViewValue::from(json!({"a": {"b": 1}})), "a.b".into()]
            ),
            ViewValue::Int(1)
        );
    }

    #[test]
    fn unknown_classes_are_reported() {
        let registry = Arc::new(Registry::default());
        let error = call_static("Podcast", "find", &[], &registry).unwrap_err();
        assert_eq!(error.to_string(), "Class \"Podcast\" not found");
    }
}
