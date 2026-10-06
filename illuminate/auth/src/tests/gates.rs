//! Gates, policies, and authorization responses.

use std::sync::{Arc, Mutex};

use illuminate_support::{Value, json};

use super::{User, abigail, app, in_request, taylor};
use crate::access::{
    AccessGate, AuthResponse, Authorizable, AuthorizationException, Gate, GateArgument,
    GateArguments, Policy, authorize,
};
use crate::{Auth, AuthUser, GenericUser};

#[derive(Debug)]
struct Post {
    user_id: i64,
    published: bool,
}

struct Comment {
    post_owner: i64,
}

struct Category {
    locked: bool,
}

#[tokio::test]
async fn gates_are_defined_with_typed_closures() {
    let _app = app();
    let gate = AccessGate::new();
    gate.define("update-post", |user: &User, post: &Post| {
        user.id == post.user_id
    });
    gate.define("edit-settings", |user: &User| user.admin);
    gate.define("comment", |user: &User, post: &Post, comment: &Comment| {
        post.published && (user.admin || comment.post_owner == user.id)
    });

    let taylor = AuthUser::new(taylor());
    let abigail = AuthUser::new(abigail());
    let post = Post {
        user_id: 1,
        published: true,
    };

    assert!(gate.for_user(&taylor).allows("update-post", &post));
    assert!(gate.for_user(&abigail).denies("update-post", &post));
    assert!(gate.for_user(&taylor).allows("edit-settings", ()));
    assert!(gate.for_user(&abigail).denies("edit-settings", ()));

    let comment = Comment { post_owner: 2 };
    assert!(gate.for_user(&abigail).allows("comment", (&post, &comment)));
    assert!(gate.for_user(&taylor).allows("comment", (&post, &comment)));

    // Missing or mistyped arguments deny.
    assert!(gate.for_user(&taylor).denies("update-post", ()));
    assert!(gate.for_user(&taylor).denies("update-post", &comment));
    // Unknown abilities deny.
    assert!(gate.for_user(&taylor).denies("launch-rockets", ()));

    // Guests are denied unless the closure accepts them.
    assert!(gate.for_guest().denies("edit-settings", ()));
    gate.define("view-post", |user: Option<&User>, post: &Post| {
        post.published || user.is_some()
    });
    assert!(gate.for_guest().allows("view-post", &post));
    assert!(gate.for_guest().denies(
        "view-post",
        &Post {
            user_id: 1,
            published: false
        }
    ));
    assert!(gate.for_user(&abigail).allows(
        "view-post",
        &Post {
            user_id: 1,
            published: false
        }
    ));

    // Users of another type can't run closures typed for `User`.
    let generic = AuthUser::new(GenericUser::new(json!({"id": 1, "admin": true})));
    assert!(gate.for_user(&generic).denies("edit-settings", ()));

    // Closures typed for any user.
    gate.define("ping", |user: &AuthUser| user.id() == json!(1));
    assert!(gate.for_user(&generic).allows("ping", ()));
}

#[tokio::test]
async fn gates_may_return_detailed_responses() {
    let _app = app();
    let gate = AccessGate::new();
    gate.define("edit-settings", |user: &User| {
        if user.admin {
            AuthResponse::allow()
        } else {
            AuthResponse::deny("You must be an administrator.")
        }
    });
    gate.define("abstain", |_user: &User| None::<bool>);

    let abigail = gate.for_user(&abigail());
    let response = abigail.inspect("edit-settings", ());
    assert!(response.denied());
    assert_eq!(response.message(), Some("You must be an administrator."));
    assert!(!abigail.allows("edit-settings", ()));

    let error = abigail.authorize("edit-settings", ()).unwrap_err();
    assert_eq!(error.to_string(), "You must be an administrator.");
    assert_eq!(error.to_http_exception().status, 403);
    assert_eq!(
        error.response().unwrap().message(),
        Some("You must be an administrator.")
    );

    assert!(
        gate.for_user(&taylor())
            .authorize("edit-settings", ())
            .is_ok()
    );

    assert_eq!(abigail.raw("abstain", ()), None);
    assert!(abigail.inspect("abstain", ()).denied());
    assert_eq!(abigail.inspect("abstain", ()).message(), None);
}

