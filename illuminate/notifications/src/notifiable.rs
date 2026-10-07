//! Notifiables: the things notifications are sent to.

use std::any::Any;
use std::future::Future;

use illuminate_mail::{Address, IntoAddresses};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value, ValueExt, json, to_value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::manager::ChannelManager;
use crate::notification::Notification;

/// What every notifiable gets for free from `#[derive(Serialize)]`: its
/// attributes (the default `mail` route reads `email`) and downcasting.
pub trait NotifiableData: Send + Sync + 'static {
    /// The notifiable's serialized attributes.
    fn notifiable_attributes(&self) -> Value;

    /// The notifiable as [`Any`], for downcasting.
    fn as_any(&self) -> &dyn Any;
}

impl<T: Serialize + Send + Sync + 'static> NotifiableData for T {
    fn notifiable_attributes(&self) -> Value {
        to_value(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Something that receives notifications — usually your `User` model
/// (Laravel's `Notifiable` trait).
///
/// Implementing it takes two methods: the key and type that identify the
/// notifiable (what the `database` channel stores as `notifiable_id` and
/// `notifiable_type`). Mail notifications go to the `email` attribute
/// unless you route them elsewhere:
///
/// ```
/// use illuminate_notifications::Notifiable;
/// use illuminate_support::{Value, json};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct User {
///     id: u64,
///     name: String,
///     email: String,
/// }
///
/// impl Notifiable for User {
///     fn notifiable_key(&self) -> Value {
///         json!(self.id)
///     }
///
///     fn notifiable_type(&self) -> String {
///         "App\\Models\\User".into()
///     }
/// }
///
/// let user = User { id: 1, name: "Taylor".into(), email: "taylor@example.com".into() };
/// assert_eq!(user.route_mail_notification(), Some(json!("taylor@example.com")));
/// ```
///
/// Then send notifications with `user.notify(InvoicePaid { .. }).await?`.
pub trait Notifiable: NotifiableData {
    /// The notifiable's key (the `notifiable_id` of database notifications).
    fn notifiable_key(&self) -> Value;

    /// The notifiable's type (the `notifiable_type` of database notifications).
    fn notifiable_type(&self) -> String;

    /// Get the notification routing information for the given channel.
    ///
    /// Defaults: `mail` routes to the `email` attribute, `database` to the
    /// notifiable itself (its type and key), and `slack` to
    /// [`route_notification_for_slack`](Notifiable::route_notification_for_slack).
    /// Override this to customize routing (Laravel's
    /// `routeNotificationForMail`, ...).
    fn route_notification_for(
        &self,
        channel: &str,
        notification: &dyn Notification,
    ) -> Option<Value> {
        match channel {
            "mail" => self.route_mail_notification(),
            "database" => Some(json!({
                "notifiable_type": self.notifiable_type(),
                "notifiable_id": self.notifiable_key(),
            })),
            "slack" => self.route_notification_for_slack(notification),
            _ => None,
        }
    }

    /// The default mail route: the `email` attribute.
    fn route_mail_notification(&self) -> Option<Value> {
        self.notifiable_attributes()
            .get("email")
            .filter(|email| !email.is_blank())
            .cloned()
    }

    /// Route notifications for the Slack channel (Laravel's
    /// `routeNotificationForSlack`): a channel name, a
    /// [`SlackRoute`](crate::slack::SlackRoute) for an external workspace,
    /// or an incoming webhook URL. `None` defers to the message's channel
    /// and your `services.slack.notifications.channel` configuration.
    ///
    /// ```
    /// use illuminate_notifications::{Notifiable, Notification};
    /// use illuminate_support::{Value, json};
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: u64,
    /// }
    ///
    /// impl Notifiable for User {
    ///     fn notifiable_key(&self) -> Value {
    ///         json!(self.id)
    ///     }
    ///
    ///     fn notifiable_type(&self) -> String {
    ///         "App\\Models\\User".into()
    ///     }
    ///
    ///     fn route_notification_for_slack(&self, _notification: &dyn Notification) -> Option<Value> {
    ///         Some("#support-channel".into())
    ///     }
    /// }
    /// ```
    fn route_notification_for_slack(&self, _notification: &dyn Notification) -> Option<Value> {
        None
    }

    /// The notifiable's preferred locale (Laravel's `HasLocalePreference`).
    fn preferred_locale(&self) -> Option<String> {
        None
    }

    /// Send the given notification (queueing it when it should be queued).
    fn notify<N: Notification>(&self, notification: N) -> impl Future<Output = Result<()>> + Send
    where
        Self: Sized,
    {
        async move {
            ChannelManager::resolve()
                .send(vec![self as &dyn Notifiable], notification)
                .await
        }
    }

    /// Send the given notification immediately.
    fn notify_now<N: Notification>(
        &self,
        notification: N,
    ) -> impl Future<Output = Result<()>> + Send
    where
        Self: Sized,
    {
        async move {
            ChannelManager::resolve()
                .send_now(vec![self as &dyn Notifiable], notification)
                .await
        }
    }

    /// Send the given notification immediately, over the given channels only.
    fn notify_now_via<N: Notification>(
        &self,
        notification: N,
        channels: &[&str],
    ) -> impl Future<Output = Result<()>> + Send
    where
        Self: Sized,
    {
        let channels: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
        async move {
            ChannelManager::resolve()
                .send_now_via(vec![self as &dyn Notifiable], notification, channels)
                .await
        }
    }
}

/// The recipients of a notification: a notifiable, a slice or `Vec` of
/// them, or a list of `&dyn Notifiable`.
pub trait IntoNotifiables<'a> {
    /// Collect the notifiables.
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable>;
}

impl<'a, T: Notifiable> IntoNotifiables<'a> for &'a T {
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable> {
        vec![self]
    }
}

