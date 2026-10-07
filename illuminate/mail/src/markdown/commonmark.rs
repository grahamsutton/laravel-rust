//! A small, dependable CommonMark converter (with GitHub-style tables).
//!
//! Laravel converts Markdown mail to HTML with `league/commonmark` and its
//! table extension. This module implements the same block structure
//! (paragraphs, headings, block quotes, lists, code blocks, thematic breaks,
//! raw HTML blocks and tables) and inline syntax (emphasis, code spans,
//! links, images, autolinks, raw HTML, entities, escapes and line breaks),
//! following the reference `commonmark.js` algorithms. Like Laravel, unsafe
//! link protocols (`javascript:`, `vbscript:`, `file:` and non-image
//! `data:`) are stripped.
//!
//! Tabs are expanded to spaces (tab stops of four) before parsing.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

/// Convert Markdown to HTML.
///
/// ```
/// use illuminate_mail::markdown::to_html;
///
/// assert_eq!(to_html("# Hello\n\nThis is **Laravel**."), "<h1>Hello</h1>\n<p>This is <strong>Laravel</strong>.</p>\n");
/// ```
pub fn to_html(markdown: &str) -> String {
    let mut parser = BlockParser::new();
    for line in markdown.split('\n') {
        parser.incorporate_line(line.strip_suffix('\r').unwrap_or(line));
    }
    parser.finish();
    let mut renderer = Renderer {
        out: String::new(),
        blocks: &parser.blocks,
        refmap: &parser.refmap,
    };
    renderer.render_children(0, false);
    renderer.out
}

// ----------------------------------------------------------------------
// Block structure
// ----------------------------------------------------------------------

const CODE_INDENT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Bullet(u8),
    Ordered(u8),
}

#[derive(Clone, Copy, Debug)]
struct ListData {
    kind: ListKind,
    start: u64,
    marker_offset: usize,
    padding: usize,
    tight: bool,
}

/// A table column alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
enum Kind {
    Document,
    BlockQuote,
    List(ListData),
    Item(ListData),
    Paragraph,
    Heading(u8),
    ThematicBreak,
    CodeBlock {
        fenced: bool,
        fence_char: u8,
        fence_len: usize,
        fence_offset: usize,
        info: String,
    },
    HtmlBlock(u8),
    Table {
        aligns: Vec<Align>,
        rows: Vec<Vec<String>>,
    },
}

#[derive(Debug)]
struct Block {
    kind: Kind,
    parent: Option<usize>,
    children: Vec<usize>,
    open: bool,
    content: String,
    last_line_blank: bool,
    last_line_checked: bool,
    start_line: usize,
}

impl Block {
    fn accepts_lines(&self) -> bool {
        matches!(
            self.kind,
            Kind::Paragraph | Kind::CodeBlock { .. } | Kind::HtmlBlock(_) | Kind::Table { .. }
        )
    }

    fn can_contain(&self, child: &Kind) -> bool {
        match self.kind {
            Kind::Document | Kind::BlockQuote | Kind::Item(_) => !matches!(child, Kind::Item(_)),
            Kind::List(_) => matches!(child, Kind::Item(_)),
            _ => false,
        }
    }
}

static HTML_BLOCK_OPEN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)^<(?:script|pre|textarea|style)(?:\s|>|$)").unwrap(),
        Regex::new(r"^<!--").unwrap(),
        Regex::new(r"^<[?]").unwrap(),
        Regex::new(r"^<![A-Za-z]").unwrap(),
        Regex::new(r"^<!\[CDATA\[").unwrap(),
        Regex::new(
            r"(?i)^</?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[123456]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?:\s|/?>|$)",
        )
        .unwrap(),
        Regex::new(&format!(r"(?i)^(?:{OPEN_TAG}|{CLOSE_TAG})\s*$")).unwrap(),
    ]
});

static HTML_BLOCK_CLOSE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)</(?:script|pre|textarea|style)>").unwrap(),
        Regex::new(r"-->").unwrap(),
        Regex::new(r"\?>").unwrap(),
        Regex::new(r">").unwrap(),
        Regex::new(r"\]\]>").unwrap(),
    ]
});

const OPEN_TAG: &str = r#"<[A-Za-z][A-Za-z0-9-]*(?:\s+[a-zA-Z_:][a-zA-Z0-9_.:-]*(?:\s*=\s*(?:[^"'=<>`\x00-\x20]+|'[^']*'|"[^"]*"))?)*\s*/?>"#;
const CLOSE_TAG: &str = r"</[A-Za-z][A-Za-z0-9-]*\s*>";

