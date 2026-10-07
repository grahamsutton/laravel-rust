//! Each provider's user is fetched and mapped onto a Socialite user.

use illuminate_http_client::Http;
use illuminate_support::json;
use laravel_socialite::Socialite;

use crate::support::*;

#[tokio::test]
async fn github_users() {
    let _app = app();
    Http::fake_urls([
        (
            "https://api.github.com/user",
            ok(json!({
                "login": "octocat",
                "id": 1,
                "node_id": "MDQ6VXNlcjE=",
                "avatar_url": "https://github.com/images/error/octocat_happy.gif",
                "name": "monalisa octocat",
                "email": "public@github.com",
                "company": "GitHub",
            })),
        ),
        (
            "https://api.github.com/user/emails",
            ok(json!([
                {"email": "secondary@github.com", "primary": false, "verified": true},
                {"email": "unverified@github.com", "primary": true, "verified": false},
                {"email": "octocat@github.com", "primary": true, "verified": true},
            ])),
        ),
    ]);

    let user = Socialite::driver("github")
        .user_from_token("token")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "1");
    assert_eq!(user.get_nickname(), Some("octocat"));
    assert_eq!(user.get_name(), Some("monalisa octocat"));
    assert_eq!(user.get_email(), Some("octocat@github.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://github.com/images/error/octocat_happy.gif")
    );
    assert_eq!(user.attribute("nodeId"), Some(&json!("MDQ6VXNlcjE=")));
    assert_eq!(user["company"], "GitHub");
    assert_eq!(user["email"], "octocat@github.com");

    let emails = sent_to("https://api.github.com/user/emails");
    assert!(emails.has_header_value("Authorization", "token token"));
}

#[tokio::test]
async fn github_only_fetches_emails_with_the_email_scope() {
    let _app = app();
    Http::fake_urls([(
        "https://api.github.com/user",
        ok(json!({"login": "octocat", "id": 1, "email": "public@github.com"})),
    )]);

    let user = Socialite::driver("github")
        .set_scopes(["read:user"])
        .user_from_token("token")
        .await
        .unwrap();

    assert_eq!(user.get_email(), Some("public@github.com"));
    Http::assert_sent_count(1);
}

#[tokio::test]
async fn github_emails_are_optional() {
    let _app = app();
    Http::fake_urls([
        (
            "https://api.github.com/user",
            ok(json!({"login": "octocat", "id": 1})),
        ),
        (
            "https://api.github.com/user/emails",
            Http::response(json!({"message": "Not Found"}), 404, &[]),
        ),
    ]);

    let user = Socialite::driver("github")
        .user_from_token("token")
        .await
        .unwrap();

    assert_eq!(user.get_email(), None);
    assert_eq!(user.get_name(), None);
}

#[tokio::test]
async fn google_users() {
    let _app = app();
    Http::fake_urls([(
        "https://www.googleapis.com/oauth2/v3/userinfo*",
        ok(json!({
            "sub": "110169484474386276334",
            "name": "Taylor Otwell",
            "given_name": "Taylor",
            "family_name": "Otwell",
            "picture": "https://lh3.googleusercontent.com/a/photo.jpg",
            "email": "taylor@laravel.com",
            "email_verified": true,
            "hd": "laravel.com",
        })),
    )]);

    let user = Socialite::driver("google")
        .user_from_token("ya29")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "110169484474386276334");
    assert_eq!(user.get_nickname(), None);
    assert_eq!(user.get_name(), Some("Taylor Otwell"));
    assert_eq!(user.get_email(), Some("taylor@laravel.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://lh3.googleusercontent.com/a/photo.jpg")
    );
    assert_eq!(
        user.attribute("avatar_original"),
        Some(&json!("https://lh3.googleusercontent.com/a/photo.jpg"))
    );
    assert_eq!(user["hd"], "laravel.com");
    // Kept for backwards compatibility...
    assert_eq!(user["id"], "110169484474386276334");
    assert_eq!(user["verified_email"], true);

    let request = sent_to("https://www.googleapis.com/oauth2/v3/userinfo");
    assert_eq!(
        request.url(),
        "https://www.googleapis.com/oauth2/v3/userinfo?prettyPrint=false"
    );
    assert!(request.has_header_value("Authorization", "Bearer ya29"));
    assert!(request.has_header_value("Accept", "application/json"));
}

#[tokio::test]
async fn facebook_users() {
    let _app = app();
    Http::fake_urls([(
        "https://graph.facebook.com/v3.3/me*",
        ok(json!({
            "name": "Taylor Otwell",
            "email": "taylor@laravel.com",
            "link": "https://www.facebook.com/app_scoped_user_id/10229/",
            "id": "10229",
        })),
    )]);

    let user = Socialite::driver("facebook")
        .user_from_token("fb-token")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "10229");
    assert_eq!(user.get_nickname(), None);
    assert_eq!(user.get_name(), Some("Taylor Otwell"));
    assert_eq!(user.get_email(), Some("taylor@laravel.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://graph.facebook.com/v3.3/10229/picture")
    );
    assert_eq!(
        user.attribute("avatar_original"),
        Some(&json!(
            "https://graph.facebook.com/v3.3/10229/picture?width=1920"
        ))
    );
    assert_eq!(
        user.attribute("profileUrl"),
        Some(&json!("https://www.facebook.com/app_scoped_user_id/10229/"))
    );

    let request = sent_to("https://graph.facebook.com/v3.3/me");
    assert_eq!(request["access_token"], "fb-token");
    assert_eq!(request["fields"], "name,email,gender,verified,link");
    assert_eq!(
        request["appsecret_proof"],
        "513054f9a4176ac5f14c36436c3ed72ee531a6bce1cb0e1d4e311e48f2fa3ff4"
    );
}

