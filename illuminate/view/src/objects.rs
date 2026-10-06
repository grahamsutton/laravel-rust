//! Objects available inside templates: dates, error bags, the request, and
//! the null object behind `optional()`.

use std::sync::Arc;

use illuminate_http::Request;
use illuminate_support::{Carbon, MessageBag, Result, Str, Value};
use indexmap::IndexMap;

use crate::exception::error;
use crate::functions::{arg, int_arg, opt_str, str_arg};
use crate::php;
use crate::value::{ArrayKey, ViewArray, ViewObject, ViewValue};

// ----------------------------------------------------------------------
// Dates
// ----------------------------------------------------------------------

/// A [`Carbon`] date inside a template: `{{ $post->published_at->diffForHumans() }}`.
#[derive(Clone, Copy, Debug)]
pub struct DateObject(pub Carbon);

/// Interpret a value as a date (date objects, date strings and timestamps).
pub(crate) fn to_carbon(value: &ViewValue) -> Option<Carbon> {
    match value {
        ViewValue::Object(object) => object.downcast_ref::<DateObject>().map(|d| d.0),
        ViewValue::Str(s) => Carbon::parse(s).ok(),
        ViewValue::Int(ts) => Some(Carbon::from_timestamp(*ts)),
        ViewValue::Null => Some(Carbon::now()),
        _ => None,
    }
}

impl DateObject {
    fn other(args: &[ViewValue]) -> Result<Carbon> {
        match args.first() {
            None => Ok(Carbon::now()),
            Some(value) => to_carbon(value).ok_or_else(|| error("Could not parse the given date")),
        }
    }

    fn date(carbon: Carbon) -> ViewValue {
        ViewValue::object(DateObject(carbon))
    }

    fn property(&self, name: &str) -> Option<ViewValue> {
        let date = &self.0;
        Some(match name {
            "year" => ViewValue::from(date.year()),
            "month" => ViewValue::from(date.month()),
            "day" => ViewValue::from(date.day()),
            "hour" => ViewValue::from(date.hour()),
            "minute" => ViewValue::from(date.minute()),
            "second" => ViewValue::from(date.second()),
            "micro" => ViewValue::from(date.micro()),
            "dayOfWeek" => ViewValue::from(date.day_of_week()),
            "dayOfYear" => ViewValue::from(date.day_of_year()),
            "daysInMonth" => ViewValue::from(date.days_in_month()),
            "timestamp" => ViewValue::from(date.timestamp()),
            "timezoneName" | "tzName" => ViewValue::from(date.timezone_name()),
            "englishDayOfWeek" => ViewValue::from(date.format("l")),
            "shortEnglishDayOfWeek" => ViewValue::from(date.format("D")),
            "englishMonth" => ViewValue::from(date.format("F")),
            "shortEnglishMonth" => ViewValue::from(date.format("M")),
            _ => return None,
        })
    }