#[tokio::test]
async fn denials_can_customize_the_status_code() {
    let _app = app();
    let gate = AccessGate::new();
    gate.define("hidden", |_: &User| AuthResponse::deny_as_not_found());
    gate.define("teapot", |_: &User| {
        AuthResponse::deny_with_status(418, "I'm a teapot.")
    });
    gate.define("nope", |_: &User| false);

    let user = gate.for_user(&taylor());
    let error = user.authorize("hidden", ()).unwrap_err();
    assert_eq!(error.status(), Some(404));
    assert_eq!(error.to_http_exception().status, 404);
    assert_eq!(error.to_http_exception().message(), "Not Found");

    let error = user.authorize("teapot", ()).unwrap_err();
    assert_eq!(error.to_http_exception().status, 418);
    assert_eq!(error.to_http_exception().message(), "I'm a teapot.");

    let error = user.authorize("nope", ()).unwrap_err();
    assert_eq!(error.message(), "This action is unauthorized.");
    assert!(!error.has_status());

    // A default denial response replaces plain denials.
    gate.default_denial_response(AuthResponse::deny_as_not_found());
    let error = gate.for_user(&taylor()).authorize("nope", ()).unwrap_err();
    assert_eq!(error.status(), Some(404));
    let error = gate
        .for_user(&taylor())
        .authorize("teapot", ())
        .unwrap_err();
    assert_eq!(error.status(), Some(418));
}

#[tokio::test]
async fn before_and_after_hooks_intercept_checks() {
    let _app = app();
    let gate = AccessGate::new();
    gate.define("delete-everything", |_: &User| false);
    gate.define("anything", |_: &User| None::<bool>);
    gate.before(|user: &User, _ability: &str| user.admin.then_some(true));

    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    gate.after(move |user: &User, ability: &str, result: Option<bool>| {
        seen.lock()
            .unwrap()
            .push(format!("{}:{ability}:{result:?}", user.name));
        None::<bool>
    });
    gate.after(
        |user: &User, ability: &str, _result: Option<bool>, _arguments: &[GateArgument]| {
            (ability == "anything" && user.id == 2).then_some(true)
        },
    );

    assert!(gate.for_user(&taylor()).allows("delete-everything", ()));
    assert!(gate.for_user(&abigail()).denies("delete-everything", ()));
    // After hooks only decide when nothing else did.
    assert!(gate.for_user(&abigail()).allows("anything", ()));

    assert_eq!(
        *log.lock().unwrap(),
        [
            "Taylor:delete-everything:Some(true)",
            "Abigail:delete-everything:Some(false)",
            "Abigail:anything:None",
        ]
    );

    // Guest-aware before hooks.
    gate.before(|user: Option<&User>, ability: &str| {
        (user.is_none() && ability == "anything").then_some(false)
    });
    assert!(gate.for_guest().denies("anything", ()));
}

struct PostPolicy;

impl Policy<Post> for PostPolicy {
    type User = User;

    fn before(&self, user: &User, ability: &str) -> Option<bool> {
        (user.admin && !ability.starts_with("force")).then_some(true)
    }

    fn view_any(&self, _user: &User) -> Option<AuthResponse> {
        Some(AuthResponse::allow())
    }

    fn view(&self, user: &User, post: &Post) -> Option<AuthResponse> {
        Some((post.published || post.user_id == user.id).into())
    }

    fn create(&self, user: &User) -> Option<AuthResponse> {
        Some(user.email_verified_at.is_some().into())
    }

    fn update(&self, user: &User, post: &Post) -> Option<AuthResponse> {
        Some(if user.id == post.user_id {
            AuthResponse::allow()
        } else {
            AuthResponse::deny("You do not own this post.")
        })
    }

    fn force_delete(&self, _user: &User, _post: &Post) -> Option<AuthResponse> {
        Some(AuthResponse::deny("Posts are forever."))
    }

    fn ability(&self, ability: &str, user: &User, post: Option<&Post>) -> Option<AuthResponse> {
        match ability {
            "publish" => Some(post.is_some_and(|post| post.user_id == user.id).into()),
            _ => None,
        }
    }

    fn guest(&self, ability: &str, post: Option<&Post>) -> Option<AuthResponse> {
        (ability == "view").then(|| post.is_some_and(|post| post.published).into())
    }
}

