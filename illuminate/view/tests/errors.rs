mod common;

use common::Views;
use illuminate_support::json;
use illuminate_view::{ViewCompilationException, ViewException};

fn view_exception(error: &illuminate_support::Error) -> &ViewException {
    error
        .downcast_ref::<ViewException>()
        .unwrap_or_else(|| panic!("expected a ViewException, got: {error:#}"))
}

#[test]
fn undefined_variables_report_the_view_and_line() {
    let views = Views::new();
    views.add(
        "profile",
        "<h1>Profile</h1>\n<p>{{ $user['name'] }}</p>\n<p>{{ $missing }}</p>\n",
    );
    let error = views
        .factory
        .make("profile", json!({"user": {"name": "Taylor"}}))
        .render()
        .unwrap_err();
    let exception = view_exception(&error);
    assert_eq!(exception.line, 3);
    assert_eq!(exception.view, "profile");
    assert_eq!(exception.message, "Undefined variable $missing");
    let path = exception.path.as_ref().unwrap();
    assert!(path.ends_with("profile.blade.html"));
    assert_eq!(
        error.to_string(),
        format!(
            "Undefined variable $missing (View: {}, line 3)",
            path.display()
        )
    );
    // The original exception is still available.
    assert!(
        exception
            .previous()
            .unwrap()
            .is::<illuminate_view::exception::ErrorException>()
    );
}

#[test]
fn errors_in_included_views_point_at_the_included_view() {
    let views = Views::new();
    views.add("partials.broken", "ok\nok\n{{ $user->name }}");
    views.add(
        "page",
        "line 1\n@include('partials.broken', ['user' => null])\n",
    );
    let error = views.factory.make("page", ()).render().unwrap_err();
    let exception = view_exception(&error);
    assert_eq!(exception.view, "partials.broken");
    assert_eq!(exception.line, 3);
    assert_eq!(
        exception.message,
        "Attempt to read property \"name\" on null"
    );
}

#[test]
fn missing_views() {
    let views = Views::new();
    let error = views.factory.make("missing", ()).render().unwrap_err();
    assert_eq!(error.to_string(), "View [missing] not found.");
    assert!(!views.factory.exists("missing"));

    views.add("page", "one\n@include('nope')");
    let error = views.factory.make("page", ()).render().unwrap_err();
    assert_eq!(view_exception(&error).line, 2);
    assert!(
        error
            .to_string()
            .starts_with("View [nope] not found. (View: ")
    );
}

#[test]
fn compilation_errors_have_line_numbers() {
    let views = Views::new();
    views.add("broken", "<div>\n@if ($a)\n    <p>Unclosed</p>\n</div>\n");
    let error = views
        .factory
        .make("broken", json!({"a": true}))
        .render()
        .unwrap_err();
    let exception = view_exception(&error);
    assert_eq!(exception.line, 2);
    assert_eq!(
        exception.message,
        "Unclosed @if directive. Did you forget an @endif?"
    );
    assert!(
        exception
            .previous()
            .unwrap()
            .is::<ViewCompilationException>()
    );

    let cases = [
        (
            "a\nb\n{{ $x + }}",
            3,
            "syntax error, unexpected end of expression",
        ),
        (
            "@foreach ($items)\n@endforeach",
            1,
            "Malformed @foreach statement.",
        ),
        (
            "@php\n$a = 1;\n$b = ;\n@endphp",
            3,
            "syntax error, unexpected token \";\", expecting an expression",
        ),
        ("x\n@endif", 2, "Unexpected @endif directive."),
        (
            "@section('a')\n@endpush",
            2,
            "Unexpected @endpush directive.",
        ),
        (
            "<x-card>\n<x-alert>\n</x-card>",
            3,
            "Unexpected closing tag </x-card>, expected </x-alert>.",
        ),
        ("{{ $a ", 1, "Unclosed echo: expected \"}}\""),
        (
            "\n\n@if (\n$a",
            3,
            "Unclosed parenthesis in directive arguments",
        ),
        (
            "@switch($a)\nstray\n@case(1)\n@endswitch",
            1,
            "Unexpected content between @switch and the first @case.",
        ),
    ];
    for (template, line, message) in cases {
        let error = views.factory.render_inline(template, ()).unwrap_err();
        let exception = view_exception(&error);
        assert_eq!(
            (exception.line, exception.message.as_str()),
            (line, message),
            "template: {template:?}"
        );
    }
}

#[test]
fn runtime_errors() {
    let views = Views::new();
    let cases = [
        ("{{ 1 / 0 }}", "Division by zero"),
        (
            "{{ $user->name() }}",
            "Call to a member function name() on null",
        ),
        (
            "{{ undefined_function() }}",
            "Call to undefined function undefined_function()",
        ),
        ("{{ Podcast::find(1) }}", "Class \"Podcast\" not found"),
        ("{{ 'a' + 1 }}", "Unsupported operand types: string + int"),
        ("{{ [1] . 'x' }}", "Array to string conversion"),
        (
            "@foreach ('text' as $x) @endforeach",
            "foreach() argument must be of type array|object, string given",
        ),
        (
            "{{ UNKNOWN_CONSTANT }}",
            "Undefined constant \"UNKNOWN_CONSTANT\"",
        ),
        (
            "{{ $items->nope() }}",
            "Call to undefined method nope() on array",
        ),
    ];
    for (template, message) in cases {
        let error = views
            .factory
            .render_inline(template, json!({"user": null, "items": [1]}))
            .unwrap_err();
        assert_eq!(
            view_exception(&error).message,
            message,
            "template: {template}"
        );
    }
}

#[test]
fn http_exceptions_pass_through_views() {
    let views = Views::new();
    let error = views
        .factory
        .render_inline("{{ abort(404) }}", ())
        .unwrap_err();
    let exception = error
        .downcast_ref::<illuminate_http::HttpException>()
        .expect("an HTTP exception");
    assert_eq!(exception.status, 404);

    let error = views
        .factory
        .render_inline("@dd($a)", json!({"a": [1]}))
        .unwrap_err();
    let exception = error
        .downcast_ref::<illuminate_http::HttpResponseException>()
        .expect("dd() returns a response");
    let response = exception.take_response().unwrap();
    assert!(response.content_string().contains("sf-dump"));
}

#[test]
fn infinite_include_recursion_is_detected() {
    let views = Views::new();
    views.add("loop", "@include('loop')");
    let error = views.factory.make("loop", ()).render().unwrap_err();
    assert!(
        error.to_string().contains("Maximum view nesting level"),
        "{error}"
    );
}
