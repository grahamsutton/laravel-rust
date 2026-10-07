//! Content matching shared by responses, views and components — Laravel's
//! `SeeInOrder` and `SeeInHtml` constraints.

/// Escape each value for HTML when asked to (`assertSee($value, $escape)`).
pub(crate) fn prepare(values: &[&str], escape: bool) -> Vec<String> {
    values
        .iter()
        .map(|value| {
            if escape {
                illuminate_support::e(value)
            } else {
                (*value).to_string()
            }
        })
        .collect()
}

/// PHP's `empty()` for strings: Laravel skips these when matching.
fn is_empty(value: &str) -> bool {
    value.is_empty() || value == "0"
}

/// Assert the values appear in order within the (entity-decoded) content.
pub(crate) fn see_in_order(content: &str, values: &[String]) -> Result<(), String> {
    let decoded = decode_html_entities(content);
    let mut position = 0;
    for value in values {
        if is_empty(value) {
            continue;
        }
        let needle = decode_html_entities(value);
        match decoded[position..].find(&needle) {
            Some(found) => position += found + needle.len(),
            None => {
                return Err(format!(
                    "Failed asserting that '{content}' contains \"{value}\" in specified order."
                ));
            }
        }
    }
    Ok(())
}

/// Assert the values appear (or, when negated, don't appear) in the text of
/// the content: tags stripped, entities decoded and whitespace collapsed.
pub(crate) fn see_in_html(
    content: &str,
    values: &[String],
    ordered: bool,
    negate: bool,
) -> Result<(), String> {
    let normalized = normalize(content);
    let mut position = 0;
    for value in values {
        if is_empty(value) {
            continue;
        }
        let needle = normalize(value);
        let found = normalized[position..]
            .find(&needle)
            .map(|found| found + position);

        if negate {
            if found.is_some() {
                return Err(format!(
                    "Failed asserting that '{content}' does not contain \"{value}\"."
                ));
            }
            continue;
        }

        match found {
            Some(found) => {
                if ordered {
                    position = found + needle.len();
                }
            }
            None => {
                return Err(format!(
                    "Failed asserting that '{content}' contains \"{value}\"{}",
                    if ordered { " in specified order." } else { "." }
                ));
            }
        }
    }
    Ok(())
}

/// Assert the content contains the value (PHPUnit's `assertStringContainsString`).
pub(crate) fn contains(content: &str, value: &str) -> Result<(), String> {
    if content.contains(value) {
        Ok(())
    } else {
        Err(format!(
            "Failed asserting that '{content}' contains \"{value}\"."
        ))
    }
}

/// Assert the content does not contain the value.
pub(crate) fn not_contains(content: &str, value: &str) -> Result<(), String> {
    if content.contains(value) {
        Err(format!(
            "Failed asserting that '{content}' does not contain \"{value}\"."
        ))
    } else {
        Ok(())
    }
}

/// Strip tags, decode entities, trim and collapse whitespace.
fn normalize(value: &str) -> String {
    let decoded = decode_html_entities(&strip_tags(value));
    let trimmed = decoded.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'));
    let mut out = String::with_capacity(trimmed.len());
    let mut in_whitespace = false;
    for c in trimmed.chars() {
        if c.is_ascii_whitespace() || c == '\x0B' {
            if !in_whitespace {
                out.push(' ');
            }
            in_whitespace = true;
        } else {
            out.push(c);
            in_whitespace = false;
        }
    }
    out
}

/// Strip HTML tags from the content.
pub(crate) fn strip_tags(html: &str) -> String {
    illuminate_support::Str::strip_tags(html)
}

/// Decode HTML entities, like PHP's `html_entity_decode($value, ENT_QUOTES, 'UTF-8')`.
pub(crate) fn decode_html_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(end) = tail[1..].find(';').map(|end| end + 1)
            && end <= 33
            && let Some(decoded) = decode_entity(&tail[1..end])
        {
            out.push_str(&decoded);
            rest = &tail[end + 1..];
            continue;
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    if let Some(number) = entity.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code).map(String::from);
    }
    let decoded = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "bull" => '•',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "hearts" => '♥',
        "check" => '✓',
        _ => return None,
    };
    Some(decoded.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            decode_html_entities("Tom &amp; Jerry &lt;3 &#039;cheese&#x27; &copy; &bogus; & more"),
            "Tom & Jerry <3 'cheese' © &bogus; & more"
        );
    }

    #[test]
    fn text_is_normalized() {
        assert_eq!(
            normalize("  <p>Hello\n   <b>World</b></p>  "),
            "Hello World"
        );
    }

    #[test]
    fn values_are_found_in_order() {
        let content = "<h1>Taylor</h1>\n<p>Abigail &amp; James</p>";
        assert!(see_in_order(content, &prepare(&["Taylor", "Abigail & James"], true)).is_ok());
        assert!(see_in_order(content, &prepare(&["James", "Taylor"], true)).is_err());
        assert!(see_in_html(content, &prepare(&["Taylor Abigail"], true), false, false).is_ok());
        assert!(see_in_html(content, &prepare(&["James", "Taylor"], true), true, false).is_err());
        assert!(see_in_html(content, &prepare(&["Dries"], true), false, true).is_ok());
    }
}