    fn method(&self, name: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        let date = self.0;
        let n = || int_arg(args, 0, 1);
        let value = match name {
            "format" | "translatedFormat" => match str_arg(args, 0) {
                Ok(format) => ViewValue::from(date.format(&format)),
                Err(e) => return Some(Err(e)),
            },
            "diffForHumans" => match args.first() {
                None | Some(ViewValue::Null) => ViewValue::from(date.diff_for_humans()),
                Some(_) => match Self::other(args) {
                    Ok(other) => ViewValue::from(date.diff_for_humans_from(&other)),
                    Err(e) => return Some(Err(e)),
                },
            },
            "toDateString" => ViewValue::from(date.to_date_string()),
            "toDateTimeString" | "__toString" => ViewValue::from(date.to_date_time_string()),
            "toTimeString" => ViewValue::from(date.to_time_string()),
            "toFormattedDateString" => ViewValue::from(date.to_formatted_date_string()),
            "toFormattedDayDateString" => ViewValue::from(date.format("D, M j, Y")),
            "toDayDateTimeString" => ViewValue::from(date.to_day_date_time_string()),
            "toIso8601String" | "toAtomString" | "toW3cString" | "toRfc3339String" => {
                ViewValue::from(date.to_iso8601_string())
            }
            "toISOString" | "toJSON" | "toJson" => ViewValue::from(date.to_json()),
            "toRfc2822String" | "toRfc822String" => ViewValue::from(date.to_rfc2822_string()),
            "toCookieString" => ViewValue::from(date.to_cookie_string()),
            "timestamp" | "getTimestamp" | "unix" => ViewValue::from(date.timestamp()),
            "isPast" => ViewValue::Bool(date.is_past()),
            "isFuture" => ViewValue::Bool(date.is_future()),
            "isToday" => ViewValue::Bool(date.is_today()),
            "isTomorrow" => ViewValue::Bool(date.is_tomorrow()),
            "isYesterday" => ViewValue::Bool(date.is_yesterday()),
            "isWeekend" => ViewValue::Bool(date.is_weekend()),
            "isWeekday" => ViewValue::Bool(date.is_weekday()),
            "isLeapYear" => ViewValue::Bool(date.is_leap_year()),
            "isSameDay" | "eq" | "equalTo" | "gt" | "greaterThan" | "isAfter" | "gte" | "greaterThanOrEqualTo"
            | "lt" | "lessThan" | "isBefore" | "lte" | "lessThanOrEqualTo" | "ne" | "notEqualTo" => {
                let other = match Self::other(args) {
                    Ok(other) => other,
                    Err(e) => return Some(Err(e)),
                };
                ViewValue::Bool(match name {
                    "isSameDay" => date.is_same_day(&other),
                    "eq" | "equalTo" => date.eq(&other),
                    "ne" | "notEqualTo" => !date.eq(&other),
                    "gt" | "greaterThan" | "isAfter" => date.gt(&other),
                    "gte" | "greaterThanOrEqualTo" => date.gte(&other),
                    "lt" | "lessThan" | "isBefore" => date.lt(&other),
                    _ => date.lte(&other),
                })
            }
            "between" | "isBetween" => match (to_carbon(arg(args, 0)), to_carbon(arg(args, 1))) {
                (Some(a), Some(b)) => ViewValue::Bool(date.between(&a, &b)),
                _ => return Some(Err(error("Could not parse the given dates"))),
            },
            "diffInSeconds" | "diffInMinutes" | "diffInHours" | "diffInDays" | "diffInWeeks" | "diffInMonths"
            | "diffInYears" => {
                let other = match Self::other(args) {
                    Ok(other) => other,
                    Err(e) => return Some(Err(e)),
                };
                ViewValue::from(match name {
                    "diffInSeconds" => date.diff_in_seconds(&other),
                    "diffInMinutes" => date.diff_in_minutes(&other),
                    "diffInHours" => date.diff_in_hours(&other),
                    "diffInDays" => date.diff_in_days(&other),
                    "diffInWeeks" => date.diff_in_weeks(&other),
                    "diffInMonths" => date.diff_in_months(&other),
                    _ => date.diff_in_years(&other),
                })
            }
            "addSecond" | "addSeconds" => Self::date(date.add_seconds(n())),
            "subSecond" | "subSeconds" => Self::date(date.sub_seconds(n())),
            "addMinute" | "addMinutes" => Self::date(date.add_minutes(n())),
            "subMinute" | "subMinutes" => Self::date(date.sub_minutes(n())),
            "addHour" | "addHours" => Self::date(date.add_hours(n())),
            "subHour" | "subHours" => Self::date(date.sub_hours(n())),
            "addDay" | "addDays" => Self::date(date.add_days(n())),
            "subDay" | "subDays" => Self::date(date.sub_days(n())),
            "addWeek" | "addWeeks" => Self::date(date.add_weeks(n())),
            "subWeek" | "subWeeks" => Self::date(date.sub_weeks(n())),
            "addMonth" | "addMonths" => Self::date(date.add_months(n())),
            "subMonth" | "subMonths" => Self::date(date.sub_months(n())),
            "addYear" | "addYears" => Self::date(date.add_years(n())),
            "subYear" | "subYears" => Self::date(date.sub_years(n())),
            "startOfDay" => Self::date(date.start_of_day()),
            "endOfDay" => Self::date(date.end_of_day()),
            "startOfWeek" => Self::date(date.start_of_week()),
            "endOfWeek" => Self::date(date.end_of_week()),
            "startOfMonth" => Self::date(date.start_of_month()),
            "endOfMonth" => Self::date(date.end_of_month()),
            "startOfYear" => Self::date(date.start_of_year()),
            "endOfYear" => Self::date(date.end_of_year()),
            "startOfHour" => Self::date(date.start_of_hour()),
            "tz" | "setTimezone" | "timezone" => match str_arg(args, 0) {
                Ok(tz) => match date.tz(&tz) {
                    Ok(converted) => Self::date(converted),
                    Err(e) => return Some(Err(e)),
                },
                Err(e) => return Some(Err(e)),
            },
            "utc" => Self::date(date.utc()),
            "copy" | "clone" | "toImmutable" | "toMutable" | "locale" => Self::date(date),
            "toArray" => ViewValue::map([
                ("year", ViewValue::from(date.year())),
                ("month", ViewValue::from(date.month())),
                ("day", ViewValue::from(date.day())),
                ("hour", ViewValue::from(date.hour())),
                ("minute", ViewValue::from(date.minute())),
                ("second", ViewValue::from(date.second())),
                ("timestamp", ViewValue::from(date.timestamp())),
                ("formatted", ViewValue::from(date.to_date_time_string())),
                ("timezone", ViewValue::from(date.timezone_name())),
            ]),
            other => return self.property(other).map(Ok),
        };
        Some(Ok(value))
    }
}

