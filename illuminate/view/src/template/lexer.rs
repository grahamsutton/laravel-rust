//! Splitting a Blade template into text, echoes, directives and component
//! tags.
//!
//! The lexer mirrors the way Laravel compiles templates, including its
//! whitespace rules: a directive that Laravel compiles into a PHP block
//! swallows the single newline that directly follows it (PHP drops a
//! newline right after `?>`), while echoes keep theirs.

use crate::exception::ViewCompilationException;

/// A raw attribute on a component or slot tag.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawAttr {
    pub name: String,
    /// The value (without quotes), when one was given.
    pub value: Option<String>,
    pub line: usize,
}

/// A template token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Text(String),
    Echo {
        src: String,
        escape: bool,
        line: usize,
    },
    Directive {
        name: String,
        args: Option<String>,
        line: usize,
    },
    PhpBlock {
        src: String,
        line: usize,
    },
    ComponentOpen {
        name: String,
        attrs: Vec<RawAttr>,
        self_closing: bool,
        line: usize,
    },
    ComponentClose {
        name: String,
        line: usize,
    },
    SlotOpen {
        inline_name: Option<String>,
        attrs: Vec<RawAttr>,
        line: usize,
    },
    SlotClose {
        line: usize,
    },
}

/// What the lexer needs to know about directives.
pub(crate) trait Directives {
    /// Determine if `name` is a directive (unknown `@words` stay text).
    fn is_directive(&self, name: &str) -> bool;
}

type LResult<T> = Result<T, ViewCompilationException>;

