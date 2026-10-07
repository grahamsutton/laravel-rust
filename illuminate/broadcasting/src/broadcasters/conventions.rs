//! Pusher's channel naming conventions (Laravel's
//! `UsePusherChannelConventions`).

/// Determine if the channel is protected by authentication.
///
/// ```
/// use illuminate_broadcasting::broadcasters::conventions::is_guarded_channel;
///
/// assert!(is_guarded_channel("private-orders.1"));
/// assert!(is_guarded_channel("presence-chat.1"));
/// assert!(!is_guarded_channel("orders"));
/// ```
pub fn is_guarded_channel(channel: &str) -> bool {
    channel.starts_with("private-") || channel.starts_with("presence-")
}

/// Remove the `private-encrypted-`, `private-` or `presence-` prefix from a
/// channel name.
///
/// ```
/// use illuminate_broadcasting::broadcasters::conventions::normalize_channel_name;
///
/// assert_eq!(normalize_channel_name("private-encrypted-orders.1"), "orders.1");
/// assert_eq!(normalize_channel_name("presence-chat.1"), "chat.1");
/// assert_eq!(normalize_channel_name("orders"), "orders");
/// ```
pub fn normalize_channel_name(channel: &str) -> String {
    for prefix in ["private-encrypted-", "private-", "presence-"] {
        if let Some(name) = channel.strip_prefix(prefix) {
            return name.to_string();
        }
    }
    channel.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_normalized_like_laravel() {
        let prefixes = [
            ("private-", true),
            ("private-encrypted-", true),
            ("presence-", true),
            ("", false),
        ];
        let channels = [
            "test",
            "test-channel",
            "test-private-channel",
            "test-presence-channel",
            "abcd.efgh",
            "abcd.efgh.ijkl",
            "test.{param}",
            "test-{param}",
            "{a}.{b}",
            "{a}-{b}",
            "{a}-{b}.{c}",
        ];
        for (prefix, guarded) in prefixes {
            for channel in channels {
                let name = format!("{prefix}{channel}");
                assert_eq!(normalize_channel_name(&name), channel, "{name}");
                assert_eq!(is_guarded_channel(&name), guarded, "{name}");
            }
        }

        for (name, normalized, guarded) in [
            ("private-private-test", "private-test", true),
            ("private-presence-test", "presence-test", true),
            ("presence-private-test", "private-test", true),
            ("presence-presence-test", "presence-test", true),
            ("public-test", "public-test", false),
        ] {
            assert_eq!(normalize_channel_name(name), normalized);
            assert_eq!(is_guarded_channel(name), guarded);
        }

        assert_eq!(
            normalize_channel_name("private-encrypted-private-123"),
            "private-123"
        );
    }
}
