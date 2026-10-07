//! Just enough TOML to customise a `Cargo.toml` line by line, keeping its
//! formatting and comments intact.

/// Quote a TOML basic string.
pub fn quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// Remove a trailing `# comment` (outside of strings).
pub fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut previous = '\0';
    for (index, character) in line.char_indices() {
        match character {
            '"' if previous != '\\' => in_string = !in_string,
            '#' if !in_string => return line[..index].trim_end(),
            _ => {}
        }
        previous = character;
    }
    line.trim_end()
}

/// Split on a separator that isn't inside a string, array, or table.
fn split_top_level(value: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut previous = '\0';

    for character in value.chars() {
        match character {
            '"' if previous != '\\' => in_string = !in_string,
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => depth -= 1,
            _ => {}
        }
        if character == separator && depth == 0 && !in_string {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(character);
        }
        previous = character;
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

/// How many more brackets / braces are opened than closed.
pub fn depth(value: &str) -> i32 {
    let mut depth = 0;
    let mut in_string = false;
    let mut previous = '\0';
    for character in strip_comment(value).chars() {
        match character {
            '"' if previous != '\\' => in_string = !in_string,
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => depth -= 1,
            _ => {}
        }
        previous = character;
    }
    depth
}

/// Split `key = value` (the value keeps its TOML spelling).
pub fn key_value(line: &str) -> Option<(String, String)> {
    let line = strip_comment(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim().trim_matches('"').to_string();
    let value = value.trim().to_string();
    (!key.is_empty() && !key.starts_with('[')).then_some((key, value))
}

/// Parse an inline table into its `(key, value)` pairs.
pub fn inline_table(value: &str) -> Option<Vec<(String, String)>> {
    let inner = value.trim().strip_prefix('{')?.strip_suffix('}')?;
    split_top_level(inner, ',')
        .iter()
        .map(|pair| key_value(pair))
        .collect()
}

/// Render pairs as an inline table: `{ version = "1", features = ["full"] }`.
pub fn render_inline_table(pairs: &[(String, String)]) -> String {
    let body: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{key} = {value}"))
        .collect();
    format!("{{ {} }}", body.join(", "))
}

/// Parse an array of strings: `["derive", "rc"]`.
pub fn string_array(value: &str) -> Vec<String> {
    value
        .trim()
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(|inner| {
            split_top_level(inner, ',')
                .into_iter()
                .map(|item| item.trim_matches('"').to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Render an array of strings.
pub fn render_string_array(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| quote(item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// The `key = value` entries of a table (values spanning lines are joined).
pub fn table_entries(manifest: &str, table: &str) -> Vec<(String, String)> {
    let header = format!("[{table}]");
    let mut entries = Vec::new();
    let mut inside = false;
    let mut pending = String::new();

    for line in manifest.lines() {
        let trimmed = line.trim();
        if pending.is_empty() && trimmed.starts_with('[') {
            inside = trimmed == header;
            continue;
        }
        if !inside || (pending.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#'))) {
            continue;
        }

        if !pending.is_empty() {
            pending.push(' ');
        }
        pending.push_str(strip_comment(trimmed));
        if depth(&pending) <= 0 {
            if let Some(entry) = key_value(&pending) {
                entries.push(entry);
            }
            pending.clear();
        }
    }

    entries
}

/// Every `[profile.*]` table, verbatim.
pub fn profile_tables(manifest: &str) -> String {
    let mut tables = Vec::new();
    let mut current: Option<Vec<&str>> = None;

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if let Some(table) = current.take() {
                tables.push(table);
            }
            if trimmed.starts_with("[profile.") {
                current = Some(vec![line]);
            }
            continue;
        }
        if let Some(table) = current.as_mut() {
            table.push(line);
        }
    }
    if let Some(table) = current {
        tables.push(table);
    }

    tables
        .into_iter()
        .map(|lines| lines.join("\n").trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_parsed() {
        assert_eq!(
            key_value("serde.workspace = true"),
            Some(("serde.workspace".into(), "true".into()))
        );
        assert_eq!(
            key_value(r#"name = "x" # comment"#),
            Some(("name".into(), r#""x""#.into()))
        );
        assert_eq!(key_value("[package]"), None);
        assert_eq!(
            inline_table(r#"{ version = "1", features = ["derive", "rc"] }"#),
            Some(vec![
                ("version".into(), r#""1""#.into()),
                ("features".into(), r#"["derive", "rc"]"#.into())
            ])
        );
        assert_eq!(inline_table(r#""1""#), None);
        assert_eq!(string_array(r#"["derive", "rc"]"#), ["derive", "rc"]);
        assert_eq!(quote(r#"C:\path "x""#), r#""C:\\path \"x\"""#);
    }

    #[test]
    fn tables_are_read() {
        let manifest = "[package]\nname = \"x\"\n\n[workspace.dependencies]\n# Comment\nserde = { version = \"1\", features = [\n  \"derive\",\n] }\ntokio = \"1\" # Async\n\n[profile.dev]\ndebug = 1\n\n[profile.dev.package.\"*\"]\nopt-level = 1\n";
        let entries = table_entries(manifest, "workspace.dependencies");
        assert_eq!(entries[0].0, "serde");
        assert_eq!(
            inline_table(&entries[0].1).unwrap()[1].1,
            r#"[ "derive", ]"#
        );
        assert_eq!(entries[1], ("tokio".into(), r#""1""#.into()));
        assert_eq!(
            profile_tables(manifest),
            "[profile.dev]\ndebug = 1\n\n[profile.dev.package.\"*\"]\nopt-level = 1"
        );
    }
}
