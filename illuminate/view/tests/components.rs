mod common;

use common::Views;
use illuminate_support::json;
use illuminate_view::{Component, ComponentArgs, ComponentView, ViewData, ViewValue, data};

#[test]
fn anonymous_components_with_props_and_attributes() {
    let views = Views::new();
    views.add(
        "components.alert",
        "@props(['type' => 'info', 'message'])\n\n<div {{ $attributes->merge(['class' => 'alert alert-'.$type]) }}>\n    {{ $message }}\n</div>\n",
    );
    let out = views.inline(
        "<x-alert type=\"error\" :message=\"$message\" class=\"mb-4\"/>",
        json!({"message": "Whoops <b>"}),
    );
    assert_eq!(
        out,
        "\n<div class=\"alert alert-error mb-4\">\n    Whoops &lt;b&gt;\n</div>\n"
    );

    let defaults = views.inline("<x-alert message=\"Hi\"/>", ());
    assert_eq!(
        defaults,
        "\n<div class=\"alert alert-info\">\n    Hi\n</div>\n"
    );
}

#[test]
fn attributes_render_and_merge() {
    let views = Views::new();
    views.add(
        "components.button",
        "<button {{ $attributes->merge(['type' => 'button']) }}>{{ $slot }}</button>",
    );
    assert_eq!(
        views.inline("<x-button type=\"submit\">Submit</x-button>", ()),
        "<button type=\"submit\">Submit</button>"
    );
    assert_eq!(
        views.inline("<x-button>Go</x-button>", ()),
        "<button type=\"button\">Go</button>"
    );
    assert_eq!(
        views.inline(
            "<x-button disabled ::class=\"{ danger: isDeleting }\">Go</x-button>",
            ()
        ),
        "<button type=\"button\" disabled=\"disabled\" :class=\"{ danger: isDeleting }\">Go</button>"
    );
    // Bound strings are escaped, static strings keep their contents.
    assert_eq!(
        views.inline(
            "<x-button :title=\"$t\" data-x=\"a &amp; b\">Go</x-button>",
            json!({"t": "<i>"})
        ),
        "<button type=\"button\" title=\"&lt;i&gt;\" data-x=\"a &amp; b\">Go</button>"
    );
    // Echoes inside static attributes are escaped.
    assert_eq!(
        views.inline(
            "<x-button class=\"p-{{ $size }}\">Go</x-button>",
            json!({"size": "4\""})
        ),
        "<button type=\"button\" class=\"p-4&quot;\">Go</button>"
    );
}

#[test]
fn attribute_bag_methods() {
    let views = Views::new();
    views.add(
        "components.panel",
        "<div {{ $attributes->class(['p-4', 'bg-red' => $hasError ?? false])->merge(['type' => 'x']) }}>|{{ $attributes->get('class') }}|{{ $attributes->has('id') ? 'id' : 'no-id' }}|{{ $attributes->hasAny(['href', 'id']) ? 'any' : 'none' }}|{{ $attributes->only(['id']) }}|{{ $attributes->except(['id', 'class']) }}|{{ $attributes->whereStartsWith('wire:model') }}|{{ $attributes->whereDoesntStartWith('wire:') }}|{{ $attributes->whereStartsWith('wire:model')->first() }}|{{ $attributes->filter(fn ($value, $key) => $key == 'id') }}|{{ $attributes['id'] }}",
    );
    let out = views.inline(
        "<x-panel id=\"main\" class=\"mt-2\" wire:model=\"name\" :has-error=\"true\"/>",
        (),
    );
    assert_eq!(
        out,
        "<div type=\"x\" class=\"p-4 bg-red mt-2\" id=\"main\" wire:model=\"name\" has-error=\"has-error\">|mt-2|id|any|id=\"main\"|wire:model=\"name\" has-error=\"has-error\"|wire:model=\"name\"|id=\"main\" class=\"mt-2\" has-error=\"has-error\"|name|id=\"main\"|main"
    );
}

#[test]
fn prepends_and_styles() {
    let views = Views::new();
    views.add(
        "components.profile",
        "<div {{ $attributes->merge(['data-controller' => $attributes->prepends('profile-controller')]) }}></div><p {{ $attributes->only('style')->style(['color: red']) }}></p>",
    );
    assert_eq!(
        views.inline(
            "<x-profile data-controller=\"extra\" style=\"margin: 0\"/>",
            ()
        ),
        "<div data-controller=\"profile-controller extra\" style=\"margin: 0;\"></div><p style=\"color: red; margin: 0;\"></p>"
    );
}

