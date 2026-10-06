//! Output formatting with Symfony-style tags.
//!
//! Console messages may contain style tags such as `<info>`, `<comment>`,
//! `<error>`, `<question>`, `<warning>` or inline styles like
//! `<fg=red;bg=white;options=bold>`. When the output is decorated the tags
//! become ANSI escape sequences; otherwise they are simply removed.
//!
//! ```
//! use illuminate_console::formatter::OutputFormatter;
//!
//! assert_eq!(OutputFormatter::format("<info>Done!</info>", false), "Done!");
//! assert_eq!(OutputFormatter::format("<info>Done!</info>", true), "\x1b[32mDone!\x1b[39m");
//!
//! // Unknown tags are left untouched...
//! assert_eq!(OutputFormatter::format("Usage: <user>", false), "Usage: <user>");
//! ```

/// A terminal style: the ANSI codes used to turn the style on and off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    set: Vec<String>,
    unset: Vec<String>,
}

impl Style {
    /// Create a style from a foreground color, background color and options.
    ///
    /// Returns `None` when a color or option is not recognized.
    pub fn new(
        foreground: Option<&str>,
        background: Option<&str>,
        options: &[&str],
    ) -> Option<Self> {
        let mut style = Style::default();

        if let Some(color) = foreground {
            let (set, unset) = color_codes(color, false)?;
            style.set.push(set);
            style.unset.push(unset);
        }

        if let Some(color) = background {
            let (set, unset) = color_codes(color, true)?;
            style.set.push(set);
            style.unset.push(unset);
        }

        for option in options {
            let (set, unset) = option_codes(option)?;
            style.set.push(set.to_string());
            style.unset.push(unset.to_string());
        }

        Some(style)
    }

    /// Resolve one of the named styles (`info`, `comment`, `question`,
    /// `error`, `warning`) or parse an inline style such as
    /// `fg=red;bg=white;options=bold`.
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();

        match spec.to_ascii_lowercase().as_str() {
            "info" => return Style::new(Some("green"), None, &[]),
            "comment" => return Style::new(Some("yellow"), None, &[]),
            "question" => return Style::new(Some("black"), Some("cyan"), &[]),
            "error" => return Style::new(Some("white"), Some("red"), &[]),
            "warning" => return Style::new(Some("yellow"), None, &[]),
            _ => {}
        }

        if !spec.contains('=') {
            return None;
        }

        let mut foreground = None;
        let mut background = None;
        let mut options: Vec<String> = Vec::new();

        for part in spec.split(';').filter(|p| !p.is_empty()) {
            let (key, value) = part.split_once('=')?;
            let value = value.trim().to_ascii_lowercase();

            match key.trim().to_ascii_lowercase().as_str() {
                "fg" => foreground = Some(value),
                "bg" => background = Some(value),
                "options" => options.extend(
                    value
                        .split(',')
                        .map(|o| o.trim().to_string())
                        .filter(|o| !o.is_empty()),
                ),
                // Hyperlinks are accepted but rendered as plain text.
                "href" => {}
                _ => return None,
            }
        }

        let options: Vec<&str> = options.iter().map(String::as_str).collect();

        Style::new(foreground.as_deref(), background.as_deref(), &options)
    }

    /// Wrap the given text in this style's escape sequences.
    pub fn apply(&self, text: &str) -> String {
        if self.set.is_empty() || text.is_empty() {
            return text.to_string();
        }

        format!(
            "\x1b[{}m{}\x1b[{}m",
            self.set.join(";"),
            text,
            self.unset.join(";")
        )
    }
}

fn color_codes(color: &str, background: bool) -> Option<(String, String)> {
    let color = color.trim().to_ascii_lowercase();
    let unset = if background { "49" } else { "39" }.to_string();

    const NAMES: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];

    if color == "default" {
        return Some((unset.clone(), unset));
    }

    if let Some(hex) = color.strip_prefix('#') {
        let hex = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => hex.to_string(),
            _ => return None,
        };
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        let (r, g, b) = (channel(0)?, channel(2)?, channel(4)?);
        let prefix = if background { 48 } else { 38 };
        return Some((format!("{prefix};2;{r};{g};{b}"), unset));
    }

    if let Some(index) = NAMES.iter().position(|name| *name == color) {
        let base = if background { 40 } else { 30 };
        return Some(((base + index).to_string(), unset));
    }

    let bright = match color.as_str() {
        "gray" | "grey" | "bright-black" => Some(0),
        other => other
            .strip_prefix("bright-")
            .and_then(|name| NAMES.iter().position(|n| *n == name)),
    };

    bright.map(|index| {
        let base = if background { 100 } else { 90 };
        ((base + index).to_string(), unset)
    })
}

fn option_codes(option: &str) -> Option<(&'static str, &'static str)> {
    match option.trim().to_ascii_lowercase().as_str() {
        "bold" => Some(("1", "22")),
        "underscore" => Some(("4", "24")),
        "blink" => Some(("5", "25")),
        "reverse" => Some(("7", "27")),
        "conceal" => Some(("8", "28")),
        _ => None,
    }
}

