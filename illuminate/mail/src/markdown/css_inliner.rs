//! Inline CSS into HTML, for email clients that ignore `<style>` blocks.
//!
//! Laravel uses `tijsverkoyen/css-to-inline-styles`; this is a faithful,
//! pragmatic port of its algorithm: rules (from the given stylesheet and
//! from `<style>` tags) are sorted by specificity, matched against every
//! element, and merged into each element's `style` attribute. Properties
//! already present in an element's own `style` attribute win, and
//! `!important` declarations beat ordinary ones.
//!
//! Supported selectors: type, universal, `.class`, `#id`, attribute
//! selectors (`[a]`, `[a=v]`, `~=`, `|=`, `^=`, `$=`, `*=`), the descendant,
//! child (`>`), adjacent (`+`) and general sibling (`~`) combinators, and the
//! `:not()`, `:first-child`, `:last-child`, `:only-child`, `:first-of-type`,
//! `:last-of-type`, `:only-of-type`, `:empty` and `:root` pseudo-classes.
//! Rules using anything else (`:hover`, pseudo-elements, ...) are skipped,
//! as are at-rules like `@media` — exactly as Laravel's inliner does.

use std::cmp::Ordering;

/// Inline the given CSS (and any CSS found in `<style>` tags) into the HTML.
///
/// ```
/// use illuminate_mail::markdown::inline_css;
///
/// let html = inline_css(r#"<p class="lead">Hi</p>"#, "p { color: red; } .lead { font-size: 16px; }");
///
/// assert_eq!(html, r#"<p class="lead" style="color: red; font-size: 16px;">Hi</p>"#);
/// ```
pub fn inline_css(html: &str, css: &str) -> String {
    let mut document = Document::parse(html);

    let mut stylesheet = String::new();
    for style in &document.styles {
        stylesheet.push_str(style);
        stylesheet.push('\n');
    }
    stylesheet.push_str(css);

    let mut rules = parse_rules(&stylesheet);
    rules.sort_by(|a, b| match a.specificity.cmp(&b.specificity) {
        Ordering::Equal => a.order.cmp(&b.order),
        other => other,
    });

    let mut computed: Vec<Vec<Property>> = vec![Vec::new(); document.elements.len()];
    for rule in &rules {
        for (element, properties) in computed.iter_mut().enumerate() {
            if rule.selector.matches(&document, element) {
                apply_properties(properties, &rule.properties, rule.specificity);
            }
        }
    }

    for (element, properties) in computed.into_iter().enumerate() {
        if properties.is_empty() {
            continue;
        }
        let inline = document.elements[element]
            .attribute("style")
            .map(|style| parse_declarations(&style))
            .unwrap_or_default();
        let mut declarations: Vec<String> = properties
            .iter()
            .filter(|p| !inline.iter().any(|(name, _)| *name == p.name))
            .map(|p| format!("{}: {};", p.name, p.value))
            .collect();
        declarations.extend(
            inline
                .iter()
                .map(|(name, value)| format!("{name}: {value};")),
        );
        document.elements[element].set_attribute("style", &declarations.join(" "));
    }

    document.to_html()
}

// ----------------------------------------------------------------------
// CSS
// ----------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Property {
    name: String,
    value: String,
    important: bool,
    specificity: Specificity,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Specificity(u32, u32, u32);

impl std::ops::Add for Specificity {
    type Output = Specificity;

    fn add(self, other: Specificity) -> Specificity {
        Specificity(self.0 + other.0, self.1 + other.1, self.2 + other.2)
    }
}

struct Rule {
    selector: Selector,
    properties: Vec<(String, String)>,
    specificity: Specificity,
    order: usize,
}

fn apply_properties(
    computed: &mut Vec<Property>,
    properties: &[(String, String)],
    specificity: Specificity,
) {
    for (name, value) in properties {
        let important = is_important(value);
        let property = Property {
            name: name.clone(),
            value: value.clone(),
            important,
            specificity,
        };
        match computed.iter().position(|p| p.name == *name) {
            Some(index) => {
                let existing = &computed[index];
                if existing.important && !important {
                    continue;
                }
                let overrule =
                    (!existing.important && important) || existing.specificity <= specificity;
                if overrule {
                    computed.remove(index);
                    computed.push(property);
                }
            }
            None => computed.push(property),
        }
    }
}

