//! Teach Blade about the rest of the framework: `@csrf`, `route()`,
//! `__()`, `@vite`, `@auth`, `@can`, `$errors`, and friends.

use std::sync::Arc;

use illuminate_http::Response;
use illuminate_support::{Error, Map, Result, Value, ValueExt, json};
use illuminate_view::{Factory, ViewObject, ViewValue};

use crate::helpers;
use crate::vite::Vite;

/// Register the view hooks, error views, and view renderers.
pub fn boot() {
    let factory = Factory::resolve();

    // `errors::404` looks in resources/views/errors.
    factory.add_namespace("errors", helpers::resource_path("views/errors"));

    // Every view gets the validation errors flashed to the session.
    factory.share_resolver(|request| {
        let mut data = Map::new();
        data.insert("errors".into(), illuminate_session::view_errors(request));
        data
    });

    register_functions(&factory);
    register_renderers(&factory);
}

fn arg(args: &[ViewValue], index: usize) -> Option<&ViewValue> {
    args.get(index).filter(|value| !value.is_null())
}

fn arg_string(args: &[ViewValue], index: usize) -> Option<String> {
    arg(args, index).map(ViewValue::to_string_lossy)
}

fn arg_json(args: &[ViewValue], index: usize) -> Value {
    arg(args, index).map(ViewValue::to_json).unwrap_or(Value::Null)
}

fn required(args: &[ViewValue], index: usize, function: &str) -> Result<String> {
    arg_string(args, index)
        .ok_or_else(|| illuminate_support::error::error!("{function}() expects at least {} argument(s).", index + 1))
}

fn strings(value: Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().map(ValueExt::to_string_lossy).collect(),
        Value::Null => Vec::new(),
        other => vec![other.to_string_lossy()],
    }
}

