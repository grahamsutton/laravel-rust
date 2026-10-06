//! Parsing PHP-flavored expressions with a Pratt parser.

use std::sync::Arc;

use super::lexer::{InterpPart, Tok, tokenize};
use super::{Arg, ArrayItem, BinaryOp, ClosureDef, Expr, MatchArm, Param, Stmt, UnaryOp};
use crate::exception::ViewCompilationException;
use crate::value::ViewValue;

type PResult<T> = Result<T, ViewCompilationException>;

/// Binding power of assignment's right-hand side.
const ASSIGN_BP: u8 = 7;
/// Binding power of the operand of `!`.
const NOT_BP: u8 = 37;
/// Binding power of the operand of unary minus, casts and `@`.
const UNARY_BP: u8 = 39;

/// Parses expressions, argument lists and statements.
pub(crate) struct Parser<'a> {
    src: &'a str,
    tokens: Vec<(Tok, usize)>,
    pos: usize,
    line: usize,
}

impl<'a> Parser<'a> {
    /// Create a parser for the given source, which starts on `line`.
    pub(crate) fn new(src: &'a str, line: usize) -> PResult<Self> {
        let tokens = tokenize(src).map_err(|(message, offset)| {
            ViewCompilationException::new(
                message,
                line + count_lines(&src[..offset.min(src.len())]),
            )
        })?;
        Ok(Self {
            src,
            tokens,
            pos: 0,
            line,
        })
    }

    /// Parse a single, complete expression.
    pub(crate) fn expression(src: &str, line: usize) -> PResult<Expr> {
        let mut parser = Parser::new(src, line)?;
        let expr = parser.parse_expr(0)?;
        parser.expect_eof()?;
        Ok(expr)
    }

    /// Parse a comma separated list of expressions (directive arguments).
    pub(crate) fn arguments(src: &str, line: usize) -> PResult<Vec<Expr>> {
        let mut parser = Parser::new(src, line)?;
        let mut args = Vec::new();
        if parser.at_eof() {
            return Ok(args);
        }
        loop {
            args.push(parser.parse_expr(0)?);
            if parser.eat_op(",") {
                if parser.at_eof() {
                    break;
                }
                continue;
            }
            break;
        }
        parser.expect_eof()?;
        Ok(args)
    }

    /// Parse a block of statements (`@php ... @endphp`).
    pub(crate) fn statements(src: &str, line: usize) -> PResult<Vec<Stmt>> {
        let mut parser = Parser::new(src, line)?;
        let mut stmts = Vec::new();
        while !parser.at_eof() {
            if parser.eat_op(";") {
                continue;
            }
            stmts.push(parser.parse_statement()?);
        }
        Ok(stmts)
    }

    // ------------------------------------------------------------------
    // Token helpers
    // ------------------------------------------------------------------

    pub(crate) fn peek(&self) -> &Tok {
        &self.tokens[self.pos].0
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let index = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[index].0
    }

