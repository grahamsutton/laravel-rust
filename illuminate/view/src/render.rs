//! Rendering parsed templates.
//!
//! A [`Renderer`] holds the state of one rendering pass — sections, stacks,
//! loops, `@once` blocks and components — just like Laravel's view factory
//! does while a view (and everything it includes) is rendered.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use illuminate_support::{Error, Result, Str};
use indexmap::IndexMap;

use crate::attributes::{ComponentAttributeBag, prop_names};
use crate::component::{ComponentArgs, ComponentObject, ComponentSlot, ComponentView, sanitize};
use crate::exception::{
    InvalidArgumentException, RuntimeException, ViewException, error, passes_through,
};
use crate::expr::eval::{Flow as StmtFlow, call_function, iterate};
use crate::expr::{Evaluator, Expr, Scope};
use crate::factory::Factory;
use crate::functions;
use crate::objects::{MessageBagObject, ViewErrorBag};
use crate::php;
use crate::registry::Registry;
use crate::statics;
use crate::template::{
    Attr, AttrPart, AttrValue, Branch, ComponentName, ComponentNode, Cond, IncludeKind, JumpKind,
    Node, OnceKey, OutputDirective, SectionEnd, SlotBody, SlotNode, Template,
};
use crate::value::{ArrayKey, ViewData, ViewValue};
use crate::view::View;

/// How deeply views may nest before we assume infinite recursion.
const MAX_DEPTH: usize = 128;
/// How many times `@for` / `@while` may loop.
const MAX_ITERATIONS: usize = 10_000_000;

/// Control flow out of a block.
enum Flow {
    Normal,
    Break(u32),
    Continue(u32),
}

/// The view a node belongs to, for error messages.
#[derive(Clone)]
pub(crate) struct ViewContext {
    pub name: Arc<str>,
    pub path: Option<Arc<Path>>,
}

/// The state of a `@foreach` loop.
#[derive(Clone)]
struct LoopState {
    iteration: i64,
    index: i64,
    remaining: i64,
    count: i64,
    first: bool,
    last: bool,
    odd: bool,
    even: bool,
    depth: i64,
    parent: ViewValue,
}

impl LoopState {
    fn to_value(&self) -> ViewValue {
        ViewValue::map([
            ("iteration", ViewValue::Int(self.iteration)),
            ("index", ViewValue::Int(self.index)),
            ("remaining", ViewValue::Int(self.remaining)),
            ("count", ViewValue::Int(self.count)),
            ("first", ViewValue::Bool(self.first)),
            ("last", ViewValue::Bool(self.last)),
            ("odd", ViewValue::Bool(self.odd)),
            ("even", ViewValue::Bool(self.even)),
            ("depth", ViewValue::Int(self.depth)),
            ("parent", self.parent.clone()),
        ])
    }
}

/// A component whose children (slots) are being rendered.
struct ComponentFrame {
    data: Arc<ViewData>,
    slots: IndexMap<String, ViewValue>,
}

/// Renders views, holding the state of a single rendering pass.
pub(crate) struct Renderer {
    factory: Factory,
    registry: Arc<Registry>,
    sections: HashMap<String, String>,
    pushes: HashMap<String, IndexMap<usize, String>>,
    prepends: HashMap<String, IndexMap<usize, String>>,
    rendered_once: HashSet<String>,
    render_count: usize,
    loops: Vec<LoopState>,
    components: Vec<ComponentFrame>,
    component_data: Vec<Arc<ViewData>>,
    fragments: IndexMap<String, String>,
    request_shared: ViewData,
    templates: HashMap<String, (Arc<Template>, ViewContext)>,
    anonymous_components: HashMap<String, Option<String>>,
    shared: ViewData,
}

static PARENT_SALT: LazyLock<String> = LazyLock::new(|| Str::random(16));