/// Tokenize a template.
pub(crate) fn tokenize(src: &str, directives: &dyn Directives) -> LResult<Vec<Token>> {
    let mut lexer = Lexer {
        src,
        pos: 0,
        line: 1,
        tokens: Vec::new(),
        text: String::new(),
        eat_newline: false,
        directives,
    };
    lexer.run()?;
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    tokens: Vec<Token>,
    text: String,
    eat_newline: bool,
    directives: &'a dyn Directives,
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Directives Laravel compiles to something other than a trailing `?>`,
/// so they don't swallow the following newline.
fn keeps_newline(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "extends" | "extendsfirst" | "class" | "style"
    )
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn advance(&mut self, len: usize) -> &'a str {
        let slice = &self.src[self.pos..self.pos + len];
        self.line += slice.bytes().filter(|b| *b == b'\n').count();
        self.pos += len;
        slice
    }

    fn push_text(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            self.tokens
                .push(Token::Text(std::mem::take(&mut self.text)));
        }
    }

    fn emit(&mut self, token: Token, eats_newline: bool) {
        self.flush_text();
        self.tokens.push(token);
        self.eat_newline = eats_newline;
    }

    fn error(&self, message: impl Into<String>, line: usize) -> ViewCompilationException {
        ViewCompilationException::new(message, line)
    }

    fn run(&mut self) -> LResult<()> {
        while self.pos < self.src.len() {
            if std::mem::take(&mut self.eat_newline) {
                if self.rest().starts_with("\r\n") {
                    self.advance(2);
                    continue;
                }
                if self.rest().starts_with('\n') {
                    self.advance(1);
                    continue;
                }
            }
            let rest = self.rest();
            let bytes = rest.as_bytes();
            match bytes[0] {
                b'{' => self.curly()?,
                b'@' => self.at()?,
                b'<' => self.angle()?,
                _ => {
                    let len = rest.find(['{', '@', '<']).unwrap_or(rest.len());
                    let text = self.advance(len);
                    self.push_text(text);
                }
            }
        }
        self.flush_text();
        Ok(())
    }

    // ------------------------------------------------------------------
    // Echoes and comments
    // ------------------------------------------------------------------

    fn curly(&mut self) -> LResult<()> {
        let rest = self.rest();
        let line = self.line;
        if let Some(comment) = rest.strip_prefix("{{--") {
            match comment.find("--}}") {
                Some(end) => {
                    self.advance(4 + end + 4);
                }
                None => return Err(self.error("Unclosed Blade comment: expected \"--}}\"", line)),
            }
            return Ok(());
        }
        let (open, close, escape) = if rest.starts_with("{!!") {
            ("{!!", "!!}", false)
        } else if rest.starts_with("{{{") {
            ("{{{", "}}}", true)
        } else if rest.starts_with("{{") {
            ("{{", "}}", true)
        } else {
            let text = self.advance(1);
            self.push_text(text);
            return Ok(());
        };
        let body_start = open.len();
        let Some(end) = find_close(&rest[body_start..], close) else {
            return Err(self.error(format!("Unclosed echo: expected \"{close}\""), line));
        };
        let src = rest[body_start..body_start + end].to_string();
        self.advance(body_start + end + close.len());
        self.emit(Token::Echo { src, escape, line }, false);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Directives
    // ------------------------------------------------------------------

    fn at(&mut self) -> LResult<()> {
        let rest = self.rest();
        let line = self.line;
        // Escaped echoes: @{{ name }} → {{ name }}
        for (open, close) in [("@{{{", "}}}"), ("@{!!", "!!}"), ("@{{", "}}")] {
            if let Some(body) = rest.strip_prefix(open) {
                let end = body
                    .find(close)
                    .map(|e| open.len() + e + close.len())
                    .unwrap_or(rest.len());
                let literal = rest[1..end].to_string();
                self.advance(end);
                self.push_text(&literal);
                return Ok(());
            }
        }
        // Directives must not be glued to a word ("user@example.com").
        let previous_is_word = self.pos > 0 && is_word(self.src.as_bytes()[self.pos - 1]);
        if previous_is_word {
            self.advance(1);
            self.push_text("@");
            return Ok(());
        }
        if starts_with_word(rest, "@verbatim") {
            let after = &rest["@verbatim".len()..];
            return match after.find("@endverbatim") {
                Some(end) => {
                    let content = after[..end].to_string();
                    self.advance("@verbatim".len() + end + "@endverbatim".len());
                    self.push_text(&content);
                    Ok(())
                }
                None => Err(self.error(
                    "Unclosed @verbatim directive. Did you forget an @endverbatim?",
                    line,
                )),
            };
        }

        let bytes = rest.as_bytes();
        let mut end = 1;
        let escaped = bytes.get(1) == Some(&b'@');
        if escaped {
            end += 1;
        }
        let name_start = end;
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
        if end == name_start {
            self.advance(1);
            self.push_text("@");
            return Ok(());
        }
        if rest[end..].starts_with("::")
            && rest.as_bytes().get(end + 2).is_some_and(|b| is_word(*b))
        {
            end += 2;
            while end < bytes.len() && is_word(bytes[end]) {
                end += 1;
            }
        }
        let name = rest[name_start..end].to_string();

        if escaped {
            // @@if (...) → @if(...)
            self.advance(end);
            let mut literal = format!("@{name}");
            if let Some(args) = self.try_args(true)? {
                literal.push('(');
                literal.push_str(&args);
                literal.push(')');
            }
            self.push_text(&literal);
            return Ok(());
        }

        if name.eq_ignore_ascii_case("php") && self.directives.is_directive("php") {
            self.advance(end);
            if let Some(args) = self.try_args(false)? {
                self.emit(
                    Token::Directive {
                        name,
                        args: Some(args),
                        line,
                    },
                    true,
                );
                return Ok(());
            }
            let rest = self.rest();
            return match rest.find("@endphp") {
                Some(close) => {
                    let src = rest[..close].to_string();
                    self.advance(close + "@endphp".len());
                    self.emit(Token::PhpBlock { src, line }, true);
                    Ok(())
                }
                None => {
                    Err(self.error("Unclosed @php directive. Did you forget an @endphp?", line))
                }
            };
        }

        if !self.directives.is_directive(&name) {
            self.advance(end);
            self.push_text(&format!("@{name}"));
            return Ok(());
        }

        self.advance(end);
        let args = self.try_args(false)?;
        let eats = !keeps_newline(&name);
        self.emit(Token::Directive { name, args, line }, eats);
        Ok(())
    }

    /// Consume `[ \t]*( ... )` if present, returning the inner source.
    fn try_args(&mut self, lenient: bool) -> LResult<Option<String>> {
        let rest = self.rest();
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        if !rest[spaces..].starts_with('(') {
            return Ok(None);
        }
        let line = self.line;
        match balanced_parens(&rest[spaces..]) {
            Some(close) => {
                let inner = rest[spaces + 1..spaces + close].to_string();
                self.advance(spaces + close + 1);
                Ok(Some(inner))
            }
            None if lenient => Ok(None),
            None => Err(self.error("Unclosed parenthesis in directive arguments", line)),
        }
    }

    // ------------------------------------------------------------------
    // Component tags
    // ------------------------------------------------------------------

    fn angle(&mut self) -> LResult<()> {
        let rest = self.rest();
        let line = self.line;
        let lower = rest.get(..9).unwrap_or(rest);
        let is_slot_open = (lower.starts_with("<x-slot") || lower.starts_with("<x:slot"))
            && rest[7..].starts_with([':', ' ', '\t', '\n', '\r', '>']);
        let is_slot_close = (lower.starts_with("</x-slot") || lower.starts_with("</x:slot"))
            && rest[8..].starts_with([':', ' ', '\t', '\n', '\r', '>']);

        if is_slot_close && let Some(end) = rest.find('>') {
            self.advance(end + 1);
            self.push_text(" ");
            self.emit(Token::SlotClose { line }, true);
            return Ok(());
        }
        if is_slot_open && let Some((token, len)) = self.slot_tag(line) {
            self.advance(len);
            self.push_text(" ");
            self.emit(token, false);
            self.push_text(" ");
            return Ok(());
        }
        if rest.starts_with("</x-") || rest.starts_with("</x:") {
            let name_len = rest[4..]
                .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')))
                .unwrap_or(rest.len() - 4);
            let name = rest[4..4 + name_len].to_string();
            let after = &rest[4 + name_len..];
            let spaces = after.len() - after.trim_start().len();
            if after[spaces..].starts_with('>') {
                self.advance(4 + name_len + spaces + 1);
                self.emit(Token::ComponentClose { name, line }, true);
                return Ok(());
            }
        }
        if (rest.starts_with("<x-") || rest.starts_with("<x:"))
            && rest[3..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
            && let Some((token, len)) = self.component_tag(line)
        {
            self.advance(len);
            self.emit(token, true);
            return Ok(());
        }
        self.advance(1);
        self.push_text("<");
        Ok(())
    }

    fn component_tag(&self, line: usize) -> Option<(Token, usize)> {
        let rest = self.rest();
        let name_len = rest[3..]
            .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')))
            .unwrap_or(rest.len() - 3);
        let name = rest[3..3 + name_len]
            .trim_end_matches(['.', ':', '-'])
            .to_string();
        let consumed_name = 3 + name.len();
        let (attrs, end, self_closing) = parse_attributes(rest, consumed_name, line)?;
        Some((
            Token::ComponentOpen {
                name,
                attrs,
                self_closing,
                line,
            },
            end,
        ))
    }

    fn slot_tag(&self, line: usize) -> Option<(Token, usize)> {
        let rest = self.rest();
        let mut index = 7;
        let mut inline_name = None;
        if rest[index..].starts_with(':') {
            let start = index + 1;
            let len = rest[start..]
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(rest.len() - start);
            if len == 0 {
                return None;
            }
            inline_name = Some(rest[start..start + len].to_string());
            index = start + len;
        }
        let (attrs, end, self_closing) = parse_attributes(rest, index, line)?;
        if self_closing {
            return None;
        }
        Some((
            Token::SlotOpen {
                inline_name,
                attrs,
                line,
            },
            end,
        ))
    }
}

fn starts_with_word(rest: &str, word: &str) -> bool {
    rest.starts_with(word) && !rest.as_bytes().get(word.len()).is_some_and(|b| is_word(*b))
}

/// Parse attributes starting at `index`, up to `>` or `/>`.
/// Returns the attributes, the index after the tag, and whether it self-closes.
fn parse_attributes(
    src: &str,
    mut index: usize,
    line: usize,
) -> Option<(Vec<RawAttr>, usize, bool)> {
    let mut attrs = Vec::new();
    let mut line = line;
    loop {
        let rest = &src[index..];
        let trimmed = rest.trim_start();
        let skipped = rest.len() - trimmed.len();
        line += rest[..skipped].bytes().filter(|b| *b == b'\n').count();
        index += skipped;
        if trimmed.starts_with("/>") {
            return Some((attrs, index + 2, true));
        }
        if trimmed.starts_with('>') {
            return Some((attrs, index + 1, false));
        }
        if trimmed.is_empty() || (skipped == 0 && !attrs.is_empty()) {
            return None;
        }
        let attr_line = line;
        // @class(...) / @style(...)
        if trimmed.starts_with("@class(") || trimmed.starts_with("@style(") {
            let directive = &trimmed[1..6];
            let close = balanced_parens(&trimmed[6..])?;
            let inner = &trimmed[7..6 + close];
            let helper = if directive == "class" {
                "toCssClasses"
            } else {
                "toCssStyles"
            };
            attrs.push(RawAttr {
                name: format!(":{directive}"),
                value: Some(format!("\\Illuminate\\Support\\Arr::{helper}({inner})")),
                line: attr_line,
            });
            line += trimmed[..7 + close].bytes().filter(|b| *b == b'\n').count();
            index += 7 + close;
            continue;
        }
        // {{ $attributes->merge(...) }}
        if let Some(echo) = trimmed.strip_prefix("{{") {
            let end = find_close(echo, "}}")?;
            let inner = trimmed[2..2 + end].trim();
            if !inner.starts_with("$attributes") {
                return None;
            }
            attrs.push(RawAttr {
                name: ":attributes".into(),
                value: Some(inner.to_string()),
                line: attr_line,
            });
            line += trimmed[..4 + end].bytes().filter(|b| *b == b'\n').count();
            index += 2 + end + 2;
            continue;
        }
        // :$userId short syntax
        if let Some(after) = trimmed.strip_prefix(":$") {
            let len = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if len == 0 {
                return None;
            }
            let var = &after[..len];
            attrs.push(RawAttr {
                name: format!(":{var}"),
                value: Some(format!("${var}")),
                line: attr_line,
            });
            index += 2 + len;
            continue;
        }
        let name_len = trimmed
            .find(|c: char| {
                !(c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '@' | '%'))
            })
            .unwrap_or(trimmed.len());
        if name_len == 0 {
            return None;
        }
        let name = trimmed[..name_len].to_string();
        index += name_len;
        let after = &src[index..];
        if let Some(value_src) = after.strip_prefix('=') {
            let (value, len) = match value_src.chars().next()? {
                quote @ ('"' | '\'') => {
                    let close = value_src[1..].find(quote)?;
                    (value_src[1..1 + close].to_string(), close + 2)
                }
                _ => {
                    let len = value_src
                        .find(|c: char| {
                            c.is_whitespace() || matches!(c, '>' | '"' | '\'' | '=' | '<')
                        })
                        .unwrap_or(value_src.len());
                    let mut len = len;
                    if value_src[..len].ends_with('/') && value_src[len..].starts_with('>') {
                        len -= 1;
                    }
                    if len == 0 {
                        return None;
                    }
                    (value_src[..len].to_string(), len)
                }
            };
            line += value.bytes().filter(|b| *b == b'\n').count();
            attrs.push(RawAttr {
                name,
                value: Some(value),
                line: attr_line,
            });
            index += 1 + len;
        } else {
            attrs.push(RawAttr {
                name,
                value: None,
                line: attr_line,
            });
        }
    }
}