#[tokio::test]
async fn policies_authorize_actions_on_models() {
    let _app = app();
    let gate = AccessGate::new();
    gate.policy::<Post, _>(PostPolicy);

    let abigail = gate.for_user(&abigail());
    let taylor = gate.for_user(&taylor());
    let own = Post {
        user_id: 2,
        published: false,
    };
    let theirs = Post {
        user_id: 1,
        published: false,
    };

    assert!(abigail.allows("update", &own));
    assert_eq!(
        abigail.inspect("update", &theirs).message(),
        Some("You do not own this post.")
    );
    assert!(abigail.allows("view", &own));
    assert!(abigail.denies("view", &theirs));
    assert!(abigail.allows(
        "view",
        &Post {
            user_id: 1,
            published: true
        }
    ));

    // Resource abilities in any spelling.
    for ability in ["viewAny", "view_any", "view-any"] {
        assert!(
            abigail.allows(ability, GateArgument::class::<Post>()),
            "{ability}"
        );
    }

    // Abilities without a model use the class.
    assert!(abigail.denies("create", GateArgument::class::<Post>()));
    assert!(taylor.allows("create", GateArgument::class::<Post>()));
    assert!(taylor.allows("create", json!("App\\Models\\Post")));

    // The policy's `before` lets administrators do (almost) anything.
    assert!(taylor.allows("update", &own));
    assert!(taylor.allows("delete", &own));
    assert_eq!(
        taylor.inspect("forceDelete", &own).message(),
        Some("Posts are forever.")
    );

    // Custom abilities.
    assert!(abigail.allows("publish", &own));
    assert!(abigail.denies("publish", &theirs));
    // Methods the policy doesn't implement deny.
    assert!(abigail.denies("restore", &own));

    assert!(gate.has_policy_for::<Post>());
    assert!(
        gate.get_policy_for::<Post>()
            .unwrap()
            .downcast::<PostPolicy>()
            .is_ok()
    );
    assert!(gate.get_policy_for::<Comment>().is_none());
    assert_eq!(gate.policies().len(), 1);
}

#[tokio::test]
async fn unhandled_policy_abilities_fall_back_to_gates() {
    let _app = app();
    let gate = AccessGate::new();
    gate.policy::<Post, _>(PostPolicy);
    gate.define("archive", |user: &User, post: &Post| {
        user.id == post.user_id
    });
    gate.define("restore", |_: &User, _: &Post| true);

    let abigail = gate.for_user(&abigail());
    assert!(abigail.allows(
        "archive",
        &Post {
            user_id: 2,
            published: false
        }
    ));
    assert!(abigail.denies(
        "archive",
        &Post {
            user_id: 1,
            published: false
        }
    ));
    assert!(abigail.allows(
        "restore",
        &Post {
            user_id: 1,
            published: false
        }
    ));

    // Guests only reach the policy's guest hook.
    let published = Post {
        user_id: 1,
        published: true,
    };
    let draft = Post {
        user_id: 1,
        published: false,
    };
    assert!(gate.for_guest().allows("view", &published));
    assert!(gate.for_guest().denies("view", &draft));
    assert!(gate.for_guest().denies("update", &published));
}

#[tokio::test]
async fn gates_check_many_abilities_at_once() {
    let _app = app();
    let gate = AccessGate::new();
    gate.define("a", |_: &User| true);
    gate.define("b", |_: &User| true);
    gate.define("c", |_: &User| false);

    let user = gate.for_user(&taylor());
    assert!(user.check(["a", "b"], ()));
    assert!(!user.check(["a", "c"], ()));
    assert!(user.any(["c", "a"], ()));
    assert!(!user.any(["c"], ()));
    assert!(user.none(["c"], ()));
    assert!(!user.none(vec!["c", "b"], ()));

    assert!(gate.has("a"));
    assert!(!gate.has("z"));
    assert!(gate.has_all(["a", "b", "c"]));
    assert!(!gate.has_all(["a", "z"]));
    assert_eq!(gate.abilities(), ["a", "b", "c"]);
}

#[tokio::test]
async fn gates_check_the_current_user() {
    let app = app();
    Gate::define("update-post", |user: &User, post: &Post| {
        user.id == post.user_id
    });
    let post = Post {
        user_id: 2,
        published: true,
    };

    in_request(|_| async {
        assert!(
            Gate::denies("update-post", &post).await,
            "guests are denied"
        );

        Auth::login(&app.user(2), false).await.unwrap();
        assert!(Gate::allows("update-post", &post).await);
        assert!(Gate::check(["update-post"], &post).await);
        assert!(Gate::any(["nope", "update-post"], &post).await);
        assert!(Gate::none(["nope"], &post).await);
        assert!(Gate::inspect("update-post", &post).await.allowed());
        assert!(Gate::raw("update-post", &post).await.is_some());
        assert!(authorize("update-post", &post).await.is_ok());

        Auth::login(&app.user(1), false).await.unwrap();
        let error: AuthorizationException =
            Gate::authorize("update-post", &post).await.unwrap_err();
        assert_eq!(error.to_http_exception().status, 403);
        assert!(Gate::for_user(&app.user(2)).allows("update-post", &post));
    })
    .await;

    assert!(Gate::has("update-post"));
    assert_eq!(Gate::abilities(), ["update-post"]);
}

