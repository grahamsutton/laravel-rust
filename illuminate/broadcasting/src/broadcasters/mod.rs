//! The broadcasters (drivers) that ship with the framework.

mod ably;
pub mod conventions;
mod log;
mod pusher;

pub use ably::AblyBroadcaster;
pub use log::{LogBroadcaster, NullBroadcaster};
pub use pusher::{Pusher, PusherBroadcaster, PusherException, PusherSettings};

/// Percent-encode a URL component (RFC 3986 unreserved characters are kept).
pub(crate) fn url_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_percent_encoded() {
        assert_eq!(url_encode("abc-1.2_~"), "abc-1.2_~");
        assert_eq!(url_encode("public:a b/c"), "public%3Aa%20b%2Fc");
    }
}