    fn advance(&mut self) -> Tok {
        let token = self.tokens[self.pos].0.clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    pub(crate) fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn is_keyword(&self, keyword: &str) -> bool {
        matches!(self.peek(), Tok::Ident(name) if name.eq_ignore_ascii_case(keyword))
    }

    pub(crate) fn eat_keyword(&mut self, keyword: &str) -> bool {
        if self.is_keyword(keyword) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> PResult<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("expecting \"{op}\"")))
        }
    }

    pub(crate) fn expect_eof(&self) -> PResult<()> {
        if self.at_eof() {
            Ok(())
        } else {
            Err(self.unexpected("expecting end of expression"))
        }
    }

    fn current_line(&self) -> usize {
        let offset = self.tokens[self.pos].1.min(self.src.len());
        self.line + count_lines(&self.src[..offset])
    }

    pub(crate) fn error(&self, message: impl Into<String>) -> ViewCompilationException {
        ViewCompilationException::new(message, self.current_line())
    }

    fn unexpected(&self, expecting: &str) -> ViewCompilationException {
        let found = match self.peek() {
            Tok::Eof => "end of expression".to_string(),
            Tok::Var(name) => format!("variable \"${name}\""),
            Tok::Ident(name) => format!("identifier \"{name}\""),
            Tok::Int(i) => format!("integer \"{i}\""),
            Tok::Float(f) => format!("number \"{f}\""),
            Tok::Str(_) | Tok::Interp(_) => "string".to_string(),
            Tok::Cast(_) => "cast".to_string(),
            Tok::Op(op) => format!("token \"{op}\""),
        };
        self.error(format!("syntax error, unexpected {found}, {expecting}"))
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    /// Parse an expression whose operators bind at least as tightly as `min_bp`.
    pub(crate) fn parse_expr(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut left = self.parse_unary()?;
        while let Some((lbp, rbp, op)) = self.infix() {
            if lbp < min_bp {
                break;
            }
            self.advance();
            left = match op {
                Infix::Ternary => {
                    if self.eat_op(":") {
                        let otherwise = self.parse_expr(rbp)?;
                        Expr::Ternary {
                            cond: Box::new(left),
                            then: None,
                            otherwise: Box::new(otherwise),
                        }
                    } else {
                        let then = self.parse_expr(0)?;
                        self.expect_op(":")?;
                        let otherwise = self.parse_expr(rbp)?;
                        Expr::Ternary {
                            cond: Box::new(left),
                            then: Some(Box::new(then)),
                            otherwise: Box::new(otherwise),
                        }
                    }
                }
                Infix::Binary(op) => {
                    let right = self.parse_expr(rbp)?;
                    Expr::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    }
                }
            };
        }
        Ok(left)
    }

    fn infix(&self) -> Option<(u8, u8, Infix)> {
        use BinaryOp::*;
        let left = |bp: u8, op: BinaryOp| Some((bp, bp + 1, Infix::Binary(op)));
        let right = |bp: u8, op: BinaryOp| Some((bp, bp, Infix::Binary(op)));
        match self.peek() {
            Tok::Ident(name) => match name.to_ascii_lowercase().as_str() {
                "or" => left(1, Or),
                "xor" => left(3, Xor),
                "and" => left(5, And),
                _ => None,
            },
            Tok::Op(op) => match *op {
                "?" => Some((9, 10, Infix::Ternary)),
                "??" => right(11, Coalesce),
                "||" => left(13, Or),
                "&&" => left(15, And),
                "|" => left(17, BitOr),
                "^" => left(19, BitXor),
                "&" => left(21, BitAnd),
                "==" => left(23, Eq),
                "!=" | "<>" => left(23, NotEq),
                "===" => left(23, Identical),
                "!==" => left(23, NotIdentical),
                "<=>" => left(23, Spaceship),
                "<" => left(25, Lt),
                "<=" => left(25, Le),
                ">" => left(25, Gt),
                ">=" => left(25, Ge),
                "." => left(27, Concat),
                "<<" => left(29, Shl),
                ">>" => left(29, Shr),
                "+" => left(31, Add),
                "-" => left(31, Sub),
                "*" => left(33, Mul),
                "/" => left(33, Div),
                "%" => left(33, Mod),
                "**" => right(41, Pow),
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let unary = |op: UnaryOp, expr: Expr| Expr::Unary {
            op,
            expr: Box::new(expr),
        };
        match self.peek().clone() {
            Tok::Op("!") => {
                self.advance();
                Ok(unary(UnaryOp::Not, self.parse_expr(NOT_BP)?))
            }
            Tok::Ident(name)
                if name.eq_ignore_ascii_case("not")
                    && !matches!(self.peek_at(1), Tok::Op("(" | "::")) =>
            {
                self.advance();
                Ok(unary(UnaryOp::Not, self.parse_expr(NOT_BP)?))
            }
            Tok::Op("-") => {
                self.advance();
                Ok(unary(UnaryOp::Neg, self.parse_expr(UNARY_BP)?))
            }
            Tok::Op("+") => {
                self.advance();
                Ok(unary(UnaryOp::Plus, self.parse_expr(UNARY_BP)?))
            }
            Tok::Op("~") => {
                self.advance();
                Ok(unary(UnaryOp::BitNot, self.parse_expr(UNARY_BP)?))
            }
            Tok::Op("@") => {
                self.advance();
                Ok(unary(UnaryOp::Silence, self.parse_expr(UNARY_BP)?))
            }
            Tok::Cast(ty) => {
                self.advance();
                Ok(Expr::Cast {
                    ty,
                    expr: Box::new(self.parse_expr(UNARY_BP)?),
                })
            }
            Tok::Op(op @ ("++" | "--")) => {
                self.advance();
                let target = self.parse_postfix()?;
                if !target.is_assignable() {
                    return Err(
                        self.error(format!("syntax error, cannot use {op} on a non-variable"))
                    );
                }
                Ok(Expr::IncDec {
                    target: Box::new(target),
                    increment: op == "++",
                    prefix: true,
                })
            }
            _ => {
                let expr = self.parse_postfix()?;
                self.parse_assignment(expr)
            }
        }
    }

    fn parse_assignment(&mut self, target: Expr) -> PResult<Expr> {
        let op = match self.peek() {
            Tok::Op("=") => None,
            Tok::Op("+=") => Some(BinaryOp::Add),
            Tok::Op("-=") => Some(BinaryOp::Sub),
            Tok::Op("*=") => Some(BinaryOp::Mul),
            Tok::Op("/=") => Some(BinaryOp::Div),
            Tok::Op("%=") => Some(BinaryOp::Mod),
            Tok::Op("**=") => Some(BinaryOp::Pow),
            Tok::Op(".=") => Some(BinaryOp::Concat),
            Tok::Op("??=") => Some(BinaryOp::Coalesce),
            Tok::Op("|=") => Some(BinaryOp::BitOr),
            Tok::Op("&=") => Some(BinaryOp::BitAnd),
            Tok::Op("^=") => Some(BinaryOp::BitXor),
            Tok::Op("<<=") => Some(BinaryOp::Shl),
            Tok::Op(">>=") => Some(BinaryOp::Shr),
            Tok::Op("++") | Tok::Op("--") if target.is_assignable() => {
                let increment = self.advance() == Tok::Op("++");
                return Ok(Expr::IncDec {
                    target: Box::new(target),
                    increment,
                    prefix: false,
                });
            }
            _ => return Ok(target),
        };
        if !target.is_assignable() {
            return Err(self.error("syntax error, cannot assign to this expression"));
        }
        self.advance();
        let value = self.parse_expr(ASSIGN_BP)?;
        Ok(Expr::Assign {
            target: Box::new(target),
            op,
            value: Box::new(value),
        })
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().clone() {
                Tok::Op(op @ ("->" | "?->")) => {
                    self.advance();
                    let nullsafe = op == "?->";
                    match self.advance() {
                        Tok::Ident(name) => {
                            if self.is_op("(") {
                                let args = self.parse_args()?;
                                expr = Expr::MethodCall {
                                    target: Box::new(expr),
                                    name,
                                    args,
                                    nullsafe,
                                };
                            } else {
                                expr = Expr::Prop {
                                    target: Box::new(expr),
                                    name: Box::new(Expr::Lit(ViewValue::Str(name))),
                                    nullsafe,
                                };
                            }
                        }
                        Tok::Var(name) => {
                            expr = Expr::Prop {
                                target: Box::new(expr),
                                name: Box::new(Expr::Var(name)),
                                nullsafe,
                            };
                        }
                        Tok::Op("{") => {
                            let name = self.parse_expr(0)?;
                            self.expect_op("}")?;
                            expr = Expr::Prop {
                                target: Box::new(expr),
                                name: Box::new(name),
                                nullsafe,
                            };
                        }
                        _ => {
                            self.pos -= 1;
                            return Err(self.unexpected("expecting a property or method name"));
                        }
                    }
                }
                Tok::Op("[") => {
                    self.advance();
                    if self.eat_op("]") {
                        expr = Expr::Index {
                            target: Box::new(expr),
                            index: None,
                        };
                    } else {
                        let index = self.parse_expr(0)?;
                        self.expect_op("]")?;
                        expr = Expr::Index {
                            target: Box::new(expr),
                            index: Some(Box::new(index)),
                        };
                    }
                }
                Tok::Op("(")
                    if matches!(
                        expr,
                        Expr::Var(_)
                            | Expr::Prop { .. }
                            | Expr::Index { .. }
                            | Expr::Closure(_)
                            | Expr::Call { .. }
                            | Expr::MethodCall { .. }
                            | Expr::Invoke { .. }
                    ) =>
                {
                    let args = self.parse_args()?;
                    expr = Expr::Invoke {
                        callee: Box::new(expr),
                        args,
                    };
                }
                Tok::Op("::") if matches!(expr, Expr::Var(_)) => {
                    self.advance();
                    // `$class::method()` — treat the variable's value as the class.
                    let Tok::Ident(method) = self.advance() else {
                        return Err(self.unexpected("expecting a method name"));
                    };
                    let args = self.parse_args()?;
                    expr = Expr::MethodCall {
                        target: Box::new(expr),
                        name: method,
                        args,
                        nullsafe: false,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_args(&mut self) -> PResult<Vec<Arg>> {
        self.expect_op("(")?;
        let mut args = Vec::new();
        while !self.eat_op(")") {
            let spread = self.eat_op("...");
            if let (Tok::Ident(_), Tok::Op(":")) = (self.peek(), self.peek_at(1))
                && !matches!(self.peek_at(2), Tok::Op(":"))
            {
                return Err(self.error("Named arguments are not supported in Blade expressions"));
            }
            let value = self.parse_expr(0)?;
            args.push(Arg { value, spread });
            if !self.eat_op(",") {
                self.expect_op(")")?;
                break;
            }
        }
        Ok(args)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        match self.advance() {
            Tok::Var(name) => Ok(Expr::Var(name)),
            Tok::Int(i) => Ok(Expr::Lit(ViewValue::Int(i))),
            Tok::Float(f) => Ok(Expr::Lit(ViewValue::Float(f))),
            Tok::Str(s) => Ok(Expr::Lit(ViewValue::Str(s.into()))),
            Tok::Interp(parts) => self.parse_interpolation(parts),
            Tok::Op("(") => {
                let expr = self.parse_expr(0)?;
                self.expect_op(")")?;
                Ok(expr)
            }
            Tok::Op("[") => {
                let items = self.parse_array_items("]")?;
                Ok(Expr::Array(items))
            }
            Tok::Op("&") => self.parse_primary(),
            Tok::Ident(name) => self.parse_identifier(name),
            Tok::Eof => Err(self.error("syntax error, unexpected end of expression")),
            _ => {
                self.pos -= 1;
                Err(self.unexpected("expecting an expression"))
            }
        }
    }

    fn parse_identifier(&mut self, name: Arc<str>) -> PResult<Expr> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "true" if !self.is_op("(") => return Ok(Expr::Lit(ViewValue::Bool(true))),
            "false" if !self.is_op("(") => return Ok(Expr::Lit(ViewValue::Bool(false))),
            "null" if !self.is_op("(") => return Ok(Expr::Lit(ViewValue::Null)),
            "array" if self.is_op("(") => {
                self.advance();
                return Ok(Expr::Array(self.parse_array_items(")")?));
            }
            "list" if self.is_op("(") => {
                self.advance();
                return Ok(Expr::Array(self.parse_array_items(")")?));
            }
            "isset" => {
                let args = self.parse_args()?;
                if args.is_empty() {
                    return Err(self.error("syntax error, isset() requires at least one argument"));
                }
                return Ok(Expr::Isset(args.into_iter().map(|a| a.value).collect()));
            }
            "empty" if self.is_op("(") => {
                self.advance();
                let expr = self.parse_expr(0)?;
                self.expect_op(")")?;
                return Ok(Expr::Empty(Box::new(expr)));
            }
            "fn" => return self.parse_arrow_function(),
            "static" if matches!(self.peek(), Tok::Ident(n) if n.eq_ignore_ascii_case("fn") || n.eq_ignore_ascii_case("function")) =>
            {
                let Tok::Ident(next) = self.advance() else {
                    unreachable!()
                };
                return if next.eq_ignore_ascii_case("fn") {
                    self.parse_arrow_function()
                } else {
                    self.parse_closure()
                };
            }
            "function" if self.is_op("(") => return self.parse_closure(),
            "match" if self.is_op("(") => return self.parse_match(),
            "new" => {
                return Err(self
                    .error("Creating objects with \"new\" is not supported in Blade expressions"));
            }
            _ => {}
        }
        if self.is_op("::") {
            self.advance();
            let class: Arc<str> = class_basename(&name).into();
            return match self.advance() {
                Tok::Ident(member) => {
                    if self.is_op("(") {
                        let args = self.parse_args()?;
                        Ok(Expr::StaticCall {
                            class,
                            method: member,
                            args,
                        })
                    } else {
                        Ok(Expr::ClassConst {
                            class,
                            name: member,
                        })
                    }
                }
                Tok::Var(_) => {
                    Err(self.error("Static properties are not supported in Blade expressions"))
                }
                _ => {
                    self.pos -= 1;
                    Err(self.unexpected("expecting a static member"))
                }
            };
        }
        if self.is_op("(") {
            let args = self.parse_args()?;
            let function: Arc<str> = name.trim_start_matches('\\').into();
            return Ok(Expr::Call {
                name: function,
                args,
            });
        }
        Ok(Expr::Const(name.trim_start_matches('\\').into()))
    }

    fn parse_array_items(&mut self, close: &str) -> PResult<Vec<ArrayItem>> {
        let mut items = Vec::new();
        while !self.eat_op(close) {
            if self.is_op(",") {
                // Skipped list() slots.
                self.advance();
                continue;
            }
            let spread = self.eat_op("...");
            let first = self.parse_expr(0)?;
            let item = if !spread && self.eat_op("=>") {
                let value = self.parse_expr(0)?;
                ArrayItem {
                    key: Some(first),
                    value,
                    spread,
                }
            } else {
                ArrayItem {
                    key: None,
                    value: first,
                    spread,
                }
            };
            items.push(item);
            if !self.eat_op(",") {
                self.expect_op(close)?;
                break;
            }
        }
        Ok(items)
    }

    fn parse_interpolation(&mut self, parts: Vec<InterpPart>) -> PResult<Expr> {
        let mut exprs = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                InterpPart::Lit(text) => exprs.push(Expr::Lit(ViewValue::Str(text.into()))),
                InterpPart::Code(code, offset) => {
                    let line = self.line + count_lines(&self.src[..offset.min(self.src.len())]);
                    exprs.push(Parser::expression(&code, line)?);
                }
            }
        }
        Ok(Expr::Interp(exprs))
    }

    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        self.expect_op("(")?;
        let mut params = Vec::new();
        while !self.eat_op(")") {
            // Skip type declarations, nullability, references and variadics.
            while let Tok::Ident(_) | Tok::Op("?" | "&" | "..." | "|") = self.peek() {
                self.advance();
            }
            let Tok::Var(name) = self.advance() else {
                self.pos -= 1;
                return Err(self.unexpected("expecting a parameter"));
            };
            let default = if self.eat_op("=") {
                Some(self.parse_expr(0)?)
            } else {
                None
            };
            params.push(Param { name, default });
            if !self.eat_op(",") {
                self.expect_op(")")?;
                break;
            }
        }
        Ok(params)
    }

    fn skip_return_type(&mut self) {
        if self.eat_op(":") {
            while let Tok::Ident(_) | Tok::Op("?" | "|") = self.peek() {
                self.advance();
            }
        }
    }

    fn parse_arrow_function(&mut self) -> PResult<Expr> {
        let params = self.parse_params()?;
        self.skip_return_type();
        self.expect_op("=>")?;
        let body = self.parse_expr(ASSIGN_BP)?;
        let mut captures = Vec::new();
        body.collect_vars(&mut captures);
        captures.retain(|name| !params.iter().any(|p| &p.name == name));
        Ok(Expr::Closure(Arc::new(ClosureDef {
            params,
            captures,
            body: vec![Stmt::Return(Some(body))],
        })))
    }

    fn parse_closure(&mut self) -> PResult<Expr> {
        let params = self.parse_params()?;
        let mut captures = Vec::new();
        if self.eat_keyword("use") {
            self.expect_op("(")?;
            while !self.eat_op(")") {
                self.eat_op("&");
                let Tok::Var(name) = self.advance() else {
                    self.pos -= 1;
                    return Err(self.unexpected("expecting a variable"));
                };
                captures.push(name);
                if !self.eat_op(",") {
                    self.expect_op(")")?;
                    break;
                }
            }
        }
        self.skip_return_type();
        let body = self.parse_block()?;
        Ok(Expr::Closure(Arc::new(ClosureDef {
            params,
            captures,
            body,
        })))
    }

    fn parse_match(&mut self) -> PResult<Expr> {
        self.expect_op("(")?;
        let subject = self.parse_expr(0)?;
        self.expect_op(")")?;
        self.expect_op("{")?;
        let mut arms = Vec::new();
        while !self.eat_op("}") {
            let conditions = if self.eat_keyword("default") {
                None
            } else {
                let mut conditions = vec![self.parse_expr(0)?];
                while self.eat_op(",") {
                    if self.is_op("=>") {
                        break;
                    }
                    conditions.push(self.parse_expr(0)?);
                }
                Some(conditions)
            };
            self.expect_op("=>")?;
            let result = self.parse_expr(0)?;
            arms.push(MatchArm { conditions, result });
            if !self.eat_op(",") {
                self.expect_op("}")?;
                break;
            }
        }
        Ok(Expr::Match {
            subject: Box::new(subject),
            arms,
        })
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn parse_block(&mut self) -> PResult<Vec<Stmt>> {
        if !self.eat_op("{") {
            // A single statement without braces.
            return Ok(vec![self.parse_statement()?]);
        }
        let mut stmts = Vec::new();
        while !self.eat_op("}") {
            if self.at_eof() {
                return Err(self.unexpected("expecting \"}\""));
            }
            if self.eat_op(";") {
                continue;
            }
            stmts.push(self.parse_statement()?);
        }
        Ok(stmts)
    }

    fn end_statement(&mut self) -> PResult<()> {
        if self.eat_op(";") || self.at_eof() || self.is_op("}") {
            Ok(())
        } else {
            Err(self.unexpected("expecting \";\""))
        }
    }

    fn parse_statement(&mut self) -> PResult<Stmt> {
        if let Tok::Ident(word) = self.peek().clone() {
            match word.to_ascii_lowercase().as_str() {
                "echo" | "print" => {
                    self.advance();
                    let mut exprs = vec![self.parse_expr(0)?];
                    while self.eat_op(",") {
                        exprs.push(self.parse_expr(0)?);
                    }
                    self.end_statement()?;
                    return Ok(Stmt::Echo(exprs));
                }
                "unset" => {
                    self.advance();
                    let args = self.parse_args()?;
                    self.end_statement()?;
                    return Ok(Stmt::Unset(args.into_iter().map(|a| a.value).collect()));
                }
                "return" => {
                    self.advance();
                    let value = if self.is_op(";") || self.at_eof() || self.is_op("}") {
                        None
                    } else {
                        Some(self.parse_expr(0)?)
                    };
                    self.end_statement()?;
                    return Ok(Stmt::Return(value));
                }
                "break" => {
                    self.advance();
                    self.end_statement()?;
                    return Ok(Stmt::Break);
                }
                "continue" => {
                    self.advance();
                    self.end_statement()?;
                    return Ok(Stmt::Continue);
                }
                "if" => {
                    self.advance();
                    return self.parse_if();
                }
                "foreach" => {
                    self.advance();
                    self.expect_op("(")?;
                    let iterable = self.parse_expr(0)?;
                    if !self.eat_keyword("as") {
                        return Err(self.unexpected("expecting \"as\""));
                    }
                    let (key, value) = self.parse_foreach_target()?;
                    self.expect_op(")")?;
                    let body = self.parse_block()?;
                    return Ok(Stmt::Foreach {
                        iterable,
                        key,
                        value,
                        body,
                    });
                }
                _ => {}
            }
        }
        let expr = self.parse_expr(0)?;
        self.end_statement()?;
        Ok(Stmt::Expr(expr))
    }

    fn parse_if(&mut self) -> PResult<Stmt> {
        let mut branches = Vec::new();
        let mut otherwise = None;
        self.expect_op("(")?;
        let cond = self.parse_expr(0)?;
        self.expect_op(")")?;
        branches.push((cond, self.parse_block()?));
        loop {
            if self.eat_keyword("elseif") {
                self.expect_op("(")?;
                let cond = self.parse_expr(0)?;
                self.expect_op(")")?;
                branches.push((cond, self.parse_block()?));
            } else if self.is_keyword("else") {
                self.advance();
                if self.eat_keyword("if") {
                    self.expect_op("(")?;
                    let cond = self.parse_expr(0)?;
                    self.expect_op(")")?;
                    branches.push((cond, self.parse_block()?));
                } else {
                    otherwise = Some(self.parse_block()?);
                    break;
                }
            } else {
                break;
            }
        }
        Ok(Stmt::If {
            branches,
            otherwise,
        })
    }

    /// Parse the `$key => $value` (or `$value`, or `[$a, $b]`) part of a foreach.
    pub(crate) fn parse_foreach_target(&mut self) -> PResult<(Option<Expr>, Expr)> {
        let first = self.parse_postfix()?;
        if self.eat_op("=>") {
            let second = self.parse_postfix()?;
            if !first.is_assignable() || !second.is_assignable() {
                return Err(self.error("syntax error, foreach targets must be variables"));
            }
            Ok((Some(first), second))
        } else {
            if !first.is_assignable() {
                return Err(self.error("syntax error, foreach targets must be variables"));
            }
            Ok((None, first))
        }
    }
}

enum Infix {
    Binary(BinaryOp),
    Ternary,
}

/// Count newlines in a string.
pub(crate) fn count_lines(s: &str) -> usize {
    s.bytes().filter(|b| *b == b'\n').count()
}

/// Strip the namespace from a class name: `\Illuminate\Support\Str` → `Str`.
pub(crate) fn class_basename(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

/// Parse a `@foreach` / `@forelse` expression: `$users as $key => $user`.
pub(crate) fn parse_foreach(src: &str, line: usize) -> PResult<(Expr, Option<Expr>, Expr)> {
    let mut parser = Parser::new(src, line)?;
    let iterable = parser.parse_expr(0)?;
    if !parser.eat_keyword("as") {
        return Err(ViewCompilationException::new(
            "Malformed @foreach statement.",
            line,
        ));
    }
    let (key, value) = parser.parse_foreach_target()?;
    parser.expect_eof()?;
    Ok((iterable, key, value))
}

/// Parse a `@for` expression: `$i = 0; $i < 10; $i++`.
pub(crate) fn parse_for(src: &str, line: usize) -> PResult<(Vec<Expr>, Vec<Expr>, Vec<Expr>)> {
    let mut parser = Parser::new(src, line)?;
    let mut sections: Vec<Vec<Expr>> = Vec::new();
    for index in 0..3 {
        let mut exprs = Vec::new();
        let terminator = if index < 2 { ";" } else { "" };
        loop {
            if (terminator.is_empty() && parser.at_eof())
                || (!terminator.is_empty() && parser.is_op(terminator))
            {
                break;
            }
            exprs.push(parser.parse_expr(0)?);
            if !parser.eat_op(",") {
                break;
            }
        }
        if !terminator.is_empty() && !parser.eat_op(terminator) {
            return Err(ViewCompilationException::new(
                "Malformed @for statement.",
                line,
            ));
        }
        sections.push(exprs);
    }
    parser.expect_eof()?;
    let step = sections.pop().unwrap_or_default();
    let cond = sections.pop().unwrap_or_default();
    let init = sections.pop().unwrap_or_default();
    Ok((init, cond, step))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Expr {
        Parser::expression(src, 1).unwrap()
    }

    #[test]
    fn it_respects_precedence() {
        let expr = parse("1 + 2 * 3");
        let Expr::Binary {
            op: BinaryOp::Add,
            right,
            ..
        } = expr
        else {
            panic!("{expr:?}")
        };
        assert!(matches!(
            *right,
            Expr::Binary {
                op: BinaryOp::Mul,
                ..
            }
        ));

        let expr = parse("! $a && $b");
        assert!(matches!(
            expr,
            Expr::Binary {
                op: BinaryOp::And,
                ..
            }
        ));

        let expr = parse("'a' . 1 + 2");
        assert!(matches!(
            expr,
            Expr::Binary {
                op: BinaryOp::Concat,
                ..
            }
        ));

        let expr = parse("-2 ** 2");
        assert!(matches!(
            expr,
            Expr::Unary {
                op: UnaryOp::Neg,
                ..
            }
        ));

        let expr = parse("$a ?? $b ? 1 : 2");
        assert!(matches!(expr, Expr::Ternary { .. }));
    }

    #[test]
    fn it_parses_assignments_inside_expressions() {
        let expr = parse("$a && $b = 5");
        let Expr::Binary {
            op: BinaryOp::And,
            right,
            ..
        } = expr
        else {
            panic!()
        };
        assert!(matches!(*right, Expr::Assign { .. }));
        assert!(matches!(parse("$items[] = 1"), Expr::Assign { .. }));
        assert!(matches!(parse("$i++"), Expr::IncDec { prefix: false, .. }));
    }

    #[test]
    fn it_parses_calls_and_closures() {
        assert!(matches!(
            parse("Str::limit($title, 20)"),
            Expr::StaticCall { .. }
        ));
        assert!(
            matches!(parse("\\Illuminate\\Support\\Str::upper('a')"), Expr::StaticCall { ref class, .. } if &**class == "Str")
        );
        assert!(matches!(
            parse("$users->map(fn ($u) => $u->name)"),
            Expr::MethodCall { .. }
        ));
        assert!(matches!(parse("$isSelected($value)"), Expr::Invoke { .. }));
        assert!(matches!(
            parse("match ($a) { 1, 2 => 'x', default => 'y' }"),
            Expr::Match { .. }
        ));
        assert!(matches!(
            parse("function ($x) use ($y) { return $x + $y; }"),
            Expr::Closure(_)
        ));
    }

    #[test]
    fn it_reports_errors_with_lines() {
        let error = Parser::expression("$a +\n\n )", 10).unwrap_err();
        assert_eq!(error.line, 12);
        assert!(
            error.message.contains("unexpected token \")\""),
            "{}",
            error.message
        );
    }

    #[test]
    fn it_parses_foreach_and_for_headers() {
        let (_, key, value) = parse_foreach("$users as $id => $user", 1).unwrap();
        assert!(key.is_some());
        assert!(matches!(value, Expr::Var(ref n) if &**n == "user"));
        let (init, cond, step) = parse_for("$i = 0; $i < 10; $i++", 1).unwrap();
        assert_eq!((init.len(), cond.len(), step.len()), (1, 1, 1));
        assert!(parse_foreach("$users", 1).is_err());
    }

    #[test]
    fn it_parses_statements() {
        let stmts = Parser::statements(
            "$a = 1; $b = [1, 2]; $b[] = 3; if ($a) { $c = 1; } else { $c = 2; }",
            1,
        )
        .unwrap();
        assert_eq!(stmts.len(), 4);
        let stmts = Parser::statements("$x = 1", 1).unwrap();
        assert_eq!(stmts.len(), 1);
    }
}