#[tokio::test]
async fn the_user_resolver_can_be_replaced() {
    let app = app();
    let abigail = AuthUser::from(&app.user(2));
    Auth::resolve_users_using(move |_guard| {
        let abigail = abigail.clone();
        async move { Some(abigail) }
    });
    Gate::define("is-abigail", |user: &User| user.name == "Abigail");
    assert!(Gate::allows("is-abigail", ()).await);
}

#[tokio::test]
async fn users_can_check_their_own_abilities() {
    let _app = app();
    Gate::policy::<Post, _>(PostPolicy);
    Gate::define("view-dashboard", |user: &User| user.admin);

    let abigail = abigail();
    let post = Post {
        user_id: 2,
        published: false,
    };
    assert!(abigail.can("update", &post));
    assert!(abigail.cannot(
        "update",
        &Post {
            user_id: 1,
            published: false
        }
    ));
    assert!(abigail.cant("view-dashboard", ()));
    assert!(abigail.can_any(["view-dashboard", "update"], &post));
    assert!(!abigail.can(["view-dashboard", "update"], &post));
    assert!(AuthUser::new(taylor()).can("view-dashboard", ()));
}

#[tokio::test]
async fn inline_authorization() {
    let _app = app();
    assert!(Gate::allow_if(true, None).is_ok());
    let error = Gate::allow_if(false, "Nope.").unwrap_err();
    assert_eq!(error.message(), "Nope.");
    assert!(Gate::deny_if(false, None).is_ok());
    assert!(Gate::deny_if(true, None).is_err());
}

#[test]
fn arguments_convert_from_many_shapes() {
    let post = Post {
        user_id: 1,
        published: true,
    };
    let category = Category { locked: false };

    let arguments: GateArguments =
        crate::IntoGateArguments::into_gate_arguments((&post, &category));
    assert_eq!(arguments.len(), 2);
    assert_eq!(arguments[0].downcast_ref::<Post>().unwrap().user_id, 1);
    assert!(!arguments[1].downcast_ref::<Category>().unwrap().locked);
    assert!(arguments[0].type_name().unwrap().ends_with("Post"));

    let arguments = crate::IntoGateArguments::into_gate_arguments(vec![json!(1), json!("two")]);
    assert_eq!(arguments[1].downcast_ref::<Value>(), Some(&json!("two")));

    let shared = GateArgument::shared(Arc::new(Post {
        user_id: 3,
        published: false,
    }));
    assert_eq!(shared.downcast_ref::<Post>().unwrap().user_id, 3);
    assert_eq!(shared.model_type_id(), Some(std::any::TypeId::of::<Post>()));
    assert_eq!(
        GateArgument::class::<Post>().class_basename().as_deref(),
        Some("Post")
    );
    assert_eq!(
        GateArgument::value("App\\Models\\Post")
            .class_basename()
            .as_deref(),
        Some("Post")
    );
    assert!(format!("{:?}", GateArgument::of(&post)).contains("Post"));
}

#[test]
fn responses_and_exceptions_mirror_laravel() {
    let response = AuthResponse::deny("Nope.").with_code("E42");
    assert_eq!(
        response.to_array(),
        json!({"allowed": false, "message": "Nope.", "code": "E42"})
    );
    assert_eq!(response.to_string(), "Nope.");
    assert_eq!(
        serde_json::to_value(&response).unwrap()["code"],
        json!("E42")
    );

    let error = response.clone().authorize().unwrap_err();
    assert_eq!(error.code(), Some("E42"));
    assert_eq!(error.to_response().message(), Some("Nope."));
    assert_eq!(error.clone().as_not_found().status(), Some(404));

    assert!(AuthResponse::from(true).allowed());
    assert!(AuthResponse::allow().with_message("Welcome!").allowed());
    assert_eq!(AuthResponse::deny(None).message(), None);
    assert_eq!(AuthResponse::deny(None).as_not_found().status(), Some(404));

    let error = AuthorizationException::with_message("Custom.").with_status(409);
    assert_eq!(error.to_http_exception().status, 409);
    assert_eq!(error.to_http_exception().message(), "Conflict");
}