static ATX_HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}(?:[ \t]+|$)").unwrap());
static SETEXT_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:=+|-+)[ \t]*$").unwrap());
static THEMATIC_BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:\*[ \t]*){3,}|(?:_[ \t]*){3,}|(?:-[ \t]*){3,})$").unwrap()
});
static ORDERED_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,9})([.)])").unwrap());
static CLOSING_FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:`{3,}|~{3,})[ \t]*$").unwrap());
static TABLE_DELIMITER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$").unwrap()
});
static REFERENCE_DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^ {0,3}\[((?:[^\\\[\]]|\\.){1,999})\]:[ \t]*(?:\n[ \t]*)?(<(?:[^<>\n\\]|\\.)*>|[^\s<>][^\s]*)(?:(?:[ \t]+|[ \t]*\n[ \t]*)("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\((?:\\.|[^()\\])*\)))?[ \t]*(?:\n|$)"#,
    )
    .unwrap()
});

struct BlockParser {
    blocks: Vec<Block>,
    tip: usize,
    old_tip: usize,
    last_matched: usize,
    all_closed: bool,
    line: String,
    line_number: usize,
    offset: usize,
    next_nonspace: usize,
    indent: usize,
    indented: bool,
    blank: bool,
    refmap: HashMap<String, (String, String)>,
}

impl BlockParser {
    fn new() -> Self {
        Self {
            blocks: vec![Block {
                kind: Kind::Document,
                parent: None,
                children: Vec::new(),
                open: true,
                content: String::new(),
                last_line_blank: false,
                last_line_checked: false,
                start_line: 0,
            }],
            tip: 0,
            old_tip: 0,
            last_matched: 0,
            all_closed: true,
            line: String::new(),
            line_number: 0,
            offset: 0,
            next_nonspace: 0,
            indent: 0,
            indented: false,
            blank: false,
            refmap: HashMap::new(),
        }
    }

    fn byte(&self, index: usize) -> Option<u8> {
        self.line.as_bytes().get(index).copied()
    }

    fn rest(&self) -> &str {
        &self.line[self.next_nonspace..]
    }

    fn find_next_nonspace(&mut self) {
        let bytes = self.line.as_bytes();
        let mut i = self.offset;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        self.next_nonspace = i;
        self.indent = i - self.offset;
        self.indented = self.indent >= CODE_INDENT;
        self.blank = i >= bytes.len();
    }

    fn advance_next_nonspace(&mut self) {
        self.offset = self.next_nonspace;
    }

    fn advance_offset(&mut self, count: usize) {
        self.offset = (self.offset + count).min(self.line.len());
    }

    fn add_child(&mut self, kind: Kind, _offset: usize) -> usize {
        while !self.blocks[self.tip].can_contain(&kind) {
            self.finalize(self.tip);
        }
        let index = self.blocks.len();
        self.blocks.push(Block {
            kind,
            parent: Some(self.tip),
            children: Vec::new(),
            open: true,
            content: String::new(),
            last_line_blank: false,
            last_line_checked: false,
            start_line: self.line_number,
        });
        self.blocks[self.tip].children.push(index);
        self.tip = index;
        index
    }

    fn add_line(&mut self) {
        let text = self.line[self.offset..].to_string();
        let tip = self.tip;
        if let Kind::Table { aligns, rows } = &mut self.blocks[tip].kind {
            if !text.trim().is_empty() {
                let mut cells = split_table_row(&text);
                cells.resize(aligns.len(), String::new());
                rows.push(cells);
            }
            return;
        }
        self.blocks[tip].content.push_str(&text);
        self.blocks[tip].content.push('\n');
    }

    fn close_unmatched_blocks(&mut self) {
        if !self.all_closed {
            while self.old_tip != self.last_matched {
                let parent = self.blocks[self.old_tip].parent.unwrap_or(0);
                self.finalize(self.old_tip);
                self.old_tip = parent;
            }
            self.all_closed = true;
        }
    }

    fn incorporate_line(&mut self, line: &str) {
        self.line = expand_tabs(line);
        self.line_number += 1;
        self.offset = 0;
        self.old_tip = self.tip;

        let mut container = 0;
        loop {
            let last_child = match self.blocks[container].children.last() {
                Some(&child) if self.blocks[child].open => child,
                _ => break,
            };
            container = last_child;
            self.find_next_nonspace();
            match self.continue_block(container) {
                Continue::Matched => {}
                Continue::Unmatched => {
                    container = self.blocks[container].parent.unwrap_or(0);
                    break;
                }
                Continue::Done => return,
            }
        }

        self.all_closed = container == self.old_tip;
        self.last_matched = container;

        let mut matched_leaf = !matches!(self.blocks[container].kind, Kind::Paragraph)
            && self.blocks[container].accepts_lines();

        while !matched_leaf {
            self.find_next_nonspace();
            if !self.indented && !maybe_special(self.byte(self.next_nonspace)) {
                self.advance_next_nonspace();
                break;
            }
            match self.try_block_starts(container) {
                Start::None => {
                    self.advance_next_nonspace();
                    break;
                }
                Start::Container => container = self.tip,
                Start::Leaf => {
                    container = self.tip;
                    matched_leaf = true;
                }
            }
        }

        if !self.all_closed && !self.blank && matches!(self.blocks[self.tip].kind, Kind::Paragraph)
        {
            // A lazy paragraph continuation line.
            self.add_line();
            return;
        }

        self.close_unmatched_blocks();
        if self.blank
            && let Some(&last) = self.blocks[container].children.last()
        {
            self.blocks[last].last_line_blank = true;
        }

        let block = &self.blocks[container];
        let last_line_blank = self.blank
            && !(matches!(block.kind, Kind::BlockQuote)
                || matches!(block.kind, Kind::CodeBlock { fenced: true, .. })
                || (matches!(block.kind, Kind::Item(_))
                    && block.children.is_empty()
                    && block.start_line == self.line_number));
        let mut current = Some(container);
        while let Some(index) = current {
            self.blocks[index].last_line_blank = last_line_blank;
            current = self.blocks[index].parent;
        }

        if self.blocks[container].accepts_lines() {
            self.add_line();
            if let Kind::HtmlBlock(kind @ 1..=5) = self.blocks[container].kind
                && HTML_BLOCK_CLOSE[kind as usize - 1].is_match(&self.line[self.offset..])
            {
                self.finalize(container);
            }
        } else if self.offset < self.line.len() && !self.blank {
            self.add_child(Kind::Paragraph, self.next_nonspace);
            self.advance_next_nonspace();
            self.add_line();
        }
    }

    fn continue_block(&mut self, container: usize) -> Continue {
        match &self.blocks[container].kind {
            Kind::Document => Continue::Matched,
            Kind::BlockQuote => {
                if !self.indented && self.byte(self.next_nonspace) == Some(b'>') {
                    self.advance_next_nonspace();
                    self.advance_offset(1);
                    if self.byte(self.offset) == Some(b' ') {
                        self.advance_offset(1);
                    }
                    Continue::Matched
                } else {
                    Continue::Unmatched
                }
            }
            Kind::List(_) => Continue::Matched,
            Kind::Item(data) => {
                let required = data.marker_offset + data.padding;
                if self.blank {
                    if self.blocks[container].children.is_empty() {
                        Continue::Unmatched
                    } else {
                        self.advance_next_nonspace();
                        Continue::Matched
                    }
                } else if self.indent >= required {
                    self.advance_offset(required);
                    Continue::Matched
                } else {
                    Continue::Unmatched
                }
            }
            Kind::Heading(_) | Kind::ThematicBreak => Continue::Unmatched,
            Kind::CodeBlock {
                fenced: true,
                fence_char,
                fence_len,
                fence_offset,
                ..
            } => {
                let (fence_char, fence_len, fence_offset) =
                    (*fence_char, *fence_len, *fence_offset);
                let rest = self.rest();
                if self.indent <= 3
                    && rest.as_bytes().first() == Some(&fence_char)
                    && CLOSING_FENCE.is_match(rest)
                    && rest.bytes().take_while(|b| *b == fence_char).count() >= fence_len
                {
                    self.finalize(container);
                    return Continue::Done;
                }
                let mut remaining = fence_offset;
                while remaining > 0 && self.byte(self.offset) == Some(b' ') {
                    self.advance_offset(1);
                    remaining -= 1;
                }
                Continue::Matched
            }
            Kind::CodeBlock { fenced: false, .. } => {
                if self.indent >= CODE_INDENT {
                    self.advance_offset(CODE_INDENT);
                    Continue::Matched
                } else if self.blank {
                    self.advance_next_nonspace();
                    Continue::Matched
                } else {
                    Continue::Unmatched
                }
            }
            Kind::HtmlBlock(kind) => {
                if self.blank && (*kind == 6 || *kind == 7) {
                    Continue::Unmatched
                } else {
                    Continue::Matched
                }
            }
            Kind::Paragraph => {
                if self.blank {
                    Continue::Unmatched
                } else {
                    Continue::Matched
                }
            }
            Kind::Table { .. } => {
                if self.blank || (!self.indented && self.interrupts_table()) {
                    Continue::Unmatched
                } else {
                    Continue::Matched
                }
            }
        }
    }

    /// Whether the current line starts a block that ends a table.
    fn interrupts_table(&self) -> bool {
        let rest = self.rest();
        rest.starts_with('>')
            || ATX_HEADING.is_match(rest)
            || THEMATIC_BREAK.is_match(rest)
            || rest.starts_with("```")
            || rest.starts_with("~~~")
            || HTML_BLOCK_OPEN[..6].iter().any(|re| re.is_match(rest))
    }

    fn try_block_starts(&mut self, container: usize) -> Start {
        let rest = self.rest().to_string();
        let first = rest.as_bytes().first().copied();

        // Block quotes.
        if !self.indented && first == Some(b'>') {
            self.advance_next_nonspace();
            self.advance_offset(1);
            if self.byte(self.offset) == Some(b' ') {
                self.advance_offset(1);
            }
            self.close_unmatched_blocks();
            self.add_child(Kind::BlockQuote, self.next_nonspace);
            return Start::Container;
        }

        // ATX headings.
        if !self.indented
            && let Some(marker) = ATX_HEADING.find(&rest)
        {
            let level = marker.as_str().trim().len() as u8;
            self.advance_next_nonspace();
            self.advance_offset(marker.end());
            self.close_unmatched_blocks();
            let heading = self.add_child(Kind::Heading(level), self.next_nonspace);
            let text = self.line[self.offset..].to_string();
            self.blocks[heading].content = strip_closing_hashes(&text);
            self.offset = self.line.len();
            return Start::Leaf;
        }

        // Fenced code blocks.
        if !self.indented
            && let Some((fence_char, fence_len)) = code_fence(&rest)
        {
            self.close_unmatched_blocks();
            self.add_child(
                Kind::CodeBlock {
                    fenced: true,
                    fence_char,
                    fence_len,
                    fence_offset: self.indent,
                    info: String::new(),
                },
                self.next_nonspace,
            );
            self.advance_next_nonspace();
            self.advance_offset(fence_len);
            return Start::Leaf;
        }

        // HTML blocks.
        if !self.indented && first == Some(b'<') {
            let in_paragraph = matches!(self.blocks[container].kind, Kind::Paragraph)
                || (!self.all_closed
                    && !self.blank
                    && matches!(self.blocks[self.tip].kind, Kind::Paragraph));
            for (index, re) in HTML_BLOCK_OPEN.iter().enumerate() {
                let kind = index as u8 + 1;
                if re.is_match(&rest) && (kind < 7 || !in_paragraph) {
                    self.close_unmatched_blocks();
                    self.add_child(Kind::HtmlBlock(kind), self.offset);
                    return Start::Leaf;
                }
            }
        }

        // Setext headings and tables (both turn a paragraph into something else).
        if !self.indented && matches!(self.blocks[container].kind, Kind::Paragraph) {
            if SETEXT_HEADING.is_match(&rest) {
                self.close_unmatched_blocks();
                let content = self.paragraph_without_references(container);
                if !content.trim().is_empty() {
                    let level = if rest.starts_with('=') { 1 } else { 2 };
                    self.blocks[container].kind = Kind::Heading(level);
                    self.blocks[container].content = content;
                    self.offset = self.line.len();
                    return Start::Leaf;
                }
            } else if TABLE_DELIMITER.is_match(&rest)
                && rest.contains(['|', ':'])
                && let Some(table) = self.start_table(container, &rest)
            {
                self.tip = table;
                self.offset = self.line.len();
                return Start::Leaf;
            }
        }

        // Thematic breaks.
        if !self.indented && THEMATIC_BREAK.is_match(&rest) {
            self.close_unmatched_blocks();
            self.add_child(Kind::ThematicBreak, self.next_nonspace);
            self.offset = self.line.len();
            return Start::Leaf;
        }

        // List items.
        if (!self.indented || matches!(self.blocks[container].kind, Kind::List(_)))
            && let Some(data) = self.parse_list_marker(container)
        {
            self.close_unmatched_blocks();
            let continues_list = match &self.blocks[self.tip].kind {
                Kind::List(existing) => existing.kind == data.kind,
                _ => false,
            };
            if !continues_list {
                self.add_child(Kind::List(data), self.next_nonspace);
            }
            self.add_child(Kind::Item(data), self.next_nonspace);
            return Start::Container;
        }

        // Indented code blocks.
        if self.indented && !matches!(self.blocks[self.tip].kind, Kind::Paragraph) && !self.blank {
            self.advance_offset(CODE_INDENT);
            self.close_unmatched_blocks();
            self.add_child(
                Kind::CodeBlock {
                    fenced: false,
                    fence_char: 0,
                    fence_len: 0,
                    fence_offset: 0,
                    info: String::new(),
                },
                self.offset,
            );
            return Start::Leaf;
        }

        Start::None
    }

    /// Turn the last line of a paragraph into a table header row, when the
    /// current line is a matching delimiter row.
    fn start_table(&mut self, paragraph: usize, delimiter: &str) -> Option<usize> {
        let content = self.blocks[paragraph]
            .content
            .trim_end_matches('\n')
            .to_string();
        let (before, header) = match content.rfind('\n') {
            Some(index) => (
                content[..index].to_string(),
                content[index + 1..].to_string(),
            ),
            None => (String::new(), content.clone()),
        };
        let header_cells = split_table_row(&header);
        let aligns: Vec<Align> = split_table_row(delimiter)
            .iter()
            .map(|cell| {
                let cell = cell.trim();
                match (cell.starts_with(':'), cell.ends_with(':')) {
                    (true, true) => Align::Center,
                    (true, false) => Align::Left,
                    (false, true) => Align::Right,
                    (false, false) => Align::None,
                }
            })
            .collect();
        if header_cells.len() != aligns.len() || header.trim().is_empty() {
            return None;
        }

        self.close_unmatched_blocks();
        let parent = self.blocks[paragraph].parent.unwrap_or(0);
        if before.trim().is_empty() {
            self.blocks[parent]
                .children
                .retain(|child| *child != paragraph);
            self.blocks[paragraph].open = false;
        } else {
            self.blocks[paragraph].content = format!("{before}\n");
            self.finalize(paragraph);
        }
        self.tip = parent;
        let table = self.add_child(
            Kind::Table {
                aligns,
                rows: vec![header_cells],
            },
            self.next_nonspace,
        );
        Some(table)
    }

    fn parse_list_marker(&mut self, container: usize) -> Option<ListData> {
        if self.indent >= 4 {
            return None;
        }
        let rest = self.rest().to_string();
        let bytes = rest.as_bytes();
        let in_paragraph = matches!(self.blocks[container].kind, Kind::Paragraph);
        let (kind, start, marker_len) = match bytes.first() {
            Some(&c @ (b'*' | b'+' | b'-')) => (ListKind::Bullet(c), 1, 1),
            _ => {
                let captures = ORDERED_MARKER.captures(&rest)?;
                let number: u64 = captures[1].parse().ok()?;
                if in_paragraph && number != 1 {
                    return None;
                }
                let delimiter = captures[2].as_bytes()[0];
                (ListKind::Ordered(delimiter), number, captures[0].len())
            }
        };
        match bytes.get(marker_len) {
            None | Some(b' ') => {}
            _ => return None,
        }
        if in_paragraph && rest[marker_len..].trim().is_empty() {
            return None;
        }

        let marker_offset = self.indent;
        self.advance_next_nonspace();
        self.advance_offset(marker_len);
        let spaces_start = self.offset;
        let mut spaces = 0;
        while spaces < 5 && self.byte(self.offset) == Some(b' ') {
            self.advance_offset(1);
            spaces += 1;
        }
        let blank_item = self.offset >= self.line.len();
        let padding = if !(1..5).contains(&spaces) || blank_item {
            self.offset = spaces_start;
            if self.byte(self.offset) == Some(b' ') {
                self.advance_offset(1);
            }
            marker_len + 1
        } else {
            marker_len + spaces
        };

        Some(ListData {
            kind,
            start,
            marker_offset,
            padding,
            tight: true,
        })
    }

    /// Strip (and remember) link reference definitions at the start of a paragraph.
    fn paragraph_without_references(&mut self, paragraph: usize) -> String {
        let mut content = self.blocks[paragraph].content.clone();
        while content.starts_with([' ', '[']) {
            let Some(captures) = REFERENCE_DEFINITION.captures(&content) else {
                break;
            };
            let label = normalize_reference(&captures[1]);
            if label.is_empty() {
                break;
            }
            let destination = captures[2].to_string();
            let destination = destination
                .strip_prefix('<')
                .and_then(|d| d.strip_suffix('>'))
                .unwrap_or(&destination);
            let title = captures
                .get(3)
                .map(|t| unescape(&t.as_str()[1..t.as_str().len() - 1]))
                .unwrap_or_default();
            self.refmap
                .entry(label)
                .or_insert_with(|| (normalize_uri(&unescape(destination)), title));
            content = content[captures[0].len()..].to_string();
        }
        content
    }

    fn finalize(&mut self, index: usize) {
        let parent = self.blocks[index].parent;
        self.blocks[index].open = false;
        match self.blocks[index].kind.clone() {
            Kind::Paragraph => {
                let content = self.paragraph_without_references(index);
                if content.trim().is_empty() {
                    if let Some(parent) = parent {
                        self.blocks[parent].children.retain(|child| *child != index);
                    }
                } else {
                    self.blocks[index].content = content;
                }
            }
            Kind::CodeBlock {
                fenced: true,
                fence_char,
                fence_len,
                fence_offset,
                ..
            } => {
                let content = std::mem::take(&mut self.blocks[index].content);
                let (first, rest) = content.split_once('\n').unwrap_or((&content, ""));
                self.blocks[index].kind = Kind::CodeBlock {
                    fenced: true,
                    fence_char,
                    fence_len,
                    fence_offset,
                    info: unescape(first.trim()),
                };
                self.blocks[index].content = rest.to_string();
            }
            Kind::CodeBlock { fenced: false, .. } => {
                let content = std::mem::take(&mut self.blocks[index].content);
                let mut lines: Vec<&str> = content.split('\n').collect();
                while lines.last().is_some_and(|line| line.trim().is_empty()) {
                    lines.pop();
                }
                self.blocks[index].content = format!("{}\n", lines.join("\n"));
            }
            Kind::HtmlBlock(_) => {
                let content = &self.blocks[index].content;
                let trimmed = content.trim_end_matches([' ', '\n']).to_string();
                self.blocks[index].content = trimmed;
            }
            Kind::List(mut data) => {
                data.tight = self.list_is_tight(index);
                self.blocks[index].kind = Kind::List(data);
            }
            _ => {}
        }
        self.tip = parent.unwrap_or(0);
    }

    fn ends_with_blank_line(&mut self, mut index: usize) -> bool {
        loop {
            if self.blocks[index].last_line_blank {
                return true;
            }
            let descend = matches!(self.blocks[index].kind, Kind::List(_) | Kind::Item(_))
                && !self.blocks[index].last_line_checked;
            self.blocks[index].last_line_checked = true;
            match (descend, self.blocks[index].children.last()) {
                (true, Some(&last)) => index = last,
                _ => return false,
            }
        }
    }

    fn list_is_tight(&mut self, list: usize) -> bool {
        let items = self.blocks[list].children.clone();
        for (i, &item) in items.iter().enumerate() {
            let has_next_item = i + 1 < items.len();
            if self.ends_with_blank_line(item) && has_next_item {
                return false;
            }
            let children = self.blocks[item].children.clone();
            for (j, &child) in children.iter().enumerate() {
                let has_next_child = j + 1 < children.len();
                if self.ends_with_blank_line(child) && (has_next_item || has_next_child) {
                    return false;
                }
            }
        }
        true
    }

    fn finish(&mut self) {
        while self.tip != 0 {
            self.finalize(self.tip);
        }
        self.finalize(0);
    }
}

