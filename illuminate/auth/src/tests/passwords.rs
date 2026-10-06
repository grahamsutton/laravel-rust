//! Password resets.

use std::sync::{Arc, Mutex};

use illuminate_support::json;

use super::{User, app};
use crate::passwords::{
    ArrayTokenRepository, Password, PasswordBroker, PasswordStatus, TokenRepository,
};
use crate::{AuthUser, UserProvider};

#[tokio::test]
async fn reset_links_are_sent_to_existing_users() {
    let _app = app();
    let sent = Arc::new(Mutex::new(None));

    let mailbox = sent.clone();
    let status = Password::send_reset_link(
        &json!({"email": "taylor@laravel.com"}),
        |user: User, token: String| async move {
            *mailbox.lock().unwrap() = Some((user.email, token));
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(status, Password::RESET_LINK_SENT);
    assert_eq!(status, "passwords.sent");

    let (email, token) = sent.lock().unwrap().clone().unwrap();
    assert_eq!(email, "taylor@laravel.com");
    assert_eq!(token.len(), 64);

    let user = Password::get_user(&json!({"email": "taylor@laravel.com", "token": "ignored"}))
        .await
        .unwrap()
        .unwrap();
    assert!(Password::token_exists(&user, &token).await.unwrap());

    // Asking again too soon is throttled.
    let status = Password::send_reset_link(
        &json!({"email": "taylor@laravel.com"}),
        |_: AuthUser, _| async { Ok(()) },
    )
    .await
    .unwrap();
    assert_eq!(status, PasswordStatus::ResetThrottled);

    let status = Password::send_reset_link(
        &json!({"email": "nobody@laravel.com"}),
        |_: AuthUser, _| async { Ok(()) },
    )
    .await
    .unwrap();
    assert_eq!(status, PasswordStatus::InvalidUser);
    assert_eq!(status.to_string(), "passwords.user");
}

#[tokio::test]
async fn passwords_are_reset_with_a_valid_token() {
    let app = app();
    let user = AuthUser::from(&app.user(2));
    let token = Password::create_token(&user).await.unwrap();

    let users = app.users.clone();
    let reset = |token: &str| json!({"email": "abigail@laravel.com", "password": "new-password", "password_confirmation": "new-password", "token": token});

    let status = Password::reset(&reset("wrong-token"), |_: User, _| async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(status, Password::INVALID_TOKEN);

    let status = Password::reset(
        &reset(&token),
        move |user: User, password: String| async move {
            let hashed = illuminate_hashing::Hash::make(&password)?;
            let updated = AuthUser::from(&user).with_auth_password(&hashed);
            users
                .rehash_password_if_required(&updated, &json!({"password": password}), true)
                .await?;
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(status, Password::PASSWORD_RESET);
    assert!(illuminate_hashing::Hash::check(
        "new-password",
        &app.user(2).password
    ));

    // Tokens are single-use.
    let status = Password::reset(&reset(&token), |_: User, _| async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(status, PasswordStatus::InvalidToken);

    let status = Password::reset(
        &json!({"email": "nobody@laravel.com", "token": token}),
        |_: User, _| async { Ok(()) },
    )
    .await
    .unwrap();
    assert_eq!(status, PasswordStatus::InvalidUser);
}

#[tokio::test]
async fn reset_tokens_expire() {
    let app = app();
    let tokens = Arc::new(ArrayTokenRepository::new(b"key".to_vec(), 3600, 0));
    let broker = PasswordBroker::new(tokens.clone(), app.users.clone());
    let user = AuthUser::from(&app.user(1));

    let token = broker.create_token(&user).await.unwrap();
    assert!(broker.token_exists(&user, &token).await.unwrap());
    assert!(
        !tokens.recently_created_token(&user).await.unwrap(),
        "throttling is disabled"
    );

    tokens.travel("taylor@laravel.com", 3601);
    assert!(!broker.token_exists(&user, &token).await.unwrap());

    tokens.delete_expired().await.unwrap();
    assert_eq!(tokens.count(), 0);

    let token = broker.create_token(&user).await.unwrap();
    broker.delete_token(&user).await.unwrap();
    assert!(!broker.token_exists(&user, &token).await.unwrap());
    assert!(
        !broker
            .get_repository()
            .recently_created_token(&user)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn brokers_are_configured_by_name() {
    let _app = app();
    let manager = Password::manager();
    assert_eq!(manager.get_default_driver(), "users");
    assert!(Arc::ptr_eq(
        &Password::broker(None).unwrap(),
        &Password::broker(Some("users")).unwrap()
    ));

    let error = Password::broker(Some("missing")).err().unwrap();
    assert_eq!(
        error.to_string(),
        "Password resetter [missing] is not defined."
    );

    // The database repository is registered by the framework.
    let error = Password::broker(Some("database")).err().unwrap();
    assert_eq!(
        error.to_string(),
        "Password reset token repository [database] is not defined."
    );

    Password::token_repository("database", |_app, config, key| {
        assert_eq!(config["provider"], json!("users"));
        assert_eq!(key.len(), 32, "the base64 application key is decoded");
        Arc::new(ArrayTokenRepository::new(key.to_vec(), 3600, 60))
    });
    assert!(Password::broker(Some("database")).is_ok());
}

#[tokio::test]
async fn callbacks_may_ask_for_the_wrong_user_type() {
    let _app = app();
    let error = Password::send_reset_link(
        &json!({"email": "taylor@laravel.com"}),
        |_: crate::GenericUser, _| async { Ok(()) },
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not a [illuminate_auth::user::GenericUser]")
    );
}

#[test]
fn statuses_have_translation_keys() {
    assert_eq!(PasswordStatus::PasswordReset.as_str(), "passwords.reset");
    assert_eq!(
        PasswordStatus::ResetThrottled.as_str(),
        "passwords.throttled"
    );
    assert!(PasswordStatus::ResetLinkSent.is_successful());
    assert!(!PasswordStatus::InvalidToken.is_successful());
    assert_eq!(
        illuminate_support::Value::from(PasswordStatus::InvalidUser),
        json!("passwords.user")
    );
}
