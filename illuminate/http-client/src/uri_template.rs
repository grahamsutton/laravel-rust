//! RFC 6570 URI templates, which power `with_url_parameters`.
//!
//! ```text
//! {+endpoint}/{page}/{version}/{topic}   →   https://laravel.com/docs/13.x/validation
//! ```

use illuminate_support::{Map, Value, ValueExt};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Everything except the RFC 3986 "unreserved" characters.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Everything except "unreserved" and "reserved" characters.
const RESERVED: &AsciiSet = &UNRESERVED
    .remove(b':')
    .remove(b'/')
    .remove(b'?')
    .remove(b'#')
    .remove(b'[')
    .remove(b']')
    .remove(b'@')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=')
    .remove(b'%');

struct Operator {
    first: &'static str,
    separator: &'static str,
    named: bool,
    if_empty: &'static str,
    reserved: bool,
}

fn operator(symbol: Option<char>) -> Operator {
    let (first, separator, named, if_empty, reserved) = match symbol {
        Some('+') => ("", ",", false, "", true),
        Some('#') => ("#", ",", false, "", true),
        Some('.') => (".", ".", false, "", false),
        Some('/') => ("/", "/", false, "", false),
        Some(';') => (";", ";", true, "", false),
        Some('?') => ("?", "&", true, "=", false),
        Some('&') => ("&", "&", true, "=", false),
        _ => ("", ",", false, "", false),
    };

    Operator {
        first,
        separator,
        named,
        if_empty,
        reserved,
    }
}

/// Expand the URI template with the given variables.
pub(crate) fn expand(template: &str, variables: &Map<String, Value>) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        output.push_str(&rest[..open]);
        let after = &rest[open + 1..];

        match after.find('}') {
            Some(close) => {
                output.push_str(&expand_expression(&after[..close], variables));
                rest = &after[close + 1..];
            }
            None => {
                output.push_str(&rest[open..]);
                rest = "";
            }
        }
    }

    output.push_str(rest);
    output
}

fn expand_expression(expression: &str, variables: &Map<String, Value>) -> String {
    let symbol = expression.chars().next().filter(|c| "+#./;?&".contains(*c));
    let op = operator(symbol);
    let list = match symbol {
        Some(symbol) => &expression[symbol.len_utf8()..],
        None => expression,
    };

    let mut output = String::new();
    let mut first = true;

    for spec in list.split(',') {
        let (name, explode, prefix) = parse_varspec(spec.trim());
        let Some(value) = variables.get(name).filter(|value| is_defined(value)) else {
            continue;
        };

        output.push_str(if first { op.first } else { op.separator });
        first = false;

        match value {
            Value::Array(items) => expand_list(&mut output, &op, name, items, explode),
            Value::Object(map) => expand_map(&mut output, &op, name, map, explode),
            scalar => {
                let mut text = scalar.to_string_lossy();
                if op.named {
                    output.push_str(name);
                    if text.is_empty() {
                        output.push_str(op.if_empty);
                        continue;
                    }
                    output.push('=');
                }
                if let Some(length) = prefix {
                    text = text.chars().take(length).collect();
                }
                output.push_str(&encode(&text, op.reserved));
            }
        }
    }

    output
}

fn parse_varspec(spec: &str) -> (&str, bool, Option<usize>) {
    if let Some(name) = spec.strip_suffix('*') {
        return (name, true, None);
    }
    match spec.split_once(':') {
        Some((name, length)) => (name, false, length.parse().ok()),
        None => (spec, false, None),
    }
}

fn is_defined(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        _ => true,
    }
}

fn expand_list(output: &mut String, op: &Operator, name: &str, items: &[Value], explode: bool) {
    let encoded = items
        .iter()
        .map(|item| encode(&item.to_string_lossy(), op.reserved));

    if !explode {
        if op.named {
            output.push_str(name);
            output.push('=');
        }
        output.push_str(&encoded.collect::<Vec<_>>().join(","));
        return;
    }

    let parts: Vec<String> = encoded
        .map(|item| match (op.named, item.is_empty()) {
            (true, true) => format!("{name}{}", op.if_empty),
            (true, false) => format!("{name}={item}"),
            (false, _) => item,
        })
        .collect();

    output.push_str(&parts.join(op.separator));
}

fn expand_map(
    output: &mut String,
    op: &Operator,
    name: &str,
    map: &Map<String, Value>,
    explode: bool,
) {
    let pairs = map.iter().map(|(key, value)| {
        (
            encode(key, op.reserved),
            encode(&value.to_string_lossy(), op.reserved),
        )
    });

    if !explode {
        if op.named {
            output.push_str(name);
            output.push('=');
        }
        let flat: Vec<String> = pairs.flat_map(|(key, value)| [key, value]).collect();
        output.push_str(&flat.join(","));
        return;
    }

    let parts: Vec<String> = pairs
        .map(|(key, value)| match (op.named, value.is_empty()) {
            (true, true) => format!("{key}{}", op.if_empty),
            _ => format!("{key}={value}"),
        })
        .collect();

    output.push_str(&parts.join(op.separator));
}

fn encode(value: &str, reserved: bool) -> String {
    let set = if reserved { RESERVED } else { UNRESERVED };
    utf8_percent_encode(value, set).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn vars() -> Map<String, Value> {
        json!({
            "var": "value",
            "hello": "Hello World!",
            "path": "/foo/bar",
            "empty": "",
            "list": ["red", "green", "blue"],
            "keys": {"semi": ";", "dot": ".", "comma": ","},
            "x": 1024,
            "y": 768,
            "undef": null,
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn it_expands_laravel_style_templates() {
        let variables = json!({
            "endpoint": "https://laravel.com",
            "page": "docs",
            "version": "13.x",
            "topic": "validation",
        });

        assert_eq!(
            expand(
                "{+endpoint}/{page}/{version}/{topic}",
                variables.as_object().unwrap()
            ),
            "https://laravel.com/docs/13.x/validation"
        );
    }

    #[test]
    fn it_follows_the_rfc_examples() {
        let v = vars();
        let cases = [
            ("{var}", "value"),
            ("{hello}", "Hello%20World%21"),
            ("{+hello}", "Hello%20World!"),
            ("{+path}/here", "/foo/bar/here"),
            ("here?ref={+path}", "here?ref=/foo/bar"),
            ("{#var}", "#value"),
            ("{#hello}", "#Hello%20World!"),
            ("map?{x,y}", "map?1024,768"),
            ("{x,hello,y}", "1024,Hello%20World%21,768"),
            ("{var:3}", "val"),
            ("{list}", "red,green,blue"),
            ("{list*}", "red,green,blue"),
            ("{keys}", "semi,%3B,dot,.,comma,%2C"),
            ("{keys*}", "semi=%3B,dot=.,comma=%2C"),
            ("{.list}", ".red,green,blue"),
            ("{.list*}", ".red.green.blue"),
            ("{/var,x}/here", "/value/1024/here"),
            ("{/list*}", "/red/green/blue"),
            ("{;x,y,empty}", ";x=1024;y=768;empty"),
            ("{;list*}", ";list=red;list=green;list=blue"),
            ("{?x,y,empty}", "?x=1024&y=768&empty="),
            ("{?list}", "?list=red,green,blue"),
            ("{?keys*}", "?semi=%3B&dot=.&comma=%2C"),
            ("?fixed=yes{&x}", "?fixed=yes&x=1024"),
            ("{undef}", ""),
            ("{?undef,var}", "?var=value"),
            ("unclosed {var", "unclosed {var"),
        ];

        for (template, expected) in cases {
            assert_eq!(expand(template, &v), expected, "{template}");
        }
    }
}
