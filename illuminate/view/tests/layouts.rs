mod common;

use common::Views;
use illuminate_support::json;

fn layout_views() -> Views {
    let views = Views::new();
    views.add(
        "layouts.app",
        "<html>\n    <head>\n        <title>App Name - @yield('title')</title>\n    </head>\n    <body>\n        @section('sidebar')\n            This is the master sidebar.\n        @show\n\n        <div class=\"container\">\n            @yield('content')\n        </div>\n    </body>\n</html>\n",
    );
    views.add(
        "child",
        "@extends('layouts.app')\n\n@section('title', 'Page Title')\n\n@section('sidebar')\n    @parent\n\n    <p>This is appended to the master sidebar.</p>\n@endsection\n\n@section('content')\n    <p>This is my body content.</p>\n@endsection\n",
    );
    views
}

#[test]
fn template_inheritance_matches_laravel() {
    let views = layout_views();
    let expected = "\n\n<html>\n    <head>\n        <title>App Name - Page Title</title>\n    </head>\n    <body>\n                        This is the master sidebar.\n        \n    <p>This is appended to the master sidebar.</p>\n\n        <div class=\"container\">\n                <p>This is my body content.</p>\n        </div>\n    </body>\n</html>\n";
    assert_eq!(views.render("child", ()), expected);
}

#[test]
fn layouts_receive_the_child_variables() {
    let views = Views::new();
    views.add("layout", "<title>{{ $title }}</title>@yield('body')");
    views.add(
        "page",
        "@extends('layout')@php($title = 'From Child')@section('body')Hi {{ $name }}@endsection",
    );
    assert_eq!(
        views.render("page", json!({"name": "Taylor"})),
        "<title>From Child</title>Hi Taylor"
    );
    views.add(
        "page2",
        "@extends('layout', ['title' => 'Passed'])@section('body')x @endsection",
    );
    assert_eq!(views.render("page2", ()), "<title>Passed</title>x ");
}

#[test]
fn yield_defaults_and_escaping() {
    let views = Views::new();
    views.add(
        "layout",
        "@yield('title', 'Default <Title>')|@yield('body')|@yield('missing')",
    );
    views.add(
        "page",
        "@extends('layout')@section('body', '<b>escaped</b>')",
    );
    assert_eq!(
        views.render("page", ()),
        "Default &lt;Title&gt;|&lt;b&gt;escaped&lt;/b&gt;|"
    );
}

#[test]
fn sections_append_and_overwrite() {
    let views = Views::new();
    views.add(
        "base",
        "@section('a')base-a @show|@section('b')base-b @show|@yield('c')",
    );
    views.add("middle", "@extends('base')@section('a')middle-a @parent @endsection\n@section('b')middle-b @overwrite\n@section('c')c1 @stop\n@section('c')c2 @append");
    assert_eq!(
        views.render("middle", ()),
        "middle-a base-a  |middle-b |c1 c2 "
    );
}

#[test]
fn multi_level_inheritance() {
    let views = Views::new();
    views.add("grand", "[@yield('content')]");
    views.add(
        "parent",
        "@extends('grand')@section('content')parent:@yield('inner')@endsection",
    );
    views.add(
        "child",
        "@extends('parent')@section('inner')child @endsection",
    );
    assert_eq!(views.render("child", ()), "[parent:child ]");
}

#[test]
fn includes_share_the_parent_scope() {
    let views = Views::new();
    views.add(
        "shared.errors",
        "<div>{{ $name }} {{ $status ?? 'none' }}</div>\n",
    );
    views.add(
        "page",
        "<div>\n    @include('shared.errors')\n    @include('shared.errors', ['status' => 'complete'])\n    @includeIf('missing.view')\n    @includeIf('shared.errors', ['name' => 'Override'])\n    @includeWhen($show, 'shared.errors')\n    @includeUnless($show, 'shared.errors')\n    @includeFirst(['custom.admin', 'shared.errors'])\n</div>",
    );
    let expected = "<div>\n    <div>Taylor none</div>\n    <div>Taylor complete</div>\n        <div>Override none</div>\n    <div>Taylor none</div>\n        <div>Taylor none</div>\n</div>";
    assert_eq!(
        views.render("page", json!({"name": "Taylor", "show": true})),
        expected
    );
}

