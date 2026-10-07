mod common;

use common::{Views, blade};
use illuminate_support::json;
use illuminate_view::{ViewObject, ViewValue, data};

#[test]
fn csrf_and_method_fields() {
    let views = Views::new();
    let form = "<form method=\"POST\" action=\"/profile\">\n    @csrf\n    @method('PUT')\n</form>";
    let error = views.inline_err(form, ());
    assert!(error.to_string().contains("csrf_token"), "{error}");

    views
        .factory
        .blade()
        .function("csrf_token", |_| Ok("token-123".into()));
    assert_eq!(
        views.inline(form, ()),
        "<form method=\"POST\" action=\"/profile\">\n    <input type=\"hidden\" name=\"_token\" value=\"token-123\" autocomplete=\"off\">    <input type=\"hidden\" name=\"_method\" value=\"PUT\"></form>"
    );
    assert_eq!(views.inline("{{ csrf_token() }}", ()), "token-123");

    views
        .factory
        .blade()
        .function("csrf_field", |_| Ok(ViewValue::html("<custom>")));
    assert_eq!(views.inline("@csrf", ()), "<custom>");
}

#[test]
fn translation_directives_use_the_translation_hooks() {
    let views = Views::new();
    assert_eq!(
        views.inline("@lang('Welcome, :name!', ['name' => 'taylor'])", ()),
        "Welcome, taylor!"
    );
    assert_eq!(views.inline("@choice('apple|apples', 3)", ()), "apples");
    assert_eq!(
        views.inline("{{ __('Hello <b>:name</b>', ['name' => 'x']) }}", ()),
        "Hello &lt;b&gt;x&lt;/b&gt;"
    );
    assert_eq!(views.inline("@lang\nraw key\n@endlang", ()), "raw key");

    views.factory.blade().function("__", |args| {
        Ok(match args[0].to_string().as_str() {
            "messages.welcome" => "Bienvenue".into(),
            other => other.into(),
        })
    });
    assert_eq!(
        views.inline("@lang('messages.welcome') {{ __('messages.welcome') }}", ()),
        "Bienvenue Bienvenue"
    );
}

#[test]
fn vite_and_inject_use_their_hooks() {
    let views = Views::new();
    assert_eq!(
        views.inline(
            "<head>@vite(['resources/css/app.css', 'resources/js/app.js'])</head>",
            ()
        ),
        "<head></head>"
    );
    views.factory.blade().function("vite", |args| {
        let entries = args[0].as_array().map(|a| a.len()).unwrap_or(0);
        Ok(ViewValue::html(format!(
            "<script type=\"module\" data-entries=\"{entries}\"></script>"
        )))
    });
    assert_eq!(
        views.inline("@vite(['a.css', 'b.js'])", ()),
        "<script type=\"module\" data-entries=\"2\"></script>"
    );

    views.factory.blade().function("app", |args| {
        assert_eq!(args[0].to_string(), "App\\Services\\MetricsService");
        Ok(ViewValue::from(json!({"monthly_revenue": 1000})))
    });
    assert_eq!(
        views.inline("@inject('metrics', 'App\\Services\\MetricsService')\n<div>Monthly Revenue: {{ $metrics->monthlyRevenue() }}.</div>", ()),
        "<div>Monthly Revenue: 1000.</div>"
    );
}

#[test]
fn dump_directive() {
    let out = blade("@dump($user)", json!({"user": {"name": "Taylor"}}));
    assert!(out.starts_with("<pre class=\"sf-dump\">"), "{out}");
    assert!(
        out.contains("&quot;name&quot;: &quot;Taylor&quot;"),
        "{out}"
    );
}

#[test]
fn compact_and_defined_vars() {
    assert_eq!(
        blade(
            "{{ json_encode(compact('a', ['b', 'missing'])) }}",
            json!({"a": 1, "b": 2})
        ),
        "{&quot;a&quot;:1,&quot;b&quot;:2}"
    );
    assert_eq!(
        blade(
            "{{ array_key_exists('x', get_defined_vars()) ? 'yes' : 'no' }}",
            json!({"x": 1})
        ),
        "yes"
    );
}

struct Money(i64);

impl ViewObject for Money {}

#[test]
fn custom_echo_handlers() {
    let views = Views::new();
    views
        .factory
        .blade()
        .stringable(|money: &Money| format!("<${:.2}>", money.0 as f64 / 100.0));
    let data = data([("money", ViewValue::object(Money(1999)))]);
    assert_eq!(
        views.inline("Cost: {{ $money }} / {!! $money !!}", data),
        "Cost: &lt;$19.99&gt; / <$19.99>"
    );
}

#[test]
fn the_app_helper() {
    let container = std::sync::Arc::new(illuminate_container::Container::new());
    let _guard = illuminate_container::Container::set_local_instance(container.clone());
    container.instance(illuminate_config::Repository::new(
        json!({"app": {"locale": "pt_BR", "env": "local"}}),
    ));
    assert_eq!(
        blade(
            "<html lang=\"{{ str_replace('_', '-', app()->getLocale()) }}\">{{ app()->environment() }} {{ app()->isLocal() ? 'local' : '' }} {{ App::getLocale() }}",
            ()
        ),
        "<html lang=\"pt-BR\">local local pt_BR"
    );
    assert_eq!(
        blade("v{{ app()->version() }}", ()),
        format!("v{}", env!("CARGO_PKG_VERSION"))
    );
    let error = Views::new().inline_err("@inject('metrics', 'Metrics')", ());
    assert!(
        error
            .to_string()
            .starts_with("No service [Metrics] is available to views."),
        "{error}"
    );
}
