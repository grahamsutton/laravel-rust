//! Converting Redis replies into [`Value`]s.

use redis::FromRedisValue;

use illuminate_support::{Map, Value};

/// Convert a Redis reply into a [`Value`], the way PHP sees replies:
/// strings are strings, integers are numbers, `nil` is `null`, a plain
/// `OK` status is `true`, and arrays are arrays.
///
/// Binary strings that aren't valid UTF-8 are converted lossily; reach for
/// [`Connection::query`](crate::Connection::query) when you need the bytes.
pub fn from_redis(value: redis::Value) -> Value {
    match value {
        redis::Value::Nil => Value::Null,
        redis::Value::Int(number) => Value::from(number),
        redis::Value::BulkString(bytes) => match String::from_utf8(bytes) {
            Ok(string) => Value::String(string),
            Err(error) => Value::String(String::from_utf8_lossy(error.as_bytes()).into_owned()),
        },
        redis::Value::Okay => Value::Bool(true),
        redis::Value::SimpleString(string) => Value::String(string),
        redis::Value::Boolean(boolean) => Value::Bool(boolean),
        redis::Value::Double(number) => serde_json::Number::from_f64(number)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(number.to_string())),
        redis::Value::Array(items) | redis::Value::Set(items) => {
            Value::Array(items.into_iter().map(from_redis).collect())
        }
        redis::Value::Push { data, .. } => Value::Array(data.into_iter().map(from_redis).collect()),
        redis::Value::Map(pairs) => Value::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key_string(key), from_redis(value)))
                .collect::<Map<String, Value>>(),
        ),
        redis::Value::Attribute { data, .. } => from_redis(*data),
        redis::Value::VerbatimString { text, .. } => Value::String(text),
        other => String::from_redis_value(other)
            .map(Value::String)
            .unwrap_or(Value::Null),
    }
}

/// A reply used as an object key.
fn key_string(value: redis::Value) -> String {
    match from_redis(value) {
        Value::String(string) => string,
        other => other.to_string(),
    }
}

/// Split a reply into pairs: a RESP3 map, or a flat `[key, value, ...]` array.
pub(crate) fn pairs(value: redis::Value) -> Vec<(redis::Value, redis::Value)> {
    match value {
        redis::Value::Map(pairs) => pairs,
        redis::Value::Array(items) => {
            let mut items = items.into_iter();
            let mut pairs = Vec::new();
            while let (Some(key), Some(value)) = (items.next(), items.next()) {
                pairs.push((key, value));
            }
            pairs
        }
        _ => Vec::new(),
    }
}

/// Determine if a reply means "it worked": `OK`, a truthy integer, or `true`.
pub(crate) fn is_ok(value: &redis::Value) -> bool {
    match value {
        redis::Value::Okay | redis::Value::Boolean(true) => true,
        redis::Value::SimpleString(status) => status.eq_ignore_ascii_case("ok"),
        redis::Value::Int(number) => *number != 0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn replies_become_values() {
        assert_eq!(from_redis(redis::Value::Nil), Value::Null);
        assert_eq!(from_redis(redis::Value::Int(42)), json!(42));
        assert_eq!(
            from_redis(redis::Value::BulkString(b"Taylor".to_vec())),
            json!("Taylor")
        );
        assert_eq!(
            from_redis(redis::Value::BulkString(vec![b'a', 0xff])),
            json!("a\u{fffd}")
        );
        assert_eq!(from_redis(redis::Value::Okay), json!(true));
        assert_eq!(
            from_redis(redis::Value::SimpleString("PONG".into())),
            json!("PONG")
        );
        assert_eq!(from_redis(redis::Value::Boolean(false)), json!(false));
        assert_eq!(from_redis(redis::Value::Double(1.5)), json!(1.5));
        assert_eq!(
            from_redis(redis::Value::Double(f64::INFINITY)),
            json!("inf")
        );
        assert_eq!(
            from_redis(redis::Value::Array(vec![
                redis::Value::Int(1),
                redis::Value::Nil,
                redis::Value::Set(vec![redis::Value::Okay]),
            ])),
            json!([1, null, [true]])
        );
        assert_eq!(
            from_redis(redis::Value::Map(vec![(
                redis::Value::BulkString(b"name".to_vec()),
                redis::Value::Int(7)
            )])),
            json!({"name": 7})
        );
        assert_eq!(
            from_redis(redis::Value::Attribute {
                data: Box::new(redis::Value::Int(3)),
                attributes: Vec::new()
            }),
            json!(3)
        );
    }

    #[test]
    fn replies_split_into_pairs() {
        let flat = redis::Value::Array(vec![
            redis::Value::BulkString(b"a".to_vec()),
            redis::Value::BulkString(b"1".to_vec()),
            redis::Value::BulkString(b"b".to_vec()),
        ]);
        assert_eq!(pairs(flat).len(), 1);
        assert!(pairs(redis::Value::Nil).is_empty());
        let map = redis::Value::Map(vec![(redis::Value::Int(1), redis::Value::Int(2))]);
        assert_eq!(pairs(map).len(), 1);
    }

    #[test]
    fn ok_replies() {
        assert!(is_ok(&redis::Value::Okay));
        assert!(is_ok(&redis::Value::SimpleString("OK".into())));
        assert!(is_ok(&redis::Value::Int(1)));
        assert!(is_ok(&redis::Value::Boolean(true)));
        assert!(!is_ok(&redis::Value::Int(0)));
        assert!(!is_ok(&redis::Value::Nil));
    }
}
