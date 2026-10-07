//! Blade templates: parsed once into a tree of nodes, rendered many times.

pub(crate) mod lexer;
pub(crate) mod parser;

use std::sync::Arc;

use crate::expr::{Expr, Stmt};

pub(crate) use parser::parse;

/// A parsed Blade template.
#[derive(Debug, Default)]
pub(crate) struct Template {
    pub nodes: Vec<Node>,
    /// `@extends` directives (rendered after the template, last first).
    pub extends: Vec<Extends>,
}

/// An `@extends` (or `@extendsFirst`) directive.
#[derive(Debug)]
pub(crate) struct Extends {
    pub args: Vec<Expr>,
    pub first: bool,
    pub line: usize,
}

/// A node in a template.
#[derive(Debug)]
pub(crate) enum Node {
    /// Literal text.
    Text(String),
    /// `{{ $value }}` (escaped) or `{!! $value !!}` (raw).
    Echo {
        expr: Expr,
        escape: bool,
        line: usize,
    },
    /// `@php ... @endphp` or `@php(...)`.
    Php { stmts: Vec<Stmt>, line: usize },
    /// Any conditional: `@if`, `@unless`, `@isset`, `@auth`, `@error`, ...
    If {
        branches: Vec<Branch>,
        otherwise: Option<Vec<Node>>,
    },
    /// `@switch`
    Switch {
        subject: Expr,
        cases: Vec<Case>,
        line: usize,
    },
    /// `@foreach` / `@forelse`
    Foreach {
        iterable: Expr,
        key: Option<Expr>,
        value: Expr,
        body: Vec<Node>,
        empty: Option<Vec<Node>>,
        line: usize,
    },
    /// `@for`
    For {
        init: Vec<Expr>,
        cond: Vec<Expr>,
        step: Vec<Expr>,
        body: Vec<Node>,
        line: usize,
    },
    /// `@while`
    While {
        cond: Expr,
        body: Vec<Node>,
        line: usize,
    },
    /// `@break` / `@continue`, optionally conditional or with a level.
    Jump {
        kind: JumpKind,
        cond: Option<Expr>,
        levels: u32,
        line: usize,
    },
    /// `@include`, `@includeIf`, `@includeWhen`, ...
    Include {
        kind: IncludeKind,
        args: Vec<Expr>,
        line: usize,
    },
    /// `@each`
    Each { args: Vec<Expr>, line: usize },
    /// `@section ... @endsection|@show|@stop|@append|@overwrite`
    Section {
        name: Expr,
        body: Vec<Node>,
        end: SectionEnd,
        line: usize,
    },
    /// `@section('title', 'Page Title')`
    SectionInline {
        name: Expr,
        content: Expr,
        line: usize,
    },
    /// `@yield('content', 'default')`
    Yield { args: Vec<Expr>, line: usize },
    /// `@parent`
    Parent { section: Arc<str> },
    /// `@push` / `@prepend` (and their `Once` variants).
    Push {
        prepend: bool,
        stack: Expr,
        once: Option<OnceKey>,
        body: Vec<Node>,
        line: usize,
    },
    /// `@pushIf($condition, 'stack') ... @elsePushIf ... @elsePush ... @endPushIf`
    PushIf {
        branches: Vec<(Expr, Expr, Vec<Node>)>,
        otherwise: Option<(Expr, Vec<Node>)>,
        line: usize,
    },
    /// `@stack('scripts')`
    Stack { args: Vec<Expr>, line: usize },
    /// `@once ... @endonce`
    Once {
        key: OnceKey,
        body: Vec<Node>,
        line: usize,
    },
    /// A component tag (`<x-alert>`) or `@component` block.
    Component(Box<ComponentNode>),
    /// A named slot (`<x-slot:title>` or `@slot('title')`).
    Slot(Box<SlotNode>),
    /// `@props([...])`
    Props { expr: Expr, line: usize },
    /// `@aware([...])`
    Aware { expr: Expr, line: usize },
    /// `@fragment('name') ... @endfragment`
    Fragment {
        name: Expr,
        body: Vec<Node>,
        line: usize,
    },
    /// `@lang ... @endlang`
    LangBlock {
        args: Vec<Expr>,
        body: Vec<Node>,
        line: usize,
    },
    /// A directive that outputs something (`@csrf`, `@json`, `@class`, custom directives...).
    Output {
        directive: OutputDirective,
        args: Vec<Expr>,
        line: usize,
    },
}