impl ViewObject for DateObject {
    fn class_name(&self) -> &str {
        "Carbon\\Carbon"
    }

    fn get(&self, property: &str) -> Option<ViewValue> {
        self.property(property)
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        self.method(method, args)
    }

    fn to_string_value(&self) -> Option<String> {
        Some(self.0.to_date_time_string())
    }

    fn to_json(&self) -> Value {
        Value::String(self.0.to_json())
    }
}

/// Call a date method on a string that holds a date.
pub(crate) fn date_string_method(value: &str, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
    if !looks_like_date(value) {
        return None;
    }
    let date = Carbon::parse(value).ok()?;
    DateObject(date).method(method, args)
}

/// Read a date property (`->year`) from a string that holds a date.
pub(crate) fn date_string_property(value: &str, property: &str) -> Option<ViewValue> {
    if !looks_like_date(value) {
        return None;
    }
    DateObject(Carbon::parse(value).ok()?).property(property)
}

fn looks_like_date(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 8 && value.as_bytes()[0].is_ascii_digit()
        || matches!(value.to_ascii_lowercase().as_str(), "now" | "today" | "tomorrow" | "yesterday")
        || value.len() > 10 && value.contains(',')
}

// ----------------------------------------------------------------------
// optional()
// ----------------------------------------------------------------------

/// The object `optional(null)` returns: every property and method is `null`.
#[derive(Clone, Copy, Debug, Default)]
pub struct OptionalObject;

impl ViewObject for OptionalObject {
    fn class_name(&self) -> &str {
        "Illuminate\\Support\\Optional"
    }

    fn get(&self, _property: &str) -> Option<ViewValue> {
        Some(ViewValue::Null)
    }

    fn call(&self, _method: &str, _args: &[ViewValue]) -> Option<Result<ViewValue>> {
        Some(Ok(ViewValue::Null))
    }

    fn offset_get(&self, _key: &ViewValue) -> Option<ViewValue> {
        Some(ViewValue::Null)
    }

    fn to_string_value(&self) -> Option<String> {
        Some(String::new())
    }

