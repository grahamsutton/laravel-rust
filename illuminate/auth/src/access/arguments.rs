//! The arguments handed to gates and policies.

use std::any::{Any, TypeId, type_name};
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use illuminate_support::{Str, Value};

/// One argument of an authorization check: a model (borrowed or shared), a
/// model *type* (Laravel's `Post::class`, for abilities like `create` that
/// don't need an instance), or a plain value.
///
/// ```
/// use illuminate_auth::GateArgument;
/// use illuminate_support::json;
///
/// struct Post { id: u64 }
///
/// let post = Post { id: 1 };
/// let argument = GateArgument::of(&post);
/// assert_eq!(argument.downcast_ref::<Post>().unwrap().id, 1);
///
/// let class = GateArgument::class::<Post>();
/// assert!(class.is_class());
///
/// let value = GateArgument::value(json!(42));
/// assert_eq!(value.as_value(), Some(&json!(42)));
/// ```
#[derive(Clone)]
pub enum GateArgument<'a> {
    /// A borrowed value (`Gate::allows("update", &post)`).
    Ref {
        value: &'a (dyn Any + Send + Sync),
        type_name: &'static str,
    },
    /// A shared value (from the `can` middleware's argument resolver).
    Shared {
        value: Arc<dyn Any + Send + Sync>,
        type_name: &'static str,
    },
    /// A model type, for checks that don't need an instance.
    Class {
        type_id: TypeId,
        type_name: &'static str,
    },
    /// A plain value (a route parameter, a string from a template, ...).
    /// Strings naming a model with a registered policy act like
    /// [`GateArgument::Class`].
    Value(Value),
}

impl<'a> GateArgument<'a> {
    /// Borrow a value as an argument.
    pub fn of<T: Any + Send + Sync>(value: &'a T) -> Self {
        GateArgument::Ref {
            value,
            type_name: type_name::<T>(),
        }
    }

    /// Use a shared value as an argument.
    pub fn shared<T: Any + Send + Sync>(value: Arc<T>) -> GateArgument<'static> {
        GateArgument::Shared {
            value,
            type_name: type_name::<T>(),
        }
    }

    /// Take ownership of a value as an argument.
    pub fn owned<T: Any + Send + Sync>(value: T) -> GateArgument<'static> {
        GateArgument::shared(Arc::new(value))
    }

    /// A model type (`Post::class`), for checks like `create` that don't
    /// involve a particular model.
    pub fn class<T: 'static>() -> GateArgument<'static> {
        GateArgument::Class {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
        }
    }

    /// A plain value.
    pub fn value(value: impl Into<Value>) -> GateArgument<'static> {
        GateArgument::Value(value.into())
    }

    /// Borrow the argument as a concrete type (`Value` arguments can be
    /// borrowed as [`Value`]).
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        match self {
            GateArgument::Ref { value, .. } => value.downcast_ref::<T>(),
            GateArgument::Shared { value, .. } => value.downcast_ref::<T>(),
            GateArgument::Class { .. } => None,
            GateArgument::Value(value) => (value as &dyn Any).downcast_ref::<T>(),
        }
    }

    /// The argument as `Any`, when it's a model instance.
    pub fn as_any(&self) -> Option<&(dyn Any + Send + Sync)> {
        match self {
            GateArgument::Ref { value, .. } => Some(*value),
            GateArgument::Shared { value, .. } => Some(value.as_ref()),
            _ => None,
        }
    }

    /// The type of the model this argument refers to (an instance's type,
    /// or the class).
    pub fn model_type_id(&self) -> Option<TypeId> {
        match self {
            GateArgument::Ref { value, .. } => Some(concrete_type_id(*value)),
            GateArgument::Shared { value, .. } => Some(concrete_type_id(value.as_ref())),
            GateArgument::Class { type_id, .. } => Some(*type_id),
            GateArgument::Value(_) => None,
        }
    }

    /// The type name of the argument (`None` for plain values).
    pub fn type_name(&self) -> Option<&'static str> {
        match self {
            GateArgument::Ref { type_name, .. }
            | GateArgument::Shared { type_name, .. }
            | GateArgument::Class { type_name, .. } => Some(type_name),
            GateArgument::Value(_) => None,
        }
    }

    /// The class name a string argument refers to (`App\Models\Post` → `Post`).
    pub fn class_basename(&self) -> Option<String> {
        match self {
            GateArgument::Class { type_name, .. } => Some(Str::class_basename(type_name)),
            GateArgument::Value(Value::String(name)) => Some(Str::class_basename(name)),
            _ => None,
        }
    }

    /// Determine if the argument is a model type rather than an instance.
    pub fn is_class(&self) -> bool {
        matches!(self, GateArgument::Class { .. })
    }

    /// The argument as a plain value, if it is one.
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            GateArgument::Value(value) => Some(value),
            _ => None,
        }
    }
}

