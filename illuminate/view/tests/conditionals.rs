mod common;

use std::sync::Arc;

use common::{Views, blade};
use illuminate_config::Repository;
use illuminate_container::Container;
use illuminate_support::json;
use illuminate_view::ViewValue;

#[test]
fn if_elseif_else() {
    let template = "@if (count($records) === 1)\n    I have one record!\n@elseif (count($records) > 1)\n    I have multiple records!\n@else\n    I don't have any records!\n@endif\n";
    assert_eq!(blade(template, json!({"records": [1]})), "    I have one record!\n");
    assert_eq!(blade(template, json!({"records": [1, 2]})), "    I have multiple records!\n");
    assert_eq!(blade(template, json!({"records": []})), "    I don't have any records!\n");
}

#[test]
fn unless_isset_and_empty() {
    assert_eq!(blade("@unless ($signedIn)You are not signed in.@endunless", json!({"signedIn": false})), "You are not signed in.");
    assert_eq!(blade("@isset($records)defined @endisset|@isset($missing)x @endisset", json!({"records": []})), "defined |");
    assert_eq!(blade("@isset($nothing)x @else not set @endisset", json!({"nothing": null})), " not set ");
    assert_eq!(blade("@empty($records)empty @endempty", json!({"records": []})), "empty ");
    assert_eq!(blade("@empty($missing)missing is empty @endempty", ()), "missing is empty ");
    assert_eq!(blade("@empty($records)empty @else full @endempty", json!({"records": [1]})), " full ");
}

#[test]
fn switch_statements() {
    let template = "@switch($i)\n    @case(1)\n        First case...\n        @break\n\n    @case(2)\n        Second case...\n        @break\n\n    @default\n        Default case...\n@endswitch\n";
    // Like Laravel, the indentation before @break is part of the case.
    assert_eq!(blade(template, json!({"i": 1})), "        First case...\n        ");
    assert_eq!(blade(template, json!({"i": "2"})), "        Second case...\n        ");
    assert_eq!(blade(template, json!({"i": 9})), "        Default case...\n");

    // Without a @break, cases fall through.
    let fallthrough = "@switch($i)@case(1)one @case(2)two @break @case(3)three @endswitch";
    assert_eq!(blade(fallthrough, json!({"i": 1})), "one two ");
}

#[test]
fn auth_and_guest_use_the_auth_check_hook() {
    let views = Views::new();
    let template = "@auth Welcome back! @else Please log in. @endauth|@guest guest @endguest|@auth('admin') admin @endauth";
    assert_eq!(views.inline(template, ()), " Please log in. | guest |");

    views.factory.blade().function("auth_check", |args| {
        Ok(ViewValue::Bool(args.first().is_none_or(|guard| guard.to_string() == "web")))
    });
    assert_eq!(views.inline(template, ()), " Welcome back! ||");
}

#[test]
fn environment_directives_use_the_app_environment_hook() {
    let views = Views::new();
    let template = "@production prod @endproduction\n@env('local') local @endenv\n@env(['staging', 'production']) staged @endenv";
    // Without a hook or configuration, the environment is "production".
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    assert_eq!(views.inline(template, ()), " prod  staged ");

    container.instance(Repository::new(json!({"app": {"env": "local"}})));
    assert_eq!(views.inline(template, ()), " local ");

    views.factory.blade().function("app_environment", |_| Ok("staging".into()));
    assert_eq!(views.inline(template, ()), " staged ");
}

#[test]
fn authorization_directives_use_the_gate_check_hook() {
    let views = Views::new();
    views.factory.blade().function("gate_check", |args| {
        let ability = args[0].to_string();
        let owner = args.get(1).and_then(|post| post.get("owner")).map(|o| o.to_string());
        Ok(ViewValue::Bool(ability == "update" && owner.as_deref() == Some("taylor")))
    });
    let template = "@can('update', $post) edit @elsecan('view', $post) view @else none @endcan|@cannot('delete', $post) no delete @endcannot|@canany(['delete', 'update'], $post) any @endcanany";
    assert_eq!(views.inline(template, json!({"post": {"owner": "taylor"}})), " edit | no delete | any ");
    assert_eq!(views.inline(template, json!({"post": {"owner": "abigail"}})), " none | no delete |");
}