fn is_important(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower
        .strip_suffix("important")
        .is_some_and(|rest| rest.trim_end().ends_with('!'))
}

/// Remove `/* comments */` from a stylesheet.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn parse_rules(css: &str) -> Vec<Rule> {
    let css = strip_comments(css);
    let bytes = css.as_bytes();
    let mut rules = Vec::new();
    let mut i = 0;
    let mut order = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'@' {
            // Skip at-rules: either `@import ...;` or `@media ... { ... }`.
            let mut depth = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b';' if depth == 0 => {
                        i += 1;
                        break;
                    }
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth <= 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        let Some(open) = css[i..].find('{').map(|o| o + i) else {
            break;
        };
        let Some(close) = css[open..].find('}').map(|c| c + open) else {
            break;
        };
        let selectors = &css[i..open];
        let declarations = parse_declarations(&css[open + 1..close]);
        i = close + 1;
        if declarations.is_empty() {
            continue;
        }
        for selector in split_top_level(selectors, ',') {
            let selector = selector.trim();
            if selector.is_empty() {
                continue;
            }
            if let Some(parsed) = Selector::parse(selector) {
                rules.push(Rule {
                    specificity: parsed.specificity(),
                    selector: parsed,
                    properties: declarations.clone(),
                    order,
                });
                order += 1;
            }
        }
    }
    rules
}

/// Parse `name: value; ...` declarations.
fn parse_declarations(block: &str) -> Vec<(String, String)> {
    split_top_level(block, ';')
        .into_iter()
        .filter_map(|declaration| {
            let (name, value) = declaration.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();
            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
            (!name.is_empty() && !value.is_empty()).then_some((name, value))
        })
        .collect()
}

/// Split on a separator, ignoring separators inside quotes and parentheses.
fn split_top_level(input: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in input.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                current.push(c);
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    current.push(c);
                }
                '(' | '[' => {
                    depth += 1;
                    current.push(c);
                }
                ')' | ']' => {
                    depth -= 1;
                    current.push(c);
                }
                c if c == separator && depth == 0 => parts.push(std::mem::take(&mut current)),
                _ => current.push(c),
            },
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

// ----------------------------------------------------------------------
// Selectors
// ----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
    Adjacent,
    Sibling,
}

#[derive(Clone, Debug)]
enum AttributeOp {
    Exists,
    Equals(String),
    Includes(String),
    DashMatch(String),
    Prefix(String),
    Suffix(String),
    Substring(String),
}

#[derive(Clone, Debug)]
enum Pseudo {
    Not(Box<Compound>),
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    Empty,
    Root,
}

#[derive(Clone, Debug, Default)]
struct Compound {
    tag: Option<String>,
    ids: Vec<String>,
    classes: Vec<String>,
    attributes: Vec<(String, AttributeOp)>,
    pseudos: Vec<Pseudo>,
}

impl Compound {
    fn specificity(&self) -> Specificity {
        let mut specificity = Specificity(
            self.ids.len() as u32,
            (self.classes.len() + self.attributes.len()) as u32,
            u32::from(self.tag.is_some()),
        );
        for pseudo in &self.pseudos {
            specificity = specificity
                + match pseudo {
                    Pseudo::Not(inner) => inner.specificity(),
                    _ => Specificity(0, 1, 0),
                };
        }
        specificity
    }