/// The concrete type behind a `dyn Any` (not the type of the reference).
fn concrete_type_id(value: &(dyn Any + Send + Sync)) -> TypeId {
    (*value).type_id()
}

impl fmt::Debug for GateArgument<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateArgument::Ref { type_name, .. } => f.debug_tuple("Ref").field(type_name).finish(),
            GateArgument::Shared { type_name, .. } => {
                f.debug_tuple("Shared").field(type_name).finish()
            }
            GateArgument::Class { type_name, .. } => {
                f.debug_tuple("Class").field(type_name).finish()
            }
            GateArgument::Value(value) => f.debug_tuple("Value").field(value).finish(),
        }
    }
}

/// The list of arguments for an authorization check.
#[derive(Clone, Debug, Default)]
pub struct GateArguments<'a>(Vec<GateArgument<'a>>);

impl<'a> GateArguments<'a> {
    /// No arguments.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Add an argument.
    pub fn push(&mut self, argument: GateArgument<'a>) {
        self.0.push(argument);
    }

    /// Add an argument, fluently.
    pub fn with(mut self, argument: GateArgument<'a>) -> Self {
        self.0.push(argument);
        self
    }

    /// The arguments as a vector.
    pub fn into_vec(self) -> Vec<GateArgument<'a>> {
        self.0
    }
}

impl<'a> Deref for GateArguments<'a> {
    type Target = [GateArgument<'a>];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> FromIterator<GateArgument<'a>> for GateArguments<'a> {
    fn from_iter<I: IntoIterator<Item = GateArgument<'a>>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// Anything that can be passed as the arguments of an authorization check:
///
/// - `()` — no arguments (`Gate::allows("edit-settings", ())`),
/// - `&post` — a model,
/// - `(&post, &comment)` / `(&a, &b, &c)` — several arguments,
/// - a [`GateArgument`] (such as `GateArgument::class::<Post>()`), a
///   `Vec` of them, or [`GateArguments`],
/// - a plain [`Value`] or `Vec<Value>`.
pub trait IntoGateArguments<'a> {
    /// Convert into the argument list.
    fn into_gate_arguments(self) -> GateArguments<'a>;
}

impl<'a> IntoGateArguments<'a> for () {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments::new()
    }
}

impl<'a, T: Any + Send + Sync> IntoGateArguments<'a> for &'a T {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments(vec![GateArgument::of(self)])
    }
}

impl<'a, A: Any + Send + Sync, B: Any + Send + Sync> IntoGateArguments<'a> for (&'a A, &'a B) {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments(vec![GateArgument::of(self.0), GateArgument::of(self.1)])
    }
}

impl<'a, A: Any + Send + Sync, B: Any + Send + Sync, C: Any + Send + Sync> IntoGateArguments<'a>
    for (&'a A, &'a B, &'a C)
{
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments(vec![
            GateArgument::of(self.0),
            GateArgument::of(self.1),
            GateArgument::of(self.2),
        ])
    }
}

impl<'a> IntoGateArguments<'a> for GateArgument<'a> {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments(vec![self])
    }
}

impl<'a> IntoGateArguments<'a> for GateArguments<'a> {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        self
    }
}

impl<'a> IntoGateArguments<'a> for Vec<GateArgument<'a>> {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments(self)
    }
}

impl<'a> IntoGateArguments<'a> for Value {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        GateArguments(vec![GateArgument::Value(self)])
    }
}

impl<'a> IntoGateArguments<'a> for Vec<Value> {
    fn into_gate_arguments(self) -> GateArguments<'a> {
        self.into_iter().map(GateArgument::Value).collect()
    }
}

/// One or many ability names: `"update"`, `["update", "delete"]`, ...
pub trait IntoAbilities {
    /// Convert into the list of ability names.
    fn into_abilities(self) -> Vec<String>;
}

impl IntoAbilities for &str {
    fn into_abilities(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoAbilities for String {
    fn into_abilities(self) -> Vec<String> {
        vec![self]
    }
}

impl IntoAbilities for &String {
    fn into_abilities(self) -> Vec<String> {
        vec![self.clone()]
    }
}

impl IntoAbilities for &[&str] {
    fn into_abilities(self) -> Vec<String> {
        self.iter().map(|ability| ability.to_string()).collect()
    }
}

impl<const N: usize> IntoAbilities for [&str; N] {
    fn into_abilities(self) -> Vec<String> {
        self.iter().map(|ability| ability.to_string()).collect()
    }
}

impl<const N: usize> IntoAbilities for &[&str; N] {
    fn into_abilities(self) -> Vec<String> {
        self.iter().map(|ability| ability.to_string()).collect()
    }
}

impl IntoAbilities for Vec<&str> {
    fn into_abilities(self) -> Vec<String> {
        self.into_iter().map(str::to_string).collect()
    }
}

impl IntoAbilities for Vec<String> {
    fn into_abilities(self) -> Vec<String> {
        self
    }
}
