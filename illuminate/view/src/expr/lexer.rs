//! Tokenizing PHP-flavored expressions.

use std::sync::Arc;

use super::CastType;

/// A piece of a double-quoted string.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InterpPart {
    /// Literal text.
    Lit(String),
    /// Embedded code (`$name`, `$user->name`, `{$expr}`), with its byte
    /// offset in the source.
    Code(String, usize),
}

/// An expression token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Var(Arc<str>),
    Ident(Arc<str>),
    Int(i64),
    Float(f64),
    Str(String),
    Interp(Vec<InterpPart>),
    Cast(CastType),
    Op(&'static str),
    Eof,
}

const OPERATORS: &[&str] = &[
    "...", "===", "!==", "<=>", "**=", "??=", "?->", "<<=", ">>=", "**", "==", "!=", "<>", "<=", ">=", "&&", "||",
    "??", "->", "=>", "::", "++", "--", "+=", "-=", "*=", "/=", ".=", "%=", "|=", "&=", "^=", "<<", ">>", "+",
    "-", "*", "/", "%", ".", "<", ">", "!", "?", ":", "=", "(", ")", "[", "]", "{", "}", ",", ";", "&", "|",
    "^", "~", "@", "\\",
];

/// A tokenizing error: the message and the byte offset.
pub(crate) type LexError = (String, usize);