#[test]
fn include_isolated_does_not_inherit() {
    let views = Views::new();
    views.add("partial", "{{ $name ?? 'no name' }}/{{ $user }}");
    assert_eq!(
        views.inline(
            "@includeIsolated('partial', ['user' => 'u'])",
            json!({"name": "Taylor"})
        ),
        "no name/u"
    );
}

#[test]
fn each_renders_a_view_per_item() {
    let views = Views::new();
    views.add(
        "job",
        "<li>{{ $key }}: {{ $job }} {{ $outer ?? 'isolated' }}</li>",
    );
    views.add("no-jobs", "<p>No jobs</p>");
    assert_eq!(
        views.inline(
            "@each('job', $jobs, 'job')",
            json!({"jobs": ["a", "b"], "outer": "x"})
        ),
        "<li>0: a isolated</li><li>1: b isolated</li>"
    );
    assert_eq!(
        views.inline("@each('job', $jobs, 'job', 'no-jobs')", json!({"jobs": []})),
        "<p>No jobs</p>"
    );
    assert_eq!(
        views.inline(
            "@each('job', $jobs, 'job', 'raw|Nothing')",
            json!({"jobs": []})
        ),
        "Nothing"
    );
}

#[test]
fn stacks_push_and_prepend() {
    let views = Views::new();
    views.add("layout", "<head>@stack('scripts')</head>@yield('content')");
    views.add(
        "page",
        "@extends('layout')\n@push('scripts')\n<script src=\"/second.js\"></script>\n@endpush\n@prepend('scripts')\n<script src=\"/first.js\"></script>\n@endprepend\n@section('content')body @include('partial')@endsection",
    );
    views.add(
        "partial",
        "@push('scripts')<script src=\"/partial.js\"></script>@endpush",
    );
    assert_eq!(
        views.render("page", ()),
        "<head><script src=\"/first.js\"></script>\n<script src=\"/second.js\"></script>\n<script src=\"/partial.js\"></script></head>body "
    );
}

#[test]
fn stacks_with_defaults_and_has_stack() {
    let views = Views::new();
    assert_eq!(
        views.inline(
            "[@stack('missing', 'default')]@hasstack('missing') has @endif",
            ()
        ),
        "[default]"
    );
    assert_eq!(
        views.inline(
            "@push('a')x\n@endpush\n@hasstack('a')has\n@endif\n@stack('a')",
            ()
        ),
        "has\nx\n"
    );
}

#[test]
fn once_and_push_once() {
    let views = Views::new();
    views.add("components.chart", "@once\n<script src=\"/chart.js\"></script>\n@endonce\n@pushOnce('scripts')<style></style>@endPushOnce\n@pushOnce('scripts', 'shared')<script src=\"/shared.js\"></script>@endPushOnce\nchart\n");
    views.add(
        "components.pie",
        "@pushOnce('scripts', 'shared')<script src=\"/shared.js\"></script>@endPushOnce\npie\n",
    );
    let out = views.inline("<x-chart/><x-chart/><x-pie/>|@stack('scripts')", ());
    assert_eq!(
        out,
        "<script src=\"/chart.js\"></script>\nchart\nchart\npie\n|<style></style><script src=\"/shared.js\"></script>"
    );
}

#[test]
fn push_if() {
    let views = Views::new();
    let template =
        "@pushIf($a, 'x')A\n@elsePushIf($b, 'x')B\n@elsePush('x')C\n@endPushIf\n@stack('x')";
    assert_eq!(
        views.inline(template, json!({"a": false, "b": true})),
        "B\n"
    );
    assert_eq!(
        views.inline(template, json!({"a": false, "b": false})),
        "C\n"
    );
}

#[test]
fn fragments() {
    let views = Views::new();
    views.add("dashboard", "<h1>Dash</h1>\n@fragment('user-list')\n<ul>@foreach ($users as $user)<li>{{ $user }}</li>@endforeach</ul>\n@endfragment\n@fragment('footer')F @endfragment");
    let view = views
        .factory
        .make("dashboard", json!({"users": ["a", "b"]}));
    assert_eq!(
        view.render().unwrap(),
        "<h1>Dash</h1>\n<ul><li>a</li><li>b</li></ul>\nF "
    );
    assert_eq!(
        view.fragment("user-list").unwrap(),
        "<ul><li>a</li><li>b</li></ul>\n"
    );
    assert_eq!(view.fragments(&["footer", "footer"]).unwrap(), "F F ");
    assert_eq!(
        view.fragment_if(false, "footer").unwrap(),
        view.render().unwrap()
    );
    assert!(view.fragment("missing").is_err());
}
