mod common;

use common::{Views, blade};
use illuminate_support::{Carbon, Result, json};
use illuminate_view::{ViewObject, ViewValue, data};

#[test]
fn literals_and_operators() {
    assert_eq!(blade("{{ 1 + 2 * 3 }}|{{ (1 + 2) * 3 }}|{{ 2 ** 10 }}|{{ 7 % 3 }}|{{ -5 + 2 }}", ()), "7|9|1024|1|-3");
    assert_eq!(blade("{{ 'a' . 'b' . 1 + 2 }}|{{ \"tab\\tend\" }}|{{ 'it\\'s' }}", ()), "ab3|tab\tend|it&#039;s");
    assert_eq!(blade("{{ true && false ? 'y' : 'n' }}|{{ null ?? 'd' }}|{{ 0 ?: 'zero' }}|{{ !empty([1]) ? 'full' : 'empty' }}", ()), "n|d|zero|full");
    assert_eq!(blade("{{ 1 <=> 2 }}|{{ '10' == '1e1' ? 'eq' : 'ne' }}|{{ 'abc' == 0 ? 'eq' : 'ne' }}|{{ null == false ? 'eq' : 'ne' }}", ()), "-1|eq|ne|eq");
    assert_eq!(blade("{{ 5 > 3 and 2 > 1 ? 'yes' : 'no' }}|{{ (int) '42abc' + 1 }}|{{ (bool) '0' ? 't' : 'f' }}|{{ (string) 1.50 }}", ()), "1|43|f|1.5"); // `and` binds looser than `?:`, just like PHP.
}

#[test]
fn string_interpolation() {
    let data = json!({"user": {"name": "Taylor", "roles": ["admin"]}, "count": 3});
    assert_eq!(
        blade("{{ \"Hello {$user['name']}, you have $count messages and the {$user['roles'][0]} role ($user[name])\" }}", data),
        "Hello Taylor, you have 3 messages and the admin role (Taylor)"
    );
}

#[test]
fn arrays_properties_and_null_safety() {
    let data = json!({"user": {"name": "Taylor", "address": null, "tags": ["a", "b"]}, "list": [10, 20]});
    assert_eq!(
        blade("{{ $user->name }}|{{ $user['name'] }}|{{ $list[1] }}|{{ $user->tags[0] }}|{{ $user->address?->city ?? 'no city' }}|{{ $user->missing ?? 'missing' }}", data),
        "Taylor|Taylor|20|a|no city|missing"
    );
    assert_eq!(blade("{{ ['a' => 1, 'b' => 2]['b'] }}|{{ [1, 2, ...[3, 4]][3] }}|{{ 'abc'[1] }}", ()), "2|4|b");
}

#[test]
fn collection_methods_on_arrays() {
    let data = json!({"users": [
        {"name": "Taylor", "age": 40, "active": true},
        {"name": "Abigail", "age": 30, "active": false},
        {"name": "James", "age": 35, "active": true},
    ]});
    let template = "{{ $users->count() }}|{{ $users->first()['name'] }}|{{ $users->last()->name }}|{{ $users->pluck('name')->implode(', ') }}|{{ $users->sum('age') }}|{{ $users->avg('age') }}|{{ $users->max('age') }}|{{ $users->where('active', true)->count() }}|{{ $users->sortBy('age')->first()->name }}|{{ $users->sortByDesc('age')->pluck('name')->first() }}|{{ $users->isEmpty() ? 'empty' : 'not empty' }}|{{ $users->take(2)->count() }}|{{ $users->firstWhere('name', 'James')->age }}|{{ $users->contains('name', 'Abigail') ? 'has' : 'not' }}";
    assert_eq!(blade(template, data.clone()), "3|Taylor|James|Taylor, Abigail, James|105|35|40|2|Abigail|Taylor|not empty|2|35|has");

    let closures = "{{ $users->map(fn ($u) => strtoupper($u->name))->implode(' ') }}|{{ $users->filter(fn ($u) => $u->age > 32)->count() }}|{{ $users->reject(fn ($u) => $u->active)->first()->name }}|{{ collect([3, 1, 2])->sort()->values()->implode(',') }}|{{ $users->groupBy('active')->keys()->implode(',') }}|{{ $users->keyBy('name')->keys()->first() }}|{{ $users->chunk(2)->count() }}|{!! json_encode($users->pluck('age', 'name')) !!}";
    assert_eq!(blade(closures, data), "TAYLOR ABIGAIL JAMES|2|Abigail|1,2,3|1,0|Taylor|2|{\"Taylor\":40,\"Abigail\":30,\"James\":35}");
}

