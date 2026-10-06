mod common;

use common::{Views, blade};
use illuminate_support::{HtmlString, json};
use illuminate_view::{ViewData, ViewValue, data};

#[test]
fn it_echoes_escaped_data() {
    assert_eq!(
        blade("Hello, {{ $name }}.", json!({"name": "Samantha"})),
        "Hello, Samantha."
    );
    assert_eq!(
        blade(
            "{{ $html }}",
            json!({"html": "<script>alert('x')</script> & \"q\""})
        ),
        "&lt;script&gt;alert(&#039;x&#039;)&lt;/script&gt; &amp; &quot;q&quot;"
    );
    // Double encoding is on by default, like Laravel.
    assert_eq!(blade("{{ $v }}", json!({"v": "&amp;"})), "&amp;amp;");
    assert_eq!(blade("{{{ $v }}}", json!({"v": "<b>"})), "&lt;b&gt;");
    assert_eq!(blade("{{$v}}", json!({"v": "tight"})), "tight");
    assert_eq!(blade("{{ $v; }}", json!({"v": "semicolon"})), "semicolon");
}

#[test]
fn it_echoes_raw_data() {
    assert_eq!(
        blade("Hello, {!! $name !!}.", json!({"name": "<b>Taylor</b>"})),
        "Hello, <b>Taylor</b>."
    );
}

#[test]
fn html_strings_are_not_escaped() {
    let data = data([("html", ViewValue::from(HtmlString::new("<em>hi</em>")))]);
    assert_eq!(blade("{{ $html }}", data), "<em>hi</em>");
}

#[test]
fn scalars_echo_like_php() {
    let out = blade(
        "{{ true }}|{{ false }}|{{ null }}|{{ 1.0 }}|{{ 0.1 + 0.2 }}|{{ 10 / 4 }}|{{ 7 }}|{{ $list }}",
        json!({"list": [1, 2]}),
    );
    assert_eq!(out, "1|||1|0.3|2.5|7|[1,2]");
}

#[test]
fn without_double_encoding_keeps_entities() {
    let views = Views::new();
    views.factory.blade().without_double_encoding();
    assert_eq!(
        views.inline("{{ $v }}", json!({"v": "&amp; <b>"})),
        "&amp; &lt;b&gt;"
    );
}

#[test]
fn comments_are_removed() {
    assert_eq!(
        blade("A{{-- This comment will not be present --}}B", ()),
        "AB"
    );
    assert_eq!(blade("A{{-- multi\nline {{ $missing }} --}}B", ()), "AB");
    assert_eq!(blade("{{-- comment --}}\nB", ()), "\nB");
}

#[test]
fn escaped_echoes_and_directives_stay_literal() {
    assert_eq!(blade("Hello, @{{ name }}.", ()), "Hello, {{ name }}.");
    assert_eq!(blade("@{!! raw !!}", ()), "{!! raw !!}");
    assert_eq!(blade("vue@{{ name }}", ()), "vue{{ name }}");
    assert_eq!(blade("@@if()", ()), "@if()");
    assert_eq!(blade("@@foreach ($a as $b)", ()), "@foreach($a as $b)");
}

#[test]
fn verbatim_blocks_are_untouched() {
    let out = blade(
        "@verbatim\n    <div>Hello, {{ name }} @if(x) </div>\n@endverbatim",
        (),
    );
    assert_eq!(out, "\n    <div>Hello, {{ name }} @if(x) </div>\n");
}

#[test]
fn unknown_directives_and_emails_are_literal() {
    assert_eq!(
        blade(
            "Mail taylor@laravel.com @media (min-width: 640px) { a { b: c } } @tailwind base;",
            ()
        ),
        "Mail taylor@laravel.com @media (min-width: 640px) { a { b: c } } @tailwind base;"
    );
    assert_eq!(
        blade("<button @click=\"open = !open\">Go</button>", ()),
        "<button @click=\"open = !open\">Go</button>"
    );
}