enum Continue {
    Matched,
    Unmatched,
    Done,
}

enum Start {
    None,
    Container,
    Leaf,
}

fn maybe_special(byte: Option<u8>) -> bool {
    matches!(
        byte,
        Some(
            b'#' | b'`' | b'~' | b'*' | b'+' | b'_' | b'=' | b'<' | b'>' | b'-' | b'|' | b':' | b'0'
                ..=b'9'
        )
    )
}

fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let spaces = 4 - (column % 4);
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(c);
            column += 1;
        }
    }
    out
}

fn code_fence(rest: &str) -> Option<(u8, usize)> {
    let first = *rest.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|b| *b == first).count();
    if len < 3 || (first == b'`' && rest[len..].contains('`')) {
        return None;
    }
    Some((first, len))
}

fn strip_closing_hashes(text: &str) -> String {
    let trimmed = text.trim_end_matches([' ', '\t']);
    let without = trimmed.trim_end_matches('#');
    if without.len() == trimmed.len() {
        return trimmed.trim_start().to_string();
    }
    if without.is_empty() || without.ends_with([' ', '\t']) {
        return without.trim().to_string();
    }
    trimmed.trim_start().to_string()
}

/// Split a table row into its cells, honoring escaped pipes.
fn split_table_row(row: &str) -> Vec<String> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = row.chars().peekable();
    let mut trailing_pipe = false;
    while let Some(c) = chars.next() {
        trailing_pipe = false;
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                cell.push('|');
            }
            '\\' => {
                cell.push('\\');
                if let Some(next) = chars.next() {
                    cell.push(next);
                }
            }
            '|' => {
                cells.push(cell.trim().to_string());
                cell.clear();
                trailing_pipe = true;
            }
            _ => cell.push(c),
        }
    }
    if !trailing_pipe || !cell.trim().is_empty() {
        cells.push(cell.trim().to_string());
    }
    cells
}