impl<'a> IntoNotifiables<'a> for &'a dyn Notifiable {
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable> {
        vec![self]
    }
}

impl<'a, T: Notifiable> IntoNotifiables<'a> for &'a [T] {
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable> {
        self.iter().map(|n| n as &dyn Notifiable).collect()
    }
}

impl<'a, T: Notifiable> IntoNotifiables<'a> for &'a Vec<T> {
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable> {
        self.iter().map(|n| n as &dyn Notifiable).collect()
    }
}

impl<'a, T: Notifiable> IntoNotifiables<'a> for Vec<&'a T> {
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable> {
        self.into_iter().map(|n| n as &dyn Notifiable).collect()
    }
}

impl<'a> IntoNotifiables<'a> for Vec<&'a dyn Notifiable> {
    fn into_notifiables(self) -> Vec<&'a dyn Notifiable> {
        self
    }
}

/// A notifiable for on-demand notifications, sent to someone who isn't a
/// user of your application.
///
/// ```
/// use illuminate_notifications::{AnonymousNotifiable, Notifiable};
/// use illuminate_support::json;
///
/// let notifiable = AnonymousNotifiable::new()
///     .route("mail", "taylor@example.com")
///     .route("slack", "#general");
///
/// assert_eq!(notifiable.routes["mail"], json!("taylor@example.com"));
/// assert!(notifiable.notifiable_key().is_null());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AnonymousNotifiable {
    /// All of the notification routing information.
    pub routes: IndexMap<String, Value>,
}

impl AnonymousNotifiable {
    /// The type recorded for on-demand notifiables.
    pub const TYPE: &'static str = "Illuminate\\Notifications\\AnonymousNotifiable";

    /// Create an anonymous notifiable without routes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add routing information to the target.
    ///
    /// # Panics
    ///
    /// The `database` channel does not support on-demand notifications.
    pub fn route(self, channel: &str, route: impl Serialize) -> Self {
        self.try_route(channel, route)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Add routing information to the target, failing for the `database` channel.
    pub fn try_route(mut self, channel: &str, route: impl Serialize) -> Result<Self> {
        if channel == "database" {
            return Err(InvalidArgumentException::new(
                "The database channel does not support on-demand notifications.",
            )
            .into());
        }
        self.routes.insert(channel.to_string(), to_value(&route));
        Ok(self)
    }

    /// Send the given notification (queueing it when it should be queued).
    pub async fn notify<N: Notification>(&self, notification: N) -> Result<()> {
        ChannelManager::resolve()
            .send(vec![self as &dyn Notifiable], notification)
            .await
    }

    /// Send the given notification immediately.
    pub async fn notify_now<N: Notification>(&self, notification: N) -> Result<()> {
        ChannelManager::resolve()
            .send_now(vec![self as &dyn Notifiable], notification)
            .await
    }
}

impl Notifiable for AnonymousNotifiable {
    fn notifiable_key(&self) -> Value {
        Value::Null
    }

    fn notifiable_type(&self) -> String {
        Self::TYPE.to_string()
    }

    fn route_notification_for(
        &self,
        channel: &str,
        _notification: &dyn Notification,
    ) -> Option<Value> {
        self.routes.get(channel).cloned()
    }
}

/// A notifiable, as recorded by events, the fake and the queue: its type,
/// key and attributes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotifiableSnapshot {
    /// The notifiable's type.
    pub notifiable_type: String,
    /// The notifiable's key.
    pub key: Value,
    /// The notifiable's attributes (an anonymous notifiable's routes).
    pub attributes: Value,
}

impl NotifiableSnapshot {
    /// Take a snapshot of a notifiable.
    pub fn of(notifiable: &dyn Notifiable) -> Self {
        Self {
            notifiable_type: notifiable.notifiable_type(),
            key: notifiable.notifiable_key(),
            attributes: notifiable.notifiable_attributes(),
        }
    }

    /// Determine if this is the given notifiable (same type and key).
    pub fn is(&self, notifiable: &dyn Notifiable) -> bool {
        self.notifiable_type == notifiable.notifiable_type()
            && same_key(&self.key, &notifiable.notifiable_key())
    }