/// One branch of a conditional.
#[derive(Debug)]
pub(crate) struct Branch {
    pub cond: Cond,
    pub body: Vec<Node>,
    pub line: usize,
}

/// The condition of a branch.
#[derive(Debug)]
pub(crate) enum Cond {
    Expr(Expr),
    Not(Expr),
    Isset(Vec<Expr>),
    Empty(Expr),
    Auth(Vec<Expr>),
    Guest(Vec<Expr>),
    Env(Vec<Expr>),
    Production,
    HasSection(Expr),
    SectionMissing(Expr),
    HasStack(Expr),
    Can(Vec<Expr>),
    Cannot(Vec<Expr>),
    CanAny(Vec<Expr>),
    /// `@error('field', 'bag')`: binds `$message`.
    Error(Vec<Expr>),
    /// `@session('key')`: binds `$value`.
    Session(Vec<Expr>),
    /// `@context('key')`: binds `$value`.
    Context(Vec<Expr>),
    /// A custom `Blade::if` condition (negated for `@unless...`).
    Custom {
        name: String,
        args: Vec<Expr>,
        negate: bool,
    },
}

/// A `@case` (or `@default`, when `test` is `None`).
#[derive(Debug)]
pub(crate) struct Case {
    pub test: Option<Expr>,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JumpKind {
    Break,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncludeKind {
    Include,
    If,
    When,
    Unless,
    First,
    Isolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionEnd {
    Stop,
    Show,
    Append,
    Overwrite,
}

/// The identity of a `@once` block.
#[derive(Debug)]
pub(crate) enum OnceKey {
    /// A unique id generated when the template was compiled.
    Generated(String),
    /// An explicit id given in the template.
    Explicit(Expr),
}

/// Directives that output a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputDirective {
    Csrf,
    Method,
    Json,
    Js,
    Class,
    Style,
    Checked,
    Selected,
    Disabled,
    Readonly,
    Required,
    Bool,
    Lang,
    Choice,
    Dump,
    Dd,
    Vite,
    ViteReactRefresh,
    Fonts,
    Inject,
    Custom(String),
}

/// A component tag.
#[derive(Debug)]
pub(crate) struct ComponentNode {
    pub name: ComponentName,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
    pub line: usize,
}

/// Which component to render.
#[derive(Debug)]
pub(crate) enum ComponentName {
    /// `<x-alert>`
    Static(String),
    /// `<x-dynamic-component :component="$name">`
    Dynamic,
    /// `@component('view.name', [...])`
    /// `@component` (`first`: `@componentFirst`, rendering the first view that exists).
    Legacy { view: Expr, data: Option<Expr>, first: bool },
}

/// An attribute on a component or slot tag.
#[derive(Debug)]
pub(crate) struct Attr {
    pub name: String,
    pub value: AttrValue,
}

#[derive(Debug)]
pub(crate) enum AttrValue {
    /// A plain attribute, possibly containing `{{ }}` echoes.
    Static(Vec<AttrPart>),
    /// A bound attribute: `:message="$message"`.
    Bound(Expr),
    /// A valueless attribute: `disabled`.
    True,
}

#[derive(Debug)]
pub(crate) enum AttrPart {
    Text(String),
    Echo { expr: Expr, escape: bool },
}

/// A named slot.
#[derive(Debug)]
pub(crate) struct SlotNode {
    pub name: Expr,
    pub attrs: Vec<Attr>,
    pub body: SlotBody,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) enum SlotBody {
    Nodes(Vec<Node>),
    Inline(Expr),
}
