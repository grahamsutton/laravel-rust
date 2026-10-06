//! Evaluating expressions against a view's variables.

use std::collections::HashMap;
use std::sync::Arc;

use illuminate_support::Result;

use super::{Arg, BinaryOp, CastType, ClosureDef, Expr, Stmt, UnaryOp};
use crate::exception::{BadMethodCallException, TypeError, error};
use crate::php::{self, Arith};
use crate::registry::Registry;
use crate::value::{ArrayKey, ViewArray, ViewClosure, ViewValue};
use crate::{functions, methods, statics};

/// The variables visible to a template.
#[derive(Clone, Default)]
pub(crate) struct Scope {
    vars: HashMap<Arc<str>, ViewValue>,
}

impl Scope {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self { vars: HashMap::with_capacity(capacity) }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&ViewValue> {
        self.vars.get(name)
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.vars.contains_key(name)
    }

    pub(crate) fn set(&mut self, name: impl Into<Arc<str>>, value: ViewValue) {
        self.vars.insert(name.into(), value);
    }

    pub(crate) fn remove(&mut self, name: &str) -> Option<ViewValue> {
        self.vars.remove(name)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Arc<str>, &ViewValue)> {
        self.vars.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.vars.len()
    }

    fn slot(&mut self, name: &Arc<str>) -> &mut ViewValue {
        self.vars.entry(name.clone()).or_insert(ViewValue::Null)
    }
}

/// Control flow out of statements.
pub(crate) enum Flow {
    Normal,
    Return(ViewValue),
    Break,
    Continue,
}

/// A segment of an assignment target path.
enum PathSeg {
    Key(ViewValue),
    Push,
}

/// Evaluates expressions and statements.
pub(crate) struct Evaluator<'a> {
    pub(crate) scope: &'a mut Scope,
    pub(crate) registry: &'a Arc<Registry>,
}