#[test]
fn slots_and_named_slots() {
    let views = Views::new();
    views.add(
        "components.alert",
        "<span class=\"alert-title\">{{ $title }}</span>\n<div class=\"alert\">\n    @if ($slot->isEmpty())\n        Empty\n    @else\n        {{ $slot }}\n    @endif\n</div>",
    );
    let template = "<x-alert>\n    <x-slot:title>\n        Server <b>Error</b>\n    </x-slot>\n\n    <strong>Whoops!</strong> Something went wrong!\n</x-alert>";
    assert_eq!(
        views.inline(template, ()),
        "<span class=\"alert-title\">Server <b>Error</b></span>\n<div class=\"alert\">\n            <strong>Whoops!</strong> Something went wrong!\n    </div>"
    );
    let named = "<x-alert><x-slot name=\"title\">Named</x-slot></x-alert>";
    assert_eq!(
        views.inline(named, ()),
        "<span class=\"alert-title\">Named</span>\n<div class=\"alert\">\n            Empty\n    </div>"
    );
}

#[test]
fn slot_attributes_and_actual_content() {
    let views = Views::new();
    views.add(
        "components.card",
        "@props(['heading', 'footer'])\n<div {{ $attributes->class(['border']) }}><h1 {{ $heading->attributes->class(['text-lg']) }}>{{ $heading }}</h1>{{ $slot }}<footer {{ $footer->attributes->class(['text-gray-700']) }}>{{ $footer }}</footer>{{ $slot->hasActualContent() ? 'Y' : 'N' }}</div>",
    );
    let template = "<x-card class=\"shadow-sm\">\n    <x-slot:heading class=\"font-bold\">\n        Heading\n    </x-slot>\n\n    Content\n\n    <x-slot:footer class=\"text-sm\">\n        Footer\n    </x-slot>\n</x-card>";
    assert_eq!(
        views.inline(template, ()),
        "<div class=\"border shadow-sm\"><h1 class=\"text-lg font-bold\">Heading</h1>Content<footer class=\"text-gray-700 text-sm\">Footer</footer>Y</div>"
    );
    assert_eq!(
        views.inline("<x-card><x-slot:heading>H</x-slot><x-slot:footer>F</x-slot><!-- only a comment --></x-card>", ()),
        "<div class=\"border\"><h1 class=\"text-lg\">H</h1><!-- only a comment --><footer class=\"text-gray-700\">F</footer>N</div>"
    );
}

#[test]
fn nested_and_index_components() {
    let views = Views::new();
    views.add(
        "components.forms.input",
        "<input name=\"{{ $name }}\" {{ $attributes }}>",
    );
    views.add(
        "components.accordion.accordion",
        "<div class=\"accordion\">{{ $slot }}</div>",
    );
    views.add(
        "components.accordion.item",
        "<section>{{ $slot }}</section>",
    );
    views.add(
        "components.card.index",
        "<div class=\"card\">{{ $slot }}</div>",
    );
    assert_eq!(
        views.inline("<x-forms.input name=\"email\" required/>", ()),
        "<input name=\"email\" name=\"email\" required=\"required\">"
    );
    assert_eq!(
        views.inline(
            "<x-accordion><x-accordion.item>One</x-accordion.item></x-accordion>",
            ()
        ),
        "<div class=\"accordion\"><section>One</section></div>"
    );
    assert_eq!(
        views.inline("<x-card>C</x-card>", ()),
        "<div class=\"card\">C</div>"
    );
}

#[test]
fn components_render_in_isolation() {
    let views = Views::new();
    views.add(
        "components.secret",
        "{{ $outer ?? 'isolated' }} {{ $shared }}",
    );
    views.factory.share("shared", "everyone");
    assert_eq!(
        views.inline("<x-secret/>", json!({"outer": "parent"})),
        "isolated everyone"
    );
}

#[test]
fn short_attribute_syntax_and_camel_case_data() {
    let views = Views::new();
    views.add(
        "components.profile",
        "{{ $userId }}-{{ $name }}-{{ $alertType }}",
    );
    assert_eq!(
        views.inline(
            "<x-profile :$userId :$name alert-type=\"danger\"/>",
            json!({"userId": 7, "name": "Taylor"})
        ),
        "7-Taylor-danger"
    );
}

#[test]
fn dynamic_components() {
    let views = Views::new();
    views.add(
        "components.secondary-button",
        "<button {{ $attributes }}>{{ $slot }}</button>",
    );
    assert_eq!(
        views.inline("<x-dynamic-component :component=\"$componentName\" class=\"mt-4\">Go</x-dynamic-component>", json!({"componentName": "secondary-button"})),
        "<button class=\"mt-4\">Go</button>"
    );
}