#[tokio::test]
async fn facebook_fields_may_be_customized() {
    let _app = app();
    Http::fake_urls([(
        "https://graph.facebook.com/v3.3/me*",
        ok(json!({"id": "1"})),
    )]);

    Socialite::driver("facebook")
        .fields(["name", "email", "first_name", "last_name"])
        .user_from_token("fb-token")
        .await
        .unwrap();

    let request = sent_to("https://graph.facebook.com/v3.3/me");
    assert_eq!(request["fields"], "name,email,first_name,last_name");
}

#[tokio::test]
async fn x_users() {
    let _app = app();
    Http::fake_urls([(
        "https://api.x.com/2/users/me*",
        ok(json!({"data": {
            "id": "2244994945",
            "name": "X Dev",
            "username": "XDevelopers",
            "profile_image_url": "https://pbs.twimg.com/profile_images/x_normal.jpg",
        }})),
    )]);

    let user = Socialite::driver("x")
        .user_from_token("x-token")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "2244994945");
    assert_eq!(user.get_nickname(), Some("XDevelopers"));
    assert_eq!(user.get_name(), Some("X Dev"));
    assert_eq!(user.get_email(), None);
    assert_eq!(
        user.get_avatar(),
        Some("https://pbs.twimg.com/profile_images/x_normal.jpg")
    );
    assert_eq!(user["username"], "XDevelopers");

    let request = sent_to("https://api.x.com/2/users/me");
    assert_eq!(request["user.fields"], "profile_image_url");
    assert!(request.has_header_value("Authorization", "Bearer x-token"));
}

#[tokio::test]
async fn linkedin_openid_users() {
    let _app = app();
    Http::fake_urls([(
        "https://api.linkedin.com/v2/userinfo",
        ok(json!({
            "sub": "782bbtaQ",
            "name": "John Doe",
            "given_name": "John",
            "family_name": "Doe",
            "picture": "https://media.licdn.com/dms/image/photo.jpg",
            "locale": "en-US",
            "email": "doe@email.com",
            "email_verified": true,
        })),
    )]);

    let user = Socialite::driver("linkedin-openid")
        .user_from_token("li-token")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "782bbtaQ");
    assert_eq!(user.get_nickname(), None);
    assert_eq!(user.get_name(), Some("John Doe"));
    assert_eq!(user.get_email(), Some("doe@email.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://media.licdn.com/dms/image/photo.jpg")
    );
    assert_eq!(user.attribute("first_name"), Some(&json!("John")));
    assert_eq!(user.attribute("last_name"), Some(&json!("Doe")));
    assert_eq!(user.attribute("email_verified"), Some(&json!(true)));

    let request = sent_to("https://api.linkedin.com/v2/userinfo");
    assert!(request.has_header_value("Authorization", "Bearer li-token"));
    assert!(request.has_header_value("X-RestLi-Protocol-Version", "2.0.0"));
}

#[tokio::test]
async fn gitlab_users() {
    let _app = app();
    Http::fake_urls([(
        "https://gitlab.com/api/v4/user*",
        ok(json!({
            "id": 1,
            "username": "john_smith",
            "name": "John Smith",
            "email": "john@example.com",
            "avatar_url": "https://gitlab.com/uploads/user/avatar/1/index.jpg",
        })),
    )]);

    let user = Socialite::driver("gitlab")
        .user_from_token("glpat")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "1");
    assert_eq!(user.get_nickname(), Some("john_smith"));
    assert_eq!(user.get_name(), Some("John Smith"));
    assert_eq!(user.get_email(), Some("john@example.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://gitlab.com/uploads/user/avatar/1/index.jpg")
    );

    let request = sent_to("https://gitlab.com/api/v4/user");
    assert_eq!(
        request.url(),
        "https://gitlab.com/api/v4/user?access_token=glpat"
    );
}

#[tokio::test]
async fn bitbucket_users() {
    let _app = app();
    Http::fake_urls([
        (
            "https://api.bitbucket.org/2.0/user?*",
            ok(json!({
                "uuid": "{8c6ab4a6-ad84-4f86-8f1d-8f6f2e1d2b3c}",
                "username": "evzijst",
                "display_name": "Erik van Zijst",
                "links": {"avatar": {"href": "https://bitbucket.org/account/evzijst/avatar/"}},
            })),
        ),
        (
            "https://api.bitbucket.org/2.0/user/emails*",
            ok(json!({"values": [
                {"type": "email", "email": "old@example.com", "is_primary": false, "is_confirmed": true},
                {"type": "email", "email": "erik@example.com", "is_primary": true, "is_confirmed": true},
            ]})),
        ),
    ]);

    let user = Socialite::driver("bitbucket")
        .user_from_token("bb")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "{8c6ab4a6-ad84-4f86-8f1d-8f6f2e1d2b3c}");
    assert_eq!(user.get_nickname(), Some("evzijst"));
    assert_eq!(user.get_name(), Some("Erik van Zijst"));
    assert_eq!(user.get_email(), Some("erik@example.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://bitbucket.org/account/evzijst/avatar/")
    );

    let request = sent_to("https://api.bitbucket.org/2.0/user/emails");
    assert_eq!(request["access_token"], "bb");
}