impl<'a> Evaluator<'a> {
    pub(crate) fn new(scope: &'a mut Scope, registry: &'a Arc<Registry>) -> Self {
        Self { scope, registry }
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    /// Execute statements, writing `echo` output to `out`.
    pub(crate) fn exec(&mut self, stmts: &[Stmt], out: &mut String) -> Result<Flow> {
        for stmt in stmts {
            match self.exec_one(stmt, out)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_one(&mut self, stmt: &Stmt, out: &mut String) -> Result<Flow> {
        match stmt {
            Stmt::Expr(expr) => {
                self.eval(expr)?;
            }
            Stmt::Echo(exprs) => {
                for expr in exprs {
                    let value = self.eval(expr)?;
                    out.push_str(&php::to_str(&value)?);
                }
            }
            Stmt::Unset(targets) => {
                for target in targets {
                    self.unset(target)?;
                }
            }
            Stmt::Return(value) => {
                let value = match value {
                    Some(expr) => self.eval(expr)?,
                    None => ViewValue::Null,
                };
                return Ok(Flow::Return(value));
            }
            Stmt::If { branches, otherwise } => {
                for (cond, body) in branches {
                    if self.eval(cond)?.truthy() {
                        return self.exec(body, out);
                    }
                }
                if let Some(body) = otherwise {
                    return self.exec(body, out);
                }
            }
            Stmt::Foreach { iterable, key, value, body } => {
                let iterable = self.eval(iterable)?;
                for (k, v) in iterate(&iterable)? {
                    if let Some(key) = key {
                        self.assign(key, k)?;
                    }
                    self.assign(value, v)?;
                    match self.exec(body, out)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Normal | Flow::Continue => {}
                    }
                }
            }
            Stmt::Break => return Ok(Flow::Break),
            Stmt::Continue => return Ok(Flow::Continue),
        }
        Ok(Flow::Normal)
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    /// Evaluate an expression.
    pub(crate) fn eval(&mut self, expr: &Expr) -> Result<ViewValue> {
        match expr {
            Expr::Lit(value) => Ok(value.clone()),
            Expr::Interp(parts) => {
                let mut out = String::new();
                for part in parts {
                    let value = self.eval(part)?;
                    out.push_str(&php::to_str(&value)?);
                }
                Ok(ViewValue::from(out))
            }
            Expr::Array(items) => self.eval_array(items),
            Expr::Var(name) => match self.scope.get(name) {
                Some(value) => Ok(value.clone()),
                None => Err(error(format!("Undefined variable ${name}"))),
            },
            Expr::Const(name) => constant(name, self.registry),
            Expr::ClassConst { class, name } => {
                if name.eq_ignore_ascii_case("class") {
                    return Ok(ViewValue::from(&**class));
                }
                let key = format!("{class}::{name}");
                match self.registry.functions.get(&key) {
                    Some(function) => function(&[]),
                    None => Err(error(format!("Undefined constant {key}"))),
                }
            }
            Expr::Prop { target, name, nullsafe } => {
                let target = self.eval(target)?;
                if *nullsafe && target.is_null() {
                    return Ok(ViewValue::Null);
                }
                let name = self.eval(name)?;
                read_property(&target, &php::to_str(&name)?)
            }
            Expr::Index { target, index } => {
                let target = self.eval(target)?;
                let Some(index) = index else {
                    return Err(error("Cannot use [] for reading"));
                };
                let index = self.eval(index)?;
                read_offset(&target, &index)
            }
            Expr::MethodCall { target, name, args, nullsafe } => {
                let target = self.eval(target)?;
                if *nullsafe && target.is_null() {
                    return Ok(ViewValue::Null);
                }
                let args = self.eval_args(args)?;
                methods::call_method(&target, name, args, self.registry)
            }
            Expr::Call { name, args } => {
                let args = self.eval_args(args)?;
                call_function(name, &args, self.registry)
            }
            Expr::StaticCall { class, method, args } => {
                let args = self.eval_args(args)?;
                statics::call_static(class, method, &args, self.registry)
            }
            Expr::Invoke { callee, args } => {
                let callee = self.eval(callee)?;
                let args = self.eval_args(args)?;
                call_callable(&callee, &args, self.registry)
            }
            Expr::Unary { op, expr } => {
                let value = match op {
                    UnaryOp::Silence => self.eval_quiet(expr)?.unwrap_or_default(),
                    _ => self.eval(expr)?,
                };
                unary(*op, value)
            }
            Expr::Binary { op, left, right } => self.eval_binary(*op, left, right),
            Expr::Ternary { cond, then, otherwise } => {
                let cond_value = self.eval(cond)?;
                if cond_value.truthy() {
                    match then {
                        Some(then) => self.eval(then),
                        None => Ok(cond_value),
                    }
                } else {
                    self.eval(otherwise)
                }
            }
            Expr::Isset(exprs) => {
                for expr in exprs {
                    match self.eval_quiet(expr)? {
                        Some(value) if !value.is_null() => {}
                        _ => return Ok(ViewValue::Bool(false)),
                    }
                }
                Ok(ViewValue::Bool(true))
            }
            Expr::Empty(expr) => {
                let value = self.eval_quiet(expr)?;
                Ok(ViewValue::Bool(!value.is_some_and(|v| v.truthy())))
            }
            Expr::Assign { target, op, value } => self.eval_assign(target, *op, value),
            Expr::IncDec { target, increment, prefix } => {
                let current = self.eval_quiet(target)?.unwrap_or_default();
                let next = match &current {
                    ViewValue::Null if *increment => ViewValue::Int(1),
                    ViewValue::Null => ViewValue::Null,
                    ViewValue::Str(s) if *increment && php::parse_numeric(s).is_none() => {
                        ViewValue::from(string_increment(s))
                    }
                    other => php::arithmetic(
                        if *increment { Arith::Add } else { Arith::Sub },
                        other,
                        &ViewValue::Int(1),
                    )?,
                };
                self.assign(target, next.clone())?;
                Ok(if *prefix { next } else { current })
            }
            Expr::Closure(def) => Ok(self.make_closure(def)),
            Expr::Match { subject, arms } => {
                let subject = self.eval(subject)?;
                let mut default = None;
                for arm in arms {
                    match &arm.conditions {
                        Some(conditions) => {
                            for condition in conditions {
                                if php::strict_eq(&self.eval(condition)?, &subject) {
                                    return self.eval(&arm.result);
                                }
                            }
                        }
                        None => default = Some(&arm.result),
                    }
                }
                match default {
                    Some(result) => self.eval(result),
                    None => Err(crate::exception::RuntimeException::new(format!(
                        "Unhandled match case {}",
                        php::json_encode(&subject, 0).unwrap_or_default()
                    ))
                    .into()),
                }
            }
            Expr::Cast { ty, expr } => {
                let value = self.eval(expr)?;
                cast(*ty, value)
            }
        }
    }

    fn eval_array(&mut self, items: &[super::ArrayItem]) -> Result<ViewValue> {
        let mut array = ViewArray::with_capacity(items.len());
        for item in items {
            let value = self.eval(&item.value)?;
            if item.spread {
                for (key, value) in iterate(&value)? {
                    match key {
                        ViewValue::Int(_) => array.push(value),
                        key => array.insert(ArrayKey::from_value(&key)?, value),
                    }
                }
                continue;
            }
            match &item.key {
                Some(key) => {
                    let key = self.eval(key)?;
                    array.insert(ArrayKey::from_value(&key)?, value);
                }
                None => array.push(value),
            }
        }
        Ok(ViewValue::Array(Arc::new(array)))
    }

    pub(crate) fn eval_args(&mut self, args: &[Arg]) -> Result<Vec<ViewValue>> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let value = self.eval(&arg.value)?;
            if arg.spread {
                values.extend(iterate(&value)?.into_iter().map(|(_, v)| v));
            } else {
                values.push(value);
            }
        }
        Ok(values)
    }

    /// Evaluate an expression without complaining about undefined variables,
    /// keys or properties (for `isset`, `empty`, `??` and `@`).
    pub(crate) fn eval_quiet(&mut self, expr: &Expr) -> Result<Option<ViewValue>> {
        match expr {
            Expr::Var(name) => Ok(self.scope.get(name).cloned()),
            Expr::Prop { target, name, .. } => {
                let Some(target) = self.eval_quiet(target)? else { return Ok(None) };
                let name = self.eval(name)?;
                let name = php::to_str(&name)?;
                Ok(match &target {
                    ViewValue::Array(array) => array.get_str(&name).cloned(),
                    ViewValue::Object(object) => object.get(&name),
                    ViewValue::Null => None,
                    other => read_property(other, &name).ok(),
                })
            }
            Expr::Index { target, index: Some(index) } => {
                let Some(target) = self.eval_quiet(target)? else { return Ok(None) };
                let index = self.eval(index)?;
                Ok(match &target {
                    ViewValue::Array(array) => array.get_value(&index).cloned(),
                    ViewValue::Object(object) => object.offset_get(&index),
                    ViewValue::Str(_) => read_offset(&target, &index).ok().filter(|v| v.truthy() || v.as_str() == Some("0")),
                    _ => None,
                })
            }
            other => self.eval(other).map(Some),
        }
    }

    fn eval_binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr) -> Result<ViewValue> {
        use BinaryOp::*;
        match op {
            And => {
                let l = self.eval(left)?.truthy();
                Ok(ViewValue::Bool(l && self.eval(right)?.truthy()))
            }
            Or => {
                let l = self.eval(left)?.truthy();
                Ok(ViewValue::Bool(l || self.eval(right)?.truthy()))
            }
            Coalesce => match self.eval_quiet(left)? {
                Some(value) if !value.is_null() => Ok(value),
                _ => self.eval(right),
            },
            _ => {
                let l = self.eval(left)?;
                let r = self.eval(right)?;
                binary(op, &l, &r)
            }
        }
    }

