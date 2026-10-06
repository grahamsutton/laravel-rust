//! Safely embed data in JavaScript.
//!
//! ```
//! use illuminate_support::{Js, json};
//!
//! assert_eq!(Js::from(&json!(["hello", "world"])).to_string(), r"JSON.parse('[\u0022hello\u0022,\u0022world\u0022]')");
//! assert_eq!(Js::from("Hello world").to_string(), "'Hello world'");
//! assert_eq!(Js::from(&true).to_string(), "true");
//! ```

use std::fmt;

use serde::Serialize;

use crate::html_string::HtmlString;
use crate::value::{Value, to_value};

/// A JavaScript expression that can be safely embedded in HTML.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Js {
    js: String,
}

impl Js {
    /// Convert the given data into a JavaScript expression.
    pub fn from<T: Serialize + ?Sized>(data: &T) -> Self {
        let value = to_value(data);
        Self {
            js: Self::expression(&value),
        }
    }

    /// Encode the data as JSON, escaping characters that are unsafe in HTML
    /// (`<`, `>`, `&`, `'`, `"`) like PHP's `JSON_HEX_*` flags.
    ///
    /// ```
    /// use illuminate_support::{Js, json};
    ///
    /// assert_eq!(Js::encode(&json!({"a": "<b>"})), r#"{"a":"\u003Cb\u003E"}"#);
    /// ```
    pub fn encode<T: Serialize + ?Sized>(data: &T) -> String {
        php_json_encode(&to_value(data))
    }

    fn expression(value: &Value) -> String {
        let json = php_json_encode(value);
        if value.is_string() {
            return format!("'{}'", &json[1..json.len() - 1]);
        }
        if json == "[]" || json == "{}" {
            return json;
        }
        if json.starts_with(['"', '{', '[']) {
            let encoded = php_json_encode(&Value::String(json));
            return format!("JSON.parse('{}')", &encoded[1..encoded.len() - 1]);
        }
        json
    }

    /// Get the JavaScript expression.
    pub fn to_html(&self) -> &str {
        &self.js
    }
}

impl fmt::Display for Js {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.js)
    }
}

impl From<Js> for HtmlString {
    fn from(js: Js) -> Self {
        HtmlString::new(js.js)
    }
}

impl From<Js> for String {
    fn from(js: Js) -> Self {
        js.js
    }
}

/// Encode a value like PHP's `json_encode` with `JSON_HEX_TAG | JSON_HEX_APOS
/// | JSON_HEX_AMP | JSON_HEX_QUOT | JSON_UNESCAPED_UNICODE` (forward slashes
/// are escaped, as PHP does by default).
pub fn php_json_encode(value: &Value) -> String {
    let mut out = String::new();
    write_json(value, &mut out);
    out
}

fn write_json(value: &Value, out: &mut String) {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => out.push_str(&value.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_json(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\u0022"),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '<' => out.push_str("\\u003C"),
            '>' => out.push_str("\\u003E"),
            '&' => out.push_str("\\u0026"),
            '\'' => out.push_str("\\u0027"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_converts_scalars() {
        assert_eq!(Js::from(&false).to_string(), "false");
        assert_eq!(Js::from(&true).to_string(), "true");
        assert_eq!(Js::from(&1).to_string(), "1");
        assert_eq!(Js::from(&1.1).to_string(), "1.1");
        assert_eq!(Js::from(&json!([])).to_string(), "[]");
        assert_eq!(Js::from(&crate::collect(Vec::<i32>::new())).to_string(), "[]");
        assert_eq!(Js::from(&json!(null)).to_string(), "null");
        assert_eq!(Js::from("Hello world").to_string(), "'Hello world'");
        assert_eq!(Js::from("Hèlló world").to_string(), "'Hèlló world'");
        assert_eq!(
            Js::from("<div class=\"foo\">'quoted html'</div>").to_string(),
            r"'\u003Cdiv class=\u0022foo\u0022\u003E\u0027quoted html\u0027\u003C\/div\u003E'"
        );
    }

    #[test]
    fn it_converts_arrays_and_objects() {
        assert_eq!(
            Js::from(&json!(["hello", "world"])).to_string(),
            r"JSON.parse('[\u0022hello\u0022,\u0022world\u0022]')"
        );
        assert_eq!(
            Js::from(&json!({"foo": "hello", "bar": "world"})).to_string(),
            r"JSON.parse('{\u0022foo\u0022:\u0022hello\u0022,\u0022bar\u0022:\u0022world\u0022}')"
        );

        #[derive(Serialize)]
        struct Data {
            foo: &'static str,
            bar: &'static str,
        }
        assert_eq!(
            Js::from(&Data { foo: "hello", bar: "world" }).to_string(),
            r"JSON.parse('{\u0022foo\u0022:\u0022hello\u0022,\u0022bar\u0022:\u0022world\u0022}')"
        );
        assert_eq!(Js::from(&json!({})).to_string(), "{}");
        assert_eq!(HtmlString::from(Js::from(&1)).to_html(), "1");
    }
}