fn normalize_reference(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ----------------------------------------------------------------------
// Rendering
// ----------------------------------------------------------------------

struct Renderer<'a> {
    out: String,
    blocks: &'a [Block],
    refmap: &'a HashMap<String, (String, String)>,
}

impl Renderer<'_> {
    fn cr(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn render_children(&mut self, index: usize, tight: bool) {
        for &child in &self.blocks[index].children {
            self.render_block(child, tight);
        }
    }

    fn inlines(&mut self, text: &str) {
        let html = render_inlines(text, self.refmap);
        self.out.push_str(&html);
    }

    fn render_block(&mut self, index: usize, tight: bool) {
        let block = &self.blocks[index];
        match &block.kind {
            Kind::Document => self.render_children(index, false),
            Kind::Paragraph => {
                let content = block.content.trim().to_string();
                if tight {
                    self.inlines(&content);
                } else {
                    self.cr();
                    self.out.push_str("<p>");
                    self.inlines(&content);
                    self.out.push_str("</p>");
                    self.cr();
                }
            }
            Kind::Heading(level) => {
                let level = *level;
                let content = block.content.trim().to_string();
                self.cr();
                self.out.push_str(&format!("<h{level}>"));
                self.inlines(&content);
                self.out.push_str(&format!("</h{level}>"));
                self.cr();
            }
            Kind::ThematicBreak => {
                self.cr();
                self.out.push_str("<hr />");
                self.cr();
            }
            Kind::CodeBlock { info, .. } => {
                self.cr();
                let language = info.split_whitespace().next().unwrap_or("");
                if language.is_empty() {
                    self.out.push_str("<pre><code>");
                } else {
                    self.out.push_str(&format!(
                        "<pre><code class=\"language-{}\">",
                        escape_html(language)
                    ));
                }
                self.out.push_str(&escape_html(&block.content));
                self.out.push_str("</code></pre>");
                self.cr();
            }
            Kind::HtmlBlock(_) => {
                self.cr();
                self.out.push_str(&block.content);
                self.cr();
            }
            Kind::BlockQuote => {
                self.cr();
                self.out.push_str("<blockquote>");
                self.cr();
                self.render_children(index, false);
                self.cr();
                self.out.push_str("</blockquote>");
                self.cr();
            }
            Kind::List(data) => {
                let tag = match data.kind {
                    ListKind::Bullet(_) => "ul",
                    ListKind::Ordered(_) => "ol",
                };
                self.cr();
                if tag == "ol" && data.start != 1 {
                    self.out.push_str(&format!("<ol start=\"{}\">", data.start));
                } else {
                    self.out.push_str(&format!("<{tag}>"));
                }
                self.cr();
                let tight = data.tight;
                self.render_children(index, tight);
                self.cr();
                self.out.push_str(&format!("</{tag}>"));
                self.cr();
            }
            Kind::Item(_) => {
                self.out.push_str("<li>");
                self.render_children(index, tight);
                self.out.push_str("</li>");
                self.cr();
            }
            Kind::Table { aligns, rows } => {
                self.cr();
                self.out.push_str("<table>\n<thead>\n<tr>\n");
                let (header, body) = rows.split_first().expect("tables always have a header row");
                for (cell, align) in header.iter().zip(aligns) {
                    self.table_cell("th", cell, *align);
                }
                self.out.push_str("</tr>\n</thead>\n");
                if !body.is_empty() {
                    self.out.push_str("<tbody>\n");
                    for row in body {
                        self.out.push_str("<tr>\n");
                        for (cell, align) in row.iter().zip(aligns) {
                            self.table_cell("td", cell, *align);
                        }
                        self.out.push_str("</tr>\n");
                    }
                    self.out.push_str("</tbody>\n");
                }
                self.out.push_str("</table>");
                self.cr();
            }
        }
    }

    fn table_cell(&mut self, tag: &str, content: &str, align: Align) {
        let align = match align {
            Align::None => "",
            Align::Left => " align=\"left\"",
            Align::Center => " align=\"center\"",
            Align::Right => " align=\"right\"",
        };
        self.out.push_str(&format!("<{tag}{align}>"));
        self.inlines(content);
        self.out.push_str(&format!("</{tag}>\n"));
    }
}

/// Escape text for HTML output.
pub(crate) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

// ----------------------------------------------------------------------
// Inlines
// ----------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Inline {
    Root,
    Text(String),
    SoftBreak,
    HardBreak,
    Code(String),
    Html(String),
    Emph,
    Strong,
    Link { destination: String, title: String },
    Image { destination: String, title: String },
}