    fn matches(&self, document: &Document, index: usize) -> bool {
        let element = &document.elements[index];
        if let Some(tag) = &self.tag
            && *tag != element.name
        {
            return false;
        }
        if !self.ids.is_empty() {
            let id = element.attribute("id").unwrap_or_default();
            if self.ids.iter().any(|wanted| *wanted != id) {
                return false;
            }
        }
        if !self.classes.is_empty() {
            let classes = element.attribute("class").unwrap_or_default();
            let classes: Vec<&str> = classes.split_whitespace().collect();
            if !self.classes.iter().all(|c| classes.contains(&c.as_str())) {
                return false;
            }
        }
        for (name, op) in &self.attributes {
            let Some(value) = element.attribute(name) else {
                return false;
            };
            let matched = match op {
                AttributeOp::Exists => true,
                AttributeOp::Equals(v) => value == *v,
                AttributeOp::Includes(v) => value.split_whitespace().any(|part| part == v),
                AttributeOp::DashMatch(v) => value == *v || value.starts_with(&format!("{v}-")),
                AttributeOp::Prefix(v) => !v.is_empty() && value.starts_with(v.as_str()),
                AttributeOp::Suffix(v) => !v.is_empty() && value.ends_with(v.as_str()),
                AttributeOp::Substring(v) => !v.is_empty() && value.contains(v.as_str()),
            };
            if !matched {
                return false;
            }
        }
        self.pseudos.iter().all(|pseudo| match pseudo {
            Pseudo::Not(inner) => !inner.matches(document, index),
            Pseudo::FirstChild => document.siblings(index).first() == Some(&index),
            Pseudo::LastChild => document.siblings(index).last() == Some(&index),
            Pseudo::OnlyChild => document.siblings(index).len() == 1,
            Pseudo::FirstOfType => document.siblings_of_type(index).first() == Some(&index),
            Pseudo::LastOfType => document.siblings_of_type(index).last() == Some(&index),
            Pseudo::OnlyOfType => document.siblings_of_type(index).len() == 1,
            Pseudo::Empty => element.children.is_empty() && !element.has_text,
            Pseudo::Root => element.parent.is_none(),
        })
    }
}

#[derive(Clone, Debug)]
struct Selector {
    /// The compounds from left to right, each with the combinator that
    /// joins it to the previous compound.
    parts: Vec<(Combinator, Compound)>,
}

impl Selector {
    fn parse(input: &str) -> Option<Selector> {
        let mut parts = Vec::new();
        let mut chars = input.trim().chars().peekable();
        let mut combinator = Combinator::Descendant;
        loop {
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            let Some(&c) = chars.peek() else { break };
            match c {
                '>' | '+' | '~' => {
                    chars.next();
                    combinator = match c {
                        '>' => Combinator::Child,
                        '+' => Combinator::Adjacent,
                        _ => Combinator::Sibling,
                    };
                    continue;
                }
                _ => {}
            }
            let compound = parse_compound(&mut chars)?;
            parts.push((combinator, compound));
            combinator = Combinator::Descendant;
        }
        (!parts.is_empty()).then_some(Selector { parts })
    }

    fn specificity(&self) -> Specificity {
        self.parts
            .iter()
            .fold(Specificity::default(), |total, (_, compound)| {
                total + compound.specificity()
            })
    }

    fn matches(&self, document: &Document, element: usize) -> bool {
        self.matches_from(document, self.parts.len() - 1, element)
    }