    fn eval_assign(&mut self, target: &Expr, op: Option<BinaryOp>, value: &Expr) -> Result<ViewValue> {
        let new_value = match op {
            None => self.eval(value)?,
            Some(BinaryOp::Coalesce) => match self.eval_quiet(target)? {
                Some(current) if !current.is_null() => return Ok(current),
                _ => self.eval(value)?,
            },
            Some(op) => {
                let current = match self.eval_quiet(target)? {
                    Some(current) => current,
                    None => {
                        if let Expr::Var(name) = target {
                            return Err(error(format!("Undefined variable ${name}")));
                        }
                        ViewValue::Null
                    }
                };
                let rhs = self.eval(value)?;
                binary(op, &current, &rhs)?
            }
        };
        self.assign(target, new_value.clone())?;
        Ok(new_value)
    }

    /// Assign a value to an assignable expression.
    pub(crate) fn assign(&mut self, target: &Expr, value: ViewValue) -> Result<()> {
        if let Expr::Array(items) = target {
            // Destructuring: [$a, $b] = $pair / ['x' => $x] = $point.
            let mut position = 0i64;
            for item in items {
                let element = match &item.key {
                    Some(key) => {
                        let key = self.eval(key)?;
                        read_offset(&value, &key)?
                    }
                    None => {
                        let element = read_offset(&value, &ViewValue::Int(position))?;
                        position += 1;
                        element
                    }
                };
                self.assign(&item.value, element)?;
            }
            return Ok(());
        }
        let (root, path) = self.resolve_path(target)?;
        let mut slot = self.scope.slot(&root);
        for segment in &path {
            slot = descend(slot, segment)?;
        }
        *slot = value;
        Ok(())
    }