#[derive(Debug)]
struct Node {
    kind: Inline,
    parent: Option<usize>,
    first_child: Option<usize>,
    last_child: Option<usize>,
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Default)]
struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    fn new_node(&mut self, kind: Inline) -> usize {
        self.nodes.push(Node {
            kind,
            parent: None,
            first_child: None,
            last_child: None,
            prev: None,
            next: None,
        });
        self.nodes.len() - 1
    }

    fn unlink(&mut self, node: usize) {
        let (prev, next, parent) = {
            let n = &self.nodes[node];
            (n.prev, n.next, n.parent)
        };
        match prev {
            Some(prev) => self.nodes[prev].next = next,
            None => {
                if let Some(parent) = parent {
                    self.nodes[parent].first_child = next;
                }
            }
        }
        match next {
            Some(next) => self.nodes[next].prev = prev,
            None => {
                if let Some(parent) = parent {
                    self.nodes[parent].last_child = prev;
                }
            }
        }
        let n = &mut self.nodes[node];
        n.parent = None;
        n.prev = None;
        n.next = None;
    }

    fn append_child(&mut self, parent: usize, child: usize) {
        self.unlink(child);
        self.nodes[child].parent = Some(parent);
        match self.nodes[parent].last_child {
            Some(last) => {
                self.nodes[last].next = Some(child);
                self.nodes[child].prev = Some(last);
            }
            None => self.nodes[parent].first_child = Some(child),
        }
        self.nodes[parent].last_child = Some(child);
    }

    fn insert_after(&mut self, node: usize, sibling: usize) {
        self.unlink(sibling);
        let next = self.nodes[node].next;
        let parent = self.nodes[node].parent;
        self.nodes[sibling].next = next;
        self.nodes[sibling].prev = Some(node);
        self.nodes[sibling].parent = parent;
        self.nodes[node].next = Some(sibling);
        match next {
            Some(next) => self.nodes[next].prev = Some(sibling),
            None => {
                if let Some(parent) = parent {
                    self.nodes[parent].last_child = Some(sibling);
                }
            }
        }
    }

    fn text_mut(&mut self, node: usize) -> Option<&mut String> {
        match &mut self.nodes[node].kind {
            Inline::Text(text) => Some(text),
            _ => None,
        }
    }
}

struct Delimiter {
    node: usize,
    ch: u8,
    count: usize,
    original: usize,
    can_open: bool,
    can_close: bool,
    prev: Option<usize>,
    next: Option<usize>,
}

struct Bracket {
    node: usize,
    prev: Option<usize>,
    prev_delimiter: Option<usize>,
    index: usize,
    image: bool,
    active: bool,
    bracket_after: bool,
}

static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:{OPEN_TAG}|{CLOSE_TAG}|<!-->|<!--->|(?s:<!--.*?-->)|(?s:<\?.*?\?>)|<![A-Za-z]+[^>]*>|(?s:<!\[CDATA\[.*?\]\]>))"
    ))
    .unwrap()
});
static EMAIL_AUTOLINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<([a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*)>").unwrap()
});
static URI_AUTOLINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<([A-Za-z][A-Za-z0-9.+-]{1,31}:[^<>\x00-\x20]*)>").unwrap());
static ENTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^&(?:#[xX][a-fA-F0-9]{1,6}|#[0-9]{1,7}|[a-zA-Z][a-zA-Z0-9]{1,31});").unwrap()
});
static LINK_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\((?:\\.|[^()\\])*\))"#).unwrap()
});
static LINK_DESTINATION_BRACES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<(?:[^<>\n\\\x00]|\\.)*>").unwrap());
static LINK_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(?:[^\\\[\]]|\\.){0,1000}\]").unwrap());

fn render_inlines(text: &str, refmap: &HashMap<String, (String, String)>) -> String {
    let mut parser = InlineParser {
        subject: text,
        pos: 0,
        tree: Tree::default(),
        delimiters: Vec::new(),
        delimiter_top: None,
        brackets: Vec::new(),
        bracket_top: None,
        refmap,
    };
    let root = parser.tree.new_node(Inline::Root);
    parser.parse(root);
    let mut out = String::new();
    render_inline_children(&parser.tree, root, &mut out);
    out
}

struct InlineParser<'a> {
    subject: &'a str,
    pos: usize,
    tree: Tree,
    delimiters: Vec<Delimiter>,
    delimiter_top: Option<usize>,
    brackets: Vec<Bracket>,
    bracket_top: Option<usize>,
    refmap: &'a HashMap<String, (String, String)>,
}

