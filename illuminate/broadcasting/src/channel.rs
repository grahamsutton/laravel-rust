//! Channels: the named streams your events are broadcast on.
//!
//! Public channels may be subscribed to by anyone. Private, presence and
//! encrypted private channels require [authorization](crate::Broadcast::channel)
//! and carry Pusher's conventional name prefixes.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A broadcast channel.
///
/// The channel's *kind* lives in its name, using Pusher's conventions:
/// `private-` for private channels, `presence-` for presence channels and
/// `private-encrypted-` for end-to-end encrypted private channels.
///
/// ```
/// use illuminate_broadcasting::{Channel, EncryptedPrivateChannel, PresenceChannel, PrivateChannel};
///
/// assert_eq!(Channel::new("orders").name(), "orders");
/// assert_eq!(PrivateChannel::new("orders.1").name(), "private-orders.1");
/// assert_eq!(PresenceChannel::new("chat.1").name(), "presence-chat.1");
/// assert_eq!(EncryptedPrivateChannel::new("orders.1").name(), "private-encrypted-orders.1");
///
/// assert!(PrivateChannel::new("orders.1").is_private());
/// assert_eq!(PrivateChannel::new("orders.1").to_string(), "private-orders.1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Channel {
    name: String,
}

impl Channel {
    /// Create a new (public) channel instance.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    /// Create a public channel. Alias of [`Channel::new`].
    pub fn public(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    /// Create a private channel (`private-{name}`).
    pub fn private(name: impl Into<String>) -> Self {
        Self::new(format!("private-{}", name.into()))
    }

    /// Create a presence channel (`presence-{name}`).
    pub fn presence(name: impl Into<String>) -> Self {
        Self::new(format!("presence-{}", name.into()))
    }

    /// Create an end-to-end encrypted private channel (`private-encrypted-{name}`).
    pub fn encrypted_private(name: impl Into<String>) -> Self {
        Self::new(format!("private-encrypted-{}", name.into()))
    }

    /// The public channel conventionally associated with an entity, such
    /// as an Eloquent model (`App.Models.User.1`).
    pub fn for_model(model: &impl HasBroadcastChannel) -> Self {
        Self::new(model.broadcast_channel())
    }

    /// The channel's full name, prefix included.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Determine if the channel is a private channel (encrypted ones included).
    pub fn is_private(&self) -> bool {
        self.name.starts_with("private-")
    }

    /// Determine if the channel is a presence channel.
    pub fn is_presence(&self) -> bool {
        self.name.starts_with("presence-")
    }

    /// Determine if the channel is an end-to-end encrypted private channel.
    pub fn is_encrypted(&self) -> bool {
        self.name.starts_with("private-encrypted-")
    }

    /// Determine if subscribing to the channel requires authorization.
    pub fn is_guarded(&self) -> bool {
        self.is_private() || self.is_presence()
    }

    /// Determine if anyone may subscribe to the channel.
    pub fn is_public(&self) -> bool {
        !self.is_guarded()
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl AsRef<str> for Channel {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

impl From<&str> for Channel {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for Channel {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

impl From<&String> for Channel {
    fn from(name: &String) -> Self {
        Self::new(name.clone())
    }
}

impl From<&Channel> for Channel {
    fn from(channel: &Channel) -> Self {
        channel.clone()
    }
}

impl PartialEq<str> for Channel {
    fn eq(&self, other: &str) -> bool {
        self.name == other
    }
}

impl PartialEq<&str> for Channel {
    fn eq(&self, other: &&str) -> bool {
        self.name == *other
    }
}

impl PartialEq<String> for Channel {
    fn eq(&self, other: &String) -> bool {
        &self.name == other
    }
}

macro_rules! channel_kind {
    ($(#[$meta:meta])* $kind:ident, $constructor:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy)]
        pub struct $kind;

        impl $kind {
            #[doc = concat!("Create a new `", $prefix, "` channel instance.")]
            #[allow(clippy::new_ret_no_self)]
            pub fn new(name: impl Into<String>) -> Channel {
                Channel::$constructor(name)
            }

            #[doc = concat!("The `", $prefix, "` channel conventionally associated with an entity.")]
            pub fn for_model(model: &impl HasBroadcastChannel) -> Channel {
                Channel::$constructor(model.broadcast_channel())
            }
        }
    };
}

channel_kind!(
    /// Private channels require the current user to be authorized to listen
    /// on them: `PrivateChannel::new("orders.1")` is `private-orders.1`.
    PrivateChannel,
    private,
    "private-"
);

channel_kind!(
    /// Presence channels are private channels that also expose who is
    /// subscribed: `PresenceChannel::new("chat.1")` is `presence-chat.1`.
    PresenceChannel,
    presence,
    "presence-"
);

channel_kind!(
    /// End-to-end encrypted private channels: only your application and its
    /// authorized clients can read the event data.
    /// `EncryptedPrivateChannel::new("orders.1")` is `private-encrypted-orders.1`.
    EncryptedPrivateChannel,
    encrypted_private,
    "private-encrypted-"
);

/// Entities (such as Eloquent models) with a conventional broadcast channel.
///
/// ```
/// use illuminate_broadcasting::{HasBroadcastChannel, PrivateChannel};
///
/// struct User { id: u64 }
///
/// impl HasBroadcastChannel for User {
///     fn broadcast_channel_route(&self) -> String {
///         "App.Models.User.{user}".into()
///     }
///
///     fn broadcast_channel(&self) -> String {
///         format!("App.Models.User.{}", self.id)
///     }
/// }
///
/// let channel = PrivateChannel::for_model(&User { id: 1 });
/// assert_eq!(channel.name(), "private-App.Models.User.1");
/// ```
pub trait HasBroadcastChannel {
    /// The broadcast channel *route* (pattern) associated with the entity.
    fn broadcast_channel_route(&self) -> String;

    /// The broadcast channel name associated with the entity.
    fn broadcast_channel(&self) -> String;
}

/// Anything that names one or more channels: a [`Channel`], a string (a
/// public channel), or a list of either.
///
/// ```
/// use illuminate_broadcasting::{Channel, IntoChannels, PrivateChannel};
///
/// assert_eq!("orders".into_channels(), vec![Channel::new("orders")]);
/// assert_eq!(
///     vec![PrivateChannel::new("a"), Channel::new("b")].into_channels(),
///     vec![Channel::new("private-a"), Channel::new("b")]
/// );
/// assert_eq!(["a", "b"].into_channels().len(), 2);
/// ```
pub trait IntoChannels {
    /// Convert into a list of channels.
    fn into_channels(self) -> Vec<Channel>;
}

impl IntoChannels for Channel {
    fn into_channels(self) -> Vec<Channel> {
        vec![self]
    }
}

impl IntoChannels for &Channel {
    fn into_channels(self) -> Vec<Channel> {
        vec![self.clone()]
    }
}

impl IntoChannels for &str {
    fn into_channels(self) -> Vec<Channel> {
        vec![Channel::new(self)]
    }
}

impl IntoChannels for String {
    fn into_channels(self) -> Vec<Channel> {
        vec![Channel::new(self)]
    }
}

impl IntoChannels for &String {
    fn into_channels(self) -> Vec<Channel> {
        vec![Channel::new(self.clone())]
    }
}

impl<T: Into<Channel>> IntoChannels for Vec<T> {
    fn into_channels(self) -> Vec<Channel> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Channel>, const N: usize> IntoChannels for [T; N] {
    fn into_channels(self) -> Vec<Channel> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Channel> + Clone> IntoChannels for &[T] {
    fn into_channels(self) -> Vec<Channel> {
        self.iter().cloned().map(Into::into).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_carry_pusher_prefixes() {
        assert_eq!(Channel::public("orders").name(), "orders");
        assert_eq!(Channel::private("orders.1").name(), "private-orders.1");
        assert_eq!(Channel::presence("chat.1").name(), "presence-chat.1");
        assert_eq!(
            Channel::encrypted_private("orders.1").name(),
            "private-encrypted-orders.1"
        );
        assert_eq!(PrivateChannel::new("a"), Channel::private("a"));
        assert_eq!(PresenceChannel::new("a"), Channel::presence("a"));
        assert_eq!(
            EncryptedPrivateChannel::new("a"),
            Channel::encrypted_private("a")
        );
    }

    #[test]
    fn channels_know_their_kind() {
        let public = Channel::new("orders");
        assert!(public.is_public() && !public.is_guarded());

        let private = PrivateChannel::new("orders.1");
        assert!(private.is_private() && private.is_guarded() && !private.is_encrypted());

        let presence = PresenceChannel::new("chat.1");
        assert!(presence.is_presence() && presence.is_guarded() && !presence.is_private());

        let encrypted = EncryptedPrivateChannel::new("orders.1");
        assert!(encrypted.is_encrypted() && encrypted.is_private() && encrypted.is_guarded());
    }

    #[test]
    fn channels_serialize_as_their_names() {
        let channel = PresenceChannel::new("chat.1");
        assert_eq!(
            serde_json::to_string(&channel).unwrap(),
            "\"presence-chat.1\""
        );
        let back: Channel = serde_json::from_str("\"presence-chat.1\"").unwrap();
        assert_eq!(back, channel);
        assert_eq!(channel, "presence-chat.1");
        assert_eq!(channel, String::from("presence-chat.1"));
    }

    struct Post {
        id: u64,
    }

    impl HasBroadcastChannel for Post {
        fn broadcast_channel_route(&self) -> String {
            "App.Models.Post.{post}".into()
        }

        fn broadcast_channel(&self) -> String {
            format!("App.Models.Post.{}", self.id)
        }
    }

    #[test]
    fn models_have_conventional_channels() {
        let post = Post { id: 7 };
        assert_eq!(Channel::for_model(&post).name(), "App.Models.Post.7");
        assert_eq!(
            PrivateChannel::for_model(&post).name(),
            "private-App.Models.Post.7"
        );
        assert_eq!(
            PresenceChannel::for_model(&post).name(),
            "presence-App.Models.Post.7"
        );
        assert_eq!(
            EncryptedPrivateChannel::for_model(&post).name(),
            "private-encrypted-App.Models.Post.7"
        );
    }

    #[test]
    fn many_things_name_channels() {
        let channel = Channel::new("a");
        assert_eq!((&channel).into_channels(), vec![Channel::new("a")]);
        assert_eq!(String::from("a").into_channels(), vec![Channel::new("a")]);
        assert_eq!(
            (&String::from("a")).into_channels(),
            vec![Channel::new("a")]
        );
        let list = [Channel::new("a"), Channel::new("b")];
        assert_eq!(list.as_slice().into_channels().len(), 2);
        assert_eq!(vec!["a", "b"].into_channels()[1], Channel::new("b"));
    }
}