/// Tokenize an expression source.
pub(crate) fn tokenize(src: &str) -> Result<Vec<(Tok, usize)>, LexError> {
    let mut lexer = Lexer { src, pos: 0, tokens: Vec::new() };
    lexer.run()?;
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    tokens: Vec<(Tok, usize)>,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || (c as u32) >= 0x80
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || (c as u32) >= 0x80
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn run(&mut self) -> Result<(), LexError> {
        loop {
            self.skip_whitespace_and_comments();
            let start = self.pos;
            let Some(c) = self.peek() else {
                self.tokens.push((Tok::Eof, self.pos));
                return Ok(());
            };
            let token = if c == '$' && self.peek_at(1).is_some_and(is_ident_start) {
                self.pos += 1;
                Tok::Var(self.ident().into())
            } else if is_ident_start(c) || (c == '\\' && self.peek_at(1).is_some_and(is_ident_start)) {
                self.qualified_ident()
            } else if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) {
                self.number()?
            } else if c == '\'' {
                Tok::Str(self.single_quoted()?)
            } else if c == '"' {
                self.double_quoted()?
            } else if c == '(' {
                match self.cast() {
                    Some(cast) => cast,
                    None => {
                        self.pos += 1;
                        Tok::Op("(")
                    }
                }
            } else {
                let op = OPERATORS
                    .iter()
                    .find(|op| self.rest().starts_with(**op))
                    .ok_or_else(|| (format!("syntax error, unexpected character \"{c}\""), self.pos))?;
                self.pos += op.len();
                Tok::Op(op)
            };
            self.tokens.push((token, start));
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_start();
            self.pos += rest.len() - trimmed.len();
            if trimmed.starts_with("//") || (trimmed.starts_with('#') && !trimmed.starts_with("#[")) {
                let end = trimmed.find('\n').unwrap_or(trimmed.len());
                self.pos += end;
            } else if trimmed.starts_with("/*") {
                let end = trimmed.find("*/").map(|i| i + 2).unwrap_or(trimmed.len());
                self.pos += end;
            } else {
                return;
            }
        }
    }

    fn ident(&mut self) -> &'a str {
        let rest = self.rest();
        let len = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
        self.pos += len;
        &rest[..len]
    }

    fn qualified_ident(&mut self) -> Tok {
        let start = self.pos;
        if self.peek() == Some('\\') {
            self.pos += 1;
        }
        loop {
            self.ident();
            if self.peek() == Some('\\') && self.peek_at(1).is_some_and(is_ident_start) {
                self.pos += 1;
            } else {
                break;
            }
        }
        Tok::Ident(self.src[start..self.pos].into())
    }

    fn number(&mut self) -> Result<Tok, LexError> {
        let start = self.pos;
        let rest = self.rest();
        let lower = rest.to_ascii_lowercase();
        for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
            if lower.starts_with(prefix) {
                let digits: String = rest[2..]
                    .chars()
                    .take_while(|c| c.is_digit(radix) || *c == '_')
                    .collect();
                self.pos += 2 + digits.len();
                let clean = digits.replace('_', "");
                return i64::from_str_radix(&clean, radix)
                    .map(Tok::Int)
                    .or_else(|_| u64::from_str_radix(&clean, radix).map(|v| Tok::Float(v as f64)))
                    .map_err(|_| ("Invalid numeric literal".to_string(), start));
            }
        }
        let bytes = rest.as_bytes();
        let mut i = 0;
        let mut is_float = false;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'.' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()) {
            is_float = true;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        } else if i < bytes.len() && bytes[i] == b'.' && !bytes.get(i + 1).is_some_and(|b| *b == b'.' || *b == b'=') {
            // "1." is a float in PHP, unless it is followed by concatenation.
            if !bytes.get(i + 1).is_some_and(|b| b.is_ascii_whitespace() || *b == b'$' || *b == b'\'' || *b == b'"') {
                is_float = true;
                i += 1;
            }
        }
        if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
            let mut j = i + 1;
            if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                j += 1;
            }
            if j < bytes.len() && bytes[j].is_ascii_digit() {
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                is_float = true;
                i = j;
            }
        }
        let text = rest[..i].replace('_', "");
        self.pos += i;
        if !is_float {
            if text.len() > 1 && text.starts_with('0') && text.chars().all(|c| c.is_digit(8)) {
                return i64::from_str_radix(&text[1..], 8)
                    .map(Tok::Int)
                    .map_err(|_| ("Invalid numeric literal".to_string(), start));
            }
            if let Ok(v) = text.parse::<i64>() {
                return Ok(Tok::Int(v));
            }
        }
        text.parse::<f64>()
            .map(Tok::Float)
            .map_err(|_| ("Invalid numeric literal".to_string(), start))
    }

    fn single_quoted(&mut self) -> Result<String, LexError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\'' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.clone().next() {
                    Some((_, next @ ('\'' | '\\'))) => {
                        out.push(next);
                        chars.next();
                    }
                    _ => out.push('\\'),
                },
                other => out.push(other),
            }
        }
        Err(("syntax error, unterminated string".to_string(), start))
    }

    fn double_quoted(&mut self) -> Result<Tok, LexError> {
        let start = self.pos;
        self.pos += 1;
        let body_start = self.pos;
        let src = self.src;
        let bytes = src.as_bytes();
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut literal = String::new();
        let mut i = body_start;
        loop {
            let Some(c) = src[i..].chars().next() else {
                return Err(("syntax error, unterminated string".to_string(), start));
            };
            match c {
                '"' => {
                    i += 1;
                    break;
                }
                '\\' => {
                    let next = src[i + 1..].chars().next();
                    match next {
                        Some('n') => literal.push('\n'),
                        Some('t') => literal.push('\t'),
                        Some('r') => literal.push('\r'),
                        Some('v') => literal.push('\x0B'),
                        Some('e') => literal.push('\x1B'),
                        Some('f') => literal.push('\x0C'),
                        Some('0'..='7') => {
                            let digits: String = src[i + 1..].chars().take(3).take_while(|c| c.is_digit(8)).collect();
                            let code = u32::from_str_radix(&digits, 8).unwrap_or(0) & 0xFF;
                            literal.push(char::from_u32(code).unwrap_or('\0'));
                            i += 1 + digits.len();
                            continue;
                        }
                        Some('x') => {
                            let digits: String =
                                src[i + 2..].chars().take(2).take_while(|c| c.is_ascii_hexdigit()).collect();
                            if digits.is_empty() {
                                literal.push_str("\\x");
                            } else {
                                let code = u32::from_str_radix(&digits, 16).unwrap_or(0);
                                literal.push(char::from_u32(code).unwrap_or('\0'));
                            }
                            i += 2 + digits.len();
                            continue;
                        }
                        Some('u') if src[i + 2..].starts_with('{') => {
                            let end = src[i + 3..].find('}').map(|e| i + 3 + e);
                            if let Some(end) = end {
                                let code = u32::from_str_radix(&src[i + 3..end], 16).unwrap_or(0xFFFD);
                                literal.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                                i = end + 1;
                                continue;
                            }
                            literal.push_str("\\u");
                        }
                        Some(ch @ ('\\' | '$' | '"')) => literal.push(ch),
                        Some(other) => {
                            literal.push('\\');
                            literal.push(other);
                        }
                        None => literal.push('\\'),
                    }
                    i += 1 + next.map(char::len_utf8).unwrap_or(0);
                }
                '$' if src[i + 1..].chars().next().is_some_and(is_ident_start) => {
                    let code_start = i;
                    let mut j = i + 1;
                    j += src[j..].find(|c: char| !is_ident_char(c)).unwrap_or(src.len() - j);
                    let mut code = src[code_start..j].to_string();
                    if src[j..].starts_with("->") && src[j + 2..].chars().next().is_some_and(is_ident_start) {
                        let prop_start = j + 2;
                        let prop_end = prop_start
                            + src[prop_start..].find(|c: char| !is_ident_char(c)).unwrap_or(src.len() - prop_start);
                        code.push_str(&src[j..prop_end]);
                        j = prop_end;
                    } else if src[j..].starts_with('[') {
                        if let Some(close) = src[j..].find(']') {
                            let key = &src[j + 1..j + close];
                            let key_code = if key.starts_with('$')
                                || key.parse::<i64>().is_ok()
                                || key.starts_with('\'')
                            {
                                key.to_string()
                            } else {
                                format!("'{}'", key.replace('\'', "\\'"))
                            };
                            code.push('[');
                            code.push_str(&key_code);
                            code.push(']');
                            j += close + 1;
                        }
                    }
                    if !literal.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                    }
                    parts.push(InterpPart::Code(code, code_start));
                    i = j;
                }
                '{' if src[i + 1..].starts_with('$') => {
                    let code_start = i + 1;
                    let end = matching_brace(src, i).ok_or(("syntax error, unterminated string".to_string(), i))?;
                    if !literal.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                    }
                    parts.push(InterpPart::Code(src[code_start..end].to_string(), code_start));
                    i = end + 1;
                }
                '$' if src[i + 1..].starts_with('{') => {
                    let end = matching_brace(src, i + 1).ok_or(("syntax error, unterminated string".to_string(), i))?;
                    let name = src[i + 2..end].trim();
                    if !literal.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                    }
                    let code = if name.chars().all(is_ident_char) { format!("${name}") } else { name.to_string() };
                    parts.push(InterpPart::Code(code, i + 2));
                    i = end + 1;
                }
                other => {
                    literal.push(other);
                    i += other.len_utf8();
                }
            }
            debug_assert!(i <= bytes.len());
        }
        self.pos = i;
        if parts.is_empty() {
            return Ok(Tok::Str(literal));
        }
        if !literal.is_empty() {
            parts.push(InterpPart::Lit(literal));
        }
        Ok(Tok::Interp(parts))
    }

    fn cast(&mut self) -> Option<Tok> {
        let rest = self.rest();
        let close = rest.find(')')?;
        let inner = rest[1..close].trim().to_ascii_lowercase();
        let ty = match inner.as_str() {
            "int" | "integer" => CastType::Int,
            "float" | "double" | "real" => CastType::Float,
            "string" => CastType::String,
            "bool" | "boolean" => CastType::Bool,
            "array" => CastType::Array,
            "object" => CastType::Object,
            _ => return None,
        };
        self.pos += close + 1;
        Some(Tok::Cast(ty))
    }
}