    fn resolve_path(&mut self, target: &Expr) -> Result<(Arc<str>, Vec<PathSeg>)> {
        match target {
            Expr::Var(name) => Ok((name.clone(), Vec::new())),
            Expr::Index { target, index } => {
                let (root, mut path) = self.resolve_path(target)?;
                match index {
                    Some(index) => path.push(PathSeg::Key(self.eval(index)?)),
                    None => path.push(PathSeg::Push),
                }
                Ok((root, path))
            }
            Expr::Prop { target, name, .. } => {
                let (root, mut path) = self.resolve_path(target)?;
                path.push(PathSeg::Key(self.eval(name)?));
                Ok((root, path))
            }
            _ => Err(error("Cannot assign to this expression")),
        }
    }

    fn unset(&mut self, target: &Expr) -> Result<()> {
        match target {
            Expr::Var(name) => {
                self.scope.remove(name);
                Ok(())
            }
            Expr::Index { target: base, index: Some(index) } | Expr::Prop { target: base, name: index, .. } => {
                let key = self.eval(index)?;
                let (root, path) = self.resolve_path(base)?;
                if !self.scope.contains(&root) {
                    return Ok(());
                }
                let mut slot = self.scope.slot(&root);
                for segment in &path {
                    slot = descend(slot, segment)?;
                }
                if let ViewValue::Array(array) = slot {
                    Arc::make_mut(array).remove(&ArrayKey::from_value(&key)?);
                }
                Ok(())
            }
            _ => Err(error("Cannot unset this expression")),
        }
    }

    fn make_closure(&mut self, def: &Arc<ClosureDef>) -> ViewValue {
        let mut captured = Scope::with_capacity(def.captures.len());
        for name in &def.captures {
            if let Some(value) = self.scope.get(name) {
                captured.set(name.clone(), value.clone());
            }
        }
        let def = def.clone();
        let registry = self.registry.clone();
        ViewValue::Closure(ViewClosure::new(move |args| {
            let mut scope = captured.clone();
            for (index, param) in def.params.iter().enumerate() {
                let value = match args.get(index) {
                    Some(value) => value.clone(),
                    None => match &param.default {
                        Some(default) => Evaluator::new(&mut scope, &registry).eval(default)?,
                        None => ViewValue::Null,
                    },
                };
                scope.set(param.name.clone(), value);
            }
            let mut out = String::new();
            match Evaluator::new(&mut scope, &registry).exec(&def.body, &mut out)? {
                Flow::Return(value) => Ok(value),
                _ => Ok(ViewValue::Null),
            }
        }))
    }
}

/// Walk one step into a value for assignment, creating arrays as needed.
fn descend<'v>(slot: &'v mut ViewValue, segment: &PathSeg) -> Result<&'v mut ViewValue> {
    if slot.is_null() {
        *slot = ViewValue::empty_array();
    }
    match slot {
        ViewValue::Array(array) => {
            let array = Arc::make_mut(array);
            match segment {
                PathSeg::Push => {
                    array.push(ViewValue::Null);
                    let key = array.keys().last().cloned().expect("an item was just pushed");
                    Ok(array.get_mut(&key).expect("the key exists"))
                }
                PathSeg::Key(key) => {
                    let key = ArrayKey::from_value(key)?;
                    if array.get(&key).is_none() {
                        array.insert(key.clone(), ViewValue::Null);
                    }
                    Ok(array.get_mut(&key).expect("the key exists"))
                }
            }
        }
        ViewValue::Object(object) => Err(error(format!("Cannot modify properties of {}", object.class_name()))),
        other => Err(error(format!("Cannot use a scalar value ({}) as an array", other.type_name()))),
    }
}

// ----------------------------------------------------------------------
// Operations
// ----------------------------------------------------------------------

fn unary(op: UnaryOp, value: ViewValue) -> Result<ViewValue> {
    Ok(match op {
        UnaryOp::Not => ViewValue::Bool(!value.truthy()),
        UnaryOp::Neg => php::arithmetic(Arith::Mul, &value, &ViewValue::Int(-1))?,
        UnaryOp::Plus => php::arithmetic(Arith::Mul, &value, &ViewValue::Int(1))?,
        UnaryOp::BitNot => ViewValue::Int(!php::to_number(&value, "~", &ViewValue::Null)?.as_i64()),
        UnaryOp::Silence => value,
    })
}