#[test]
fn forwarding_attributes_to_child_components() {
    let views = Views::new();
    views.add(
        "components.base-button",
        "<button {{ $attributes->merge(['class' => 'btn']) }}>{{ $slot }}</button>",
    );
    views.add("components.danger-button", "<x-base-button {{ $attributes->merge(['class' => 'btn-danger']) }}>{{ $slot }}</x-base-button>");
    assert_eq!(
        views.inline(
            "<x-danger-button class=\"w-full\" id=\"delete\">Delete</x-danger-button>",
            ()
        ),
        "<button class=\"btn btn-danger w-full\" id=\"delete\">Delete</button>"
    );
}

#[test]
fn aware_reads_parent_component_data() {
    let views = Views::new();
    views.add("components.menu.index", "@props(['color' => 'gray'])\n<ul {{ $attributes->merge(['class' => 'bg-'.$color.'-200']) }}>{{ $slot }}</ul>");
    views.add("components.menu.item", "@aware(['color' => 'gray'])\n<li {{ $attributes->merge(['class' => 'text-'.$color.'-800']) }}>{{ $slot }}</li>");
    assert_eq!(
        views.inline(
            "<x-menu color=\"purple\"><x-menu.item>A</x-menu.item></x-menu>",
            ()
        ),
        "<ul class=\"bg-purple-200\"><li class=\"text-purple-800\">A</li></ul>"
    );
    assert_eq!(
        views.inline("<x-menu><x-menu.item>A</x-menu.item></x-menu>", ()),
        "<ul class=\"bg-gray-200\"><li class=\"text-gray-800\">A</li></ul>"
    );
}

#[test]
fn layouts_using_components() {
    let views = Views::new();
    views.add(
        "components.layout",
        "<html>\n    <head>\n        <title>{{ $title ?? 'Todo Manager' }}</title>\n    </head>\n    <body>\n        <h1>Todos</h1>\n        <hr/>\n        {{ $slot }}\n    </body>\n</html>\n",
    );
    views.add("tasks", "<x-layout>\n    <x-slot:title>\n        Custom Title\n    </x-slot>\n\n    @foreach ($tasks as $task)\n        <div>{{ $task }}</div>\n    @endforeach\n</x-layout>\n");
    assert_eq!(
        views.render("tasks", json!({"tasks": ["One", "Two"]})),
        "<html>\n    <head>\n        <title>Custom Title</title>\n    </head>\n    <body>\n        <h1>Todos</h1>\n        <hr/>\n        <div>One</div>\n            <div>Two</div>\n    </body>\n</html>\n"
    );
    views.add("plain", "<x-layout>Hi</x-layout>");
    assert!(
        views
            .render("plain", ())
            .contains("<title>Todo Manager</title>")
    );
}

#[test]
fn component_tags_swallow_the_following_newline() {
    let views = Views::new();
    views.add("components.badge", "<span>{{ $slot }}</span>\n");
    assert_eq!(
        views.inline("<div>\n    <x-badge>New</x-badge>\n</div>", ()),
        "<div>\n    <span>New</span>\n</div>"
    );
    assert_eq!(
        views.inline("<div>\n    <x-badge/>\n</div>", ()),
        "<div>\n    <span></span>\n</div>"
    );
}

struct Alert {
    kind: String,
    message: String,
}

impl Component for Alert {
    fn render(&self) -> ComponentView {
        ComponentView::view("components.class-alert")
    }

    fn data(&self) -> ViewData {
        data([
            ("type", ViewValue::from(self.kind.as_str())),
            ("message", ViewValue::from(self.message.as_str())),
            (
                "isUrgent",
                ViewValue::closure(|args| {
                    Ok(ViewValue::Bool(
                        args.first().is_some_and(|a| a.to_string() == "error"),
                    ))
                }),
            ),
        ])
    }
}

#[test]
fn class_based_components() {
    let views = Views::new();
    views.add(
        "components.class-alert",
        "<div {{ $attributes->merge(['class' => 'alert-'.$type]) }}>{{ $message }} {{ $isUrgent($type) ? '!' : '' }} {{ $componentName }}</div>",
    );
    views
        .factory
        .blade()
        .component("alert", |args: &mut ComponentArgs| {
            Ok(Alert {
                kind: args.take_string("type").unwrap_or_else(|| "info".into()),
                message: args.take_string("message").unwrap_or_default(),
            })
        });
    assert_eq!(
        views.inline(
            "<x-alert type=\"error\" :message=\"$msg\" class=\"mb-4\"/>",
            json!({"msg": "Oops"})
        ),
        "<div class=\"alert-error mb-4\">Oops ! alert</div>"
    );
}

