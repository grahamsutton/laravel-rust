//! Global scope objects.

use super::builder::Builder;
use super::model::Model;

/// A global scope: a constraint added to every query for a model.
///
/// Writing a scope as a type lets you reuse it across models and name it
/// when you remove it. Register it in the model's `boot` with
/// [`Model::add_global_scope_object`], or let the derive do it with
/// `#[scoped_by(...)]` (Laravel's `#[ScopedBy]` attribute):
///
/// ```
/// use illuminate_database::eloquent::*;
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// # use illuminate_database::DatabaseManager;
///
/// pub struct ActiveScope;
///
/// impl<M: Model> Scope<M> for ActiveScope {
///     fn apply(&self, query: Builder<M>) -> Builder<M> {
///         query.where_("active", true)
///     }
/// }
///
/// #[derive(Debug, Clone, Default, Model)]
/// #[scoped_by(ActiveScope)]
/// pub struct User {
///     pub id: u64,
///     pub active: bool,
/// }
///
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container.clone());
/// # container.instance(DatabaseManager::from_config(json!({
/// #     "default": "sqlite",
/// #     "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
/// # })));
/// assert_eq!(User::query().to_sql(), "select * from \"users\" where \"active\" = ?");
///
/// let everyone = User::without_global_scope_object::<ActiveScope>();
/// assert_eq!(everyone.to_sql(), "select * from \"users\"");
/// ```
///
/// A scope may be generic over the model, as above, so one type serves
/// many models, or implemented for a single model (`impl Scope<User> for
/// ...`) when it needs that model's columns.
pub trait Scope<M: Model>: Send + Sync + 'static {
    /// Apply the scope to the query.
    fn apply(&self, query: Builder<M>) -> Builder<M>;
}

/// The name a scope object is registered under: its type's name, without
/// the module path or generic arguments (`AncientScope`). Pass it to
/// `without_global_scope` to remove the scope.
///
/// ```
/// use illuminate_database::eloquent::scope_name;
///
/// struct AncientScope;
///
/// assert_eq!(scope_name::<AncientScope>(), "AncientScope");
/// ```
pub fn scope_name<S: ?Sized>() -> String {
    short_type_name(std::any::type_name::<S>())
}

/// A type name without its module path or generic arguments.
pub(crate) fn short_type_name(full: &str) -> String {
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Generic<T>(T);

    #[test]
    fn scope_names_drop_paths_and_generics() {
        assert_eq!(scope_name::<Generic<String>>(), "Generic");
        assert_eq!(scope_name::<String>(), "String");
        let _ = Generic(1).0;
    }
}