/// Find the matching `)` for the `(` at the start of `src`, skipping strings.
/// Returns the index of the closing parenthesis.
pub(crate) fn balanced_parens(src: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Find `close` in `src`, skipping over quoted strings. Falls back to the
/// first occurrence when quotes are unbalanced.
pub(crate) fn find_close(src: &str, close: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if src[i..].starts_with(close) {
            return Some(i);
        }
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                let mut j = i + 1;
                let mut closed = false;
                while j < bytes.len() {
                    if bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == quote {
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                if !closed {
                    return src.find(close);
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct All;

    impl Directives for All {
        fn is_directive(&self, name: &str) -> bool {
            !matches!(name, "media" | "example")
        }
    }

    fn lex(src: &str) -> Vec<Token> {
        tokenize(src, &All).unwrap()
    }

    fn text(s: &str) -> Token {
        Token::Text(s.into())
    }

    #[test]
    fn it_splits_text_echoes_and_comments() {
        assert_eq!(
            lex("Hello, {{ $name }}.{{-- secret --}}{!! $raw !!}"),
            vec![
                text("Hello, "),
                Token::Echo {
                    src: " $name ".into(),
                    escape: true,
                    line: 1
                },
                text("."),
                Token::Echo {
                    src: " $raw ".into(),
                    escape: false,
                    line: 1
                },
            ]
        );
    }

    #[test]
    fn directives_swallow_one_newline() {
        assert_eq!(
            lex("@if ($a)\nA\n@endif\n\nB"),
            vec![
                Token::Directive {
                    name: "if".into(),
                    args: Some("$a".into()),
                    line: 1
                },
                text("A\n"),
                Token::Directive {
                    name: "endif".into(),
                    args: None,
                    line: 3
                },
                text("\nB"),
            ]
        );
    }

    #[test]
    fn escapes_and_emails_stay_literal() {
        assert_eq!(
            lex("@{{ name }} @@if($x) user@example.com @media (x)"),
            vec![text("{{ name }} @if($x) user@example.com @media (x)")]
        );
    }

    #[test]
    fn verbatim_and_php_blocks() {
        assert_eq!(
            lex("@verbatim {{ x }} @endverbatim"),
            vec![text(" {{ x }} ")]
        );
        assert_eq!(
            lex("@php\n$a = 1;\n@endphp\nX"),
            vec![
                Token::PhpBlock {
                    src: "\n$a = 1;\n".into(),
                    line: 1
                },
                text("X")
            ]
        );
        assert_eq!(
            lex("@php($a = 1)\nX"),
            vec![
                Token::Directive {
                    name: "php".into(),
                    args: Some("$a = 1".into()),
                    line: 1
                },
                text("X")
            ]
        );
    }

    #[test]
    fn nested_parentheses_and_strings_in_arguments() {
        assert_eq!(
            lex("@if (count($a) > 0 && $b == ')')x"),
            vec![
                Token::Directive {
                    name: "if".into(),
                    args: Some("count($a) > 0 && $b == ')'".into()),
                    line: 1
                },
                text("x")
            ]
        );
    }

    #[test]
    fn component_tags() {
        let tokens = lex(
            "<x-alert type=\"error\" :message=\"$msg\" disabled class='a {{ $b }}'>\nHi</x-alert>\n<x-forms.input :$name/>",
        );
        assert_eq!(
            tokens[0],
            Token::ComponentOpen {
                name: "alert".into(),
                attrs: vec![
                    RawAttr {
                        name: "type".into(),
                        value: Some("error".into()),
                        line: 1
                    },
                    RawAttr {
                        name: ":message".into(),
                        value: Some("$msg".into()),
                        line: 1
                    },
                    RawAttr {
                        name: "disabled".into(),
                        value: None,
                        line: 1
                    },
                    RawAttr {
                        name: "class".into(),
                        value: Some("a {{ $b }}".into()),
                        line: 1
                    },
                ],
                self_closing: false,
                line: 1
            }
        );
        assert_eq!(tokens[1], text("Hi"));
        assert_eq!(
            tokens[2],
            Token::ComponentClose {
                name: "alert".into(),
                line: 2
            }
        );
        assert_eq!(
            tokens[3],
            Token::ComponentOpen {
                name: "forms.input".into(),
                attrs: vec![RawAttr {
                    name: ":name".into(),
                    value: Some("$name".into()),
                    line: 3
                }],
                self_closing: true,
                line: 3
            }
        );
    }

    #[test]
    fn slot_tags() {
        let tokens = lex("<x-slot:title class=\"bold\">T</x-slot>");
        assert_eq!(
            tokens,
            vec![
                text(" "),
                Token::SlotOpen {
                    inline_name: Some("title".into()),
                    attrs: vec![RawAttr {
                        name: "class".into(),
                        value: Some("bold".into()),
                        line: 1
                    }],
                    line: 1
                },
                text(" T "),
                Token::SlotClose { line: 1 },
            ]
        );
    }

    #[test]
    fn non_component_tags_are_text() {
        assert_eq!(lex("<xml><x-></div>"), vec![text("<xml><x-></div>")]);
    }
}
