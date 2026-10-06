//! Turning tokens into a tree of template nodes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use illuminate_support::Str;

use super::lexer::{Directives, RawAttr, Token, tokenize};
use super::{
    Attr, AttrPart, AttrValue, Branch, Case, ComponentName, ComponentNode, Cond, Extends,
    IncludeKind, JumpKind, Node, OnceKey, OutputDirective, SectionEnd, SlotBody, SlotNode,
    Template,
};
use crate::exception::ViewCompilationException;
use crate::expr::parser::{parse_for, parse_foreach};
use crate::expr::{Expr, Parser as ExprParser, Stmt};
use crate::registry::Registry;
use crate::value::ViewValue;

type PResult<T> = Result<T, ViewCompilationException>;

/// The built-in directives (lowercase; Blade matches them case-insensitively).
const BUILTIN: &[&str] = &[
    "if",
    "elseif",
    "else",
    "endif",
    "unless",
    "endunless",
    "isset",
    "endisset",
    "empty",
    "endempty",
    "auth",
    "elseauth",
    "endauth",
    "guest",
    "elseguest",
    "endguest",
    "env",
    "endenv",
    "production",
    "endproduction",
    "hassection",
    "sectionmissing",
    "hasstack",
    "switch",
    "case",
    "default",
    "endswitch",
    "once",
    "endonce",
    "error",
    "enderror",
    "session",
    "endsession",
    "can",
    "cannot",
    "canany",
    "elsecan",
    "elsecannot",
    "elsecanany",
    "endcan",
    "endcannot",
    "endcanany",
    "checked",
    "selected",
    "disabled",
    "readonly",
    "required",
    "bool",
    "class",
    "style",
    "for",
    "endfor",
    "foreach",
    "endforeach",
    "forelse",
    "endforelse",
    "while",
    "endwhile",
    "break",
    "continue",
    "extends",
    "extendsfirst",
    "section",
    "endsection",
    "show",
    "stop",
    "append",
    "overwrite",
    "yield",
    "parent",
    "push",
    "endpush",
    "pushonce",
    "endpushonce",
    "prepend",
    "endprepend",
    "prependonce",
    "endprependonce",
    "pushif",
    "elsepushif",
    "elsepush",
    "endpushif",
    "stack",
    "include",
    "includeif",
    "includewhen",
    "includeunless",
    "includefirst",
    "includeisolated",
    "each",
    "csrf",
    "method",
    "dd",
    "dump",
    "vite",
    "vitereactrefresh",
    "json",
    "js",
    "lang",
    "endlang",
    "choice",
    "inject",
    "php",
    "unset",
    "component",
    "endcomponent",
    "slot",
    "endslot",
    "props",
    "aware",
    "fragment",
    "endfragment",
    "use",
];

/// The `@end...` directives that close any conditional (they all compile to
/// `endif` in Laravel).
const IF_ENDS: &[&str] = &[
    "endif",
    "endunless",
    "endisset",
    "endempty",
    "endauth",
    "endguest",
    "endenv",
    "endproduction",
    "enderror",
    "endsession",
    "endcan",
    "endcannot",
    "endcanany",
];

static ONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct DirectiveSet<'a>(&'a Registry);

impl Directives for DirectiveSet<'_> {
    fn is_directive(&self, name: &str) -> bool {
        let registry = self.0;
        if registry.directives.contains_key(name) || registry.conditions.contains_key(name) {
            return true;
        }
        for prefix in ["unless", "else", "end"] {
            if let Some(rest) = name.strip_prefix(prefix)
                && registry.conditions.contains_key(rest)
            {
                return true;
            }
        }
        BUILTIN.contains(&name.to_ascii_lowercase().as_str())
    }
}

/// Parse a Blade template.
pub(crate) fn parse(src: &str, registry: &Registry) -> PResult<Template> {
    let tokens = tokenize(src, &DirectiveSet(registry))?;
    let mut parser = TemplateParser {
        tokens,
        pos: 0,
        registry,
        extends: Vec::new(),
        sections: Vec::new(),
    };
    let (mut nodes, stop) = parser.parse_nodes(&|_| false)?;
    debug_assert!(stop.is_none());
    if !parser.extends.is_empty()
        && let Some(Node::Text(text)) = nodes.first_mut()
    {
        let trimmed = text.trim_start_matches('\n').to_string();
        *text = trimmed;
        if text.is_empty() {
            nodes.remove(0);
        }
    }
    Ok(Template {
        nodes,
        extends: parser.extends,
    })
}

/// A token that ended a block.
struct Stop {
    name: String,
    args: Option<String>,
    line: usize,
}

struct TemplateParser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    registry: &'a Registry,
    extends: Vec<Extends>,
    sections: Vec<Arc<str>>,
}

fn push_node(nodes: &mut Vec<Node>, node: Node) {
    if let Node::Text(text) = &node {
        if text.is_empty() {
            return;
        }
        if let Some(Node::Text(previous)) = nodes.last_mut() {
            previous.push_str(text);
            return;
        }
    }
    nodes.push(node);
}