fn register_functions(factory: &Factory) {
    let blade = factory.blade();

    // Sessions and CSRF protection.
    blade.function("csrf_token", |_| Ok(illuminate_session::csrf_token().into()));
    blade.function("csrf_field", |_| Ok(illuminate_session::csrf_field().into()));
    blade.function("session", |args| {
        let Some(key) = arg_string(args, 0) else {
            return Ok(ViewValue::Null);
        };
        let default = arg_json(args, 1);
        let value = illuminate_session::try_session()
            .map(|session| session.get(&key))
            .filter(|value| !value.is_null())
            .unwrap_or(default);
        Ok(value.into())
    });
    blade.function("old", |args| {
        let key = arg_string(args, 0).unwrap_or_default();
        Ok(illuminate_session::old(&key, arg_json(args, 1)).into())
    });

    // Translations.
    let translate = |args: &[ViewValue]| -> Result<ViewValue> {
        let Some(key) = arg_string(args, 0) else {
            return Ok(ViewValue::Null);
        };
        let replace = match arg_json(args, 1) {
            Value::Null => json!({}),
            replace => replace,
        };
        let locale = arg_string(args, 2);
        Ok(illuminate_translation::Lang::get_value(&key, &replace, locale.as_deref(), true).into())
    };
    blade.function("__", translate);
    blade.function("trans", translate);
    blade.function("Lang::get", translate);
    blade.function("trans_choice", |args| {
        let key = required(args, 0, "trans_choice")?;
        let count = arg(args, 1).and_then(ViewValue::as_i64).unwrap_or(1);
        let replace = match arg_json(args, 2) {
            Value::Null => json!({}),
            replace => replace,
        };
        let line = match arg_string(args, 3) {
            Some(locale) => illuminate_translation::Lang::choice_in(&key, count, &replace, &locale),
            None => illuminate_translation::Lang::choice_with(&key, count, &replace),
        };
        Ok(line.into())
    });
    blade.function("app_locale", |_| Ok(illuminate_translation::Lang::get_locale().into()));
    blade.function("app_environment", |_| Ok(helpers::app_environment().into()));

    // URLs and routes.
    blade.function("route", |args| {
        let name = required(args, 0, "route")?;
        let absolute = arg(args, 2).is_none_or(ViewValue::truthy);
        let parameters = arg_json(args, 1);
        let url = if absolute {
            illuminate_routing::route(&name, parameters)?
        } else {
            illuminate_routing::URL::route_with(&name, parameters, false)?
        };
        Ok(url.into())
    });
    blade.function("url", |args| match arg_string(args, 0) {
        Some(path) => Ok(illuminate_routing::url(&path).into()),
        None => Ok(ViewValue::object(UrlObject)),
    });
    blade.function("secure_url", |args| Ok(illuminate_routing::secure_url(&required(args, 0, "secure_url")?).into()));
    blade.function("asset", |args| Ok(illuminate_routing::asset(&required(args, 0, "asset")?).into()));
    blade.function("secure_asset", |args| {
        Ok(illuminate_routing::secure_asset(&required(args, 0, "secure_asset")?).into())
    });
    blade.function("Route::has", |args| {
        let names = strings(arg_json(args, 0));
        Ok(names.iter().all(|name| illuminate_routing::Route::has(name)).into())
    });
    blade.function("Route::currentRouteName", |_| {
        Ok(illuminate_routing::Route::current_route_name().into())
    });
    blade.function("Route::is", |args| {
        let patterns: Vec<String> = args.iter().flat_map(|a| strings(a.to_json())).collect();
        let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
        Ok(illuminate_routing::Route::current_route_named(&patterns).into())
    });

    // Paths.
    for (name, helper) in [
        ("base_path", helpers::base_path as fn(&str) -> String),
        ("app_path", helpers::app_path),
        ("config_path", helpers::config_path),
        ("database_path", helpers::database_path),
        ("lang_path", helpers::lang_path),
        ("public_path", helpers::public_path),
        ("resource_path", helpers::resource_path),
        ("storage_path", helpers::storage_path),
    ] {
        blade.function(name, move |args| Ok(helper(&arg_string(args, 0).unwrap_or_default()).into()));
    }

    // The filesystem.
    blade.function("file_exists", |args| {
        Ok(std::path::Path::new(&arg_string(args, 0).unwrap_or_default()).exists().into())
    });
    blade.function("is_file", |args| {
        Ok(std::path::Path::new(&arg_string(args, 0).unwrap_or_default()).is_file().into())
    });
    blade.function("is_dir", |args| {
        Ok(std::path::Path::new(&arg_string(args, 0).unwrap_or_default()).is_dir().into())
    });

    // Vite.
    blade.function("vite", |args| {
        let entries = strings(arg_json(args, 0));
        let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
        let build = arg_string(args, 1);
        Ok(Vite::render_from(&entries, build.as_deref())?.into())
    });
    blade.function("vite_react_refresh", |_| {
        Ok(Vite::react_refresh().map(ViewValue::from).unwrap_or(ViewValue::Null))
    });
    blade.function("Vite::asset", |args| {
        Ok(Vite::asset_from(&required(args, 0, "Vite::asset")?, arg_string(args, 1).as_deref())?.into())
    });
    blade.function("Vite::content", |args| Ok(Vite::content(&required(args, 0, "Vite::content")?)?.into()));
    blade.function("Vite::isRunningHot", |_| Ok(Vite::is_running_hot().into()));

    // Authentication and authorization.
    blade.function("auth_check", |args| {
        Ok(illuminate_auth::blade::auth_check(arg_string(args, 0).as_deref()).into())
    });
    blade.function("auth", |args| Ok(ViewValue::object(AuthObject { guard: arg_string(args, 0) })));
    blade.function("Auth::check", |args| {
        Ok(illuminate_auth::blade::auth_check(arg_string(args, 0).as_deref()).into())
    });
    blade.function("Auth::guest", |args| {
        Ok(illuminate_auth::blade::guest_check(arg_string(args, 0).as_deref()).into())
    });
    blade.function("Auth::user", |args| Ok(AuthObject { guard: arg_string(args, 0) }.user()));
    blade.function("Auth::id", |args| Ok(AuthObject { guard: arg_string(args, 0) }.id()));
    let gate_check = |args: &[ViewValue]| -> Result<ViewValue> {
        let ability = required(args, 0, "gate_check")?;
        let arguments: Vec<Value> = args.iter().skip(1).map(ViewValue::to_json).collect();
        Ok(illuminate_auth::blade::gate_check(&ability, arguments).into())
    };
    blade.function("gate_check", gate_check);
    blade.function("Gate::allows", gate_check);
    blade.function("Gate::denies", move |args| Ok((!gate_check(args)?.truthy()).into()));
}