impl InlineParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.subject.as_bytes().get(self.pos).copied()
    }

    fn text(&mut self, parent: usize, text: &str) -> usize {
        let node = self.tree.new_node(Inline::Text(text.to_string()));
        self.tree.append_child(parent, node);
        node
    }

    fn append(&mut self, parent: usize, kind: Inline) -> usize {
        let node = self.tree.new_node(kind);
        self.tree.append_child(parent, node);
        node
    }

    fn parse(&mut self, root: usize) {
        while let Some(c) = self.peek() {
            match c {
                b'\n' => self.parse_newline(root),
                b'\\' => self.parse_backslash(root),
                b'`' => self.parse_backticks(root),
                b'*' | b'_' => self.handle_delimiter(c, root),
                b'[' => {
                    self.pos += 1;
                    let node = self.text(root, "[");
                    self.add_bracket(node, self.pos, false);
                }
                b'!' => {
                    self.pos += 1;
                    if self.peek() == Some(b'[') {
                        self.pos += 1;
                        let node = self.text(root, "![");
                        self.add_bracket(node, self.pos, true);
                    } else {
                        self.text(root, "!");
                    }
                }
                b']' => self.parse_close_bracket(root),
                b'<' => self.parse_less_than(root),
                b'&' => self.parse_entity(root),
                _ => self.parse_string(root),
            }
        }
        self.process_emphasis(None);
    }

    fn parse_string(&mut self, root: usize) {
        let rest = &self.subject[self.pos..];
        // Every special character is dispatched by `parse`, so the run is never empty.
        let end = rest
            .find(['\n', '\\', '`', '*', '_', '[', ']', '!', '<', '&'])
            .filter(|end| *end > 0)
            .unwrap_or(rest.len());
        let text = rest[..end].to_string();
        self.pos += end;
        self.text(root, &text);
    }

    fn parse_newline(&mut self, root: usize) {
        self.pos += 1;
        let mut hard = false;
        if let Some(last) = self.tree.nodes[root].last_child
            && let Some(text) = self.tree.text_mut(last)
            && text.ends_with(' ')
        {
            hard = text.ends_with("  ");
            let trimmed = text.trim_end_matches(' ').len();
            text.truncate(trimmed);
        }
        self.append(
            root,
            if hard {
                Inline::HardBreak
            } else {
                Inline::SoftBreak
            },
        );
        while self.peek() == Some(b' ') {
            self.pos += 1;
        }
    }

    fn parse_backslash(&mut self, root: usize) {
        self.pos += 1;
        match self.peek() {
            Some(b'\n') => {
                self.pos += 1;
                self.append(root, Inline::HardBreak);
                while self.peek() == Some(b' ') {
                    self.pos += 1;
                }
            }
            Some(c) if c.is_ascii_punctuation() => {
                self.pos += 1;
                self.text(root, &(c as char).to_string());
            }
            _ => {
                self.text(root, "\\");
            }
        }
    }

    fn parse_backticks(&mut self, root: usize) {
        let start = self.pos;
        let ticks = self.subject[start..]
            .bytes()
            .take_while(|b| *b == b'`')
            .count();
        self.pos += ticks;
        let after_open = self.pos;
        let bytes = self.subject.as_bytes();
        let mut i = after_open;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                let run = self.subject[i..].bytes().take_while(|b| *b == b'`').count();
                if run == ticks {
                    let contents = self.subject[after_open..i].replace('\n', " ");
                    let literal = if contents.len() >= 2
                        && contents.starts_with(' ')
                        && contents.ends_with(' ')
                        && contents.contains(|c| c != ' ')
                    {
                        contents[1..contents.len() - 1].to_string()
                    } else {
                        contents
                    };
                    self.pos = i + run;
                    self.append(root, Inline::Code(literal));
                    return;
                }
                i += run;
            } else {
                i += 1;
            }
        }
        self.pos = after_open;
        let text = "`".repeat(ticks);
        self.text(root, &text);
    }

    fn char_before(&self, pos: usize) -> char {
        self.subject[..pos].chars().next_back().unwrap_or('\n')
    }

    fn char_at(&self, pos: usize) -> char {
        self.subject[pos..].chars().next().unwrap_or('\n')
    }

    fn handle_delimiter(&mut self, c: u8, root: usize) {
        let start = self.pos;
        let count = self.subject[start..]
            .bytes()
            .take_while(|b| *b == c)
            .count();
        let before = self.char_before(start);
        let after = self.char_at(start + count);
        let after_whitespace = after.is_whitespace();
        let after_punctuation = is_punctuation(after);
        let before_whitespace = before.is_whitespace();
        let before_punctuation = is_punctuation(before);
        let left_flanking =
            !after_whitespace && (!after_punctuation || before_whitespace || before_punctuation);
        let right_flanking =
            !before_whitespace && (!before_punctuation || after_whitespace || after_punctuation);
        let (can_open, can_close) = if c == b'_' {
            (
                left_flanking && (!right_flanking || before_punctuation),
                right_flanking && (!left_flanking || after_punctuation),
            )
        } else {
            (left_flanking, right_flanking)
        };
        self.pos += count;
        let text = (c as char).to_string().repeat(count);
        let node = self.text(root, &text);
        if can_open || can_close {
            let index = self.delimiters.len();
            self.delimiters.push(Delimiter {
                node,
                ch: c,
                count,
                original: count,
                can_open,
                can_close,
                prev: self.delimiter_top,
                next: None,
            });
            if let Some(prev) = self.delimiter_top {
                self.delimiters[prev].next = Some(index);
            }
            self.delimiter_top = Some(index);
        }
    }

    fn remove_delimiter(&mut self, index: usize) {
        let (prev, next) = (self.delimiters[index].prev, self.delimiters[index].next);
        if let Some(prev) = prev {
            self.delimiters[prev].next = next;
        }
        match next {
            Some(next) => self.delimiters[next].prev = prev,
            None => self.delimiter_top = prev,
        }
    }

    fn process_emphasis(&mut self, stack_bottom: Option<usize>) {
        let mut openers_bottom = [stack_bottom; 12];

        // Find the first closer above the stack bottom.
        let mut closer = self.delimiter_top;
        while let Some(current) = closer {
            if self.delimiters[current].prev == stack_bottom {
                break;
            }
            closer = self.delimiters[current].prev;
        }

        while let Some(closer_index) = closer {
            if !self.delimiters[closer_index].can_close {
                closer = self.delimiters[closer_index].next;
                continue;
            }
            let (closer_char, closer_original, closer_can_open) = {
                let d = &self.delimiters[closer_index];
                (d.ch, d.original, d.can_open)
            };
            let bottom_index = if closer_char == b'_' { 0 } else { 6 }
                + if closer_can_open { 3 } else { 0 }
                + closer_original % 3;

            let mut opener = self.delimiters[closer_index].prev;
            let mut found = false;
            while let Some(opener_index) = opener {
                if Some(opener_index) == stack_bottom
                    || Some(opener_index) == openers_bottom[bottom_index]
                {
                    break;
                }
                let o = &self.delimiters[opener_index];
                let odd_match = (closer_can_open || o.can_close)
                    && !closer_original.is_multiple_of(3)
                    && (o.original + closer_original).is_multiple_of(3);
                if o.ch == closer_char && o.can_open && !odd_match {
                    found = true;
                    break;
                }
                opener = o.prev;
            }

            let old_closer = closer_index;
            if let (true, Some(opener_index)) = (found, opener) {
                let use_delims = if self.delimiters[closer_index].count >= 2
                    && self.delimiters[opener_index].count >= 2
                {
                    2
                } else {
                    1
                };
                let opener_node = self.delimiters[opener_index].node;
                let closer_node = self.delimiters[closer_index].node;
                self.delimiters[opener_index].count -= use_delims;
                self.delimiters[closer_index].count -= use_delims;
                for node in [opener_node, closer_node] {
                    if let Some(text) = self.tree.text_mut(node) {
                        let len = text.len() - use_delims;
                        text.truncate(len);
                    }
                }

                let emph = self.tree.new_node(if use_delims == 1 {
                    Inline::Emph
                } else {
                    Inline::Strong
                });
                let mut current = self.tree.nodes[opener_node].next;
                while let Some(node) = current {
                    if node == closer_node {
                        break;
                    }
                    current = self.tree.nodes[node].next;
                    self.tree.append_child(emph, node);
                }
                self.tree.insert_after(opener_node, emph);

                // Remove the delimiters between the opener and the closer.
                self.delimiters[opener_index].next = Some(closer_index);
                self.delimiters[closer_index].prev = Some(opener_index);

                if self.delimiters[opener_index].count == 0 {
                    self.tree.unlink(opener_node);
                    self.remove_delimiter(opener_index);
                }
                if self.delimiters[closer_index].count == 0 {
                    self.tree.unlink(closer_node);
                    let next = self.delimiters[closer_index].next;
                    self.remove_delimiter(closer_index);
                    closer = next;
                }
            } else {
                closer = self.delimiters[closer_index].next;
                openers_bottom[bottom_index] = self.delimiters[old_closer].prev;
                if !self.delimiters[old_closer].can_open {
                    self.remove_delimiter(old_closer);
                }
            }
        }

        // Remove every delimiter above the stack bottom.
        while let Some(top) = self.delimiter_top {
            if Some(top) == stack_bottom {
                break;
            }
            self.remove_delimiter(top);
        }
    }

    fn add_bracket(&mut self, node: usize, index: usize, image: bool) {
        if let Some(top) = self.bracket_top {
            self.brackets[top].bracket_after = true;
        }
        self.brackets.push(Bracket {
            node,
            prev: self.bracket_top,
            prev_delimiter: self.delimiter_top,
            index,
            image,
            active: true,
            bracket_after: false,
        });
        self.bracket_top = Some(self.brackets.len() - 1);
    }

    fn remove_bracket(&mut self) {
        if let Some(top) = self.bracket_top {
            self.bracket_top = self.brackets[top].prev;
        }
    }

    fn skip_spaces_and_newline(&mut self) {
        let bytes = self.subject.as_bytes();
        while self.pos < bytes.len() && bytes[self.pos] == b' ' {
            self.pos += 1;
        }
        if self.pos < bytes.len() && bytes[self.pos] == b'\n' {
            self.pos += 1;
            while self.pos < bytes.len() && bytes[self.pos] == b' ' {
                self.pos += 1;
            }
        }
    }

    fn parse_link_destination(&mut self) -> Option<String> {
        let rest = &self.subject[self.pos..];
        if let Some(m) = LINK_DESTINATION_BRACES.find(rest) {
            let inner = &m.as_str()[1..m.as_str().len() - 1];
            self.pos += m.end();
            return Some(normalize_uri(&unescape(inner)));
        }
        if rest.starts_with('<') {
            return None;
        }
        let start = self.pos;
        let bytes = self.subject.as_bytes();
        let mut depth = 0usize;
        while self.pos < bytes.len() {
            let c = bytes[self.pos];
            if c == b'\\'
                && bytes
                    .get(self.pos + 1)
                    .is_some_and(u8::is_ascii_punctuation)
            {
                self.pos += 2;
            } else if c == b'(' {
                depth += 1;
                self.pos += 1;
            } else if c == b')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                self.pos += 1;
            } else if c.is_ascii_whitespace() || c.is_ascii_control() {
                break;
            } else {
                self.pos += 1;
            }
        }
        if (self.pos == start && self.peek() != Some(b')')) || depth != 0 {
            return None;
        }
        Some(normalize_uri(&unescape(&self.subject[start..self.pos])))
    }

    fn parse_link_title(&mut self) -> Option<String> {
        let m = LINK_TITLE.find(&self.subject[self.pos..])?;
        let raw = m.as_str();
        self.pos += m.end();
        Some(unescape(&raw[1..raw.len() - 1]))
    }

    fn parse_close_bracket(&mut self, root: usize) {
        self.pos += 1;
        let start = self.pos;
        let Some(opener_index) = self.bracket_top else {
            self.text(root, "]");
            return;
        };
        if !self.brackets[opener_index].active {
            self.text(root, "]");
            self.remove_bracket();
            return;
        }
        let image = self.brackets[opener_index].image;

        let mut matched = None;
        let save = self.pos;
        if self.peek() == Some(b'(') {
            self.pos += 1;
            self.skip_spaces_and_newline();
            if let Some(destination) = self.parse_link_destination() {
                let before_title = self.pos;
                self.skip_spaces_and_newline();
                let title = if self.pos > before_title {
                    self.parse_link_title()
                } else {
                    None
                };
                self.skip_spaces_and_newline();
                if self.peek() == Some(b')') {
                    self.pos += 1;
                    matched = Some((destination, title.unwrap_or_default()));
                }
            }
            if matched.is_none() {
                self.pos = save;
            }
        }

        if matched.is_none() {
            let before_label = self.pos;
            let label_len = LINK_LABEL
                .find(&self.subject[self.pos..])
                .map_or(0, |m| m.end());
            let label = if label_len > 2 {
                Some(self.subject[before_label + 1..before_label + label_len - 1].to_string())
            } else if !self.brackets[opener_index].bracket_after {
                Some(self.subject[self.brackets[opener_index].index..start - 1].to_string())
            } else {
                None
            };
            if label_len == 0 {
                self.pos = save;
            } else {
                self.pos = before_label + label_len;
            }
            if let Some(label) = label
                && let Some((destination, title)) = self.refmap.get(&normalize_reference(&label))
            {
                matched = Some((destination.clone(), title.clone()));
            } else if label_len > 0 {
                self.pos = save;
            }
        }

        let Some((destination, title)) = matched else {
            self.remove_bracket();
            self.pos = start;
            self.text(root, "]");
            return;
        };

        let link = self.tree.new_node(if image {
            Inline::Image { destination, title }
        } else {
            Inline::Link { destination, title }
        });
        let opener_node = self.brackets[opener_index].node;
        let mut current = self.tree.nodes[opener_node].next;
        while let Some(node) = current {
            current = self.tree.nodes[node].next;
            self.tree.append_child(link, node);
        }
        self.tree.append_child(root, link);
        self.process_emphasis(self.brackets[opener_index].prev_delimiter);
        self.remove_bracket();
        self.tree.unlink(opener_node);

        if !image {
            let mut current = self.bracket_top;
            while let Some(index) = current {
                if !self.brackets[index].image {
                    self.brackets[index].active = false;
                }
                current = self.brackets[index].prev;
            }
        }
    }

    fn parse_less_than(&mut self, root: usize) {
        let rest = &self.subject[self.pos..];
        if let Some(captures) = EMAIL_AUTOLINK.captures(rest) {
            let address = captures[1].to_string();
            self.pos += captures[0].len();
            let link = self.append(
                root,
                Inline::Link {
                    destination: normalize_uri(&format!("mailto:{address}")),
                    title: String::new(),
                },
            );
            let text = self.tree.new_node(Inline::Text(address));
            self.tree.append_child(link, text);
            return;
        }
        if let Some(captures) = URI_AUTOLINK.captures(rest) {
            let uri = captures[1].to_string();
            self.pos += captures[0].len();
            let link = self.append(
                root,
                Inline::Link {
                    destination: normalize_uri(&uri),
                    title: String::new(),
                },
            );
            let text = self.tree.new_node(Inline::Text(uri));
            self.tree.append_child(link, text);
            return;
        }
        if let Some(m) = HTML_TAG.find(rest) {
            let html = m.as_str().to_string();
            self.pos += m.end();
            self.append(root, Inline::Html(html));
            return;
        }
        self.pos += 1;
        self.text(root, "<");
    }

    fn parse_entity(&mut self, root: usize) {
        let rest = &self.subject[self.pos..];
        if let Some(m) = ENTITY.find(rest) {
            let entity = m.as_str().to_string();
            self.pos += m.end();
            match decode_entity(&entity) {
                Some(decoded) => {
                    self.text(root, &decoded);
                }
                None => {
                    self.append(root, Inline::Html(entity));
                }
            }
            return;
        }
        self.pos += 1;
        self.text(root, "&");
    }
}

