//! Blade's expression language: the PHP-flavored expressions found inside
//! `{{ }}`, directive arguments and `@php` blocks.
//!
//! Expressions are parsed once, when a template is compiled, and evaluated
//! against the view's variables every time the view renders.

pub(crate) mod eval;
pub(crate) mod lexer;
pub(crate) mod parser;

use std::sync::Arc;

use crate::value::ViewValue;

pub(crate) use eval::{Evaluator, Scope};
pub(crate) use parser::Parser;

/// A parsed expression.
#[derive(Debug, Clone)]
pub(crate) enum Expr {
    /// A literal: `null`, `true`, `42`, `1.5`, `'string'`.
    Lit(ViewValue),
    /// A double-quoted string with interpolation: `"Hello {$name}"`.
    Interp(Vec<Expr>),
    /// An array literal: `[1, 'a' => 2, ...$rest]`.
    Array(Vec<ArrayItem>),
    /// A variable: `$name`.
    Var(Arc<str>),
    /// A bare constant: `PHP_EOL`, `JSON_PRETTY_PRINT`.
    Const(Arc<str>),
    /// Property access: `$user->name`, `$user?->name`, `$user->{$key}`.
    Prop {
        target: Box<Expr>,
        name: Box<Expr>,
        nullsafe: bool,
    },
    /// Offset access: `$user['name']` (index is `None` for `$items[]`).
    Index {
        target: Box<Expr>,
        index: Option<Box<Expr>>,
    },
    /// A method call: `$users->count()`, `$user?->posts()`.
    MethodCall {
        target: Box<Expr>,
        name: Arc<str>,
        args: Vec<Arg>,
        nullsafe: bool,
    },
    /// A function call: `count($users)`.
    Call { name: Arc<str>, args: Vec<Arg> },
    /// A static call: `Str::limit($title, 20)`.
    StaticCall {
        class: Arc<str>,
        method: Arc<str>,
        args: Vec<Arg>,
    },
    /// A class constant: `Foo::BAR`, `Foo::class`.
    ClassConst { class: Arc<str>, name: Arc<str> },
    /// Invoking a callable value: `$format($value)`.
    Invoke { callee: Box<Expr>, args: Vec<Arg> },
    /// A unary operation: `! $done`, `-$amount`.
    Unary { op: UnaryOp, expr: Box<Expr> },
    /// A binary operation.
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// The ternary operator (`then` is `None` for `?:`).
    Ternary {
        cond: Box<Expr>,
        then: Option<Box<Expr>>,
        otherwise: Box<Expr>,
    },
    /// `isset($a, $b)`
    Isset(Vec<Expr>),
    /// `empty($a)`
    Empty(Box<Expr>),
    /// Assignment: `$a = 1`, `$a .= 'x'`, `$a ??= []`.
    Assign {
        target: Box<Expr>,
        op: Option<BinaryOp>,
        value: Box<Expr>,
    },
    /// `$i++`, `--$i`
    IncDec {
        target: Box<Expr>,
        increment: bool,
        prefix: bool,
    },
    /// An arrow function or closure.
    Closure(Arc<ClosureDef>),
    /// `match ($status) { 'a', 'b' => 1, default => 2 }`
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// A cast: `(int) $value`.
    Cast { ty: CastType, expr: Box<Expr> },
}

/// An item in an array literal.
#[derive(Debug, Clone)]
pub(crate) struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
    pub spread: bool,
}

/// A call argument.
#[derive(Debug, Clone)]
pub(crate) struct Arg {
    pub value: Expr,
    pub spread: bool,
}

/// An arm of a `match` expression (`conditions` is `None` for `default`).
#[derive(Debug, Clone)]
pub(crate) struct MatchArm {
    pub conditions: Option<Vec<Expr>>,
    pub result: Expr,
}

/// A closure parameter.
#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub name: Arc<str>,
    pub default: Option<Expr>,
}

/// An arrow function (`fn ($x) => ...`) or closure (`function ($x) use ($y) { ... }`).
#[derive(Debug, Clone)]
pub(crate) struct ClosureDef {
    pub params: Vec<Param>,
    /// Variables captured from the surrounding scope.
    pub captures: Vec<Arc<str>>,
    pub body: Vec<Stmt>,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Not,
    Neg,
    Plus,
    BitNot,
    Silence,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    NotEq,
    Identical,
    NotIdentical,
    Lt,
    Le,
    Gt,
    Ge,
    Spaceship,
    And,
    Or,
    Xor,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Coalesce,
}

/// Cast types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CastType {
    Int,
    Float,
    String,
    Bool,
    Array,
    Object,
}