/// Render views for `Route::view`, error pages, and maintenance mode.
fn register_renderers(factory: &Factory) {
    let render = {
        let factory = factory.clone();
        move |name: &str, data: Value| -> Option<String> {
            if !factory.exists(name) {
                return None;
            }
            factory.make(name, data).render().ok()
        }
    };

    if let Some(handler) = illuminate_container::try_app::<crate::exceptions::Handler>() {
        handler.render_views_using(render.clone());
    }
    super::set_view_renderer(render);

    if let Some(router) = illuminate_container::try_app::<illuminate_routing::Router>() {
        let factory = factory.clone();
        router.set_view_renderer(Arc::new(move |name: &str, data: Value| -> Result<Response> {
            let html = factory.make(name, data).render()?;
            Ok(Response::new(html))
        }));
    }
}

/// `url()` without arguments: `url()->current()`, `url()->previous()`.
#[derive(Debug)]
struct UrlObject;

impl ViewObject for UrlObject {
    fn class_name(&self) -> &str {
        "Illuminate\\Routing\\UrlGenerator"
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        use illuminate_routing::URL;
        let value: Result<ViewValue> = match method {
            "current" => Ok(URL::current().into()),
            "full" => Ok(URL::full().into()),
            "previous" => Ok(match arg_string(args, 0) {
                Some(fallback) => URL::previous_or(&fallback),
                None => URL::previous(),
            }
            .into()),
            "previousPath" => Ok(URL::previous_path().into()),
            "to" => Ok(URL::to(&arg_string(args, 0).unwrap_or_default()).into()),
            "route" => URL::route(&arg_string(args, 0).unwrap_or_default(), arg_json(args, 1))
                .map(ViewValue::from)
                .map_err(Error::from),
            "asset" => Ok(illuminate_routing::asset(&arg_string(args, 0).unwrap_or_default()).into()),
            _ => return None,
        };
        Some(value)
    }

    fn to_string_value(&self) -> Option<String> {
        Some(illuminate_routing::URL::current())
    }
}

/// `auth()` in templates: `auth()->check()`, `auth()->user()->name`.
#[derive(Debug)]
struct AuthObject {
    guard: Option<String>,
}

impl AuthObject {
    fn user(&self) -> ViewValue {
        illuminate_auth::blade::auth_user(self.guard.as_deref())
            .map(|user| ViewValue::from(user.to_value()))
            .unwrap_or(ViewValue::Null)
    }

    fn id(&self) -> ViewValue {
        illuminate_auth::blade::auth_user(self.guard.as_deref())
            .map(|user| ViewValue::from(user.id()))
            .unwrap_or(ViewValue::Null)
    }
}

impl ViewObject for AuthObject {
    fn class_name(&self) -> &str {
        "Illuminate\\Auth\\AuthManager"
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        let guard = self.guard.as_deref();
        Some(Ok(match method {
            "check" => illuminate_auth::blade::auth_check(guard).into(),
            "guest" => illuminate_auth::blade::guest_check(guard).into(),
            "user" => self.user(),
            "id" => self.id(),
            "guard" => ViewValue::object(AuthObject { guard: arg_string(args, 0) }),
            _ => return None,
        }))
    }
}