#[tokio::test]
async fn slack_users() {
    let _app = app();
    Http::fake_urls([
        (
            "https://slack.com/api/oauth.v2.access",
            ok(json!({
                "ok": true,
                "app_id": "A0KRD7HC3",
                "authed_user": {
                    "id": "U1234",
                    "scope": "identity.basic,identity.email",
                    "access_token": "xoxp-1234",
                    "token_type": "user",
                },
                "team": {"id": "T9TK3CUKW"},
            })),
        ),
        (
            "https://slack.com/api/users.identity",
            ok(json!({
                "ok": true,
                "user": {
                    "name": "Sonny Whether",
                    "id": "U0G9QF9C6",
                    "email": "bobby@slack-corp.com",
                    "image_512": "https://cdn.example.com/sonny_512.jpg",
                },
                "team": {"id": "T0G9PQBBK"},
            })),
        ),
    ]);
    let session = session();
    session.put("state", "the-state");

    let user = handling(
        get("/auth/callback?code=c&state=the-state", &session),
        async { Socialite::driver("slack").user().await },
    )
    .await
    .unwrap();

    assert_eq!(user.get_id(), "U0G9QF9C6");
    assert_eq!(user.get_name(), Some("Sonny Whether"));
    assert_eq!(user.get_email(), Some("bobby@slack-corp.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://cdn.example.com/sonny_512.jpg")
    );
    assert_eq!(user.attribute("organization_id"), Some(&json!("T0G9PQBBK")));
    // The user token lives under `authed_user`...
    assert_eq!(user.token, "xoxp-1234");
    assert_eq!(user.approved_scopes, ["identity.basic", "identity.email"]);
    assert_eq!(user.access_token_response_body["token_type"], "user");

    let identity = sent_to("https://slack.com/api/users.identity");
    assert!(identity.has_header_value("Authorization", "Bearer xoxp-1234"));
}

#[tokio::test]
async fn slack_bot_users_only_carry_the_token() {
    let _app = app();
    Http::fake_urls([(
        "https://slack.com/api/oauth.v2.access",
        ok(json!({
            "ok": true,
            "access_token": "xoxb-fake-bot-token",
            "token_type": "bot",
            "scope": "chat:write,chat:write.public",
            "bot_user_id": "U0KRQLJ9H",
            "app_id": "A0KRD7HC3",
            "team": {"name": "Slack Softball Team", "id": "T9TK3CUKW"},
        })),
    )]);
    let session = session();
    session.put("state", "the-state");

    let user = handling(
        get("/auth/callback?code=c&state=the-state", &session),
        async { Socialite::driver("slack").as_bot_user().user().await },
    )
    .await
    .unwrap();

    assert_eq!(
        user.token,
        "xoxb-fake-bot-token"
    );
    assert_eq!(user.get_id(), "");
    assert_eq!(user.get_name(), None);
    assert_eq!(user.approved_scopes, ["chat:write", "chat:write.public"]);
    Http::assert_sent_count(1);

    // The state is still checked.
    let error = handling(
        get("/auth/callback?code=c&state=the-state", &session),
        async { Socialite::driver("slack").as_bot_user().user().await },
    )
    .await
    .unwrap_err();
    assert!(error.is::<laravel_socialite::InvalidStateException>());
}

#[tokio::test]
async fn slack_openid_users() {
    let _app = app();
    Http::fake_urls([(
        "https://slack.com/api/openid.connect.userInfo",
        ok(json!({
            "ok": true,
            "sub": "U0R7JM",
            "https://slack.com/user_id": "U0R7JM",
            "https://slack.com/team_id": "T0R7GR",
            "email": "krane@slack-corp.com",
            "email_verified": true,
            "name": "krane",
            "picture": "https://secure.gravatar.com/avatar/krane.png",
            "given_name": "Bront",
            "family_name": "Labradoodle",
        })),
    )]);

    let user = Socialite::driver("slack-openid")
        .user_from_token("xoxp-oidc")
        .await
        .unwrap();

    assert_eq!(user.get_id(), "U0R7JM");
    assert_eq!(user.get_nickname(), None);
    assert_eq!(user.get_name(), Some("krane"));
    assert_eq!(user.get_email(), Some("krane@slack-corp.com"));
    assert_eq!(
        user.get_avatar(),
        Some("https://secure.gravatar.com/avatar/krane.png")
    );
    assert_eq!(user.attribute("organization_id"), Some(&json!("T0R7GR")));
}