/// The placeholder `@parent` leaves in a section.
fn parent_placeholder(section: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in PARENT_SALT.bytes().chain(section.bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("##parent-placeholder-{hash:016x}##")
}

impl Renderer {
    pub(crate) fn new(factory: &Factory) -> Self {
        Self {
            factory: factory.clone(),
            registry: factory.blade().registry(),
            sections: HashMap::new(),
            pushes: HashMap::new(),
            prepends: HashMap::new(),
            rendered_once: HashSet::new(),
            render_count: 0,
            loops: Vec::new(),
            components: Vec::new(),
            component_data: Vec::new(),
            fragments: IndexMap::new(),
            request_shared: factory.request_shared_data(),
            templates: HashMap::new(),
            anonymous_components: HashMap::new(),
            shared: factory.shared_data(),
        }
    }

    /// The fragments captured while rendering.
    pub(crate) fn fragments(&self) -> &IndexMap<String, String> {
        &self.fragments
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    /// Render a view (running its composers), returning the HTML and the
    /// final data the view was rendered with.
    pub(crate) fn render_view(&mut self, view: &View) -> Result<(String, ViewData)> {
        let mut view = view.clone();
        self.factory.call_composers(&mut view);
        let template = self.load(&view)?;
        let ctx = view.context();
        let data = view.data().clone();
        let html = self.render_template(&template, &ctx, &data)?;
        Ok((html, data))
    }

    fn load(&self, view: &View) -> Result<Arc<Template>> {
        view.template().map_err(|e| {
            if passes_through(&e) {
                e
            } else {
                let line = e
                    .downcast_ref::<crate::exception::ViewCompilationException>()
                    .map(|c| c.line)
                    .unwrap_or(0);
                if line == 0 {
                    e
                } else {
                    let ctx = view.context();
                    ViewException::wrap(
                        e,
                        &*ctx.name,
                        ctx.path.as_ref().map(|p| p.to_path_buf()),
                        line,
                    )
                    .into()
                }
            }
        })
    }

    /// Build the variables for a view: shared data, request data, then the
    /// view's own data.
    fn scope_for(&self, data: &ViewData) -> Scope {
        let shared = &self.shared;
        let mut scope =
            Scope::with_capacity(shared.len() + self.request_shared.len() + data.len() + 1);
        for (key, value) in shared
            .iter()
            .chain(self.request_shared.iter())
            .chain(data.iter())
        {
            scope.set(key.as_str(), value.clone());
        }
        match scope.get("errors") {
            None | Some(ViewValue::Null) => {
                scope.set("errors", ViewValue::object(ViewErrorBag::new()))
            }
            Some(value @ ViewValue::Array(_)) => {
                let bag = ViewErrorBag::from_value(value);
                scope.set("errors", ViewValue::object(bag));
            }
            Some(_) => {}
        }
        scope
    }

    fn render_template(
        &mut self,
        template: &Template,
        ctx: &ViewContext,
        data: &ViewData,
    ) -> Result<String> {
        if self.render_count >= MAX_DEPTH {
            return Err(RuntimeException::new(format!(
                "Maximum view nesting level of {MAX_DEPTH} reached while rendering [{}]. Is a view including itself?",
                ctx.name
            ))
            .into());
        }
        self.render_count += 1;
        let result = self.render_template_inner(template, ctx, data);
        self.render_count -= 1;
        result
    }

    fn render_template_inner(
        &mut self,
        template: &Template,
        ctx: &ViewContext,
        data: &ViewData,
    ) -> Result<String> {
        let mut scope = self.scope_for(data);
        let mut out = String::new();
        self.render_nodes(&template.nodes, &mut scope, &mut out, ctx)?;
        for extends in template.extends.iter().rev() {
            let mut args = Vec::with_capacity(extends.args.len());
            for arg in &extends.args {
                args.push(self.eval(arg, &mut scope, ctx, extends.line)?);
            }
            let name = if extends.first {
                self.first_existing(functions::arg(&args, 0))
                    .map_err(|e| self.wrap(e, ctx, extends.line))?
            } else {
                php::to_str(functions::arg(&args, 0))
                    .map_err(|e| self.wrap(e, ctx, extends.line))?
            };
            let mut data = scope_to_data(&scope);
            merge_data(&mut data, functions::arg(&args, 1));
            let html = self
                .render_named(&name, data)
                .map_err(|e| self.wrap(e, ctx, extends.line))?;
            out.push_str(&html);
        }
        Ok(out)
    }

    /// Render a view by name with the given data (creators and composers run).
    fn render_named(&mut self, name: &str, data: ViewData) -> Result<String> {
        let mut view = View::named(&self.factory, name, data);
        self.factory.call_creators(&mut view);
        self.factory.call_composers(&mut view);
        // Views included many times (in a loop, say) are loaded once per render.
        let (template, ctx) = match self.templates.get(name) {
            Some(cached) => cached.clone(),
            None => {
                let loaded = (self.load(&view)?, view.context());
                self.templates.insert(name.to_string(), loaded.clone());
                loaded
            }
        };
        self.render_template(&template, &ctx, view.data())
    }

    fn first_existing(&self, names: &ViewValue) -> Result<String> {
        let candidates: Vec<String> = match names {
            ViewValue::Array(list) => list.values().map(php::to_str).collect::<Result<_>>()?,
            other => vec![php::to_str(other)?],
        };
        candidates
            .into_iter()
            .find(|name| self.factory.exists(name))
            .ok_or_else(|| {
                InvalidArgumentException::new("None of the views in the given array exist.").into()
            })
    }

    // ------------------------------------------------------------------
    // Nodes
    // ------------------------------------------------------------------

    fn wrap(&self, error: Error, ctx: &ViewContext, line: usize) -> Error {
        if passes_through(&error) {
            error
        } else {
            ViewException::wrap(
                error,
                &*ctx.name,
                ctx.path.as_ref().map(|p| p.to_path_buf()),
                line,
            )
            .into()
        }
    }

    fn eval(
        &self,
        expr: &Expr,
        scope: &mut Scope,
        ctx: &ViewContext,
        line: usize,
    ) -> Result<ViewValue> {
        Evaluator::new(scope, &self.registry)
            .eval(expr)
            .map_err(|e| self.wrap(e, ctx, line))
    }

    fn eval_all(
        &self,
        exprs: &[Expr],
        scope: &mut Scope,
        ctx: &ViewContext,
        line: usize,
    ) -> Result<Vec<ViewValue>> {
        let mut values = Vec::with_capacity(exprs.len());
        for expr in exprs {
            values.push(self.eval(expr, scope, ctx, line)?);
        }
        Ok(values)
    }

    fn render_nodes(
        &mut self,
        nodes: &[Node],
        scope: &mut Scope,
        out: &mut String,
        ctx: &ViewContext,
    ) -> Result<Flow> {
        for node in nodes {
            match self.render_node(node, scope, out, ctx)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    fn render_node(
        &mut self,
        node: &Node,
        scope: &mut Scope,
        out: &mut String,
        ctx: &ViewContext,
    ) -> Result<Flow> {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Echo { expr, escape, line } => {
                let value = self.eval(expr, scope, ctx, *line)?;
                let line = *line;
                if *escape {
                    self.echo_escaped(&value, out)
                        .map_err(|e| self.wrap(e, ctx, line))?;
                } else {
                    self.echo_unescaped(&value, out)
                        .map_err(|e| self.wrap(e, ctx, line))?;
                }
            }
            Node::Php { stmts, line } => {
                let result = Evaluator::new(scope, &self.registry).exec(stmts, out);
                match result.map_err(|e| self.wrap(e, ctx, *line))? {
                    StmtFlow::Break => return Ok(Flow::Break(1)),
                    StmtFlow::Continue => return Ok(Flow::Continue(1)),
                    StmtFlow::Normal | StmtFlow::Return(_) => {}
                }
            }
            Node::If {
                branches,
                otherwise,
            } => return self.render_if(branches, otherwise.as_deref(), scope, out, ctx),
            Node::Switch {
                subject,
                cases,
                line,
            } => {
                let subject = self.eval(subject, scope, ctx, *line)?;
                let mut start = None;
                for (index, case) in cases.iter().enumerate() {
                    if let Some(test) = &case.test {
                        let value = self.eval(test, scope, ctx, *line)?;
                        if php::loose_eq(&value, &subject) {
                            start = Some(index);
                            break;
                        }
                    }
                }
                let start = start.or_else(|| cases.iter().position(|c| c.test.is_none()));
                if let Some(start) = start {
                    for case in &cases[start..] {
                        match self.render_nodes(&case.body, scope, out, ctx)? {
                            Flow::Normal => {}
                            Flow::Break(1) | Flow::Continue(1) => break,
                            Flow::Break(n) => return Ok(Flow::Break(n - 1)),
                            Flow::Continue(n) => return Ok(Flow::Continue(n - 1)),
                        }
                    }
                }
            }
            Node::Foreach {
                iterable,
                key,
                value,
                body,
                empty,
                line,
            } => {
                return self.render_foreach(
                    iterable,
                    key.as_ref(),
                    value,
                    body,
                    empty.as_deref(),
                    *line,
                    scope,
                    out,
                    ctx,
                );
            }
            Node::For {
                init,
                cond,
                step,
                body,
                line,
            } => {
                for expr in init {
                    self.eval(expr, scope, ctx, *line)?;
                }
                let mut iterations = 0usize;
                loop {
                    let mut proceed = true;
                    for expr in cond {
                        proceed = self.eval(expr, scope, ctx, *line)?.truthy();
                    }
                    if !proceed {
                        break;
                    }
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        return Err(self.wrap(
                            error("Maximum @for iterations exceeded"),
                            ctx,
                            *line,
                        ));
                    }
                    match self.render_nodes(body, scope, out, ctx)? {
                        Flow::Normal | Flow::Continue(1) => {}
                        Flow::Break(1) => break,
                        Flow::Break(n) => return Ok(Flow::Break(n - 1)),
                        Flow::Continue(n) => return Ok(Flow::Continue(n - 1)),
                    }
                    for expr in step {
                        self.eval(expr, scope, ctx, *line)?;
                    }
                }
            }
            Node::While { cond, body, line } => {
                let mut iterations = 0usize;
                while self.eval(cond, scope, ctx, *line)?.truthy() {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        return Err(self.wrap(
                            error("Maximum @while iterations exceeded"),
                            ctx,
                            *line,
                        ));
                    }
                    match self.render_nodes(body, scope, out, ctx)? {
                        Flow::Normal | Flow::Continue(1) => {}
                        Flow::Break(1) => break,
                        Flow::Break(n) => return Ok(Flow::Break(n - 1)),
                        Flow::Continue(n) => return Ok(Flow::Continue(n - 1)),
                    }
                }
            }
            Node::Jump {
                kind,
                cond,
                levels,
                line,
            } => {
                if let Some(cond) = cond
                    && !self.eval(cond, scope, ctx, *line)?.truthy()
                {
                    return Ok(Flow::Normal);
                }
                return Ok(match kind {
                    JumpKind::Break => Flow::Break(*levels),
                    JumpKind::Continue => Flow::Continue(*levels),
                });
            }
            Node::Include { kind, args, line } => {
                let values = self.eval_all(args, scope, ctx, *line)?;
                let html = self
                    .include(*kind, &values, scope)
                    .map_err(|e| self.wrap(e, ctx, *line))?;
                out.push_str(&html);
            }
            Node::Each { args, line } => {
                let values = self.eval_all(args, scope, ctx, *line)?;
                let html = self.each(&values).map_err(|e| self.wrap(e, ctx, *line))?;
                out.push_str(&html);
            }
            Node::Section {
                name,
                body,
                end,
                line,
            } => {
                let name = self.eval(name, scope, ctx, *line)?.to_string_lossy();
                let mut content = String::new();
                let flow = self.render_nodes(body, scope, &mut content, ctx)?;
                match end {
                    SectionEnd::Overwrite => {
                        self.sections.insert(name, content);
                    }
                    SectionEnd::Append => self.sections.entry(name).or_default().push_str(&content),
                    SectionEnd::Stop => self.extend_section(&name, content),
                    SectionEnd::Show => {
                        self.extend_section(&name, content);
                        out.push_str(&self.yield_content(&name, ""));
                    }
                }
                if !matches!(flow, Flow::Normal) {
                    return Ok(flow);
                }
            }
            Node::SectionInline {
                name,
                content,
                line,
            } => {
                let name = self.eval(name, scope, ctx, *line)?.to_string_lossy();
                let content = self.eval(content, scope, ctx, *line)?;
                let content = self
                    .escape_value(&content)
                    .map_err(|e| self.wrap(e, ctx, *line))?;
                self.extend_section(&name, content);
            }
            Node::Yield { args, line } => {
                let values = self.eval_all(args, scope, ctx, *line)?;
                let name = functions::arg(&values, 0).to_string_lossy();
                let default = match values.get(1) {
                    Some(default) => self
                        .escape_value(default)
                        .map_err(|e| self.wrap(e, ctx, *line))?,
                    None => String::new(),
                };
                out.push_str(&self.yield_content(&name, &default));
            }
            Node::Parent { section } => out.push_str(&parent_placeholder(section)),
            Node::Push {
                prepend,
                stack,
                once,
                body,
                line,
            } => {
                let stack = self.eval(stack, scope, ctx, *line)?.to_string_lossy();
                if let Some(once) = once {
                    let key = self.once_key(once, scope, ctx, *line)?;
                    if !self.rendered_once.insert(key) {
                        return Ok(Flow::Normal);
                    }
                }
                let mut content = String::new();
                let flow = self.render_nodes(body, scope, &mut content, ctx)?;
                if *prepend {
                    self.extend_prepend(&stack, content);
                } else {
                    self.extend_push(&stack, content);
                }
                if !matches!(flow, Flow::Normal) {
                    return Ok(flow);
                }
            }
            Node::PushIf {
                branches,
                otherwise,
                line,
            } => {
                for (cond, stack, body) in branches {
                    if self.eval(cond, scope, ctx, *line)?.truthy() {
                        let stack = self.eval(stack, scope, ctx, *line)?.to_string_lossy();
                        let mut content = String::new();
                        let flow = self.render_nodes(body, scope, &mut content, ctx)?;
                        self.extend_push(&stack, content);
                        return Ok(flow);
                    }
                }
                if let Some((stack, body)) = otherwise {
                    let stack = self.eval(stack, scope, ctx, *line)?.to_string_lossy();
                    let mut content = String::new();
                    let flow = self.render_nodes(body, scope, &mut content, ctx)?;
                    self.extend_push(&stack, content);
                    return Ok(flow);
                }
            }
            Node::Stack { args, line } => {
                let values = self.eval_all(args, scope, ctx, *line)?;
                let name = functions::arg(&values, 0).to_string_lossy();
                let default = values
                    .get(1)
                    .map(ViewValue::to_string_lossy)
                    .unwrap_or_default();
                out.push_str(&self.yield_push_content(&name, &default));
            }
            Node::Once { key, body, line } => {
                let key = self.once_key(key, scope, ctx, *line)?;
                if self.rendered_once.insert(key) {
                    return self.render_nodes(body, scope, out, ctx);
                }
            }
            Node::Component(component) => return self.render_component(component, scope, out, ctx),
            Node::Slot(slot) => self.render_slot(slot, scope, ctx)?,
            Node::Props { expr, line } => {
                let props = self.eval(expr, scope, ctx, *line)?;
                apply_props(&props, scope);
            }
            Node::Aware { expr, line } => {
                let aware = self.eval(expr, scope, ctx, *line)?;
                if let ViewValue::Array(list) = &aware {
                    for (key, value) in list.iter() {
                        let (name, default) = match key {
                            ArrayKey::Int(_) => (value.to_string_lossy(), ViewValue::Null),
                            ArrayKey::Str(name) => (name.to_string(), value.clone()),
                        };
                        let resolved = self.consumable_component_data(&name).unwrap_or(default);
                        scope.set(name.as_str(), resolved);
                    }
                }
            }
            Node::Fragment { name, body, line } => {
                let name = self.eval(name, scope, ctx, *line)?.to_string_lossy();
                let mut content = String::new();
                let flow = self.render_nodes(body, scope, &mut content, ctx)?;
                out.push_str(&content);
                self.fragments.insert(name, content);
                if !matches!(flow, Flow::Normal) {
                    return Ok(flow);
                }
            }
            Node::LangBlock { args, body, line } => {
                let mut key = String::new();
                self.render_nodes(body, scope, &mut key, ctx)?;
                let mut values = vec![ViewValue::from(key.trim())];
                values.extend(self.eval_all(args, scope, ctx, *line)?);
                let translated = call_function("__", &values, &self.registry)
                    .map_err(|e| self.wrap(e, ctx, *line))?;
                echo_raw(&translated, out).map_err(|e| self.wrap(e, ctx, *line))?;
            }
            Node::Output {
                directive,
                args,
                line,
            } => {
                let values = self.eval_all(args, scope, ctx, *line)?;
                self.output(directive, values, scope, out)
                    .map_err(|e| self.wrap(e, ctx, *line))?;
            }
        }
        Ok(Flow::Normal)
    }

    fn once_key(
        &self,
        key: &OnceKey,
        scope: &mut Scope,
        ctx: &ViewContext,
        line: usize,
    ) -> Result<String> {
        Ok(match key {
            OnceKey::Generated(id) => id.clone(),
            OnceKey::Explicit(expr) => format!(
                "__explicit:{}",
                self.eval(expr, scope, ctx, line)?.to_string_lossy()
            ),
        })
    }

    // ------------------------------------------------------------------
    // Echoing
    // ------------------------------------------------------------------

    /// Echo without escaping (`{!! !!}`), honoring custom echo handlers.
    fn echo_unescaped(&self, value: &ViewValue, out: &mut String) -> Result<()> {
        match self.registry.apply_echo_handlers(value) {
            Some(handled) => {
                out.push_str(&handled);
                Ok(())
            }
            None => echo_raw(value, out),
        }
    }

    fn escape_value(&self, value: &ViewValue) -> Result<String> {
        let mut out = String::new();
        self.echo_escaped(value, &mut out)?;
        Ok(out)
    }

    fn echo_escaped(&self, value: &ViewValue, out: &mut String) -> Result<()> {
        if let Some(handled) = self.registry.apply_echo_handlers(value) {
            out.push_str(&self.registry.escape(&handled));
            return Ok(());
        }
        match value {
            ViewValue::Html(html) => out.push_str(html),
            ViewValue::Null => {}
            ViewValue::Str(s) => out.push_str(&self.registry.escape(s)),
            ViewValue::Object(object) => match object.to_html() {
                Some(html) => out.push_str(&html),
                None => out.push_str(&self.registry.escape(&php::to_str(value)?)),
            },
            ViewValue::Closure(_) => {
                return Err(error(
                    "Object of class Closure could not be converted to string",
                ));
            }
            other => out.push_str(&self.registry.escape(&other.to_string_lossy())),
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Conditionals & loops
    // ------------------------------------------------------------------

    fn render_if(
        &mut self,
        branches: &[Branch],
        otherwise: Option<&[Node]>,
        scope: &mut Scope,
        out: &mut String,
        ctx: &ViewContext,
    ) -> Result<Flow> {
        for branch in branches {
            let (passes, binding) = self.condition(&branch.cond, scope, ctx, branch.line)?;
            if !passes {
                continue;
            }
            return match binding {
                Some((name, value)) => {
                    let previous = scope.remove(name);
                    scope.set(name, value);
                    let flow = self.render_nodes(&branch.body, scope, out, ctx);
                    scope.remove(name);
                    if let Some(previous) = previous {
                        scope.set(name, previous);
                    }
                    flow
                }
                None => self.render_nodes(&branch.body, scope, out, ctx),
            };
        }
        match otherwise {
            Some(body) => self.render_nodes(body, scope, out, ctx),
            None => Ok(Flow::Normal),
        }
    }

    /// Evaluate a condition, returning whether it passes and an optional
    /// variable to bind while its body renders (`$message`, `$value`).
    fn condition(
        &mut self,
        cond: &Cond,
        scope: &mut Scope,
        ctx: &ViewContext,
        line: usize,
    ) -> Result<(bool, Option<(&'static str, ViewValue)>)> {
        let wrap = |this: &Self, e: Error| this.wrap(e, ctx, line);
        let call = |this: &Self, name: &str, args: &[ViewValue]| {
            call_function(name, args, &this.registry).map_err(|e| wrap(this, e))
        };
        Ok(match cond {
            Cond::Expr(expr) => (self.eval(expr, scope, ctx, line)?.truthy(), None),
            Cond::Not(expr) => (!self.eval(expr, scope, ctx, line)?.truthy(), None),
            Cond::Isset(exprs) => {
                let mut all = !exprs.is_empty();
                for expr in exprs {
                    let value = Evaluator::new(scope, &self.registry)
                        .eval_quiet(expr)
                        .map_err(|e| wrap(self, e))?;
                    if value.is_none_or(|v| v.is_null()) {
                        all = false;
                        break;
                    }
                }
                (all, None)
            }
            Cond::Empty(expr) => {
                let value = Evaluator::new(scope, &self.registry)
                    .eval_quiet(expr)
                    .map_err(|e| wrap(self, e))?;
                (!value.is_some_and(|v| v.truthy()), None)
            }
            Cond::Auth(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                (call(self, "auth_check", &args)?.truthy(), None)
            }
            Cond::Guest(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                (!call(self, "auth_check", &args)?.truthy(), None)
            }
            Cond::Env(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                (
                    statics::environment(&args, &self.registry)
                        .map_err(|e| wrap(self, e))?
                        .truthy(),
                    None,
                )
            }
            Cond::Production => (
                statics::environment(&[ViewValue::from("production")], &self.registry)
                    .map_err(|e| wrap(self, e))?
                    .truthy(),
                None,
            ),
            Cond::HasSection(name) => {
                let name = self.eval(name, scope, ctx, line)?.to_string_lossy();
                (
                    !php::php_trim(&self.yield_content(&name, "")).is_empty(),
                    None,
                )
            }
            Cond::SectionMissing(name) => {
                let name = self.eval(name, scope, ctx, line)?.to_string_lossy();
                (
                    php::php_trim(&self.yield_content(&name, "")).is_empty(),
                    None,
                )
            }
            Cond::HasStack(name) => {
                let name = self.eval(name, scope, ctx, line)?.to_string_lossy();
                (!self.is_stack_empty(&name), None)
            }
            Cond::Can(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                (call(self, "gate_check", &args)?.truthy(), None)
            }
            Cond::Cannot(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                (!call(self, "gate_check", &args)?.truthy(), None)
            }
            Cond::CanAny(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                (
                    statics::gate_any(&args, &self.registry)
                        .map_err(|e| wrap(self, e))?
                        .truthy(),
                    None,
                )
            }
            Cond::Error(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                let field = php::to_str(functions::arg(&args, 0)).map_err(|e| wrap(self, e))?;
                let bag_name = match args.get(1) {
                    Some(name) if !name.is_null() => name.to_string_lossy(),
                    _ => "default".to_string(),
                };
                let bag = match scope.get("errors") {
                    Some(ViewValue::Object(errors)) => {
                        if let Some(errors) = errors.downcast_ref::<ViewErrorBag>() {
                            Some(errors.get_bag(&bag_name))
                        } else {
                            errors
                                .downcast_ref::<MessageBagObject>()
                                .map(|bag| bag.0.clone())
                        }
                    }
                    Some(value @ ViewValue::Array(_)) => {
                        Some(ViewErrorBag::from_value(value).get_bag(&bag_name))
                    }
                    _ => None,
                };
                match bag {
                    Some(bag) if bag.has(&field) => {
                        let message = bag.first(&field).unwrap_or_default().to_string();
                        (true, Some(("message", ViewValue::from(message))))
                    }
                    _ => (false, None),
                }
            }
            Cond::Session(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                let key = functions::arg(&args, 0).clone();
                let value = call(self, "session", &[key])?;
                if value.is_null() {
                    (false, None)
                } else {
                    (true, Some(("value", value)))
                }
            }
            Cond::Context(args) => {
                let args = self.eval_all(args, scope, ctx, line)?;
                let key = functions::arg(&args, 0).clone();
                if call(self, "context_has", std::slice::from_ref(&key))?.truthy() {
                    (true, Some(("value", call(self, "context", &[key])?)))
                } else {
                    (false, None)
                }
            }
            Cond::Custom { name, args, negate } => {
                let args = self.eval_all(args, scope, ctx, line)?;
                let handler = self
                    .registry
                    .conditions
                    .get(name)
                    .cloned()
                    .ok_or_else(|| wrap(self, error(format!("Unknown condition [{name}]"))))?;
                (handler(&args) != *negate, None)
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_foreach(
        &mut self,
        iterable: &Expr,
        key: Option<&Expr>,
        value: &Expr,
        body: &[Node],
        empty: Option<&[Node]>,
        line: usize,
        scope: &mut Scope,
        out: &mut String,
        ctx: &ViewContext,
    ) -> Result<Flow> {
        let data = self.eval(iterable, scope, ctx, line)?;
        let items = iterate(&data).map_err(|e| self.wrap(e, ctx, line))?;
        let count = items.len() as i64;
        let parent = self
            .loops
            .last()
            .map(LoopState::to_value)
            .unwrap_or_default();
        self.loops.push(LoopState {
            iteration: 0,
            index: 0,
            remaining: count,
            count,
            first: true,
            last: count == 1,
            odd: false,
            even: true,
            depth: self.loops.len() as i64 + 1,
            parent,
        });
        let mut result = Ok(Flow::Normal);
        for (k, v) in items {
            {
                let state = self.loops.last_mut().expect("the loop was just pushed");
                let iteration = state.iteration;
                state.iteration = iteration + 1;
                state.index = iteration;
                state.first = iteration == 0;
                state.odd = !state.odd;
                state.even = !state.even;
                state.remaining -= 1;
                state.last = iteration == state.count - 1;
                scope.set("loop", state.to_value());
            }
            let assigned = {
                let mut evaluator = Evaluator::new(scope, &self.registry);
                key.map(|key| evaluator.assign(key, k))
                    .transpose()
                    .and_then(|_| evaluator.assign(value, v))
            };
            if let Err(e) = assigned {
                result = Err(self.wrap(e, ctx, line));
                break;
            }
            match self.render_nodes(body, scope, out, ctx) {
                Ok(Flow::Normal) | Ok(Flow::Continue(1)) => {}
                Ok(Flow::Break(1)) => break,
                Ok(Flow::Break(n)) => {
                    result = Ok(Flow::Break(n - 1));
                    break;
                }
                Ok(Flow::Continue(n)) => {
                    result = Ok(Flow::Continue(n - 1));
                    break;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.loops.pop();
        match self.loops.last() {
            Some(parent) => scope.set("loop", parent.to_value()),
            None => scope.set("loop", ViewValue::Null),
        }
        if count == 0
            && let Some(empty) = empty
        {
            return self.render_nodes(empty, scope, out, ctx);
        }
        result
    }

    // ------------------------------------------------------------------
    // Includes
    // ------------------------------------------------------------------

    fn include(&mut self, kind: IncludeKind, args: &[ViewValue], scope: &Scope) -> Result<String> {
        let (name, data, isolated) = match kind {
            IncludeKind::Include | IncludeKind::If | IncludeKind::Isolated => {
                let name = php::to_str(functions::arg(args, 0))?;
                if kind == IncludeKind::If && !self.factory.exists(&name) {
                    return Ok(String::new());
                }
                (
                    name,
                    functions::arg(args, 1).clone(),
                    kind == IncludeKind::Isolated,
                )
            }
            IncludeKind::When | IncludeKind::Unless => {
                let condition = functions::arg(args, 0).truthy();
                if condition != (kind == IncludeKind::When) {
                    return Ok(String::new());
                }
                (
                    php::to_str(functions::arg(args, 1))?,
                    functions::arg(args, 2).clone(),
                    false,
                )
            }
            IncludeKind::First => (
                self.first_existing(functions::arg(args, 0))?,
                functions::arg(args, 1).clone(),
                false,
            ),
        };
        let mut view_data = if isolated {
            ViewData::new()
        } else {
            scope_to_data(scope)
        };
        merge_data(&mut view_data, &data);
        self.render_named(&name, view_data)
    }

    fn each(&mut self, args: &[ViewValue]) -> Result<String> {
        let view = php::to_str(functions::arg(args, 0))?;
        let items = iterate(functions::arg(args, 1))?;
        let iterator = php::to_str(functions::arg(args, 2))?;
        let mut out = String::new();
        if items.is_empty() {
            if let Some(empty) = args.get(3) {
                let empty = php::to_str(empty)?;
                match empty.strip_prefix("raw|") {
                    Some(raw) => out.push_str(raw),
                    None => out.push_str(&self.render_named(&empty, ViewData::new())?),
                }
            }
            return Ok(out);
        }
        for (key, value) in items {
            let mut data = ViewData::new();
            data.insert("key".into(), key);
            data.insert(iterator.clone(), value);
            out.push_str(&self.render_named(&view, data)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Sections & stacks
    // ------------------------------------------------------------------

    fn extend_section(&mut self, name: &str, content: String) {
        let content = match self.sections.get(name) {
            Some(existing) => existing.replace(&parent_placeholder(name), &content),
            None => content,
        };
        self.sections.insert(name.to_string(), content);
    }

    fn yield_content(&self, name: &str, default: &str) -> String {
        let content = self
            .sections
            .get(name)
            .map(String::as_str)
            .unwrap_or(default);
        content
            .replace("@@parent", "--parent--holder--")
            .replace(&parent_placeholder(name), "")
            .replace("--parent--holder--", "@parent")
    }

    fn extend_push(&mut self, stack: &str, content: String) {
        self.pushes
            .entry(stack.to_string())
            .or_default()
            .entry(self.render_count)
            .or_default()
            .push_str(&content);
    }

    fn extend_prepend(&mut self, stack: &str, content: String) {
        let entry = self
            .prepends
            .entry(stack.to_string())
            .or_default()
            .entry(self.render_count)
            .or_default();
        entry.insert_str(0, &content);
    }

    fn is_stack_empty(&self, stack: &str) -> bool {
        !self.pushes.contains_key(stack) && !self.prepends.contains_key(stack)
    }

    fn yield_push_content(&self, stack: &str, default: &str) -> String {
        if self.is_stack_empty(stack) {
            return default.to_string();
        }
        let mut out = String::new();
        if let Some(prepends) = self.prepends.get(stack) {
            for content in prepends.values().rev() {
                out.push_str(content);
            }
        }
        if let Some(pushes) = self.pushes.get(stack) {
            for content in pushes.values() {
                out.push_str(content);
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // Components
    // ------------------------------------------------------------------

    fn evaluate_attrs(
        &self,
        attrs: &[Attr],
        scope: &mut Scope,
        ctx: &ViewContext,
        line: usize,
    ) -> Result<IndexMap<String, (ViewValue, bool)>> {
        let mut values = IndexMap::with_capacity(attrs.len());
        for attr in attrs {
            let (value, bound) = match &attr.value {
                AttrValue::True => (ViewValue::Bool(true), true),
                AttrValue::Bound(expr) => (self.eval(expr, scope, ctx, line)?, true),
                AttrValue::Static(parts) => {
                    let mut text = String::new();
                    for part in parts {
                        match part {
                            AttrPart::Text(t) => text.push_str(t),
                            AttrPart::Echo { expr, escape } => {
                                let value = self.eval(expr, scope, ctx, line)?;
                                if *escape {
                                    self.echo_escaped(&value, &mut text)
                                        .map_err(|e| self.wrap(e, ctx, line))?;
                                } else {
                                    self.echo_unescaped(&value, &mut text)
                                        .map_err(|e| self.wrap(e, ctx, line))?;
                                }
                            }
                        }
                    }
                    (ViewValue::from(text), false)
                }
            };
            values.insert(attr.name.clone(), (value, bound));
        }
        Ok(values)
    }

    fn render_component(
        &mut self,
        node: &ComponentNode,
        scope: &mut Scope,
        out: &mut String,
        ctx: &ViewContext,
    ) -> Result<Flow> {
        let line = node.line;
        let mut values = self.evaluate_attrs(&node.attrs, scope, ctx, line)?;

        // @component('view', [...]) — the original component syntax.
        if let ComponentName::Legacy { view, data, first } = &node.name {
            let view = self.eval(view, scope, ctx, line)?;
            let view = if *first {
                self.first_existing(&view)?
            } else {
                view.to_string_lossy()
            };
            let mut component_data = ViewData::new();
            if let Some(data) = data {
                let data = self.eval(data, scope, ctx, line)?;
                merge_data(&mut component_data, &data);
            }
            let frame = Arc::new(component_data);
            let (slot, slots, flow) =
                self.render_children(node, frame.clone(), None, scope, ctx)?;
            let mut component_data = (*frame).clone();
            component_data.insert("slot".into(), slot);
            for (name, value) in slots {
                component_data.insert(name, value);
            }
            let html = self.render_component_view(
                &ComponentView::View(view),
                component_data,
                frame,
                ctx,
                line,
            )?;
            out.push_str(&html);
            return Ok(flow);
        }

        let name = match &node.name {
            ComponentName::Static(name) => name.clone(),
            _ => {
                let Some((component, _)) = values.shift_remove("component") else {
                    return Err(self.wrap(
                        error("The dynamic-component requires a [component] attribute."),
                        ctx,
                        line,
                    ));
                };
                component.to_string_lossy()
            }
        };

        if let Some(factory) = self.registry.components.get(&name).cloned() {
            // A class-based component.
            let mut args = ComponentArgs {
                values,
                name: name.clone(),
            };
            let component = factory(&mut args).map_err(|e| self.wrap(e, ctx, line))?;
            if !component.should_render() {
                return Ok(Flow::Normal);
            }
            let attributes = args.into_attribute_bag();
            let mut data = component.data();
            data.insert("attributes".into(), ViewValue::object(attributes.clone()));
            data.insert("componentName".into(), ViewValue::from(name.as_str()));
            let mut frame_data = data.clone();
            for (key, value) in attributes.all() {
                frame_data
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            let frame = Arc::new(frame_data);
            let object = ViewValue::object(ComponentObject {
                data: frame.clone(),
            });
            let (slot, slots, flow) =
                self.render_children(node, frame.clone(), Some(object), scope, ctx)?;
            data.insert("slot".into(), slot);
            for (slot_name, value) in slots {
                data.insert(slot_name, value);
            }
            let view = component.render();
            let html = self.render_component_view(&view, data, frame, ctx, line)?;
            out.push_str(&html);
            return Ok(flow);
        }

        // An anonymous component.
        let view_name = self.resolve_anonymous_component(&name).ok_or_else(|| {
            self.wrap(
                InvalidArgumentException::new(format!(
                    "Unable to locate a class or view for component [{name}]."
                ))
                .into(),
                ctx,
                line,
            )
        })?;
        let mut bag_values = IndexMap::with_capacity(values.len());
        let mut data = ViewData::new();
        let mut passed_bag: Option<ComponentAttributeBag> = None;
        for (key, (value, bound)) in &values {
            if key == "attributes"
                && let Some(bag) = value.downcast_ref::<ComponentAttributeBag>()
            {
                passed_bag = Some(bag.clone());
            }
            bag_values.insert(
                key.clone(),
                if *bound && key != "attributes" {
                    sanitize(value.clone())
                } else {
                    value.clone()
                },
            );
            data.insert(Str::camel(key), value.clone());
        }
        let mut attributes = ComponentAttributeBag::new();
        attributes.set_attributes(bag_values);

        let mut component_data = ViewData::new();
        if let Some(passed) = passed_bag {
            for (key, value) in passed.all() {
                component_data.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in attributes.all() {
            component_data.insert(key.clone(), value.clone());
        }
        for (key, value) in data {
            component_data.insert(key, value);
        }
        component_data.insert("attributes".into(), ViewValue::object(attributes));

        let frame = Arc::new(component_data);
        let object = ViewValue::object(ComponentObject {
            data: frame.clone(),
        });
        let (slot, slots, flow) =
            self.render_children(node, frame.clone(), Some(object), scope, ctx)?;
        let mut component_data = (*frame).clone();
        component_data.insert("slot".into(), slot);
        for (slot_name, value) in slots {
            component_data.insert(slot_name, value);
        }
        let html = self.render_component_view(
            &ComponentView::View(view_name),
            component_data,
            frame,
            ctx,
            line,
        )?;
        out.push_str(&html);
        Ok(flow)
    }

    /// Render a component's children, collecting the default and named slots.
    fn render_children(
        &mut self,
        node: &ComponentNode,
        frame_data: Arc<ViewData>,
        component: Option<ViewValue>,
        scope: &mut Scope,
        ctx: &ViewContext,
    ) -> Result<(ViewValue, IndexMap<String, ViewValue>, Flow)> {
        let previous = component.as_ref().and_then(|_| scope.remove("component"));
        if let Some(component) = component.clone() {
            scope.set("component", component);
        }
        self.components.push(ComponentFrame {
            data: frame_data,
            slots: IndexMap::new(),
        });
        let mut default = String::new();
        let result = self.render_nodes(&node.children, scope, &mut default, ctx);
        let frame = self.components.pop().expect("the frame was just pushed");
        if component.is_some() {
            scope.remove("component");
            if let Some(previous) = previous {
                scope.set("component", previous);
            }
        }
        let flow = result?;
        let slot = ViewValue::object(ComponentSlot::new(
            php::php_trim(&default),
            ComponentAttributeBag::new(),
        ));
        Ok((slot, frame.slots, flow))
    }

    fn render_slot(&mut self, slot: &SlotNode, scope: &mut Scope, ctx: &ViewContext) -> Result<()> {
        let line = slot.line;
        if self.components.is_empty() {
            return Err(self.wrap(
                error("Slots may only be used inside a component."),
                ctx,
                line,
            ));
        }
        let name = self.eval(&slot.name, scope, ctx, line)?.to_string_lossy();
        let value = match &slot.body {
            SlotBody::Inline(expr) => self.eval(expr, scope, ctx, line)?,
            SlotBody::Nodes(nodes) => {
                let values = self.evaluate_attrs(&slot.attrs, scope, ctx, line)?;
                let mut attributes = ComponentAttributeBag::new();
                attributes.set_attributes(
                    values
                        .into_iter()
                        .map(|(k, (v, bound))| {
                            (
                                k.clone(),
                                if bound && k != "attributes" {
                                    sanitize(v)
                                } else {
                                    v
                                },
                            )
                        })
                        .collect(),
                );
                let mut content = String::new();
                self.render_nodes(nodes, scope, &mut content, ctx)?;
                ViewValue::object(ComponentSlot::new(php::php_trim(&content), attributes))
            }
        };
        if let Some(frame) = self.components.last_mut() {
            frame.slots.insert(name, value);
        }
        Ok(())
    }

    fn render_component_view(
        &mut self,
        view: &ComponentView,
        data: ViewData,
        frame: Arc<ViewData>,
        ctx: &ViewContext,
        line: usize,
    ) -> Result<String> {
        self.component_data.push(frame);
        let result = match view {
            ComponentView::View(name) => self.render_named(name, data),
            ComponentView::Inline(template) => {
                if self.factory.exists(template) {
                    self.render_named(template, data)
                } else {
                    let compiled = self.factory.blade().compile_string(template).map_err(|e| {
                        ViewException::wrap(e, "__components::inline", None, 0).into()
                    });
                    match compiled {
                        Ok(compiled) => {
                            let inline_ctx = ViewContext {
                                name: "__components::inline".into(),
                                path: None,
                            };
                            let mut view =
                                View::inline_template(&self.factory, compiled.clone(), data);
                            self.factory.call_composers(&mut view);
                            self.render_template(&compiled, &inline_ctx, view.data())
                        }
                        Err(e) => Err(e),
                    }
                }
            }
        };
        self.component_data.pop();
        result.map_err(|e| self.wrap(e, ctx, line))
    }

    /// Laravel's `getConsumableComponentData`: the data of the components
    /// being rendered (innermost first), then of the components whose slots
    /// are being rendered.
    fn consumable_component_data(&self, key: &str) -> Option<ViewValue> {
        self.component_data
            .iter()
            .rev()
            .find_map(|data| data.get(key).cloned())
            .or_else(|| {
                self.components
                    .iter()
                    .rev()
                    .find_map(|frame| frame.data.get(key).cloned())
            })
    }

    /// Resolve an anonymous component's view, once per render.
    fn resolve_anonymous_component(&mut self, name: &str) -> Option<String> {
        if let Some(found) = self.anonymous_components.get(name) {
            return found.clone();
        }
        let found = self.find_anonymous_component(name);
        self.anonymous_components
            .insert(name.to_string(), found.clone());
        found
    }

    fn find_anonymous_component(&self, name: &str) -> Option<String> {
        let registry = &self.registry;
        let last_segment = |n: &str| {
            n.rsplit('.')
                .next()
                .unwrap_or(n)
                .rsplit(':')
                .next()
                .unwrap_or(n)
                .to_string()
        };

        // Registered anonymous component namespaces, then the default `components` directory.
        let mut candidates: Vec<(String, String)> = Vec::new();
        for (prefix, directory) in &registry.anonymous_namespaces {
            if let Some(rest) = name.strip_prefix(&format!("{prefix}::")) {
                candidates.push((rest.to_string(), format!("{directory}.")));
            }
        }
        candidates.push((name.to_string(), "components.".to_string()));
        for (component, prefix) in candidates {
            let base = match component.split_once("::") {
                Some((namespace, rest)) => format!("{namespace}::{prefix}{rest}"),
                None => format!("{prefix}{component}"),
            };
            for guess in [
                base.clone(),
                format!("{base}.index"),
                format!("{base}.{}", last_segment(&component)),
            ] {
                if self.factory.exists(&guess) {
                    return Some(guess);
                }
            }
        }

        // Extra anonymous component paths.
        for path in &registry.anonymous_paths {
            let component = match (&path.prefix, name.split_once("::")) {
                (Some(prefix), Some((namespace, rest))) if namespace == prefix => rest.to_string(),
                (_, Some(_)) => continue,
                (Some(_), None) => continue,
                (None, None) => name.to_string(),
            };
            let last = last_segment(&component);
            for guess in [
                component.clone(),
                format!("{component}.index"),
                format!("{component}.{last}"),
            ] {
                if let Some(found) = self.factory.find_in_directory(&path.path, &guess) {
                    return Some(format!("__path::{}", found.display()));
                }
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Output directives
    // ------------------------------------------------------------------

    fn output(
        &mut self,
        directive: &OutputDirective,
        args: Vec<ViewValue>,
        scope: &mut Scope,
        out: &mut String,
    ) -> Result<()> {
        let a0 = functions::arg(&args, 0);
        let call = |name: &str, args: &[ViewValue]| call_function(name, args, &self.registry);
        match directive {
            OutputDirective::Csrf => echo_raw(&call("csrf_field", &[])?, out)?,
            OutputDirective::Method => echo_raw(&call("method_field", &args)?, out)?,
            OutputDirective::Json => {
                let flags = match args.get(1) {
                    Some(flags) => flags.as_i64().unwrap_or(0),
                    None => php::BLADE_JSON_FLAGS,
                };
                out.push_str(&php::json_encode(a0, flags)?);
            }
            OutputDirective::Js => {
                out.push_str(&php::js_from(a0, functions::int_arg(&args, 1, 0))?)
            }
            OutputDirective::Class => {
                out.push_str("class=\"");
                out.push_str(&php::css_classes(&wrap_list(a0)));
                out.push('"');
            }
            OutputDirective::Style => {
                out.push_str("style=\"");
                out.push_str(&php::css_styles(&wrap_list(a0)));
                out.push('"');
            }
            OutputDirective::Checked
            | OutputDirective::Selected
            | OutputDirective::Disabled
            | OutputDirective::Readonly
            | OutputDirective::Required => {
                if a0.truthy() {
                    out.push_str(match directive {
                        OutputDirective::Checked => "checked",
                        OutputDirective::Selected => "selected",
                        OutputDirective::Disabled => "disabled",
                        OutputDirective::Readonly => "readonly",
                        _ => "required",
                    });
                }
            }
            OutputDirective::Bool => out.push_str(if a0.truthy() { "true" } else { "false" }),
            OutputDirective::Lang => echo_raw(&call("__", &args)?, out)?,
            OutputDirective::Choice => echo_raw(&call("trans_choice", &args)?, out)?,
            OutputDirective::Dump => {
                for value in &args {
                    out.push_str(&functions::dump_html(value));
                }
            }
            OutputDirective::Dd => {
                call("dd", &args)?;
            }
            OutputDirective::Vite => echo_raw(&call("vite", &args)?, out)?,
            OutputDirective::ViteReactRefresh => echo_raw(&call("vite_react_refresh", &[])?, out)?,
            OutputDirective::Fonts => echo_raw(&call("fonts", &args)?, out)?,
            OutputDirective::Inject => {
                let variable = a0.to_string_lossy();
                let service = call("app", &args[1..])?;
                scope.set(variable.as_str(), service);
            }
            OutputDirective::Custom(name) => {
                let handler = self
                    .registry
                    .directives
                    .get(name)
                    .cloned()
                    .ok_or_else(|| error(format!("Unknown directive [@{name}]")))?;
                out.push_str(&handler(&args)?);
            }
        }
        Ok(())
    }
}

fn wrap_list(value: &ViewValue) -> ViewValue {
    match value {
        ViewValue::Array(_) => value.clone(),
        ViewValue::Null => ViewValue::empty_array(),
        other => ViewValue::list([other.clone()]),
    }
}

/// Echo a value without escaping (`{!! !!}`).
fn echo_raw(value: &ViewValue, out: &mut String) -> Result<()> {
    match value {
        ViewValue::Closure(_) => Err(error(
            "Object of class Closure could not be converted to string",
        )),
        ViewValue::Object(object) => {
            out.push_str(
                &object
                    .to_html()
                    .or_else(|| object.to_string_value())
                    .unwrap_or_else(|| value.to_string_lossy()),
            );
            Ok(())
        }
        other => {
            out.push_str(&other.to_string_lossy());
            Ok(())
        }
    }
}

/// The variables of a scope, as view data (for includes and layouts).
fn scope_to_data(scope: &Scope) -> ViewData {
    let mut data = ViewData::with_capacity(scope.len());
    for (key, value) in scope.iter() {
        data.insert(key.to_string(), value.clone());
    }
    data
}

/// Merge an array value into view data.
fn merge_data(data: &mut ViewData, value: &ViewValue) {
    if let Some(array) = functions::to_array(value) {
        for (key, value) in array.iter() {
            data.insert(key.to_string(), value.clone());
        }
    }
}

/// Apply `@props`: pull the declared props out of `$attributes` into
/// variables, apply defaults, and hide non-prop attributes.
fn apply_props(props: &ViewValue, scope: &mut Scope) {
    let attributes = scope
        .get("attributes")
        .and_then(|a| a.downcast_ref::<ComponentAttributeBag>().cloned())
        .unwrap_or_default();
    let names = prop_names(props);
    let mut remaining = IndexMap::new();
    for (key, value) in attributes.all() {
        if names.contains(key) {
            if scope.get(key).is_none_or(ViewValue::is_null) {
                scope.set(key.as_str(), value.clone());
            }
        } else {
            remaining.insert(key.clone(), value.clone());
        }
    }
    let mut bag = ComponentAttributeBag::new();
    bag.set_attributes(remaining.clone());
    scope.set("attributes", ViewValue::object(bag));
    if let ViewValue::Array(list) = props {
        for (key, default) in list.iter() {
            if let ArrayKey::Str(name) = key
                && scope.get(name).is_none_or(ViewValue::is_null)
            {
                scope.set(name.clone(), default.clone());
            }
        }
    }
    for key in remaining.keys() {
        scope.remove(key);
    }
}