fn render_inline_children(tree: &Tree, parent: usize, out: &mut String) {
    let mut current = tree.nodes[parent].first_child;
    while let Some(node) = current {
        render_inline(tree, node, out);
        current = tree.nodes[node].next;
    }
}

fn render_inline(tree: &Tree, node: usize, out: &mut String) {
    match &tree.nodes[node].kind {
        Inline::Root => render_inline_children(tree, node, out),
        Inline::Text(text) => out.push_str(&escape_html(text)),
        Inline::SoftBreak => out.push('\n'),
        Inline::HardBreak => out.push_str("<br />\n"),
        Inline::Code(code) => {
            out.push_str("<code>");
            out.push_str(&escape_html(code));
            out.push_str("</code>");
        }
        Inline::Html(html) => out.push_str(html),
        Inline::Emph => {
            out.push_str("<em>");
            render_inline_children(tree, node, out);
            out.push_str("</em>");
        }
        Inline::Strong => {
            out.push_str("<strong>");
            render_inline_children(tree, node, out);
            out.push_str("</strong>");
        }
        Inline::Link { destination, title } => {
            out.push_str("<a");
            if !is_unsafe_link(destination) {
                out.push_str(&format!(" href=\"{}\"", escape_html(destination)));
            }
            if !title.is_empty() {
                out.push_str(&format!(" title=\"{}\"", escape_html(title)));
            }
            out.push('>');
            render_inline_children(tree, node, out);
            out.push_str("</a>");
        }
        Inline::Image { destination, title } => {
            let source = if is_unsafe_link(destination) {
                String::new()
            } else {
                escape_html(destination)
            };
            let mut alt = String::new();
            plain_text(tree, node, &mut alt);
            out.push_str(&format!(
                "<img src=\"{source}\" alt=\"{}\"",
                escape_html(&alt)
            ));
            if !title.is_empty() {
                out.push_str(&format!(" title=\"{}\"", escape_html(title)));
            }
            out.push_str(" />");
        }
    }
}

fn plain_text(tree: &Tree, parent: usize, out: &mut String) {
    let mut current = tree.nodes[parent].first_child;
    while let Some(node) = current {
        match &tree.nodes[node].kind {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
            Inline::Html(_) => {}
            _ => plain_text(tree, node, out),
        }
        current = tree.nodes[node].next;
    }
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(c as u32, 0x2010..=0x2027 | 0x2030..=0x205E | 0x00A1..=0x00BF | 0x3000..=0x303F | 0xFF01..=0xFF0F)
}

/// Whether a link points somewhere Laravel considers unsafe.
fn is_unsafe_link(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    let unsafe_protocol = ["javascript:", "vbscript:", "file:", "data:"]
        .iter()
        .any(|p| lower.starts_with(p));
    let safe_data = [
        "data:image/png",
        "data:image/gif",
        "data:image/jpeg",
        "data:image/webp",
    ]
    .iter()
    .any(|p| lower.starts_with(p));
    unsafe_protocol && !safe_data
}

/// Process backslash escapes and entities in link destinations and titles.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        let c = bytes[i];
        if c == b'\\' && bytes.get(i + 1).is_some_and(u8::is_ascii_punctuation) {
            out.push(bytes[i + 1] as char);
            i += 2;
        } else if c == b'&'
            && let Some(m) = ENTITY.find(&text[i..])
        {
            match decode_entity(m.as_str()) {
                Some(decoded) => out.push_str(&decoded),
                None => out.push_str(m.as_str()),
            }
            i += m.end();
        } else {
            let ch = text[i..].chars().next().unwrap_or_default();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Percent-encode a URL the way CommonMark's reference implementation does.
fn normalize_uri(uri: &str) -> String {
    const KEEP: &str = ";/?:@&=+$,-_.!~*'()#";
    let mut out = String::with_capacity(uri.len());
    let bytes = uri.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
        {
            out.push_str(&uri[i..i + 3]);
            i += 3;
            continue;
        }
        if b.is_ascii_alphanumeric() || KEEP.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        i += 1;
    }
    out
}