/// A statement inside `@php` blocks and closures.
#[derive(Debug, Clone)]
pub(crate) enum Stmt {
    Expr(Expr),
    Echo(Vec<Expr>),
    Unset(Vec<Expr>),
    Return(Option<Expr>),
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        otherwise: Option<Vec<Stmt>>,
    },
    Foreach {
        iterable: Expr,
        key: Option<Expr>,
        value: Expr,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
}

impl Expr {
    /// Determine if the expression may be assigned to.
    pub(crate) fn is_assignable(&self) -> bool {
        match self {
            Expr::Var(_) => true,
            Expr::Index { target, .. } | Expr::Prop { target, .. } => target.is_assignable(),
            Expr::Array(items) => items.iter().all(|item| item.value.is_assignable()),
            _ => false,
        }
    }

    /// Collect the variables an expression reads (for closure captures).
    pub(crate) fn collect_vars(&self, out: &mut Vec<Arc<str>>) {
        let mut push = |name: &Arc<str>| {
            if !out.iter().any(|n| n == name) {
                out.push(name.clone());
            }
        };
        match self {
            Expr::Var(name) => push(name),
            Expr::Lit(_) | Expr::Const(_) | Expr::ClassConst { .. } => {}
            Expr::Interp(parts) | Expr::Isset(parts) => parts.iter().for_each(|p| p.collect_vars(out)),
            Expr::Array(items) => {
                for item in items {
                    if let Some(key) = &item.key {
                        key.collect_vars(out);
                    }
                    item.value.collect_vars(out);
                }
            }
            Expr::Prop { target, name, .. } => {
                target.collect_vars(out);
                name.collect_vars(out);
            }
            Expr::Index { target, index } => {
                target.collect_vars(out);
                if let Some(index) = index {
                    index.collect_vars(out);
                }
            }
            Expr::MethodCall { target, args, .. } => {
                target.collect_vars(out);
                args.iter().for_each(|a| a.value.collect_vars(out));
            }
            Expr::Call { args, .. } | Expr::StaticCall { args, .. } => {
                args.iter().for_each(|a| a.value.collect_vars(out))
            }
            Expr::Invoke { callee, args } => {
                callee.collect_vars(out);
                args.iter().for_each(|a| a.value.collect_vars(out));
            }
            Expr::Unary { expr, .. } | Expr::Empty(expr) | Expr::Cast { expr, .. } => expr.collect_vars(out),
            Expr::Binary { left, right, .. } => {
                left.collect_vars(out);
                right.collect_vars(out);
            }
            Expr::Ternary { cond, then, otherwise } => {
                cond.collect_vars(out);
                if let Some(then) = then {
                    then.collect_vars(out);
                }
                otherwise.collect_vars(out);
            }
            Expr::Assign { target, value, .. } => {
                target.collect_vars(out);
                value.collect_vars(out);
            }
            Expr::IncDec { target, .. } => target.collect_vars(out),
            Expr::Closure(def) => {
                // Nested arrow functions capture from us, too.
                let mut inner = Vec::new();
                for stmt in &def.body {
                    stmt.collect_vars(&mut inner);
                }
                for name in inner.iter().chain(def.captures.iter()) {
                    if !def.params.iter().any(|p| &p.name == name) {
                        push(name);
                    }
                }
            }
            Expr::Match { subject, arms } => {
                subject.collect_vars(out);
                for arm in arms {
                    if let Some(conditions) = &arm.conditions {
                        conditions.iter().for_each(|c| c.collect_vars(out));
                    }
                    arm.result.collect_vars(out);
                }
            }
        }
    }
}

impl Stmt {
    pub(crate) fn collect_vars(&self, out: &mut Vec<Arc<str>>) {
        match self {
            Stmt::Expr(e) => e.collect_vars(out),
            Stmt::Echo(es) | Stmt::Unset(es) => es.iter().for_each(|e| e.collect_vars(out)),
            Stmt::Return(e) => {
                if let Some(e) = e {
                    e.collect_vars(out);
                }
            }
            Stmt::If { branches, otherwise } => {
                for (cond, body) in branches {
                    cond.collect_vars(out);
                    body.iter().for_each(|s| s.collect_vars(out));
                }
                if let Some(body) = otherwise {
                    body.iter().for_each(|s| s.collect_vars(out));
                }
            }
            Stmt::Foreach { iterable, key, value, body } => {
                iterable.collect_vars(out);
                if let Some(key) = key {
                    key.collect_vars(out);
                }
                value.collect_vars(out);
                body.iter().for_each(|s| s.collect_vars(out));
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }
}