    fn matches_from(&self, document: &Document, part: usize, element: usize) -> bool {
        let (combinator, compound) = &self.parts[part];
        if !compound.matches(document, element) {
            return false;
        }
        if part == 0 {
            return true;
        }
        match combinator {
            Combinator::Descendant => {
                let mut current = document.elements[element].parent;
                while let Some(ancestor) = current {
                    if self.matches_from(document, part - 1, ancestor) {
                        return true;
                    }
                    current = document.elements[ancestor].parent;
                }
                false
            }
            Combinator::Child => document.elements[element]
                .parent
                .is_some_and(|parent| self.matches_from(document, part - 1, parent)),
            Combinator::Adjacent => {
                let siblings = document.siblings(element);
                let position = siblings.iter().position(|s| *s == element).unwrap_or(0);
                position > 0 && self.matches_from(document, part - 1, siblings[position - 1])
            }
            Combinator::Sibling => {
                let siblings = document.siblings(element);
                siblings
                    .iter()
                    .take_while(|s| **s != element)
                    .any(|s| self.matches_from(document, part - 1, *s))
            }
        }
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii()
}

fn parse_ident(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut ident = String::new();
    while let Some(&c) = chars.peek() {
        if is_ident_char(c) {
            ident.push(c);
            chars.next();
        } else if c == '\\' {
            chars.next();
            if let Some(escaped) = chars.next() {
                ident.push(escaped);
            }
        } else {
            break;
        }
    }
    ident
}

fn parse_compound(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<Compound> {
    let mut compound = Compound::default();
    let mut any = false;
    while let Some(&c) = chars.peek() {
        match c {
            '*' => {
                chars.next();
            }
            '#' => {
                chars.next();
                compound.ids.push(parse_ident(chars));
            }
            '.' => {
                chars.next();
                compound.classes.push(parse_ident(chars));
            }
            '[' => {
                chars.next();
                let mut inner = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    inner.push(c);
                }
                compound.attributes.push(parse_attribute_selector(&inner)?);
            }
            ':' => {
                chars.next();
                if chars.peek() == Some(&':') {
                    // Pseudo-elements can't be inlined.
                    return None;
                }
                let name = parse_ident(chars).to_ascii_lowercase();
                let pseudo = match name.as_str() {
                    "not" => {
                        if chars.next() != Some('(') {
                            return None;
                        }
                        let mut depth = 1;
                        let mut inner = String::new();
                        for c in chars.by_ref() {
                            match c {
                                '(' => depth += 1,
                                ')' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            inner.push(c);
                        }
                        let mut inner_chars = inner.trim().chars().peekable();
                        let inner_compound = parse_compound(&mut inner_chars)?;
                        if inner_chars.next().is_some() {
                            return None;
                        }
                        Pseudo::Not(Box::new(inner_compound))
                    }
                    "first-child" => Pseudo::FirstChild,
                    "last-child" => Pseudo::LastChild,
                    "only-child" => Pseudo::OnlyChild,
                    "first-of-type" => Pseudo::FirstOfType,
                    "last-of-type" => Pseudo::LastOfType,
                    "only-of-type" => Pseudo::OnlyOfType,
                    "empty" => Pseudo::Empty,
                    "root" => Pseudo::Root,
                    _ => return None,
                };
                compound.pseudos.push(pseudo);
            }
            c if is_ident_char(c) => {
                compound.tag = Some(parse_ident(chars).to_ascii_lowercase());
            }
            c if c.is_whitespace() || matches!(c, '>' | '+' | '~') => break,
            _ => return None,
        }
        any = true;
    }
    any.then_some(compound)
}

fn parse_attribute_selector(inner: &str) -> Option<(String, AttributeOp)> {
    let inner = inner.trim();
    let unquote = |v: &str| v.trim().trim_matches(['"', '\'']).to_string();
    for (op, make) in [
        ("~=", AttributeOp::Includes as fn(String) -> AttributeOp),
        ("|=", AttributeOp::DashMatch),
        ("^=", AttributeOp::Prefix),
        ("$=", AttributeOp::Suffix),
        ("*=", AttributeOp::Substring),
        ("=", AttributeOp::Equals),
    ] {
        if let Some((name, value)) = inner.split_once(op) {
            return Some((name.trim().to_ascii_lowercase(), make(unquote(value))));
        }
    }
    (!inner.is_empty()).then(|| (inner.to_ascii_lowercase(), AttributeOp::Exists))
}

// ----------------------------------------------------------------------
// HTML
// ----------------------------------------------------------------------

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

enum Token {
    Raw(String),
    Tag(usize),
}

struct Element {
    name: String,
    attributes: Vec<(String, Option<String>)>,
    self_closing: bool,
    parent: Option<usize>,
    children: Vec<usize>,
    has_text: bool,
    dirty: bool,
    raw: String,
}

impl Element {
    fn attribute(&self, name: &str) -> Option<String> {
        self.attributes
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone().unwrap_or_default())
    }

    fn set_attribute(&mut self, name: &str, value: &str) {
        let value = value.replace('"', "&quot;");
        match self
            .attributes
            .iter_mut()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
        {
            Some((_, existing)) => *existing = Some(value),
            None => self.attributes.push((name.to_string(), Some(value))),
        }
        self.dirty = true;
    }

    fn to_html(&self) -> String {
        if !self.dirty {
            return self.raw.clone();
        }
        let mut out = format!("<{}", self.name);
        for (key, value) in &self.attributes {
            match value {
                Some(value) => out.push_str(&format!(" {key}=\"{value}\"")),
                None => out.push_str(&format!(" {key}")),
            }
        }
        if self.self_closing {
            out.push_str(" /");
        }
        out.push('>');
        out
    }
}

struct Document {
    tokens: Vec<Token>,
    elements: Vec<Element>,
    roots: Vec<usize>,
    styles: Vec<String>,
}

impl Document {
    fn parse(html: &str) -> Document {
        let mut document = Document {
            tokens: Vec::new(),
            elements: Vec::new(),
            roots: Vec::new(),
            styles: Vec::new(),
        };
        let mut stack: Vec<usize> = Vec::new();
        let bytes = html.as_bytes();
        let mut i = 0;
        let mut text_start = 0;

        let flush_text = |document: &mut Document, stack: &[usize], from: usize, to: usize| {
            if to > from {
                let text = &html[from..to];
                if let Some(&parent) = stack.last()
                    && !text.trim().is_empty()
                {
                    document.elements[parent].has_text = true;
                }
                document.tokens.push(Token::Raw(text.to_string()));
            }
        };

        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let rest = &html[i..];
            if rest.starts_with("<!--") {
                flush_text(&mut document, &stack, text_start, i);
                let end = rest.find("-->").map_or(html.len(), |e| i + e + 3);
                document.tokens.push(Token::Raw(html[i..end].to_string()));
                i = end;
                text_start = i;
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                flush_text(&mut document, &stack, text_start, i);
                let end = rest.find('>').map_or(html.len(), |e| i + e + 1);
                document.tokens.push(Token::Raw(html[i..end].to_string()));
                i = end;
                text_start = i;
                continue;
            }
            if let Some(name) = rest.strip_prefix("</") {
                let name_len = name
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                    .unwrap_or(name.len());
                if name_len == 0 {
                    i += 1;
                    continue;
                }
                flush_text(&mut document, &stack, text_start, i);
                let tag = name[..name_len].to_ascii_lowercase();
                let end = rest.find('>').map_or(html.len(), |e| i + e + 1);
                document.tokens.push(Token::Raw(html[i..end].to_string()));
                if let Some(position) = stack
                    .iter()
                    .rposition(|e| document.elements[*e].name == tag)
                {
                    stack.truncate(position);
                }
                i = end;
                text_start = i;
                continue;
            }
            let Some((element, end)) = parse_start_tag(html, i) else {
                i += 1;
                continue;
            };
            flush_text(&mut document, &stack, text_start, i);
            let index = document.elements.len();
            let name = element.name.clone();
            let self_closing = element.self_closing;
            document.elements.push(Element {
                parent: stack.last().copied(),
                ..element
            });
            match stack.last() {
                Some(&parent) => document.elements[parent].children.push(index),
                None => document.roots.push(index),
            }
            document.tokens.push(Token::Tag(index));
            i = end;
            text_start = i;

            if name == "style" || name == "script" {
                // Raw text elements: copy their contents verbatim.
                let close = format!("</{name}");
                let content_end = html[i..]
                    .to_ascii_lowercase()
                    .find(&close)
                    .map_or(html.len(), |e| i + e);
                if name == "style" {
                    document.styles.push(html[i..content_end].to_string());
                }
                if content_end > i {
                    document
                        .tokens
                        .push(Token::Raw(html[i..content_end].to_string()));
                }
                i = content_end;
                text_start = i;
                if i < html.len() {
                    let end = html[i..].find('>').map_or(html.len(), |e| i + e + 1);
                    document.tokens.push(Token::Raw(html[i..end].to_string()));
                    i = end;
                    text_start = i;
                }
                continue;
            }
            if !self_closing && !VOID_ELEMENTS.contains(&name.as_str()) {
                stack.push(index);
            }
        }
        flush_text(&mut document, &stack, text_start, html.len());
        document
    }