#[test]
fn serialized_models_answer_accessor_methods() {
    let data = json!({"post": {"title": "Hello", "created_at": "2024-03-12T15:30:00Z", "comment_count": 3}});
    assert_eq!(
        blade("{{ $post->commentCount() }}|{{ $post->created_at->format('M j, Y') }}|{{ $post->created_at->year }}|{{ $post->createdAt()->toDateString() }}", data),
        "3|Mar 12, 2024|2024|2024-03-12"
    );
}

#[test]
fn static_support_classes() {
    assert_eq!(
        blade("{{ Str::limit('The quick brown fox', 9) }}|{{ Str::title('hello world') }}|{{ Str::plural('post', 2) }}|{{ Str::slug('Hello World') }}|{{ \\Illuminate\\Support\\Str::upper('a') }}|{{ str('taylor')->ucfirst() }}", ()),
        "The quick...|Hello World|posts|hello-world|A|Taylor"
    );
    assert_eq!(
        blade("{{ Number::format(1234567.891, 2) }}|{{ Number::currency(9.5) }}|{{ Number::percentage(50) }}|{{ Number::fileSize(1024) }}|{{ Number::abbreviate(1500) }}", ()),
        "1,234,567.89|$9.50|50%|1 KB|2K"
    );
    assert_eq!(blade("{{ Arr::get(['a' => ['b' => 1]], 'a.b') }}|{{ Arr::toCssClasses(['x', 'y' => false]) }}", ()), "1|x");
}

#[test]
fn php_functions() {
    assert_eq!(
        blade("{{ sprintf('%05.2f %s', 3.14159, 'pi') }}|{{ number_format(1234.5) }}|{{ number_format(1234.567, 2) }}|{{ round(2.5) }}|{{ floor(2.7) }}|{{ max([1, 9, 3]) }}|{{ str_repeat('ab', 2) }}|{{ ucwords('hello there') }}|{{ substr('Laravel', 0, 3) }}|{{ in_array(2, [1, 2]) ? 'in' : 'out' }}", ()),
        "03.14 pi|1,235|1,234.57|3|2|9|abab|Hello There|Lar|in"
    );
    assert_eq!(
        blade("{{ implode(',', array_map(fn ($n) => $n * 2, [1, 2, 3])) }}|{{ implode(',', array_filter([1, 0, 2])) }}|{{ implode(',', array_keys(['a' => 1, 'b' => 2])) }}|{{ implode('-', explode(' ', 'a b c')) }}|{{ implode(',', range(1, 5)) }}|{{ count(array_merge([1], [2, 3])) }}", ()),
        "2,4,6|1,2|a,b|a-b-c|1,2,3,4,5|3"
    );
    assert_eq!(blade("{!! nl2br(e(\"a\\n<b>\")) !!}|{{ strip_tags('<p>Hi</p>') }}|{{ trim('  x  ') }}|{{ strlen('héllo') }}|{{ mb_strlen('héllo') }}", ()), "a<br />\n&lt;b&gt;|Hi|x|6|5");
    assert_eq!(blade("{{ json_encode(['a' => 1, 'b' => [true, null]]) }}|{{ json_decode('{\"x\": 5}')['x'] }}", ()), "{&quot;a&quot;:1,&quot;b&quot;:[true,null]}|5");
    assert_eq!(blade("{{ data_get(['a' => ['b' => 'c']], 'a.b') }}|{{ value(fn () => 'called') }}|{{ blank('  ') ? 'blank' : '' }}|{{ filled(0) ? 'filled' : '' }}|{{ optional(null)->name ?? 'none' }}", ()), "c|called|blank|filled|none");
}