    fn truthy(&self) -> bool {
        false
    }

    fn to_json(&self) -> Value {
        Value::Null
    }
}

// ----------------------------------------------------------------------
// Error bags
// ----------------------------------------------------------------------

/// A [`MessageBag`] inside a template (`$errors->first('email')`).
#[derive(Clone, Debug, Default)]
pub struct MessageBagObject(pub Arc<MessageBag>);

fn format_message(format: Option<&str>, message: &str, key: &str) -> String {
    match format {
        Some(format) => format.replace(":message", message).replace(":key", key),
        None => message.to_string(),
    }
}

fn key_list(value: &ViewValue) -> Result<Vec<String>> {
    match value {
        ViewValue::Array(keys) => keys.values().map(php::to_str).collect(),
        other => Ok(vec![php::to_str(other)?]),
    }
}

impl MessageBagObject {
    fn messages_for(&self, key: &str, format: Option<&str>) -> ViewValue {
        let bag = &self.0;
        if key.contains('*') {
            let mut grouped = ViewArray::new();
            for (name, messages) in bag.messages() {
                if Str::is(key, name) {
                    grouped.set(
                        name,
                        ViewValue::list(messages.iter().map(|m| ViewValue::from(format_message(format, m, name)))),
                    );
                }
            }
            return ViewValue::from(grouped);
        }
        ViewValue::list(bag.get(key).into_iter().map(|m| ViewValue::from(format_message(format, m, key))))
    }

    fn all_messages(&self, format: Option<&str>) -> ViewValue {
        let mut list = Vec::new();
        for (key, messages) in self.0.messages() {
            for message in messages {
                list.push(ViewValue::from(format_message(format, message, key)));
            }
        }
        ViewValue::list(list)
    }

    fn messages_value(&self) -> ViewValue {
        ViewValue::map(self.0.messages().iter().map(|(key, messages)| {
            (ArrayKey::new(key), ViewValue::list(messages.iter().map(|m| ViewValue::from(m.as_str()))))
        }))
    }

    fn method(&self, name: &str, args: &[ViewValue]) -> Result<Option<ViewValue>> {
        let bag = &self.0;
        Ok(Some(match name {
            "has" => match args.first() {
                None | Some(ViewValue::Null) => ViewValue::Bool(bag.any()),
                Some(keys) => ViewValue::Bool(key_list(keys)?.iter().all(|k| bag.has(k))),
            },
            "hasAny" => {
                let keys: Vec<String> = if args.len() > 1 {
                    args.iter().map(php::to_str).collect::<Result<_>>()?
                } else {
                    key_list(arg(args, 0))?
                };
                ViewValue::Bool(keys.iter().any(|k| bag.has(k)))
            }
            "missing" => ViewValue::Bool(key_list(arg(args, 0))?.iter().all(|k| !bag.has(k))),
            "first" => {
                let format = opt_str(args, 1)?;
                match opt_str(args, 0)? {
                    Some(key) => {
                        let message = if key.contains('*') {
                            bag.messages()
                                .iter()
                                .filter(|(name, _)| Str::is(&key, name))
                                .flat_map(|(name, m)| m.first().map(|m| (name.clone(), m.clone())))
                                .next()
                        } else {
                            bag.first(&key).map(|m| (key.clone(), m.to_string()))
                        };
                        match message {
                            Some((name, message)) => ViewValue::from(format_message(format.as_deref(), &message, &name)),
                            None => ViewValue::from(""),
                        }
                    }
                    None => match bag.messages().iter().find_map(|(k, m)| m.first().map(|m| (k, m))) {
                        Some((key, message)) => ViewValue::from(format_message(format.as_deref(), message, key)),
                        None => ViewValue::from(""),
                    },
                }
            }
            "get" => self.messages_for(&str_arg(args, 0)?, opt_str(args, 1)?.as_deref()),
            "all" => self.all_messages(opt_str(args, 0)?.as_deref()),
            "any" | "isNotEmpty" => ViewValue::Bool(bag.any()),
            "isEmpty" => ViewValue::Bool(bag.is_empty()),
            "count" => ViewValue::from(bag.count()),
            "keys" => ViewValue::list(bag.keys().into_iter().map(ViewValue::from)),
            "messages" | "getMessages" | "toArray" | "jsonSerialize" => self.messages_value(),
            "toJson" => ViewValue::from(php::json_encode(&self.messages_value(), 0)?),
            "getMessageBag" | "getBag" => ViewValue::object(self.clone()),
            _ => return Ok(None),
        }))
    }
}