/// Find the `}` matching the `{` at `open`, skipping strings.
fn matching_brace(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, c) in src[open..].char_indices() {
        let index = open + offset;
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' => quote = Some('\''),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn it_tokenizes_variables_and_operators() {
        assert_eq!(
            toks("$user?->name ?? 'Guest'"),
            vec![
                Tok::Var("user".into()),
                Tok::Op("?->"),
                Tok::Ident("name".into()),
                Tok::Op("??"),
                Tok::Str("Guest".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn it_tokenizes_numbers() {
        assert_eq!(toks("42 1.5 .5 1e3 0x1F 1_000"), vec![
            Tok::Int(42),
            Tok::Float(1.5),
            Tok::Float(0.5),
            Tok::Float(1000.0),
            Tok::Int(31),
            Tok::Int(1000),
            Tok::Eof
        ]);
    }

    #[test]
    fn it_tokenizes_interpolated_strings() {
        assert_eq!(
            toks(r#""Hello $name, {$user->name}! \$5""#),
            vec![
                Tok::Interp(vec![
                    InterpPart::Lit("Hello ".into()),
                    InterpPart::Code("$name".into(), 7),
                    InterpPart::Lit(", ".into()),
                    InterpPart::Code("$user->name".into(), 15),
                    InterpPart::Lit("! $5".into()),
                ]),
                Tok::Eof
            ]
        );
        assert_eq!(toks(r#""plain\n""#), vec![Tok::Str("plain\n".into()), Tok::Eof]);
        assert_eq!(toks(r"'it\'s \n'"), vec![Tok::Str("it's \\n".into()), Tok::Eof]);
    }

    #[test]
    fn it_tokenizes_casts_and_namespaces() {
        assert_eq!(
            toks("(int) \\Illuminate\\Support\\Str::of"),
            vec![
                Tok::Cast(CastType::Int),
                Tok::Ident("\\Illuminate\\Support\\Str".into()),
                Tok::Op("::"),
                Tok::Ident("of".into()),
                Tok::Eof
            ]
        );
    }
}