/// Apply a (non short-circuiting) binary operator.
pub(crate) fn binary(op: BinaryOp, l: &ViewValue, r: &ViewValue) -> Result<ViewValue> {
    use BinaryOp::*;
    let int = |v: &ViewValue, sym: &str| php::to_number(v, sym, r).map(|n| n.as_i64());
    Ok(match op {
        Add => php::arithmetic(Arith::Add, l, r)?,
        Sub => php::arithmetic(Arith::Sub, l, r)?,
        Mul => php::arithmetic(Arith::Mul, l, r)?,
        Div => php::arithmetic(Arith::Div, l, r)?,
        Mod => php::arithmetic(Arith::Mod, l, r)?,
        Pow => php::arithmetic(Arith::Pow, l, r)?,
        Concat => {
            let mut s = php::to_str(l)?;
            s.push_str(&php::to_str(r)?);
            ViewValue::from(s)
        }
        Eq => ViewValue::Bool(php::loose_eq(l, r)),
        NotEq => ViewValue::Bool(!php::loose_eq(l, r)),
        Identical => ViewValue::Bool(php::strict_eq(l, r)),
        NotIdentical => ViewValue::Bool(!php::strict_eq(l, r)),
        Lt => ViewValue::Bool(php::compare(l, r).is_lt()),
        Le => ViewValue::Bool(php::compare(l, r).is_le()),
        Gt => ViewValue::Bool(php::compare(l, r).is_gt()),
        Ge => ViewValue::Bool(php::compare(l, r).is_ge()),
        Spaceship => ViewValue::Int(php::compare(l, r) as i64),
        And => ViewValue::Bool(l.truthy() && r.truthy()),
        Or => ViewValue::Bool(l.truthy() || r.truthy()),
        Xor => ViewValue::Bool(l.truthy() != r.truthy()),
        BitAnd => ViewValue::Int(int(l, "&")? & int(r, "&")?),
        BitOr => ViewValue::Int(int(l, "|")? | int(r, "|")?),
        BitXor => ViewValue::Int(int(l, "^")? ^ int(r, "^")?),
        Shl => ViewValue::Int(int(l, "<<")?.wrapping_shl(int(r, "<<")? as u32)),
        Shr => ViewValue::Int(int(l, ">>")?.wrapping_shr(int(r, ">>")? as u32)),
        Coalesce => {
            if l.is_null() {
                r.clone()
            } else {
                l.clone()
            }
        }
    })
}

fn cast(ty: CastType, value: ViewValue) -> Result<ViewValue> {
    Ok(match ty {
        CastType::Int => ViewValue::Int(match &value {
            ViewValue::Str(s) => php::parse_numeric(s)
                .map(|n| n.as_i64())
                .unwrap_or_else(|| leading_int(s)),
            ViewValue::Array(a) => !a.is_empty() as i64,
            other => other.as_i64().unwrap_or(1),
        }),
        CastType::Float => ViewValue::Float(match &value {
            ViewValue::Str(s) => php::parse_numeric(s).map(|n| n.as_f64()).unwrap_or(0.0),
            ViewValue::Array(a) => !a.is_empty() as i64 as f64,
            other => other.as_f64().unwrap_or(1.0),
        }),
        CastType::String => ViewValue::from(php::to_str(&value)?),
        CastType::Bool => ViewValue::Bool(value.truthy()),
        CastType::Array | CastType::Object => match value {
            ViewValue::Array(_) => value,
            ViewValue::Null => ViewValue::empty_array(),
            ViewValue::Object(object) => ViewValue::from(object.to_json()),
            other => ViewValue::list([other]),
        },
    })
}

fn leading_int(s: &str) -> i64 {
    let t = s.trim_start();
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    t[..end].parse().unwrap_or(0)
}

/// PHP's alphanumeric string increment ("a" → "b", "Az" → "Ba", "zz" → "aaa").
fn string_increment(s: &str) -> String {
    if s.is_empty() {
        return "1".into();
    }
    let mut chars: Vec<char> = s.chars().collect();
    let mut i = chars.len();
    loop {
        if i == 0 {
            let first = chars[0];
            let prefix = if first.is_ascii_digit() {
                '1'
            } else if first.is_ascii_uppercase() {
                'A'
            } else {
                'a'
            };
            chars.insert(0, prefix);
            break;
        }
        i -= 1;
        match chars[i] {
            'z' => chars[i] = 'a',
            'Z' => chars[i] = 'A',
            '9' => chars[i] = '0',
            c if c.is_ascii_alphanumeric() => {
                chars[i] = (c as u8 + 1) as char;
                break;
            }
            _ => break,
        }
    }
    chars.into_iter().collect()
}