/// Decode an HTML entity (`&amp;`, `&#39;`, `&#x27;`...). Unknown named
/// entities return `None` so they can be passed through untouched.
pub(crate) fn decode_entity(entity: &str) -> Option<String> {
    let name = entity.strip_prefix('&')?.strip_suffix(';')?;
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse::<u32>().ok()?,
        };
        let c = if code == 0 {
            '\u{FFFD}'
        } else {
            char::from_u32(code).unwrap_or('\u{FFFD}')
        };
        return Some(c.to_string());
    }
    named_entity(name).map(str::to_string)
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "sbquo" => "‚",
        "ldquo" => "“",
        "rdquo" => "”",
        "bdquo" => "„",
        "laquo" => "«",
        "raquo" => "»",
        "bull" => "•",
        "middot" => "·",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "deg" => "°",
        "times" => "×",
        "divide" => "÷",
        "plusmn" => "±",
        "sect" => "§",
        "para" => "¶",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "iexcl" => "¡",
        "iquest" => "¿",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "harr" => "↔",
        "hearts" => "♥",
        "check" => "✓",
        "auml" => "ä",
        "ouml" => "ö",
        "uuml" => "ü",
        "Auml" => "Ä",
        "Ouml" => "Ö",
        "Uuml" => "Ü",
        "szlig" => "ß",
        "eacute" => "é",
        "egrave" => "è",
        "aacute" => "á",
        "agrave" => "à",
        "iacute" => "í",
        "oacute" => "ó",
        "uacute" => "ú",
        "ntilde" => "ñ",
        "Ntilde" => "Ñ",
        "ccedil" => "ç",
        "Eacute" => "É",
        "thinsp" => "\u{2009}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "zwnj" => "\u{200c}",
        "zwj" => "\u{200d}",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn paragraphs_and_headings() {
        assert_eq!(to_html("Hello world"), "<p>Hello world</p>\n");
        assert_eq!(
            to_html("# One\n## Two ##\n###### Six"),
            "<h1>One</h1>\n<h2>Two</h2>\n<h6>Six</h6>\n"
        );
        assert_eq!(to_html("#NoHeading"), "<p>#NoHeading</p>\n");
        assert_eq!(
            to_html("Title\n=====\n\nSub\n---"),
            "<h1>Title</h1>\n<h2>Sub</h2>\n"
        );
        assert_eq!(
            to_html("one\ntwo\n\nthree"),
            "<p>one\ntwo</p>\n<p>three</p>\n"
        );
        assert_eq!(to_html(""), "");
    }

    #[test]
    fn emphasis_and_strong() {
        assert_eq!(
            to_html("*a* _b_ **c** __d__"),
            "<p><em>a</em> <em>b</em> <strong>c</strong> <strong>d</strong></p>\n"
        );
        assert_eq!(
            to_html("***both***"),
            "<p><em><strong>both</strong></em></p>\n"
        );
        assert_eq!(to_html("snake_case_word"), "<p>snake_case_word</p>\n");
        assert_eq!(to_html("*not emphasized"), "<p>*not emphasized</p>\n");
        assert_eq!(
            to_html("**bold *and italic***"),
            "<p><strong>bold <em>and italic</em></strong></p>\n"
        );
        assert_eq!(to_html("2 * 3 * 4"), "<p>2 * 3 * 4</p>\n");
    }

    #[test]
    fn code() {
        assert_eq!(
            to_html("Use `php artisan`"),
            "<p>Use <code>php artisan</code></p>\n"
        );
        assert_eq!(to_html("`` a`b ``"), "<p><code>a`b</code></p>\n");
        assert_eq!(
            to_html("```php\necho 'hi';\n```"),
            "<pre><code class=\"language-php\">echo 'hi';\n</code></pre>\n"
        );
        assert_eq!(
            to_html("    indented <code>\n\nafter"),
            "<pre><code>indented &lt;code&gt;\n</code></pre>\n<p>after</p>\n"
        );
        assert_eq!(
            to_html("~~~\nunclosed"),
            "<pre><code>unclosed\n</code></pre>\n"
        );
    }

    #[test]
    fn links_and_images() {
        assert_eq!(
            to_html("[Laravel](https://laravel.com)"),
            "<p><a href=\"https://laravel.com\">Laravel</a></p>\n"
        );
        assert_eq!(
            to_html("[Docs](https://laravel.com/docs \"The docs\")"),
            "<p><a href=\"https://laravel.com/docs\" title=\"The docs\">Docs</a></p>\n"
        );
        assert_eq!(
            to_html("![Logo](/logo.png)"),
            "<p><img src=\"/logo.png\" alt=\"Logo\" /></p>\n"
        );
        assert_eq!(
            to_html("<https://laravel.com>"),
            "<p><a href=\"https://laravel.com\">https://laravel.com</a></p>\n"
        );
        assert_eq!(
            to_html("<taylor@laravel.com>"),
            "<p><a href=\"mailto:taylor@laravel.com\">taylor@laravel.com</a></p>\n"
        );
        assert_eq!(to_html("[x](javascript:alert(1))"), "<p><a>x</a></p>\n");
        assert_eq!(
            to_html("[ref]\n\n[ref]: https://laravel.com"),
            "<p><a href=\"https://laravel.com\">ref</a></p>\n"
        );
        assert_eq!(to_html("[not a link]"), "<p>[not a link]</p>\n");
        assert_eq!(
            to_html("[a](https://x.com/?a=1&amp;b=2)"),
            "<p><a href=\"https://x.com/?a=1&amp;b=2\">a</a></p>\n"
        );
        assert_eq!(
            to_html("[**bold** link](/x)"),
            "<p><a href=\"/x\"><strong>bold</strong> link</a></p>\n"
        );
        assert_eq!(to_html("[a b](/my page)"), "<p>[a b](/my page)</p>\n");
    }

    #[test]
    fn lists() {
        assert_eq!(
            to_html("- one\n- two"),
            "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n"
        );
        assert_eq!(
            to_html("1. one\n2. two"),
            "<ol>\n<li>one</li>\n<li>two</li>\n</ol>\n"
        );
        assert_eq!(
            to_html("3) three"),
            "<ol start=\"3\">\n<li>three</li>\n</ol>\n"
        );
        assert_eq!(
            to_html("- one\n\n- two"),
            "<ul>\n<li>\n<p>one</p>\n</li>\n<li>\n<p>two</p>\n</li>\n</ul>\n"
        );
        assert_eq!(
            to_html("- one\n  - nested\n- two"),
            "<ul>\n<li>one\n<ul>\n<li>nested</li>\n</ul>\n</li>\n<li>two</li>\n</ul>\n"
        );
        assert_eq!(
            to_html("- a\n+ b"),
            "<ul>\n<li>a</li>\n</ul>\n<ul>\n<li>b</li>\n</ul>\n"
        );
        assert_eq!(
            to_html("Text\n- item"),
            "<p>Text</p>\n<ul>\n<li>item</li>\n</ul>\n"
        );
    }

    #[test]
    fn block_quotes_and_breaks() {
        assert_eq!(
            to_html("> quoted\ncontinued"),
            "<blockquote>\n<p>quoted\ncontinued</p>\n</blockquote>\n"
        );
        assert_eq!(to_html("***"), "<hr />\n");
        assert_eq!(to_html("a  \nb"), "<p>a<br />\nb</p>\n");
        assert_eq!(to_html("a\\\nb"), "<p>a<br />\nb</p>\n");
    }

    #[test]
    fn html_and_escaping() {
        assert_eq!(
            to_html("<table class=\"action\">\n<tr><td>x</td></tr>\n</table>\n\nAfter"),
            "<table class=\"action\">\n<tr><td>x</td></tr>\n</table>\n<p>After</p>\n"
        );
        assert_eq!(
            to_html("Thanks,<br>\nLaravel"),
            "<p>Thanks,<br>\nLaravel</p>\n"
        );
        assert_eq!(to_html("a < b & c > d"), "<p>a &lt; b &amp; c &gt; d</p>\n");
        assert_eq!(
            to_html("&copy; &amp; &#39; &hearts; &bogus"),
            "<p>© &amp; ' ♥ &amp;bogus</p>\n"
        );
        assert_eq!(to_html("\\*literal\\*"), "<p>*literal*</p>\n");
        assert_eq!(
            to_html("<!-- comment -->\ntext"),
            "<!-- comment -->\n<p>text</p>\n"
        );
    }

    #[test]
    fn tables() {
        let html = to_html(
            "| Laravel | Table |\n|:-------:|------:|\n| Col 2 is | Centered |\n| Col 3 is | `Right` |",
        );
        assert_eq!(
            html,
            "<table>\n<thead>\n<tr>\n<th align=\"center\">Laravel</th>\n<th align=\"right\">Table</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td align=\"center\">Col 2 is</td>\n<td align=\"right\">Centered</td>\n</tr>\n<tr>\n<td align=\"center\">Col 3 is</td>\n<td align=\"right\"><code>Right</code></td>\n</tr>\n</tbody>\n</table>\n"
        );
        assert_eq!(
            to_html("Intro\na | b\n--- | ---\n1 | 2\n\nafter"),
            "<p>Intro</p>\n<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>1</td>\n<td>2</td>\n</tr>\n</tbody>\n</table>\n<p>after</p>\n"
        );
        // A mismatched delimiter row is not a table.
        assert_eq!(
            to_html("a | b\n--- | --- | ---"),
            "<p>a | b\n--- | --- | ---</p>\n"
        );
    }
}