/// Formats console messages, turning style tags into ANSI escape sequences.
pub struct OutputFormatter;

impl OutputFormatter {
    /// Format the message. When `decorated` is false, style tags are removed.
    pub fn format(message: &str, decorated: bool) -> String {
        let mut output = String::with_capacity(message.len());
        let mut stack: Vec<Style> = Vec::new();
        let mut text = String::new();

        let flush = |text: &mut String, output: &mut String, stack: &[Style]| {
            if text.is_empty() {
                return;
            }
            match (decorated, stack.last()) {
                (true, Some(style)) => output.push_str(&style.apply(text)),
                _ => output.push_str(text),
            }
            text.clear();
        };

        let chars: Vec<char> = message.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];

            // An escaped tag ("\<info>") is printed literally.
            if c == '\\' && chars.get(i + 1) == Some(&'<') {
                text.push('<');
                i += 2;
                continue;
            }

            if c == '<'
                && let Some(end) = chars[i + 1..]
                    .iter()
                    .position(|ch| *ch == '>' || *ch == '<')
            {
                let end = i + 1 + end;

                if chars[end] == '>' {
                    let tag: String = chars[i + 1..end].iter().collect();

                    if let Some(closing) = tag.strip_prefix('/') {
                        if closing.is_empty()
                            || (is_tag_name(closing) && Style::parse(closing).is_some())
                        {
                            flush(&mut text, &mut output, &stack);
                            stack.pop();
                            i = end + 1;
                            continue;
                        }
                    } else if is_tag_name(&tag)
                        && let Some(style) = Style::parse(&tag)
                    {
                        flush(&mut text, &mut output, &stack);
                        stack.push(style);
                        i = end + 1;
                        continue;
                    }
                }
            }

            text.push(c);
            i += 1;
        }

        flush(&mut text, &mut output, &stack);

        output
    }

    /// Escape the given text so that it is never interpreted as style tags.
    pub fn escape(text: &str) -> String {
        text.replace('<', "\\<")
    }

    /// Remove every style tag and ANSI escape sequence from the message.
    pub fn strip(message: &str) -> String {
        strip_ansi(&Self::format(message, false))
    }

    /// The display width of the message, ignoring tags and escape sequences.
    pub fn width(message: &str) -> usize {
        Self::strip(message).chars().count()
    }
}

fn is_tag_name(tag: &str) -> bool {
    tag.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// Remove ANSI escape sequences from the given string.
pub fn strip_ansi(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        output.push(c);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_strips_known_tags_when_not_decorated() {
        assert_eq!(
            OutputFormatter::format("<info>a</info> <comment>b</comment>", false),
            "a b"
        );
        assert_eq!(
            OutputFormatter::format("<fg=red;options=bold>x</>", false),
            "x"
        );
        assert_eq!(OutputFormatter::format("<error>e</error>", false), "e");
    }

    #[test]
    fn it_leaves_unknown_tags_alone() {
        assert_eq!(
            OutputFormatter::format("mail:send <user>", false),
            "mail:send <user>"
        );
        assert_eq!(OutputFormatter::format("a < b > c", false), "a < b > c");
        assert_eq!(OutputFormatter::format("<>", false), "<>");
        assert_eq!(OutputFormatter::format("</>", false), "");
        assert_eq!(OutputFormatter::format("a</user>", false), "a</user>");
    }

    #[test]
    fn it_handles_escaped_tags() {
        assert_eq!(OutputFormatter::format("\\<info>x", false), "<info>x");
        assert_eq!(OutputFormatter::escape("<info>"), "\\<info>");
    }

    #[test]
    fn it_applies_ansi_codes() {
        assert_eq!(
            OutputFormatter::format("<comment>x</comment>", true),
            "\x1b[33mx\x1b[39m"
        );
        assert_eq!(
            OutputFormatter::format("<error>x</error>", true),
            "\x1b[37;41mx\x1b[39;49m"
        );
        assert_eq!(
            OutputFormatter::format("<fg=gray;options=bold>x</>", true),
            "\x1b[90;1mx\x1b[39;22m"
        );
        assert_eq!(
            OutputFormatter::format("<fg=#6C7280>x</>", true),
            "\x1b[38;2;108;114;128mx\x1b[39m"
        );
    }

    #[test]
    fn it_supports_nested_styles() {
        assert_eq!(
            OutputFormatter::format("<info>a<comment>b</comment>c</info>", true),
            "\x1b[32ma\x1b[39m\x1b[33mb\x1b[39m\x1b[32mc\x1b[39m"
        );
    }

    #[test]
    fn it_measures_width() {
        assert_eq!(OutputFormatter::width("<info>hello</info>"), 5);
        assert_eq!(OutputFormatter::width("\x1b[32mhi\x1b[39m"), 2);
        assert_eq!(OutputFormatter::width("⇂ x"), 3);
    }

    #[test]
    fn invalid_inline_styles_are_literal() {
        assert_eq!(
            OutputFormatter::format("<fg=nope>x</>", false),
            "<fg=nope>x"
        );
        assert_eq!(OutputFormatter::format("<foo=bar>x", false), "<foo=bar>x");
    }
}