// ----------------------------------------------------------------------
// Reading values
// ----------------------------------------------------------------------

/// Read `$target->name`.
pub(crate) fn read_property(target: &ViewValue, name: &str) -> Result<ViewValue> {
    match target {
        ViewValue::Array(array) => Ok(array.get_str(name).cloned().unwrap_or_default()),
        ViewValue::Object(object) => object
            .get(name)
            .ok_or_else(|| error(format!("Undefined property: {}::${name}", object.class_name()))),
        ViewValue::Str(s) => methods::date_property(s, name).ok_or_else(|| {
            error(format!("Attempt to read property \"{name}\" on string"))
        }),
        ViewValue::Null => Err(error(format!("Attempt to read property \"{name}\" on null"))),
        other => Err(error(format!("Attempt to read property \"{name}\" on {}", other.type_name()))),
    }
}

/// Read `$target[$index]`.
pub(crate) fn read_offset(target: &ViewValue, index: &ViewValue) -> Result<ViewValue> {
    match target {
        ViewValue::Array(array) => Ok(array.get_value(index).cloned().unwrap_or_default()),
        ViewValue::Object(object) => Ok(object.offset_get(index).unwrap_or_default()),
        ViewValue::Str(s) => {
            let Some(position) = index.as_i64() else {
                return Err(TypeError::new(format!("Cannot access offset of type {} on string", index.type_name())).into());
            };
            let chars: Vec<char> = s.chars().collect();
            let position = if position < 0 { chars.len() as i64 + position } else { position };
            Ok(chars
                .get(position.max(0) as usize)
                .filter(|_| position >= 0)
                .map(|c| ViewValue::from(c.to_string()))
                .unwrap_or_else(|| ViewValue::from("")))
        }
        _ => Ok(ViewValue::Null),
    }
}

/// The key / value pairs of an iterable value.
pub(crate) fn iterate(value: &ViewValue) -> Result<Vec<(ViewValue, ViewValue)>> {
    match value {
        ViewValue::Array(array) => {
            if let Some(items) = methods::paginator_items(array) {
                return Ok(items.iter().map(|(k, v)| (k.to_value(), v.clone())).collect());
            }
            Ok(array.iter().map(|(k, v)| (k.to_value(), v.clone())).collect())
        }
        ViewValue::Null => Ok(Vec::new()),
        ViewValue::Object(object) => object.iterate().ok_or_else(|| {
            TypeError::new(format!("{} is not iterable", object.class_name())).into()
        }),
        other => Err(TypeError::new(format!(
            "foreach() argument must be of type array|object, {} given",
            other.type_name()
        ))
        .into()),
    }
}

/// Call a function by name: registered functions first, then built-ins.
pub(crate) fn call_function(name: &str, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    if let Some(function) = registry.functions.get(name) {
        return function(args);
    }
    let lower = name.to_ascii_lowercase();
    if lower != name {
        if let Some(function) = registry.functions.get(lower.as_str()) {
            return function(args);
        }
    }
    match functions::call_builtin(&lower, args, registry) {
        Some(result) => result,
        None => Err(BadMethodCallException::new(format!("Call to undefined function {name}()")).into()),
    }
}

/// Determine if a function exists.
pub(crate) fn function_exists(name: &str, registry: &Registry) -> bool {
    registry.functions.contains_key(name) || functions::is_builtin(&name.to_ascii_lowercase())
}

/// Call anything callable: closures, invokable objects, and function names.
pub(crate) fn call_callable(callee: &ViewValue, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
    match callee {
        ViewValue::Closure(closure) => closure.call(args),
        ViewValue::Object(object) => object
            .invoke(args)
            .unwrap_or_else(|| Err(error(format!("Object of type {} is not callable", object.class_name())))),
        ViewValue::Str(name) => {
            if let Some((class, method)) = name.split_once("::") {
                statics::call_static(super::parser::class_basename(class), method, args, registry)
            } else {
                call_function(name, args, registry)
            }
        }
        other => Err(error(format!("Value of type {} is not callable", other.type_name()))),
    }
}