fn lower(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Parse a list of expressions from directive arguments.
fn args_list(args: &Option<String>, line: usize) -> PResult<Vec<Expr>> {
    match args {
        Some(src) => ExprParser::arguments(src, line),
        None => Ok(Vec::new()),
    }
}

fn required_expr(args: &Option<String>, line: usize, directive: &str) -> PResult<Expr> {
    match args {
        Some(src) if !src.trim().is_empty() => ExprParser::expression(src, line),
        _ => Err(ViewCompilationException::new(
            format!("The @{directive} directive requires an argument."),
            line,
        )),
    }
}

fn first_arg(args: &Option<String>, line: usize, directive: &str) -> PResult<Expr> {
    let mut list = args_list(args, line)?;
    if list.is_empty() {
        return Err(ViewCompilationException::new(
            format!("The @{directive} directive requires an argument."),
            line,
        ));
    }
    Ok(list.remove(0))
}

fn unclosed(directive: &str, end: &str, line: usize) -> ViewCompilationException {
    ViewCompilationException::new(
        format!("Unclosed @{directive} directive. Did you forget an @{end}?"),
        line,
    )
}

impl TemplateParser<'_> {
    fn next(&mut self) -> Option<Token> {
        let token = self
            .tokens
            .get_mut(self.pos)
            .map(|t| std::mem::replace(t, Token::Text(String::new())));
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    /// Parse nodes until a directive for which `stop` returns true.
    fn parse_nodes(&mut self, stop: &dyn Fn(&Stop) -> bool) -> PResult<(Vec<Node>, Option<Stop>)> {
        let mut nodes = Vec::new();
        while let Some(token) = self.next() {
            match token {
                Token::Text(text) => push_node(&mut nodes, Node::Text(text)),
                Token::Echo { src, escape, line } => {
                    let trimmed = src.trim_end();
                    let trimmed = trimmed.strip_suffix(';').unwrap_or(trimmed);
                    let expr = ExprParser::expression(trimmed, line)?;
                    nodes.push(Node::Echo { expr, escape, line });
                }
                Token::PhpBlock { src, line } => {
                    let stmts = ExprParser::statements(&src, line)?;
                    nodes.push(Node::Php { stmts, line });
                }
                Token::ComponentOpen {
                    name,
                    attrs,
                    self_closing,
                    line,
                } => {
                    let node = self.component(name, attrs, self_closing, line)?;
                    nodes.push(node);
                }
                Token::ComponentClose { name, line } => {
                    let candidate = Stop {
                        name: format!("</x-{name}>"),
                        args: None,
                        line,
                    };
                    if stop(&candidate) {
                        return Ok((nodes, Some(candidate)));
                    }
                    return Err(ViewCompilationException::new(
                        format!("Unexpected closing tag </x-{name}>."),
                        line,
                    ));
                }
                Token::SlotOpen {
                    inline_name,
                    attrs,
                    line,
                } => {
                    let node = self.slot_tag(inline_name, attrs, line)?;
                    nodes.push(node);
                }
                Token::SlotClose { line } => {
                    let candidate = Stop {
                        name: "</x-slot>".into(),
                        args: None,
                        line,
                    };
                    if stop(&candidate) {
                        return Ok((nodes, Some(candidate)));
                    }
                    return Err(ViewCompilationException::new(
                        "Unexpected closing tag </x-slot>.",
                        line,
                    ));
                }
                Token::Directive { name, args, line } => {
                    let candidate = Stop { name, args, line };
                    if stop(&candidate) {
                        return Ok((nodes, Some(candidate)));
                    }
                    if let Some(node) = self.directive(candidate)? {
                        push_node(&mut nodes, node);
                    }
                }
            }
        }
        Ok((nodes, None))
    }

    /// Parse a block that must end with one of `ends`.
    fn block(&mut self, directive: &str, ends: &[&str], line: usize) -> PResult<(Vec<Node>, Stop)> {
        let (body, stop) = self.parse_nodes(&|s| ends.contains(&lower(&s.name).as_str()))?;
        match stop {
            Some(stop) => Ok((body, stop)),
            None => Err(unclosed(directive, ends[0], line)),
        }
    }

    fn is_condition_name(&self, name: &str) -> bool {
        self.registry.conditions.contains_key(name)
    }

    fn directive(&mut self, stop: Stop) -> PResult<Option<Node>> {
        let Stop { name, args, line } = stop;

        // Custom directives and conditionals come first, case-sensitively.
        if self.registry.directives.contains_key(&name) {
            let args = args_list(&args, line)?;
            return Ok(Some(Node::Output {
                directive: OutputDirective::Custom(name),
                args,
                line,
            }));
        }
        if self.is_condition_name(&name) {
            let cond = Cond::Custom {
                name: name.clone(),
                args: args_list(&args, line)?,
                negate: false,
            };
            return self.if_chain(cond, &name, line).map(Some);
        }
        if let Some(rest) = name.strip_prefix("unless")
            && self.is_condition_name(rest)
        {
            let cond = Cond::Custom {
                name: rest.to_string(),
                args: args_list(&args, line)?,
                negate: true,
            };
            return self.if_chain(cond, &name, line).map(Some);
        }

        let output = |directive: OutputDirective, args: Vec<Expr>| {
            Ok(Some(Node::Output {
                directive,
                args,
                line,
            }))
        };
        let lowered = lower(&name);
        match lowered.as_str() {
            // Conditionals
            "if" => {
                let cond = Cond::Expr(required_expr(&args, line, "if")?);
                self.if_chain(cond, "if", line).map(Some)
            }
            "unless" => {
                let cond = Cond::Not(required_expr(&args, line, "unless")?);
                self.if_chain(cond, "unless", line).map(Some)
            }
            "isset" => {
                let cond = Cond::Isset(args_list(&args, line)?);
                self.if_chain(cond, "isset", line).map(Some)
            }
            "empty" if args.is_some() => {
                let cond = Cond::Empty(required_expr(&args, line, "empty")?);
                self.if_chain(cond, "empty", line).map(Some)
            }
            "auth" => {
                let cond = Cond::Auth(args_list(&args, line)?);
                self.if_chain(cond, "auth", line).map(Some)
            }
            "guest" => {
                let cond = Cond::Guest(args_list(&args, line)?);
                self.if_chain(cond, "guest", line).map(Some)
            }
            "env" => {
                let cond = Cond::Env(args_list(&args, line)?);
                self.if_chain(cond, "env", line).map(Some)
            }
            "production" => self
                .if_chain(Cond::Production, "production", line)
                .map(Some),
            "hassection" => {
                let cond = Cond::HasSection(first_arg(&args, line, "hasSection")?);
                self.if_chain(cond, "hasSection", line).map(Some)
            }
            "sectionmissing" => {
                let cond = Cond::SectionMissing(first_arg(&args, line, "sectionMissing")?);
                self.if_chain(cond, "sectionMissing", line).map(Some)
            }
            "hasstack" => {
                let cond = Cond::HasStack(first_arg(&args, line, "hasstack")?);
                self.if_chain(cond, "hasstack", line).map(Some)
            }
            "error" => {
                let cond = Cond::Error(args_list(&args, line)?);
                self.if_chain(cond, "error", line).map(Some)
            }
            "session" => {
                let cond = Cond::Session(args_list(&args, line)?);
                self.if_chain(cond, "session", line).map(Some)
            }
            "can" => {
                let cond = Cond::Can(args_list(&args, line)?);
                self.if_chain(cond, "can", line).map(Some)
            }
            "cannot" => {
                let cond = Cond::Cannot(args_list(&args, line)?);
                self.if_chain(cond, "cannot", line).map(Some)
            }
            "canany" => {
                let cond = Cond::CanAny(args_list(&args, line)?);
                self.if_chain(cond, "canany", line).map(Some)
            }
            "switch" => self.switch(&args, line).map(Some),

            // Loops
            "foreach" | "forelse" => {
                let src = args.as_deref().ok_or_else(|| {
                    ViewCompilationException::new(format!("Malformed @{name} statement."), line)
                })?;
                let (iterable, key, value) = parse_foreach(src, line).map_err(|e| {
                    if e.message.contains("Malformed") {
                        ViewCompilationException::new(format!("Malformed @{name} statement."), line)
                    } else {
                        e
                    }
                })?;
                if lowered == "foreach" {
                    let (body, _) = self.block("foreach", &["endforeach"], line)?;
                    Ok(Some(Node::Foreach {
                        iterable,
                        key,
                        value,
                        body,
                        empty: None,
                        line,
                    }))
                } else {
                    let (body, stop) = self.parse_nodes(&|s| {
                        let n = lower(&s.name);
                        (n == "empty" && s.args.is_none()) || n == "endforelse"
                    })?;
                    let stop = stop.ok_or_else(|| unclosed("forelse", "endforelse", line))?;
                    let empty = if lower(&stop.name) == "empty" {
                        let (empty, _) = self.block("forelse", &["endforelse"], line)?;
                        Some(empty)
                    } else {
                        None
                    };
                    Ok(Some(Node::Foreach {
                        iterable,
                        key,
                        value,
                        body,
                        empty,
                        line,
                    }))
                }
            }
            "for" => {
                let src = args.as_deref().ok_or_else(|| {
                    ViewCompilationException::new("Malformed @for statement.", line)
                })?;
                let (init, cond, step) = parse_for(src, line)?;
                let (body, _) = self.block("for", &["endfor"], line)?;
                Ok(Some(Node::For {
                    init,
                    cond,
                    step,
                    body,
                    line,
                }))
            }
            "while" => {
                let cond = required_expr(&args, line, "while")?;
                let (body, _) = self.block("while", &["endwhile"], line)?;
                Ok(Some(Node::While { cond, body, line }))
            }
            "break" | "continue" => {
                let kind = if lowered == "break" {
                    JumpKind::Break
                } else {
                    JumpKind::Continue
                };
                let (cond, levels) = match &args {
                    None => (None, 1),
                    Some(src) => match src.trim().parse::<i64>() {
                        Ok(levels) => (None, levels.max(1) as u32),
                        Err(_) => (Some(ExprParser::expression(src, line)?), 1),
                    },
                };
                Ok(Some(Node::Jump {
                    kind,
                    cond,
                    levels,
                    line,
                }))
            }

            // Includes
            "include" | "includeif" | "includewhen" | "includeunless" | "includefirst"
            | "includeisolated" => {
                let kind = match lowered.as_str() {
                    "include" => IncludeKind::Include,
                    "includeif" => IncludeKind::If,
                    "includewhen" => IncludeKind::When,
                    "includeunless" => IncludeKind::Unless,
                    "includefirst" => IncludeKind::First,
                    _ => IncludeKind::Isolated,
                };
                let args = args_list(&args, line)?;
                let required = if matches!(kind, IncludeKind::When | IncludeKind::Unless) {
                    2
                } else {
                    1
                };
                if args.len() < required {
                    return Err(ViewCompilationException::new(
                        format!("The @{name} directive requires a view name."),
                        line,
                    ));
                }
                Ok(Some(Node::Include { kind, args, line }))
            }
            "each" => {
                let args = args_list(&args, line)?;
                if args.len() < 3 {
                    return Err(ViewCompilationException::new(
                        "The @each directive requires a view, a collection and a variable name.",
                        line,
                    ));
                }
                Ok(Some(Node::Each { args, line }))
            }

            // Layouts
            "extends" | "extendsfirst" => {
                let args = args_list(&args, line)?;
                if args.is_empty() {
                    return Err(ViewCompilationException::new(
                        format!("The @{name} directive requires a view name."),
                        line,
                    ));
                }
                self.extends.push(Extends {
                    args,
                    first: lowered == "extendsfirst",
                    line,
                });
                Ok(None)
            }
            "section" => {
                let mut list = args_list(&args, line)?;
                if list.is_empty() {
                    return Err(ViewCompilationException::new(
                        "The @section directive requires a name.",
                        line,
                    ));
                }
                if list.len() >= 2 {
                    let content = list.remove(1);
                    let name = list.remove(0);
                    return Ok(Some(Node::SectionInline {
                        name,
                        content,
                        line,
                    }));
                }
                let name = list.remove(0);
                let section_name: Arc<str> = match &name {
                    Expr::Lit(ViewValue::Str(s)) => s.clone(),
                    _ => args
                        .as_deref()
                        .unwrap_or("")
                        .trim()
                        .trim_matches(['\'', '"'])
                        .into(),
                };
                self.sections.push(section_name);
                let result = self.block(
                    "section",
                    &["endsection", "stop", "show", "append", "overwrite"],
                    line,
                );
                self.sections.pop();
                let (body, stop) = result?;
                let end = match lower(&stop.name).as_str() {
                    "show" => SectionEnd::Show,
                    "append" => SectionEnd::Append,
                    "overwrite" => SectionEnd::Overwrite,
                    _ => SectionEnd::Stop,
                };
                Ok(Some(Node::Section {
                    name,
                    body,
                    end,
                    line,
                }))
            }
            "yield" => {
                let args = args_list(&args, line)?;
                if args.is_empty() {
                    return Err(ViewCompilationException::new(
                        "The @yield directive requires a section name.",
                        line,
                    ));
                }
                Ok(Some(Node::Yield { args, line }))
            }
            "parent" => {
                let section = self.sections.last().cloned().unwrap_or_else(|| "".into());
                Ok(Some(Node::Parent { section }))
            }
            "endsection" | "stop" | "show" | "append" | "overwrite" => {
                Err(ViewCompilationException::new(
                    "Cannot end a section without first starting one.",
                    line,
                ))
            }

            // Stacks
            "push" | "prepend" | "pushonce" | "prependonce" => {
                let mut list = args_list(&args, line)?;
                if list.is_empty() {
                    return Err(ViewCompilationException::new(
                        format!("The @{name} directive requires a stack name."),
                        line,
                    ));
                }
                let stack = list.remove(0);
                let once = if lowered.ends_with("once") {
                    Some(match list.into_iter().next() {
                        Some(id) => OnceKey::Explicit(id),
                        None => generated_once_key(),
                    })
                } else {
                    None
                };
                let end = format!("end{lowered}");
                let (body, _) = self.block(&name, &[end.as_str()], line)?;
                Ok(Some(Node::Push {
                    prepend: lowered.starts_with("prepend"),
                    stack,
                    once,
                    body,
                    line,
                }))
            }
            "pushif" => self.push_if(&args, line).map(Some),
            "stack" => {
                let args = args_list(&args, line)?;
                if args.is_empty() {
                    return Err(ViewCompilationException::new(
                        "The @stack directive requires a stack name.",
                        line,
                    ));
                }
                Ok(Some(Node::Stack { args, line }))
            }
            "once" => {
                let key = match args_list(&args, line)?.into_iter().next() {
                    Some(id) => OnceKey::Explicit(id),
                    None => generated_once_key(),
                };
                let (body, _) = self.block("once", &["endonce"], line)?;
                Ok(Some(Node::Once { key, body, line }))
            }

            // Components
            "props" => Ok(Some(Node::Props {
                expr: required_expr(&args, line, "props")?,
                line,
            })),
            "aware" => Ok(Some(Node::Aware {
                expr: required_expr(&args, line, "aware")?,
                line,
            })),
            "component" => {
                let mut list = args_list(&args, line)?;
                if list.is_empty() {
                    return Err(ViewCompilationException::new(
                        "The @component directive requires a view name.",
                        line,
                    ));
                }
                let view = list.remove(0);
                let data = list.into_iter().next();
                let (children, _) = self.block("component", &["endcomponent"], line)?;
                Ok(Some(Node::Component(Box::new(ComponentNode {
                    name: ComponentName::Legacy { view, data },
                    attrs: Vec::new(),
                    children,
                    line,
                }))))
            }
            "slot" => {
                let mut list = args_list(&args, line)?;
                if list.is_empty() {
                    return Err(ViewCompilationException::new(
                        "The @slot directive requires a name.",
                        line,
                    ));
                }
                let name = list.remove(0);
                let content = if list.is_empty() {
                    None
                } else {
                    Some(list.remove(0))
                };
                match content {
                    Some(content) if !matches!(content, Expr::Lit(ViewValue::Null)) => {
                        Ok(Some(Node::Slot(Box::new(SlotNode {
                            name,
                            attrs: Vec::new(),
                            body: SlotBody::Inline(content),
                            line,
                        }))))
                    }
                    _ => {
                        let (body, _) = self.block("slot", &["endslot"], line)?;
                        Ok(Some(Node::Slot(Box::new(SlotNode {
                            name,
                            attrs: Vec::new(),
                            body: SlotBody::Nodes(body),
                            line,
                        }))))
                    }
                }
            }
            "fragment" => {
                let name = first_arg(&args, line, "fragment")?;
                let (body, _) = self.block("fragment", &["endfragment"], line)?;
                Ok(Some(Node::Fragment { name, body, line }))
            }

            // Raw PHP
            "php" => {
                let src = args.as_deref().unwrap_or("");
                let stmts = ExprParser::statements(src, line)?;
                Ok(Some(Node::Php { stmts, line }))
            }
            "unset" => {
                let targets = args_list(&args, line)?;
                Ok(Some(Node::Php {
                    stmts: vec![Stmt::Unset(targets)],
                    line,
                }))
            }
            "use" => Ok(None),

            // Output
            "csrf" => output(OutputDirective::Csrf, Vec::new()),
            "method" => output(OutputDirective::Method, args_list(&args, line)?),
            "json" => output(OutputDirective::Json, args_list(&args, line)?),
            "js" => output(OutputDirective::Js, args_list(&args, line)?),
            "class" => output(OutputDirective::Class, args_list(&args, line)?),
            "style" => output(OutputDirective::Style, args_list(&args, line)?),
            "checked" => output(OutputDirective::Checked, args_list(&args, line)?),
            "selected" => output(OutputDirective::Selected, args_list(&args, line)?),
            "disabled" => output(OutputDirective::Disabled, args_list(&args, line)?),
            "readonly" => output(OutputDirective::Readonly, args_list(&args, line)?),
            "required" => output(OutputDirective::Required, args_list(&args, line)?),
            "bool" => output(OutputDirective::Bool, args_list(&args, line)?),
            "lang" if args.is_none() => {
                let (body, _) = self.block("lang", &["endlang"], line)?;
                Ok(Some(Node::LangBlock {
                    args: Vec::new(),
                    body,
                    line,
                }))
            }
            "lang" => output(OutputDirective::Lang, args_list(&args, line)?),
            "choice" => output(OutputDirective::Choice, args_list(&args, line)?),
            "dump" => output(OutputDirective::Dump, args_list(&args, line)?),
            "dd" => output(OutputDirective::Dd, args_list(&args, line)?),
            "vite" => output(OutputDirective::Vite, args_list(&args, line)?),
            "vitereactrefresh" => output(OutputDirective::ViteReactRefresh, Vec::new()),
            "inject" => {
                let list = args_list(&args, line)?;
                if list.len() < 2 {
                    return Err(ViewCompilationException::new(
                        "The @inject directive requires a variable name and a service.",
                        line,
                    ));
                }
                output(OutputDirective::Inject, list)
            }

            // Anything else here is out of place.
            _ => Err(ViewCompilationException::new(
                format!("Unexpected @{name} directive."),
                line,
            )),
        }
    }

    fn if_chain(&mut self, first: Cond, directive: &str, line: usize) -> PResult<Node> {
        let mut branches = Vec::new();
        let mut cond = first;
        let mut cond_line = line;
        loop {
            let registry = self.registry;
            let (body, stop) = self.parse_nodes(&|s| {
                is_if_end(registry, &s.name) || is_else_branch(registry, &s.name)
            })?;
            branches.push(Branch {
                cond,
                body,
                line: cond_line,
            });
            let Some(stop) = stop else {
                let end = match directive {
                    "if" | "unless" | "isset" | "empty" | "auth" | "guest" | "env"
                    | "production" | "error" | "session" | "can" | "cannot" | "canany" => {
                        format!("end{directive}")
                    }
                    "hasSection" | "sectionMissing" | "hasstack" => "endif".to_string(),
                    custom => format!("end{}", custom.strip_prefix("unless").unwrap_or(custom)),
                };
                return Err(unclosed(directive, &end, line));
            };
            if is_if_end(registry, &stop.name) {
                return Ok(Node::If {
                    branches,
                    otherwise: None,
                });
            }
            let lowered = lower(&stop.name);
            if lowered == "else" {
                let (body, end) = self.parse_nodes(&|s| {
                    is_if_end(registry, &s.name) || is_else_branch(registry, &s.name)
                })?;
                match end {
                    Some(end) if is_if_end(registry, &end.name) => {
                        return Ok(Node::If {
                            branches,
                            otherwise: Some(body),
                        });
                    }
                    Some(end) => {
                        return Err(ViewCompilationException::new(
                            format!("Unexpected @{} after @else.", end.name),
                            end.line,
                        ));
                    }
                    None => return Err(unclosed(directive, "endif", line)),
                }
            }
            cond_line = stop.line;
            cond = match lowered.as_str() {
                "elseif" => Cond::Expr(required_expr(&stop.args, stop.line, "elseif")?),
                "elseauth" => Cond::Auth(args_list(&stop.args, stop.line)?),
                "elseguest" => Cond::Guest(args_list(&stop.args, stop.line)?),
                "elsecan" => Cond::Can(args_list(&stop.args, stop.line)?),
                "elsecannot" => Cond::Cannot(args_list(&stop.args, stop.line)?),
                "elsecanany" => Cond::CanAny(args_list(&stop.args, stop.line)?),
                _ => {
                    let name = stop
                        .name
                        .strip_prefix("else")
                        .unwrap_or(&stop.name)
                        .to_string();
                    Cond::Custom {
                        name,
                        args: args_list(&stop.args, stop.line)?,
                        negate: false,
                    }
                }
            };
        }
    }

    fn switch(&mut self, args: &Option<String>, line: usize) -> PResult<Node> {
        let subject = required_expr(args, line, "switch")?;
        let is_case =
            |s: &Stop| matches!(lower(&s.name).as_str(), "case" | "default" | "endswitch");
        let (preamble, mut stop) = self.parse_nodes(&is_case)?;
        if preamble
            .iter()
            .any(|n| !matches!(n, Node::Text(t) if t.trim().is_empty()))
        {
            return Err(ViewCompilationException::new(
                "Unexpected content between @switch and the first @case.",
                line,
            ));
        }
        let mut cases = Vec::new();
        loop {
            let Some(current) = stop else {
                return Err(unclosed("switch", "endswitch", line));
            };
            let test = match lower(&current.name).as_str() {
                "endswitch" => break,
                "case" => Some(required_expr(&current.args, current.line, "case")?),
                _ => None,
            };
            let (body, next) = self.parse_nodes(&is_case)?;
            cases.push(Case { test, body });
            stop = next;
        }
        Ok(Node::Switch {
            subject,
            cases,
            line,
        })
    }

    fn push_if(&mut self, args: &Option<String>, line: usize) -> PResult<Node> {
        let mut branches = Vec::new();
        let mut otherwise = None;
        let mut current_args = args_list(args, line)?;
        let mut current_line = line;
        loop {
            if current_args.len() < 2 {
                return Err(ViewCompilationException::new(
                    "The @pushIf directive requires a condition and a stack name.",
                    current_line,
                ));
            }
            let stack = current_args.pop().expect("two arguments");
            let cond = current_args.remove(0);
            let (body, stop) = self.parse_nodes(&|s| {
                matches!(
                    lower(&s.name).as_str(),
                    "elsepushif" | "elsepush" | "endpushif"
                )
            })?;
            branches.push((cond, stack, body));
            let stop = stop.ok_or_else(|| unclosed("pushIf", "endPushIf", line))?;
            match lower(&stop.name).as_str() {
                "endpushif" => break,
                "elsepushif" => {
                    current_args = args_list(&stop.args, stop.line)?;
                    current_line = stop.line;
                }
                _ => {
                    let stack = first_arg(&stop.args, stop.line, "elsePush")?;
                    let (body, _) = self.block("pushIf", &["endpushif"], line)?;
                    otherwise = Some((stack, body));
                    break;
                }
            }
        }
        Ok(Node::PushIf {
            branches,
            otherwise,
            line,
        })
    }

    // ------------------------------------------------------------------
    // Components
    // ------------------------------------------------------------------

    fn component(
        &mut self,
        name: String,
        attrs: Vec<RawAttr>,
        self_closing: bool,
        line: usize,
    ) -> PResult<Node> {
        let attrs = convert_attrs(attrs)?;
        let children = if self_closing {
            Vec::new()
        } else {
            let close = format!("</x-{name}>");
            let (children, stop) =
                self.parse_nodes(&|s| s.name.starts_with("</x-") && s.name != "</x-slot>")?;
            match stop {
                Some(stop) if stop.name == close => children,
                Some(stop) => {
                    return Err(ViewCompilationException::new(
                        format!("Unexpected closing tag {}, expected {close}.", stop.name),
                        stop.line,
                    ));
                }
                None => {
                    return Err(ViewCompilationException::new(
                        format!("Unclosed component <x-{name}>. Did you forget {close}?"),
                        line,
                    ));
                }
            }
        };
        let name = if name == "dynamic-component" {
            ComponentName::Dynamic
        } else {
            ComponentName::Static(name)
        };
        Ok(Node::Component(Box::new(ComponentNode {
            name,
            attrs,
            children,
            line,
        })))
    }

    fn slot_tag(
        &mut self,
        inline_name: Option<String>,
        attrs: Vec<RawAttr>,
        line: usize,
    ) -> PResult<Node> {
        let mut attrs = attrs;
        let name = match inline_name {
            Some(inline) => {
                let name = if inline.contains('-') {
                    Str::camel(&inline)
                } else {
                    inline
                };
                Expr::Lit(ViewValue::from(name))
            }
            None => {
                if let Some(index) = attrs.iter().position(|a| a.name == "name") {
                    let attr = attrs.remove(index);
                    Expr::Lit(ViewValue::from(attr.value.unwrap_or_default()))
                } else if let Some(index) = attrs.iter().position(|a| a.name == ":name") {
                    let attr = attrs.remove(index);
                    ExprParser::expression(attr.value.as_deref().unwrap_or("null"), attr.line)?
                } else {
                    Expr::Lit(ViewValue::from("slot"))
                }
            }
        };
        let attrs = convert_attrs(attrs)?;
        let (body, stop) = self.parse_nodes(&|s| s.name == "</x-slot>")?;
        if stop.is_none() {
            return Err(ViewCompilationException::new(
                "Unclosed <x-slot>. Did you forget </x-slot>?",
                line,
            ));
        }
        Ok(Node::Slot(Box::new(SlotNode {
            name,
            attrs,
            body: SlotBody::Nodes(body),
            line,
        })))
    }
}

fn is_if_end(registry: &Registry, name: &str) -> bool {
    IF_ENDS.contains(&lower(name).as_str())
        || name
            .strip_prefix("end")
            .is_some_and(|rest| registry.conditions.contains_key(rest))
}

fn is_else_branch(registry: &Registry, name: &str) -> bool {
    matches!(
        lower(name).as_str(),
        "else" | "elseif" | "elseauth" | "elseguest" | "elsecan" | "elsecannot" | "elsecanany"
    ) || name
        .strip_prefix("else")
        .is_some_and(|rest| registry.conditions.contains_key(rest))
}

fn generated_once_key() -> OnceKey {
    OnceKey::Generated(format!(
        "__once_{}",
        ONCE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Convert raw tag attributes into component attributes.
fn convert_attrs(raw: Vec<RawAttr>) -> PResult<Vec<Attr>> {
    let mut attrs = Vec::with_capacity(raw.len());
    for attr in raw {
        let RawAttr { name, value, line } = attr;
        if let Some(escaped) = name.strip_prefix("::") {
            let value = match value {
                Some(v) => AttrValue::Static(vec![AttrPart::Text(v)]),
                None => AttrValue::True,
            };
            attrs.push(Attr {
                name: format!(":{escaped}"),
                value,
            });
            continue;
        }
        if let Some(bound) = name.strip_prefix(':') {
            match value {
                Some(src) => {
                    let expr = ExprParser::expression(&src, line)?;
                    attrs.push(Attr {
                        name: bound.to_string(),
                        value: AttrValue::Bound(expr),
                    });
                }
                None => attrs.push(Attr {
                    name,
                    value: AttrValue::True,
                }),
            }
            continue;
        }
        let value = match value {
            None => AttrValue::True,
            Some(src) => AttrValue::Static(attribute_parts(&src, line)?),
        };
        attrs.push(Attr { name, value });
    }
    Ok(attrs)
}

/// Split a static attribute value into text and `{{ }}` echoes.
fn attribute_parts(src: &str, line: usize) -> PResult<Vec<AttrPart>> {
    let mut parts = Vec::new();
    let mut rest = src;
    let mut line = line;
    loop {
        let Some(start) = rest.find("{{").into_iter().chain(rest.find("{!!")).min() else {
            if !rest.is_empty() {
                parts.push(AttrPart::Text(rest.to_string()));
            }
            break;
        };
        let escaped_echo = start > 0 && rest.as_bytes()[start - 1] == b'@';
        let (open, close, escape) = if rest[start..].starts_with("{!!") {
            ("{!!", "!!}", false)
        } else {
            ("{{", "}}", true)
        };
        let Some(end) = super::lexer::find_close(&rest[start + open.len()..], close) else {
            parts.push(AttrPart::Text(rest.to_string()));
            break;
        };
        let body_start = start + open.len();
        let body = &rest[body_start..body_start + end];
        if escaped_echo {
            parts.push(AttrPart::Text(format!(
                "{}{}",
                &rest[..start - 1],
                &rest[start..body_start + end + close.len()]
            )));
        } else {
            if start > 0 {
                parts.push(AttrPart::Text(rest[..start].to_string()));
            }
            line += rest[..start].bytes().filter(|b| *b == b'\n').count();
            let trimmed = body.trim_end();
            let trimmed = trimmed.strip_suffix(';').unwrap_or(trimmed);
            parts.push(AttrPart::Echo {
                expr: ExprParser::expression(trimmed, line)?,
                escape,
            });
        }
        line += rest[start..body_start + end]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        rest = &rest[body_start + end + close.len()..];
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Template {
        parse(src, &Registry::default()).unwrap()
    }

    fn parse_err(src: &str) -> ViewCompilationException {
        parse(src, &Registry::default()).unwrap_err()
    }

    #[test]
    fn it_parses_conditionals() {
        let template = parse_ok("@if($a) A @elseif($b) B @else C @endif");
        let Node::If {
            branches,
            otherwise,
            ..
        } = &template.nodes[0]
        else {
            panic!()
        };
        assert_eq!(branches.len(), 2);
        assert!(otherwise.is_some());

        let template =
            parse_ok("@isset($a) A @endisset @auth B @else C @endauth @hasSection('x') D @endif");
        assert_eq!(
            template
                .nodes
                .iter()
                .filter(|n| matches!(n, Node::If { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn it_parses_loops() {
        let template = parse_ok("@forelse($users as $user) {{ $user }} @empty none @endforelse");
        let Node::Foreach { empty, .. } = &template.nodes[0] else {
            panic!()
        };
        assert!(empty.is_some());
        let template = parse_ok("@foreach($a as $k => $v) @continue($k == 1) @break @endforeach");
        assert!(matches!(
            &template.nodes[0],
            Node::Foreach { key: Some(_), .. }
        ));
    }

    #[test]
    fn it_reports_unclosed_blocks_with_line_numbers() {
        let error = parse_err("<div>\n@if($a)\nA\n");
        assert_eq!(error.line, 2);
        assert_eq!(
            error.message,
            "Unclosed @if directive. Did you forget an @endif?"
        );

        let error = parse_err("@foreach($a as $b)\n@endif");
        assert_eq!(error.line, 2);
        assert_eq!(error.message, "Unexpected @endif directive.");

        let error = parse_err("@foreach($a as $b)\nx");
        assert_eq!(error.line, 1);
        assert_eq!(
            error.message,
            "Unclosed @foreach directive. Did you forget an @endforeach?"
        );

        let error = parse_err("one\ntwo\n@endforeach");
        assert_eq!(error.line, 3);
        assert_eq!(error.message, "Unexpected @endforeach directive.");

        let error = parse_err("line 1\n{{ $a + }}");
        assert_eq!(error.line, 2);

        let error = parse_err("@foreach($users)\n@endforeach");
        assert_eq!(error.message, "Malformed @foreach statement.");
    }

    #[test]
    fn it_parses_components_and_slots() {
        let template = parse_ok(
            "<x-card class=\"p-{{ $size }}\" :title=\"$t\" ::class=\"{ a: b }\" disabled>\n<x-slot:footer>F</x-slot>\nBody\n</x-card>",
        );
        let Node::Component(component) = &template.nodes[0] else {
            panic!("{:?}", template.nodes)
        };
        assert_eq!(component.attrs.len(), 4);
        assert!(matches!(component.attrs[1].value, AttrValue::Bound(_)));
        assert_eq!(component.attrs[2].name, ":class");
        assert!(
            component
                .children
                .iter()
                .any(|n| matches!(n, Node::Slot(_)))
        );

        let error = parse_err("<x-card>\n<x-alert>\n</x-card>");
        assert_eq!(error.line, 3);
    }

    #[test]
    fn extends_trims_leading_newlines() {
        let template = parse_ok("@extends('layout')\n\n@section('a', 'b')\n\nX");
        assert_eq!(template.extends.len(), 1);
        assert!(matches!(&template.nodes[0], Node::SectionInline { .. }));
    }
}