impl ViewObject for MessageBagObject {
    fn class_name(&self) -> &str {
        "Illuminate\\Support\\MessageBag"
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        self.method(method, args).transpose()
    }

    fn count(&self) -> Option<usize> {
        Some(self.0.count())
    }

    fn iterate(&self) -> Option<Vec<(ViewValue, ViewValue)>> {
        match self.messages_value() {
            ViewValue::Array(array) => Some(array.iter().map(|(k, v)| (k.to_value(), v.clone())).collect()),
            _ => None,
        }
    }

    fn to_string_value(&self) -> Option<String> {
        php::json_encode(&self.messages_value(), 0).ok()
    }

    fn to_json(&self) -> Value {
        serde_json::to_value(&*self.0).unwrap_or(Value::Null)
    }
}

/// The `$errors` variable: named bags of validation errors. Methods called
/// on it are forwarded to the "default" bag, just like Laravel's
/// `ViewErrorBag`.
#[derive(Clone, Debug, Default)]
pub struct ViewErrorBag {
    bags: IndexMap<String, Arc<MessageBag>>,
}

impl ViewErrorBag {
    /// An empty error bag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a named bag.
    pub fn put(mut self, name: impl Into<String>, bag: MessageBag) -> Self {
        self.bags.insert(name.into(), Arc::new(bag));
        self
    }

    /// Get a named bag (empty if it doesn't exist).
    pub fn get_bag(&self, name: &str) -> Arc<MessageBag> {
        self.bags.get(name).cloned().unwrap_or_default()
    }

    /// Determine if a named bag exists.
    pub fn has_bag(&self, name: &str) -> bool {
        self.bags.contains_key(name)
    }

    /// Build an error bag from JSON-shaped data: either
    /// `{"bag": {"field": ["message"]}}` or a flat `{"field": ["message"]}`
    /// (which becomes the default bag).
    pub fn from_value(value: &ViewValue) -> Self {
        let mut errors = ViewErrorBag::new();
        let Some(array) = value.as_array() else { return errors };
        let mut default = MessageBag::new();
        for (key, item) in array.iter() {
            let key = key.to_string();
            match item {
                ViewValue::Array(inner) if !inner.is_list() => {
                    let mut bag = MessageBag::new();
                    for (field, messages) in inner.iter() {
                        add_messages(&mut bag, &field.to_string(), messages);
                    }
                    errors.bags.insert(key, Arc::new(bag));
                }
                messages => add_messages(&mut default, &key, messages),
            }
        }
        if !default.is_empty() || !errors.bags.contains_key("default") {
            if let Some(existing) = errors.bags.get("default") {
                let mut merged = (**existing).clone();
                merged.merge(&default);
                default = merged;
            }
            errors.bags.insert("default".into(), Arc::new(default));
        }
        errors
    }

    fn default_bag(&self) -> MessageBagObject {
        MessageBagObject(self.get_bag("default"))
    }
}

fn add_messages(bag: &mut MessageBag, field: &str, messages: &ViewValue) {
    match messages {
        ViewValue::Array(list) => {
            for message in list.values() {
                bag.add(field, message.to_string_lossy());
            }
        }
        ViewValue::Null => {}
        other => {
            bag.add(field, other.to_string_lossy());
        }
    }
}

impl ViewObject for ViewErrorBag {
    fn class_name(&self) -> &str {
        "Illuminate\\Support\\ViewErrorBag"
    }