/// Resolve a constant.
fn constant(name: &str, registry: &Registry) -> Result<ViewValue> {
    use crate::php::*;
    let value = match name {
        "PHP_EOL" => ViewValue::from("\n"),
        "PHP_INT_MAX" => ViewValue::Int(i64::MAX),
        "PHP_INT_MIN" => ViewValue::Int(i64::MIN),
        "PHP_INT_SIZE" => ViewValue::Int(8),
        "PHP_FLOAT_EPSILON" => ViewValue::Float(f64::EPSILON),
        "PHP_FLOAT_MAX" => ViewValue::Float(f64::MAX),
        "M_PI" => ViewValue::Float(std::f64::consts::PI),
        "M_E" => ViewValue::Float(std::f64::consts::E),
        "NAN" => ViewValue::Float(f64::NAN),
        "INF" => ViewValue::Float(f64::INFINITY),
        "JSON_HEX_TAG" => ViewValue::Int(JSON_HEX_TAG),
        "JSON_HEX_AMP" => ViewValue::Int(JSON_HEX_AMP),
        "JSON_HEX_APOS" => ViewValue::Int(JSON_HEX_APOS),
        "JSON_HEX_QUOT" => ViewValue::Int(JSON_HEX_QUOT),
        "JSON_FORCE_OBJECT" => ViewValue::Int(JSON_FORCE_OBJECT),
        "JSON_NUMERIC_CHECK" => ViewValue::Int(JSON_NUMERIC_CHECK),
        "JSON_UNESCAPED_SLASHES" => ViewValue::Int(JSON_UNESCAPED_SLASHES),
        "JSON_PRETTY_PRINT" => ViewValue::Int(JSON_PRETTY_PRINT),
        "JSON_UNESCAPED_UNICODE" => ViewValue::Int(JSON_UNESCAPED_UNICODE),
        "JSON_PRESERVE_ZERO_FRACTION" => ViewValue::Int(JSON_PRESERVE_ZERO_FRACTION),
        "JSON_THROW_ON_ERROR" => ViewValue::Int(JSON_THROW_ON_ERROR),
        "JSON_PARTIAL_OUTPUT_ON_ERROR" => ViewValue::Int(512),
        "JSON_INVALID_UTF8_SUBSTITUTE" => ViewValue::Int(2_097_152),
        "ENT_QUOTES" => ViewValue::Int(3),
        "ENT_COMPAT" => ViewValue::Int(2),
        "ENT_NOQUOTES" => ViewValue::Int(0),
        "ENT_HTML5" => ViewValue::Int(48),
        "ENT_HTML401" => ViewValue::Int(0),
        "ENT_SUBSTITUTE" => ViewValue::Int(8),
        "SORT_REGULAR" => ViewValue::Int(0),
        "SORT_NUMERIC" => ViewValue::Int(1),
        "SORT_STRING" => ViewValue::Int(2),
        "SORT_NATURAL" => ViewValue::Int(6),
        "SORT_FLAG_CASE" => ViewValue::Int(8),
        "ARRAY_FILTER_USE_KEY" => ViewValue::Int(2),
        "ARRAY_FILTER_USE_BOTH" => ViewValue::Int(1),
        "COUNT_RECURSIVE" => ViewValue::Int(1),
        "STR_PAD_LEFT" => ViewValue::Int(0),
        "STR_PAD_RIGHT" => ViewValue::Int(1),
        "STR_PAD_BOTH" => ViewValue::Int(2),
        "PHP_ROUND_HALF_UP" => ViewValue::Int(1),
        "PHP_ROUND_HALF_DOWN" => ViewValue::Int(2),
        "PHP_ROUND_HALF_EVEN" => ViewValue::Int(3),
        "PHP_ROUND_HALF_ODD" => ViewValue::Int(4),
        _ => {
            if let Some(function) = registry.functions.get(name) {
                return function(&[]);
            }
            return Err(error(format!("Undefined constant \"{name}\"")));
        }
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Parser;
    use illuminate_support::json;

    fn eval_with(src: &str, scope: &mut Scope) -> Result<ViewValue> {
        let registry = Arc::new(Registry::default());
        let expr = Parser::expression(src, 1)?;
        Evaluator::new(scope, &registry).eval(&expr)
    }

    fn eval(src: &str) -> ViewValue {
        let mut scope = Scope::default();
        scope.set("user", ViewValue::from(json!({"name": "Taylor", "roles": ["admin", "dev"], "age": 40})));
        scope.set("items", ViewValue::from(json!([1, 2, 3])));
        scope.set("nothing", ViewValue::Null);
        eval_with(src, &mut scope).unwrap()
    }

    #[test]
    fn it_evaluates_arithmetic_and_strings() {
        assert_eq!(eval("1 + 2 * 3"), ViewValue::Int(7));
        assert_eq!(eval("10 / 4"), ViewValue::Float(2.5));
        assert_eq!(eval("2 ** 3 ** 2"), ViewValue::Int(512));
        assert_eq!(eval("-2 ** 2"), ViewValue::Int(-4));
        assert_eq!(eval("7 % 3"), ViewValue::Int(1));
        assert_eq!(eval("'a' . 1 + 2"), ViewValue::from("a3"));
        assert_eq!(eval("\"Hi {$user->name}, you are $user->age\""), ViewValue::from("Hi Taylor, you are 40"));
        assert_eq!(eval("\"Role: {$user['roles'][0]}\""), ViewValue::from("Role: admin"));
    }

    #[test]
    fn it_evaluates_comparisons_and_logic() {
        assert_eq!(eval("'10' == 10"), ViewValue::Bool(true));
        assert_eq!(eval("'10' === 10"), ViewValue::Bool(false));
        assert_eq!(eval("1 <=> 2"), ViewValue::Int(-1));
        assert_eq!(eval("! $nothing && true"), ViewValue::Bool(true));
        assert_eq!(eval("true xor true"), ViewValue::Bool(false));
        assert_eq!(eval("$nothing ?: 'fallback'"), ViewValue::from("fallback"));
        assert_eq!(eval("$missing ?? $nothing ?? 'deep'"), ViewValue::from("deep"));
        assert_eq!(eval("$user['missing'] ?? 'none'"), ViewValue::from("none"));
        assert_eq!(eval("isset($user->name, $items)"), ViewValue::Bool(true));
        assert_eq!(eval("isset($user->nope)"), ViewValue::Bool(false));
        assert_eq!(eval("empty($nothing) && empty($missing) && ! empty($items)"), ViewValue::Bool(true));
        assert_eq!(eval("match (2) { 1 => 'one', 2, 3 => 'few', default => 'many' }"), ViewValue::from("few"));
    }

    #[test]
    fn it_reports_undefined_variables() {
        let mut scope = Scope::default();
        let error = eval_with("$missing + 1", &mut scope).unwrap_err();
        assert_eq!(error.to_string(), "Undefined variable $missing");
        let error = eval_with("$nothing->name", &mut Scope::default()).unwrap_err();
        assert_eq!(error.to_string(), "Undefined variable $nothing");
    }

    #[test]
    fn it_reads_properties_of_null_strictly() {
        let mut scope = Scope::default();
        scope.set("user", ViewValue::Null);
        let error = eval_with("$user->name", &mut scope).unwrap_err();
        assert_eq!(error.to_string(), "Attempt to read property \"name\" on null");
        assert_eq!(eval_with("$user?->name", &mut scope).unwrap(), ViewValue::Null);
    }

    #[test]
    fn it_assigns_values() {
        let mut scope = Scope::default();
        let registry = Arc::new(Registry::default());
        let stmts = crate::expr::Parser::statements(
            "$a = 1; $a += 2; $b = []; $b[] = 'x'; $b['k']['j'] = 'y'; $c = 'a'; $c .= 'b'; $i = 0; $i++; ++$i; $d ??= 5; [$p, $q] = [1, 2];",
            1,
        )
        .unwrap();
        let mut out = String::new();
        Evaluator::new(&mut scope, &registry).exec(&stmts, &mut out).unwrap();
        assert_eq!(scope.get("a"), Some(&ViewValue::Int(3)));
        assert_eq!(scope.get("b").unwrap().to_json(), json!({"0": "x", "k": {"j": "y"}}));
        assert_eq!(scope.get("c"), Some(&ViewValue::from("ab")));
        assert_eq!(scope.get("i"), Some(&ViewValue::Int(2)));
        assert_eq!(scope.get("d"), Some(&ViewValue::Int(5)));
        assert_eq!(scope.get("q"), Some(&ViewValue::Int(2)));
    }

    #[test]
    fn closures_capture_their_scope() {
        let mut scope = Scope::default();
        scope.set("factor", ViewValue::Int(3));
        let closure = eval_with("fn ($x) => $x * $factor", &mut scope).unwrap();
        let ViewValue::Closure(closure) = closure else { panic!() };
        assert_eq!(closure.call(&[ViewValue::Int(2)]).unwrap(), ViewValue::Int(6));
        assert_eq!(eval_with("(fn () => 'called')()", &mut scope).unwrap(), ViewValue::from("called"));
    }

    #[test]
    fn strings_increment_like_php() {
        assert_eq!(string_increment("a"), "b");
        assert_eq!(string_increment("Az"), "Ba");
        assert_eq!(string_increment("zz"), "aaa");
    }
}