    /// The element children of an element's parent (or the root elements).
    fn siblings(&self, element: usize) -> &[usize] {
        match self.elements[element].parent {
            Some(parent) => &self.elements[parent].children,
            None => &self.roots,
        }
    }

    fn siblings_of_type(&self, element: usize) -> Vec<usize> {
        let name = &self.elements[element].name;
        self.siblings(element)
            .iter()
            .copied()
            .filter(|s| self.elements[*s].name == *name)
            .collect()
    }

    fn to_html(&self) -> String {
        let mut out = String::new();
        for token in &self.tokens {
            match token {
                Token::Raw(raw) => out.push_str(raw),
                Token::Tag(index) => out.push_str(&self.elements[*index].to_html()),
            }
        }
        out
    }
}

/// Parse a start tag at `start`, returning the element and the index just
/// past the tag.
fn parse_start_tag(html: &str, start: usize) -> Option<(Element, usize)> {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let name_start = i;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b':')
    {
        i += 1;
    }
    if i == name_start || !bytes[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = html[name_start..i].to_ascii_lowercase();
    let mut attributes = Vec::new();
    let mut self_closing = false;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        match bytes[i] {
            b'>' => {
                i += 1;
                break;
            }
            b'/' if bytes.get(i + 1) == Some(&b'>') => {
                self_closing = true;
                i += 2;
                break;
            }
            b'/' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let key_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'=' | b'>' | b'/')
        {
            i += 1;
        }
        let key = html[key_start..i].to_string();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let value = match bytes.get(i) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let value_start = i + 1;
                    let end = html[value_start..].find(quote as char)? + value_start;
                    i = end + 1;
                    let value = &html[value_start..end];
                    if quote == b'\'' {
                        value.replace('"', "&quot;")
                    } else {
                        value.to_string()
                    }
                }
                _ => {
                    let value_start = i;
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                        i += 1;
                    }
                    html[value_start..i].to_string()
                }
            };
            attributes.push((key, Some(value)));
        } else if !key.is_empty() {
            attributes.push((key, None));
        }
    }
    Some((
        Element {
            name,
            attributes,
            self_closing,
            parent: None,
            children: Vec::new(),
            has_text: false,
            dirty: false,
            raw: html[start..i].to_string(),
        },
        i,
    ))
}