    fn get(&self, property: &str) -> Option<ViewValue> {
        Some(ViewValue::object(MessageBagObject(self.get_bag(property))))
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        match method {
            "getBag" => Some(
                str_arg(args, 0).map(|name| ViewValue::object(MessageBagObject(self.get_bag(&name)))),
            ),
            "hasBag" => Some(Ok(ViewValue::Bool(
                self.has_bag(&opt_str(args, 0).ok().flatten().unwrap_or_else(|| "default".into())),
            ))),
            "getBags" => Some(Ok(ViewValue::map(
                self.bags
                    .iter()
                    .map(|(k, v)| (ArrayKey::new(k), ViewValue::object(MessageBagObject(v.clone())))),
            ))),
            other => self.default_bag().call(other, args),
        }
    }

    fn count(&self) -> Option<usize> {
        Some(self.get_bag("default").count())
    }

    fn to_string_value(&self) -> Option<String> {
        self.default_bag().to_string_value()
    }

    fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        for (name, bag) in &self.bags {
            map.insert(name.clone(), serde_json::to_value(&**bag).unwrap_or(Value::Null));
        }
        Value::Object(map)
    }
}

// ----------------------------------------------------------------------
// The request
// ----------------------------------------------------------------------

/// The current request inside a template: `request()->is('admin/*')`.
#[derive(Clone)]
pub struct RequestObject(pub Request);

fn patterns(args: &[ViewValue]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for value in args {
        out.extend(key_list(value)?);
    }
    Ok(out)
}

impl ViewObject for RequestObject {
    fn class_name(&self) -> &str {
        "Illuminate\\Http\\Request"
    }

    fn get(&self, property: &str) -> Option<ViewValue> {
        Some(ViewValue::from(self.0.input(property)))
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        let request = &self.0;
        let result = (|| -> Result<ViewValue> {
            Ok(match method {
                "is" => ViewValue::Bool(patterns(args)?.iter().any(|p| request.is(p))),
                "routeIs" => ViewValue::Bool(patterns(args)?.iter().any(|p| request.route_is(p))),
                "fullUrlIs" => ViewValue::Bool(patterns(args)?.iter().any(|p| request.full_url_is(p))),
                "path" => ViewValue::from(request.path()),
                "url" => ViewValue::from(request.url()),
                "fullUrl" => ViewValue::from(request.full_url()),
                "root" => ViewValue::from(request.root()),
                "method" => ViewValue::from(request.method().as_str()),
                "isMethod" => ViewValue::Bool(request.is_method(&str_arg(args, 0)?)),
                "input" | "get" => match opt_str(args, 0)? {
                    Some(key) => {
                        let value = ViewValue::from(request.input(&key));
                        if value.is_null() { arg(args, 1).clone() } else { value }
                    }
                    None => ViewValue::from(request.all()),
                },
                "query" => match opt_str(args, 0)? {
                    Some(key) => {
                        let value = ViewValue::from(request.query(&key));
                        if value.is_null() { arg(args, 1).clone() } else { value }
                    }
                    None => ViewValue::from(request.query_all()),
                },
                "all" => ViewValue::from(request.all()),
                "only" => {
                    let keys = patterns(args)?;
                    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                    ViewValue::from(request.only(&keys))
                }
                "except" => {
                    let keys = patterns(args)?;
                    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                    ViewValue::from(request.except(&keys))
                }
                "has" => ViewValue::Bool(patterns(args)?.iter().all(|k| request.has(k))),
                "hasAny" => ViewValue::Bool(patterns(args)?.iter().any(|k| request.has(k))),
                "filled" => ViewValue::Bool(patterns(args)?.iter().all(|k| request.filled(k))),
                "missing" => ViewValue::Bool(patterns(args)?.iter().all(|k| request.missing(k))),
                "boolean" => ViewValue::Bool(request.boolean(&str_arg(args, 0)?)),
                "integer" => ViewValue::from(request.integer(&str_arg(args, 0)?)),
                "string" => ViewValue::from(request.string(&str_arg(args, 0)?)),
                "header" => match opt_str(args, 0)? {
                    Some(name) => request
                        .header(&name)
                        .map(ViewValue::from)
                        .unwrap_or_else(|| arg(args, 1).clone()),
                    None => ViewValue::Null,
                },
                "hasHeader" => ViewValue::Bool(request.has_header(&str_arg(args, 0)?)),
                "ip" => ViewValue::from(request.ip()),
                "userAgent" => ViewValue::from(request.user_agent()),
                "ajax" => ViewValue::Bool(request.ajax()),
                "pjax" => ViewValue::Bool(request.pjax()),
                "secure" => ViewValue::Bool(request.secure()),
                "wantsJson" => ViewValue::Bool(request.wants_json()),
                "expectsJson" => ViewValue::Bool(request.expects_json()),
                "isJson" => ViewValue::Bool(request.is_json()),
                "segment" => ViewValue::from(request.segment(int_arg(args, 0, 1).max(1) as usize)),
                "segments" => ViewValue::list(request.segments().into_iter().map(ViewValue::from)),
                "route" => match opt_str(args, 0)? {
                    Some(name) => request.route(&name).map(ViewValue::from).unwrap_or_else(|| arg(args, 1).clone()),
                    None => ViewValue::from(request.route_name()),
                },
                "host" => ViewValue::from(request.host()),
                "getHost" => ViewValue::from(request.host()),
                "scheme" | "getScheme" => ViewValue::from(request.scheme()),
                _ => return Err(error(format!("Call to undefined method Illuminate\\Http\\Request::{method}()"))),
            })
        })();
        Some(result)
    }