    /// Determine if this is an on-demand (anonymous) notifiable.
    pub fn is_anonymous(&self) -> bool {
        self.notifiable_type == AnonymousNotifiable::TYPE
    }

    /// The anonymous notifiable, when this is one.
    pub fn anonymous(&self) -> Option<AnonymousNotifiable> {
        self.is_anonymous()
            .then(|| serde_json::from_value(self.attributes.clone()).ok())
            .flatten()
    }
}

/// Compare keys loosely, like PHP (`1 == "1"`).
fn same_key(a: &Value, b: &Value) -> bool {
    a == b || (!a.is_null() && !b.is_null() && a.to_string_lossy() == b.to_string_lossy())
}

/// Turn a `mail` route into addresses: a string, a list of strings (or
/// objects with an `email`), or an `{"email": "name"}` map.
pub(crate) fn mail_route_addresses(route: &Value) -> Vec<Address> {
    match route {
        Value::String(email) => email.as_str().into_addresses(),
        Value::Array(items) => items.iter().flat_map(mail_route_addresses).collect(),
        Value::Object(map) if map.contains_key("email") => vec![Address {
            address: map["email"].to_string_lossy(),
            name: map
                .get("name")
                .filter(|name| !name.is_blank())
                .map(ValueExt::to_string_lossy),
        }],
        Value::Object(map) => map
            .iter()
            .map(|(email, name)| Address {
                address: email.clone(),
                name: (!name.is_blank()).then(|| name.to_string_lossy()),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct User {
        id: u64,
        email: Option<String>,
    }

    impl Notifiable for User {
        fn notifiable_key(&self) -> Value {
            json!(self.id)
        }

        fn notifiable_type(&self) -> String {
            "App\\Models\\User".into()
        }
    }

    #[derive(Serialize)]
    struct Ping;

    impl Notification for Ping {
        fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
            vec!["mail".into()]
        }
    }

    #[test]
    fn notifiables_route_mail_to_their_email() {
        let user = User {
            id: 1,
            email: Some("taylor@example.com".into()),
        };
        assert_eq!(
            user.route_notification_for("mail", &Ping),
            Some(json!("taylor@example.com"))
        );
        assert_eq!(
            user.route_notification_for("database", &Ping),
            Some(json!({"notifiable_type": "App\\Models\\User", "notifiable_id": 1}))
        );
        assert_eq!(user.route_notification_for("slack", &Ping), None);
        assert_eq!(
            User { id: 2, email: None }.route_notification_for("mail", &Ping),
            None
        );

        let snapshot = NotifiableSnapshot::of(&user);
        assert!(snapshot.is(&user));
        assert!(!snapshot.is(&User { id: 2, email: None }));
        assert!(!snapshot.is_anonymous());
        assert!(snapshot.anonymous().is_none());
    }

    #[test]
    fn anonymous_notifiables_use_their_routes() {
        let notifiable =
            AnonymousNotifiable::new().route("mail", ["a@example.com", "b@example.com"]);
        let route = notifiable.route_notification_for("mail", &Ping).unwrap();
        assert_eq!(mail_route_addresses(&route).len(), 2);
        assert!(
            AnonymousNotifiable::new()
                .try_route("database", "x")
                .is_err()
        );
        let snapshot = NotifiableSnapshot::of(&notifiable);
        assert!(snapshot.is_anonymous());
        assert_eq!(snapshot.anonymous().unwrap(), notifiable);
    }

    #[test]
    #[should_panic(expected = "The database channel does not support on-demand notifications.")]
    fn anonymous_notifiables_reject_the_database_channel() {
        let _ = AnonymousNotifiable::new().route("database", "x");
    }

    #[test]
    fn mail_routes_become_addresses() {
        assert_eq!(
            mail_route_addresses(&json!("a@example.com")),
            vec![Address::email("a@example.com")]
        );
        assert_eq!(
            mail_route_addresses(&json!({"a@example.com": "A"})),
            vec![Address::new("a@example.com", "A")]
        );
        assert_eq!(
            mail_route_addresses(&json!([{"email": "a@example.com", "name": "A"}])),
            vec![Address::new("a@example.com", "A")]
        );
        assert!(mail_route_addresses(&Value::Null).is_empty());
    }

    #[test]
    fn many_things_are_notifiables() {
        let users = vec![User { id: 1, email: None }, User { id: 2, email: None }];
        assert_eq!((&users).into_notifiables().len(), 2);
        assert_eq!(users.as_slice().into_notifiables().len(), 2);
        assert_eq!(vec![&users[0]].into_notifiables().len(), 1);
        assert_eq!((&users[0]).into_notifiables().len(), 1);
        let dynamic: &dyn Notifiable = &users[1];
        assert_eq!(dynamic.into_notifiables()[0].notifiable_key(), json!(2));
        assert_eq!(vec![dynamic].into_notifiables().len(), 1);
    }
}