#[cfg(test)]
mod tests {
    use super::inline_css;

    #[test]
    fn type_class_and_id_selectors_are_inlined() {
        let html = inline_css(
            r#"<div id="main" class="box wide"><p>Hello</p><span>x</span></div>"#,
            "div { color: red; } .box { padding: 0; } #main { margin: 0; } .missing { color: blue; }",
        );
        assert_eq!(
            html,
            r#"<div id="main" class="box wide" style="color: red; padding: 0; margin: 0;"><p>Hello</p><span>x</span></div>"#
        );
    }

    #[test]
    fn specificity_decides_which_rule_wins() {
        let html = inline_css(
            r#"<p class="sub">Hi</p>"#,
            "p.sub { font-size: 12px; } p { font-size: 16px; color: gray; }",
        );
        assert_eq!(
            html,
            r#"<p class="sub" style="color: gray; font-size: 12px;">Hi</p>"#
        );
    }

    #[test]
    fn inline_styles_and_important_declarations() {
        let html = inline_css(
            r#"<td style="border: hidden !important;" class="body">x</td>"#,
            ".body { border: 1px solid red; width: 100% !important; } td { width: 50%; }",
        );
        assert_eq!(
            html,
            r#"<td style="width: 100% !important; border: hidden !important;" class="body">x</td>"#
        );
    }

    #[test]
    fn combinators_and_pseudo_classes() {
        let html = inline_css(
            "<body><div class=\"header\"><a href=\"/\">Home</a></div><a>Other</a><br><code>c</code></body>",
            ".header a { color: black; } body > a { color: blue; } body *:not(br):not(code) { position: relative; }",
        );
        assert_eq!(
            html,
            "<body><div class=\"header\" style=\"position: relative;\"><a href=\"/\" style=\"position: relative; color: black;\">Home</a></div><a style=\"color: blue; position: relative;\">Other</a><br><code>c</code></body>"
        );

        let html = inline_css(
            "<div class=\"panel-item\"><p>one</p><p>two</p></div>",
            ".panel-item p:last-of-type { margin-bottom: 0; } p:first-child { color: red; }",
        );
        assert_eq!(
            html,
            "<div class=\"panel-item\"><p style=\"color: red;\">one</p><p style=\"margin-bottom: 0;\">two</p></div>"
        );
    }

    #[test]
    fn unsupported_rules_and_at_rules_are_skipped() {
        let html = inline_css(
            "<html><head><style>@media only screen and (max-width: 600px) { .inner { width: 100% !important; } } a:hover { color: red; } a { color: blue; }</style></head><body><a class=\"inner\">x</a></body></html>",
            "a::before { content: 'x'; } /* comment */ .inner { font-weight: bold; }",
        );
        assert!(
            html.contains("<a class=\"inner\" style=\"color: blue; font-weight: bold;\">x</a>")
        );
        assert!(html.contains("@media only screen"));
    }

    #[test]
    fn markup_is_otherwise_untouched() {
        let source = "<!DOCTYPE html>\n<!-- note -->\n<p data-x='a \"b\"' hidden>&copy; 2024</p><img src=\"x.png\" />";
        assert_eq!(inline_css(source, ""), source);
        let html = inline_css(source, "img { border: none; }");
        assert!(html.ends_with("<img src=\"x.png\" style=\"border: none;\" />"));
    }
}
