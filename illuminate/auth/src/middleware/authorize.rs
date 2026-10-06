use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::{Result, Value};

use super::{MiddlewareFactory, within_request};
use crate::access::{GateArgument, gate};
use crate::facade::manager;

/// The `can` middleware: authorize an ability before the request reaches
/// the route.
///
/// `can:update,post` authorizes `update` against the `post` route
/// parameter, `can:create,App\Models\Post` (or `can:create,Post`) against
/// the `Post` model type, and `can:view-dashboard` without any argument.
///
/// Route parameters are plain strings until something binds them to
/// models; register an argument resolver to do that (typically the
/// framework wires it to route model binding):
///
/// ```ignore
/// Authorize::resolve_argument_using(|request, parameter| match parameter {
///     "post" => Some(GateArgument::owned(find_post(request.route("post")?)?)),
///     _ => None,
/// });
/// ```
///
/// When the check fails, an `AuthorizationException` (a `403`) is thrown.
#[derive(Clone, Debug)]
pub struct Authorize {
    ability: String,
    models: Vec<String>,
}

impl Authorize {
    /// Authorize the ability against the given route parameters / model types.
    pub fn using(ability: &str, models: &[&str]) -> Self {
        Self {
            ability: ability.to_string(),
            models: models.iter().map(|model| model.to_string()).collect(),
        }
    }

    /// Build the middleware from route parameters (`can:update,post`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        let mut parameters = parameters
            .iter()
            .map(|parameter| parameter.trim().to_string());
        Self {
            ability: parameters.next().unwrap_or_default(),
            models: parameters.filter(|model| !model.is_empty()).collect(),
        }
    }

    /// The factory registered for the `can` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// Register how route parameters become gate arguments.
    pub fn resolve_argument_using(
        resolver: impl Fn(&Request, &str) -> Option<GateArgument<'static>> + Send + Sync + 'static,
    ) {
        manager().hooks().set_argument_resolver(resolver);
    }

    /// The ability being authorized.
    pub fn ability(&self) -> &str {
        &self.ability
    }

    /// The route parameters / model types given to the gate.
    pub fn models(&self) -> &[String] {
        &self.models
    }

    /// The gate arguments for the request.
    pub fn gate_arguments(&self, request: &Request) -> Vec<GateArgument<'static>> {
        self.models
            .iter()
            .map(|model| Self::argument(request, model))
            .collect()
    }

    fn argument(request: &Request, model: &str) -> GateArgument<'static> {
        if let Some(argument) = manager().hooks().resolve_argument(request, model) {
            return argument;
        }
        // Class names are handed to the gate as-is, so it can find the
        // model's policy.
        if model.contains('\\') || model.contains("::") {
            return GateArgument::Value(Value::String(model.to_string()));
        }
        if let Some(value) = request.route(model) {
            return GateArgument::Value(Value::String(value));
        }
        let quoted = ['\'', '"']
            .iter()
            .find_map(|quote| model.strip_prefix(*quote)?.strip_suffix(*quote));
        GateArgument::Value(Value::String(quoted.unwrap_or(model).to_string()))
    }
}

#[async_trait]
impl Middleware for Authorize {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let arguments = self.gate_arguments(&request);
            gate().authorize(&self.ability, arguments).await?;
            Ok(next.run(request.clone()).await)
        })
        .await
    }
}
