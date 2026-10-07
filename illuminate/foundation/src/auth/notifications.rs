//! The notifications sent by the authentication system: password reset
//! links and email verification links (Laravel's `ResetPassword` and
//! `VerifyEmail`).

use std::sync::{Arc, RwLock};

use illuminate_auth::{FromAuthUser, PasswordStatus};
use illuminate_notifications::{MailMessage, Notifiable, Notification};
use illuminate_support::{Carbon, Result, Value, ValueExt, json};
use illuminate_translation::{__, __with};
use serde::Serialize;

type UrlCallback = Arc<dyn Fn(&dyn Notifiable, &str) -> String + Send + Sync>;
type MailCallback = Arc<dyn Fn(&dyn Notifiable, &str) -> MailMessage + Send + Sync>;

static RESET_URL: RwLock<Option<UrlCallback>> = RwLock::new(None);
static RESET_MAIL: RwLock<Option<MailCallback>> = RwLock::new(None);
static VERIFY_URL: RwLock<Option<UrlCallback>> = RwLock::new(None);
static VERIFY_MAIL: RwLock<Option<MailCallback>> = RwLock::new(None);

fn attribute(notifiable: &dyn Notifiable, key: &str) -> String {
    notifiable
        .notifiable_attributes()
        .get(key)
        .map(ValueExt::to_string_lossy)
        .unwrap_or_default()
}

/// The password reset link: "You are receiving this email because we
/// received a password reset request for your account."
#[derive(Clone, Debug, Serialize)]
pub struct ResetPassword {
    /// The password reset token.
    pub token: String,
}

impl ResetPassword {
    /// Create the notification for the given token.
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into() }
    }

    /// Customize the reset URL (an SPA's reset page, say).
    pub fn create_url_using(callback: impl Fn(&dyn Notifiable, &str) -> String + Send + Sync + 'static) {
        *RESET_URL.write().unwrap() = Some(Arc::new(callback));
    }

    /// Customize the whole mail message; the callback receives the token.
    pub fn to_mail_using(callback: impl Fn(&dyn Notifiable, &str) -> MailMessage + Send + Sync + 'static) {
        *RESET_MAIL.write().unwrap() = Some(Arc::new(callback));
    }

    /// The reset URL: the `password.reset` route with the token and email.
    pub fn reset_url(&self, notifiable: &dyn Notifiable) -> String {
        if let Some(callback) = RESET_URL.read().unwrap().clone() {
            return callback(notifiable, &self.token);
        }
        illuminate_routing::route(
            "password.reset",
            json!({"token": self.token, "email": attribute(notifiable, "email")}),
        )
        .unwrap_or_default()
    }
}

impl Notification for ResetPassword {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    fn to_mail(&self, notifiable: &dyn Notifiable) -> Option<MailMessage> {
        if let Some(callback) = RESET_MAIL.read().unwrap().clone() {
            return Some(callback(notifiable, &self.token));
        }
        let broker = illuminate_config::config_or("auth.defaults.passwords", "users").to_string_lossy();
        let expire = illuminate_config::config_or(&format!("auth.passwords.{broker}.expire"), 60);
        Some(
            MailMessage::new()
                .subject(__("Reset Password Notification"))
                .line(__("You are receiving this email because we received a password reset request for your account."))
                .action(__("Reset Password"), self.reset_url(notifiable))
                .line(__with("This password reset link will expire in :count minutes.", json!({"count": expire})))
                .line(__("If you did not request a password reset, no further action is required.")),
        )
    }
}

/// The email verification link: "Please click the button below to verify
/// your email address."
#[derive(Clone, Debug, Default, Serialize)]
pub struct VerifyEmail;

impl VerifyEmail {
    /// Customize the verification URL.
    pub fn create_url_using(callback: impl Fn(&dyn Notifiable, &str) -> String + Send + Sync + 'static) {
        *VERIFY_URL.write().unwrap() = Some(Arc::new(callback));
    }

    /// Customize the whole mail message; the callback receives the URL.
    pub fn to_mail_using(callback: impl Fn(&dyn Notifiable, &str) -> MailMessage + Send + Sync + 'static) {
        *VERIFY_MAIL.write().unwrap() = Some(Arc::new(callback));
    }

    /// The verification URL: a temporary signed `verification.verify` route
    /// carrying the user's key and a hash of their email.
    pub fn verification_url(&self, notifiable: &dyn Notifiable) -> String {
        let hash = {
            use sha1::{Digest, Sha1};
            hex::encode(Sha1::digest(attribute(notifiable, "email").as_bytes()))
        };
        if let Some(callback) = VERIFY_URL.read().unwrap().clone() {
            return callback(notifiable, &hash);
        }
        let minutes = illuminate_config::config_or("auth.verification.expire", 60).to_i64_lossy().unwrap_or(60);
        illuminate_routing::URL::temporary_signed_route(
            "verification.verify",
            Carbon::now().add_minutes(minutes),
            json!({"id": notifiable.notifiable_key(), "hash": hash}),
        )
        .unwrap_or_default()
    }
}

impl Notification for VerifyEmail {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    fn to_mail(&self, notifiable: &dyn Notifiable) -> Option<MailMessage> {
        let url = self.verification_url(notifiable);
        if let Some(callback) = VERIFY_MAIL.read().unwrap().clone() {
            return Some(callback(notifiable, &url));
        }
        Some(
            MailMessage::new()
                .subject(__("Verify Email Address"))
                .line(__("Please click the button below to verify your email address."))
                .action(__("Verify Email Address"), url)
                .line(__("If you did not create an account, no further action is required.")),
        )
    }
}

/// Send a password reset link to the user matching the credentials —
/// Laravel's `Password::sendResetLink($request->only('email'))`, which
/// emails the [`ResetPassword`] notification.
///
/// ```ignore
/// let status = send_password_reset_link::<User>(&request.only(&["email"])).await?;
///
/// if status == PasswordStatus::ResetLinkSent { ... }
/// ```
pub async fn send_password_reset_link<U>(credentials: &Value) -> Result<PasswordStatus>
where
    U: FromAuthUser + Notifiable + Send,
{
    illuminate_auth::Password::send_reset_link(credentials, |user: U, token: String| async move {
        user.notify_now(ResetPassword::new(token)).await
    })
    .await
}

/// Send the user an email verification link (Laravel's
/// `$user->sendEmailVerificationNotification()`).
pub async fn send_email_verification_notification<U: Notifiable>(user: &U) -> Result<()> {
    user.notify(VerifyEmail).await
}