#[test]
fn echoes_keep_their_newlines_and_directives_swallow_one() {
    let out = blade("{{ $a }}\n{{ $b }}\n", json!({"a": 1, "b": 2}));
    assert_eq!(out, "1\n2\n");

    let template =
        "<ul>\n    @foreach ($items as $item)\n    <li>{{ $item }}</li>\n    @endforeach\n</ul>\n";
    assert_eq!(
        blade(template, json!({"items": ["a", "b"]})),
        "<ul>\n        <li>a</li>\n        <li>b</li>\n    </ul>\n"
    );

    // Trailing spaces after a directive keep the newline (like Laravel).
    assert_eq!(blade("@if(true)  \nA @endif", ()), "  \nA ");
    assert_eq!(blade("@if(true)\r\nA @endif", ()), "A ");
    // Like Laravel, a directive glued to a word is plain text.
    assert_eq!(blade("@if(true)A@endif @endif", ()), "A@endif ");
}

#[test]
fn echoes_can_call_functions() {
    let out = blade(
        "{{ strtoupper($name) }} {{ count($items) }} {{ implode(', ', $items) }}",
        json!({"name": "taylor", "items": ["a", "b"]}),
    );
    assert_eq!(out, "TAYLOR 2 a, b");
}

#[test]
fn json_and_js_directives() {
    let data = json!({"data": {"name": "<Taylor>", "tags": ["a/b"]}});
    assert_eq!(
        blade("var app = @json($data);", data.clone()),
        "var app = {\"name\":\"\\u003CTaylor\\u003E\",\"tags\":[\"a\\/b\"]};"
    );
    assert_eq!(
        blade("@json($data, JSON_PRETTY_PRINT)", json!({"data": {"a": 1}})),
        "{\n    \"a\": 1\n}"
    );
    assert_eq!(blade("{{ Js::from('it\\'s') }}", ()), "'it\\u0027s'");
    assert_eq!(
        blade("@js($data)", json!({"data": ["x"]})),
        "JSON.parse('[\\u0022x\\u0022]')"
    );
}

#[test]
fn class_and_style_directives() {
    let template = "<span @class([\n    'p-4',\n    'font-bold' => $isActive,\n    'text-gray-500' => ! $isActive,\n    'bg-red' => $hasError,\n])></span>";
    assert_eq!(
        blade(template, json!({"isActive": false, "hasError": true})),
        "<span class=\"p-4 text-gray-500 bg-red\"></span>"
    );
    assert_eq!(
        blade(
            "<span @style(['background-color: red', 'font-weight: bold' => $isActive])></span>",
            json!({"isActive": true})
        ),
        "<span style=\"background-color: red; font-weight: bold;\"></span>"
    );
}

#[test]
fn attribute_directives() {
    let template = "<input @checked($a) @selected($b) @disabled($c) @readonly($d) @required($e)>";
    assert_eq!(
        blade(
            template,
            json!({"a": true, "b": false, "c": 1, "d": "", "e": "yes"})
        ),
        "<input checked  disabled  required>"
    );
    assert_eq!(blade("{{ 1 }}@bool(true)/@bool(0)", ()), "1true/false");
}

#[test]
fn php_blocks_assign_variables() {
    let template = "@php\n    $counter = 1;\n    $counter += 2;\n    $items = [];\n    $items[] = 'a';\n    $items['k'] = 'b';\n@endphp\n{{ $counter }} {{ implode(',', $items) }}";
    assert_eq!(blade(template, ()), "3 a,b");
    assert_eq!(blade("@php($x = 5){{ $x * 2 }}", ()), "10");
    assert_eq!(blade("@php $y = 'inline'; @endphp{{ $y }}", ()), "inline");
    assert_eq!(blade("@php echo '<b>'; @endphp", ()), "<b>");
    assert_eq!(
        blade("@php($a = 1)\n@php($a++)\n@php(++$a)\n{{ $a }}", ()),
        "3"
    );
    assert_eq!(
        blade(
            "@php($x = 1) @unset($x) {{ isset($x) ? 'set' : 'unset' }}",
            ()
        ),
        "  unset"
    );
}

#[test]
fn view_data_can_be_built_by_hand() {
    let mut data = ViewData::new();
    data.insert("count".into(), 3.into());
    data.insert("names".into(), ViewValue::from(vec!["a", "b"]));
    assert_eq!(
        blade("{{ $count }}: {{ implode(' ', $names) }}", data),
        "3: a b"
    );
}