#[test]
fn has_section_and_section_missing() {
    let views = Views::new();
    views.add("layout", "@hasSection('navigation')\n<nav>@yield('navigation')</nav>\n@endif\n@sectionMissing('footer')\nno footer\n@endif\n");
    views.add("page", "@extends('layout')\n@section('navigation')\nLinks\n@endsection\n");
    assert_eq!(views.render("page", ()), "<nav>Links\n</nav>\nno footer\n");
}

#[test]
fn session_directive_binds_value() {
    let views = Views::new();
    views.factory.blade().function("session", |args| {
        Ok(if args[0].to_string() == "status" { "Profile updated!".into() } else { ViewValue::Null })
    });
    let template = "@session('status')\n<div>{{ $value }}</div>\n@endsession\n@session('missing') x @endsession";
    assert_eq!(views.inline(template, ()), "<div>Profile updated!</div>\n");
}

#[test]
fn error_directive_reads_the_error_bag() {
    let template = "<input class=\"@error('title') is-invalid @enderror\">\n@error('title')\n<div class=\"alert\">{{ $message }}</div>\n@enderror\n@error('email', 'login') {{ $message }} @else valid @enderror";
    let with_errors = json!({"errors": {
        "default": {"title": ["The title field is required."]},
        "login": {"email": ["Bad email."]},
    }});
    assert_eq!(
        blade(template, with_errors),
        "<input class=\" is-invalid \">\n<div class=\"alert\">The title field is required.</div>\n Bad email. "
    );
    assert_eq!(blade(template, ()), "<input class=\"\">\n valid ");
    // A flat bag works too.
    assert_eq!(blade("@error('name'){{ $message }}@enderror", json!({"errors": {"name": ["Too short."]}})), "Too short.");
}

#[test]
fn errors_variable_is_always_available() {
    assert_eq!(blade("{{ $errors->any() ? 'yes' : 'no' }} {{ $errors->count() }}", ()), "no 0");
    let data = json!({"errors": {"email": ["Invalid.", "Taken."]}});
    assert_eq!(
        blade("@if ($errors->any())<ul>@foreach ($errors->all() as $error)<li>{{ $error }}</li>@endforeach</ul>@endif {{ $errors->first('email') }} {{ $errors->has('email') ? 'y' : 'n' }}", data),
        "<ul><li>Invalid.</li><li>Taken.</li></ul> Invalid. y"
    );
    assert_eq!(
        blade("{{ $errors->login->first('password') }}|{{ $errors->getBag('login')->count() }}", json!({"errors": {"login": {"password": ["Wrong."]}}})),
        "Wrong.|1"
    );
}

#[test]
fn custom_if_statements() {
    let views = Views::new();
    let disk = Arc::new(std::sync::RwLock::new("local".to_string()));
    let current = disk.clone();
    views.factory.blade().if_("disk", move |args| args.first().is_some_and(|v| v.to_string() == *current.read().unwrap()));
    let template = "@disk('local')\nlocal\n@elsedisk('s3')\ns3\n@else\nother\n@enddisk\n@unlessdisk('local')\nnot local\n@enddisk";
    assert_eq!(views.inline(template, ()), "local\n");
    *disk.write().unwrap() = "s3".into();
    assert_eq!(views.inline(template, ()), "s3\nnot local\n");
    *disk.write().unwrap() = "ftp".into();
    assert_eq!(views.inline(template, ()), "other\nnot local\n");
    assert!(views.factory.blade().check("disk", &["ftp".into()]));
}

#[test]
fn nested_conditionals() {
    let template = "@if($a)\nA\n@if($b)\nB\n@else\n!B\n@endif\n@elseif($c)\nC\n@endif";
    assert_eq!(blade(template, json!({"a": true, "b": false, "c": true})), "A\n!B\n");
    assert_eq!(blade(template, json!({"a": false, "b": true, "c": true})), "C\n");
}
