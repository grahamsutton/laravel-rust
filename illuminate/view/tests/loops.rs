mod common;

use common::blade;
use illuminate_support::json;

#[test]
fn for_loops() {
    let template = "@for ($i = 0; $i < 3; $i++)\nThe current value is {{ $i }}\n@endfor\n";
    assert_eq!(
        blade(template, ()),
        "The current value is 0\nThe current value is 1\nThe current value is 2\n"
    );
    assert_eq!(
        blade("@for ($i = 10; $i > 0; $i -= 4){{ $i }} @endfor", ()),
        "10 6 2 "
    );
}

#[test]
fn foreach_loops() {
    let template =
        "@foreach ($users as $user)\n<p>This is user {{ $user['id'] }}</p>\n@endforeach\n";
    assert_eq!(
        blade(template, json!({"users": [{"id": 1}, {"id": 2}]})),
        "<p>This is user 1</p>\n<p>This is user 2</p>\n"
    );
    assert_eq!(
        blade(
            "@foreach ($prices as $item => $price){{ $item }}={{ $price }};@endforeach",
            json!({"prices": {"apple": 1, "pear": 2}})
        ),
        "apple=1;pear=2;"
    );
    assert_eq!(
        blade(
            "@foreach ($users as $user){{ $user->name }} @endforeach",
            json!({"users": [{"name": "Taylor"}]})
        ),
        "Taylor "
    );
    assert_eq!(
        blade(
            "@foreach ([[1, 2], [3, 4]] as [$a, $b]){{ $a + $b }} @endforeach",
            ()
        ),
        "3 7 "
    );
    assert_eq!(
        blade(
            "@foreach ($nothing as $x)x @endforeach!",
            json!({"nothing": null})
        ),
        "!"
    );
}

#[test]
fn forelse_loops() {
    let template = "@forelse ($users as $user)\n    <li>{{ $user }}</li>\n@empty\n    <p>No users</p>\n@endforelse\n";
    assert_eq!(
        blade(template, json!({"users": ["a", "b"]})),
        "    <li>a</li>\n    <li>b</li>\n"
    );
    assert_eq!(
        blade(template, json!({"users": []})),
        "    <p>No users</p>\n"
    );
}

#[test]
fn while_loops() {
    assert_eq!(
        blade("@php($i = 0)@while ($i < 3){{ $i++ }}@endwhile", ()),
        "012"
    );
}

#[test]
fn break_and_continue() {
    let template = "@foreach ($users as $user)\n@if ($user['type'] == 1)\n@continue\n@endif\n<li>{{ $user['name'] }}</li>\n@if ($user['number'] == 5)\n@break\n@endif\n@endforeach";
    let users = json!({"users": [
        {"type": 1, "name": "skip", "number": 1},
        {"type": 2, "name": "a", "number": 2},
        {"type": 2, "name": "b", "number": 5},
        {"type": 2, "name": "c", "number": 6},
    ]});
    assert_eq!(blade(template, users.clone()), "<li>a</li>\n<li>b</li>\n");

    let compact = "@foreach ($users as $user)\n@continue($user['type'] == 1)\n<li>{{ $user['name'] }}</li>\n@break($user['number'] == 5)\n@endforeach";
    assert_eq!(blade(compact, users), "<li>a</li>\n<li>b</li>\n");

    let nested = "@foreach ([1, 2] as $a)@foreach ([1, 2, 3] as $b)@continue(2)@endforeach{{ $a }}@endforeach|@foreach ([1, 2] as $a)@foreach ([1, 2] as $b){{ $a }}{{ $b }} @break(2)@endforeach\n@endforeach";
    assert_eq!(blade(nested, ()), "|11 ");
}

#[test]
fn the_loop_variable() {
    let template = "@foreach ($items as $item){{ $loop->index }}/{{ $loop->iteration }}/{{ $loop->remaining }}/{{ $loop->count }}/{{ $loop->first ? 'F' : '' }}{{ $loop->last ? 'L' : '' }}/{{ $loop->even ? 'E' : 'O' }}/{{ $loop->depth }} @endforeach";
    assert_eq!(
        blade(template, json!({"items": ["a", "b", "c"]})),
        "0/1/2/3/F/O/1 1/2/1/3//E/1 2/3/0/3/L/O/1 "
    );
}

#[test]
fn nested_loops_expose_the_parent() {
    let template = "@foreach ($users as $user)@foreach ($user['posts'] as $post)@if ($loop->parent->first)[first:{{ $post }}]@else[{{ $post }}@{{ $loop->depth }}]@endif\n@endforeach{{ $loop->depth }}@endforeach{{ $loop ?? 'none' }}";
    let data = json!({"users": [{"posts": ["a", "b"]}, {"posts": ["c"]}]});
    assert_eq!(
        blade(template, data),
        "[first:a][first:b]1[c{{ $loop->depth }}]1none"
    );
}

#[test]
fn loops_over_collections_and_paginators() {
    let paginator = json!({"users": {
        "current_page": 1, "data": [{"name": "Taylor"}, {"name": "Abigail"}], "first_page_url": "/?page=1",
        "from": 1, "last_page": 1, "last_page_url": "/?page=1", "links": [], "next_page_url": null,
        "path": "/", "per_page": 15, "prev_page_url": null, "to": 2, "total": 2
    }});
    let template = "@foreach ($users as $user){{ $user->name }} @endforeach\n({{ $users->total() }} total, page {{ $users->currentPage() }}, {{ count($users) }} shown)";
    assert_eq!(
        blade(template, paginator),
        "Taylor Abigail (2 total, page 1, 2 shown)"
    );
}