#[test]
fn dates() {
    Carbon::set_test_now(None);
    assert_eq!(blade("{{ date('Y-m-d', 0) }}", ()), "1970-01-01");
    assert_eq!(blade("{{ Carbon::parse('2024-03-12')->addDays(3)->format('l jS') }}", ()), "Friday 15th");
    let published = json!({"published": "2020-01-01 10:00:00"});
    assert_eq!(blade("{{ $published->isPast() ? 'past' : 'future' }}|{{ $published->toFormattedDateString() }}", published), "past|Jan 1, 2020");
    let date = Carbon::parse("2024-06-01 12:00:00").unwrap();
    assert_eq!(blade("{{ $date->format('D, M j') }}|{{ $date->month }}", data([("date", date)])), "Sat, Jun 1|6");
    assert!(blade("{{ now()->year }}", ()).parse::<i32>().unwrap() >= 2024);
}

#[test]
fn closures_and_match() {
    assert_eq!(
        blade("@php($double = fn ($x) => $x * 2)\n{{ $double(21) }}|{{ match($status) { 'draft', 'pending' => 'Waiting', 'live' => 'Live', default => 'Unknown' } }}", json!({"status": "pending"})),
        "42|Waiting"
    );
    assert_eq!(blade("@php($add = function ($a, $b = 10) use ($base) { return $a + $b + $base; })\n{{ $add(1) }}", json!({"base": 100})), "111");
}

#[test]
fn custom_functions_override_builtins() {
    let views = Views::new();
    views.factory.blade().function("route", |args| Ok(format!("/{}", args[0].to_string().replace('.', "/")).into()));
    views.factory.blade().function("Route::has", |args| Ok((args[0].to_string() == "login").into()));
    views.factory.blade().function("old", |args| Ok(args.get(1).cloned().unwrap_or_default()));
    assert_eq!(
        views.inline("<a href=\"{{ route('users.index') }}\">@if (Route::has('login'))Login @endif{{ old('email', 'x@y.z') }}</a>", ()),
        "<a href=\"/users/index\">Login x@y.z</a>"
    );
    let error = views.inline_err("{{ asset('app.css') }}", ());
    assert!(error.to_string().starts_with("Call to undefined function asset()"), "{error}");
}

struct Podcast;

impl ViewObject for Podcast {
    fn class_name(&self) -> &str {
        "App\\Models\\Podcast"
    }

    fn get(&self, property: &str) -> Option<ViewValue> {
        (property == "title").then(|| "Laravel News".into())
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        match method {
            "episodes" => Some(Ok(ViewValue::list((1..=args.first().and_then(ViewValue::as_i64).unwrap_or(2)).map(ViewValue::from)))),
            _ => None,
        }
    }

    fn to_string_value(&self) -> Option<String> {
        Some("<podcast>".into())
    }
}

#[test]
fn view_objects() {
    let views = Views::new();
    let data = data([("podcast", ViewValue::object(Podcast))]);
    assert_eq!(
        views.inline("{{ $podcast->title }}|{{ count($podcast->episodes(3)) }}|{{ $podcast }}", data.clone()),
        "Laravel News|3|&lt;podcast&gt;"
    );
    let error = views.inline_err("{{ $podcast->nope() }}", data);
    assert_eq!(error.to_string(), "Call to undefined method App\\Models\\Podcast::nope() (View: __inline, line 1)");
}

#[test]
fn request_helper() {
    let request = illuminate_http::Request::create("/admin/users?sort=name", "GET");
    let html = illuminate_http::with_request_sync(request, || {
        blade("{{ request()->path() }}|{{ request()->is('admin/*') ? 'admin' : 'public' }}|{{ request('sort') }}|{{ request()->query('missing', 'default') }}", ())
    });
    assert_eq!(html, "admin/users|admin|name|default");
}