struct Inline;

impl Component for Inline {
    fn render(&self) -> ComponentView {
        ComponentView::inline("<div class=\"inline\">{{ $slot }} {{ $label }}</div>")
    }

    fn data(&self) -> ViewData {
        data([("label", "L")])
    }
}

struct Hidden;

impl Component for Hidden {
    fn render(&self) -> ComponentView {
        ComponentView::inline("never")
    }

    fn should_render(&self) -> bool {
        false
    }
}

#[test]
fn inline_and_conditional_class_components() {
    let views = Views::new();
    views
        .factory
        .blade()
        .component("inline", |_: &mut ComponentArgs| Ok(Inline));
    views
        .factory
        .blade()
        .component("hidden", |_: &mut ComponentArgs| Ok(Hidden));
    assert_eq!(
        views.inline(
            "<x-inline>Body</x-inline>|<x-hidden>{{ $missing }}</x-hidden>|",
            ()
        ),
        "<div class=\"inline\">Body L</div>||"
    );
}

#[test]
fn scoped_slots_can_use_the_component() {
    let views = Views::new();
    views.add("components.class-alert", "<h1>{{ $title }}</h1>{{ $slot }}");
    views
        .factory
        .blade()
        .component("alert", |_: &mut ComponentArgs| Ok(Scoped));
    assert_eq!(
        views.inline("<x-alert><x-slot:title>{{ $component->formatAlert('Server Error') }}</x-slot>Body</x-alert>", ()),
        "<h1>SERVER ERROR</h1>Body"
    );
}

struct Scoped;

impl Component for Scoped {
    fn render(&self) -> ComponentView {
        ComponentView::view("components.class-alert")
    }

    fn data(&self) -> ViewData {
        data([(
            "formatAlert",
            ViewValue::closure(|args| Ok(args[0].to_string().to_uppercase().into())),
        )])
    }
}

#[test]
fn legacy_component_directive() {
    let views = Views::new();
    views.add(
        "alert",
        "<div class=\"{{ $type }}\"><b>{{ $title }}</b>{{ $slot }}</div>",
    );
    assert_eq!(
        views.inline("@component('alert', ['type' => 'error'])\n@slot('title')\nForbidden\n@endslot\nYou are not allowed.\n@endcomponent", ()),
        "<div class=\"error\"><b>Forbidden</b>You are not allowed.</div>"
    );
    assert_eq!(
        views.inline(
            "@component('alert', ['type' => 'x'])@slot('title', 'Inline')@endcomponent",
            ()
        ),
        "<div class=\"x\"><b>Inline</b></div>"
    );
}

#[test]
fn anonymous_component_paths_and_namespaces() {
    let views = Views::new();
    let extra = tempfile::tempdir().unwrap();
    std::fs::write(
        extra.path().join("panel.blade.html"),
        "<aside>{{ $slot }}</aside>",
    )
    .unwrap();
    views
        .factory
        .blade()
        .anonymous_component_path(extra.path(), None);
    views
        .factory
        .blade()
        .anonymous_component_path(extra.path(), Some("dashboard"));
    assert_eq!(
        views.inline(
            "<x-panel>A</x-panel><x-dashboard::panel>B</x-dashboard::panel>",
            ()
        ),
        "<aside>A</aside><aside>B</aside>"
    );

    views.add(
        "flights.components.ticket",
        "<ticket>{{ $number }}</ticket>",
    );
    views
        .factory
        .blade()
        .anonymous_component_namespace("flights.components", "flights");
    assert_eq!(
        views.inline("<x-flights::ticket number=\"7\"/>", ()),
        "<ticket>7</ticket>"
    );

    let mail = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(mail.path().join("components")).unwrap();
    std::fs::write(
        mail.path().join("components/button.blade.html"),
        "<a>{{ $slot }}</a>",
    )
    .unwrap();
    views.factory.add_namespace("mail", mail.path());
    assert_eq!(
        views.inline("<x-mail::button>Go</x-mail::button>", ()),
        "<a>Go</a>"
    );
}

#[test]
fn missing_components_are_reported() {
    let views = Views::new();
    let error = views.inline_err("line one\n<x-missing/>", ());
    assert_eq!(
        error.to_string(),
        "Unable to locate a class or view for component [missing]. (View: __inline, line 2)"
    );
}