    fn to_json(&self) -> Value {
        self.0.all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn error_bags_accept_nested_and_flat_shapes() {
        let nested = ViewErrorBag::from_value(&ViewValue::from(json!({
            "default": {"email": ["The email field is required."]},
            "login": {"password": ["Wrong password."]},
        })));
        assert!(nested.get_bag("default").has("email"));
        assert!(nested.get_bag("login").has("password"));

        let flat = ViewErrorBag::from_value(&ViewValue::from(json!({"email": ["Required."], "name": "Too short."})));
        assert_eq!(flat.get_bag("default").first("name"), Some("Too short."));
    }

    #[test]
    fn error_bags_forward_to_the_default_bag() {
        let errors = ViewErrorBag::from_value(&ViewValue::from(json!({"email": ["Required.", "Invalid."]})));
        let call = |method: &str, args: Vec<ViewValue>| errors.call(method, &args).unwrap().unwrap();
        assert_eq!(call("has", vec!["email".into()]), ViewValue::Bool(true));
        assert_eq!(call("first", vec!["email".into()]), ViewValue::from("Required."));
        assert_eq!(call("first", vec!["email".into(), "<li>:message</li>".into()]), ViewValue::from("<li>Required.</li>"));
        assert_eq!(call("count", vec![]), ViewValue::Int(2));
        assert_eq!(call("any", vec![]), ViewValue::Bool(true));
        assert_eq!(call("all", vec![]).to_json(), json!(["Required.", "Invalid."]));
    }

    #[test]
    fn dates_expose_carbon_methods() {
        let date = DateObject(Carbon::parse("2024-03-12 15:30:00").unwrap());
        assert_eq!(date.call("format", &["Y".into()]).unwrap().unwrap(), ViewValue::from("2024"));
        assert_eq!(date.get("month"), Some(ViewValue::Int(3)));
        assert_eq!(
            date.call("addDays", &[2.into()]).unwrap().unwrap().to_string_lossy(),
            "2024-03-14 15:30:00"
        );
        assert!(date_string_method("2024-03-12", "toFormattedDateString", &[]).is_some());
        assert!(date_string_method("hello", "format", &["Y".into()]).is_none());
    }
}
