//! HTTP Digest authentication (RFC 7616), for `with_digest_auth`.

use md5::{Digest as _, Md5};
use rand::Rng;
use sha2::Sha256;

/// A parsed `WWW-Authenticate: Digest ...` challenge.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Challenge {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    algorithm: String,
    qop: Option<String>,
}

impl Challenge {
    /// Parse a digest challenge from a `WWW-Authenticate` header value.
    pub(crate) fn parse(header: &str) -> Option<Self> {
        let (scheme, params) = header.trim().split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("digest") {
            return None;
        }

        let mut challenge = Challenge {
            algorithm: "MD5".to_string(),
            ..Default::default()
        };

        for (key, value) in parse_params(params) {
            match key.to_ascii_lowercase().as_str() {
                "realm" => challenge.realm = value,
                "nonce" => challenge.nonce = value,
                "opaque" => challenge.opaque = Some(value),
                "algorithm" => challenge.algorithm = value,
                "qop" => {
                    // Prefer "auth" when the server offers a choice.
                    let options: Vec<&str> = value.split(',').map(str::trim).collect();
                    challenge.qop = options
                        .iter()
                        .find(|option| option.eq_ignore_ascii_case("auth"))
                        .or(options.first())
                        .map(|option| option.to_string());
                }
                _ => {}
            }
        }

        (!challenge.nonce.is_empty()).then_some(challenge)
    }

    /// Build the `Authorization` header answering the challenge.
    pub(crate) fn authorize(
        &self,
        username: &str,
        password: &str,
        method: &str,
        uri: &str,
    ) -> String {
        let cnonce = hex::encode(rand::rng().random::<[u8; 8]>());
        self.authorize_with_cnonce(username, password, method, uri, &cnonce)
    }

    fn authorize_with_cnonce(
        &self,
        username: &str,
        password: &str,
        method: &str,
        uri: &str,
        cnonce: &str,
    ) -> String {
        let algorithm = self.algorithm.to_ascii_uppercase();
        let hash = |value: String| match algorithm.trim_end_matches("-SESS") {
            "SHA-256" => hex::encode(Sha256::digest(value.as_bytes())),
            _ => hex::encode(Md5::digest(value.as_bytes())),
        };

        let nc = "00000001";
        let mut ha1 = hash(format!("{username}:{}:{password}", self.realm));
        if algorithm.ends_with("-SESS") {
            ha1 = hash(format!("{ha1}:{}:{cnonce}", self.nonce));
        }
        let ha2 = hash(format!("{method}:{uri}"));

        let response = match &self.qop {
            Some(qop) => hash(format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", self.nonce)),
            None => hash(format!("{ha1}:{}:{ha2}", self.nonce)),
        };

        let mut header = format!(
            "Digest username=\"{username}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", algorithm={}, response=\"{response}\"",
            self.realm, self.nonce, self.algorithm
        );

        if let Some(qop) = &self.qop {
            header.push_str(&format!(", qop={qop}, nc={nc}, cnonce=\"{cnonce}\""));
        }
        if let Some(opaque) = &self.opaque {
            header.push_str(&format!(", opaque=\"{opaque}\""));
        }

        header
    }
}

/// Parse `key=value, key="quoted, value"` parameters.
fn parse_params(input: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut rest = input.trim();

    while !rest.is_empty() {
        let Some((key, after)) = rest.split_once('=') else {
            break;
        };
        let key = key.trim().trim_start_matches(',').trim().to_string();
        let after = after.trim_start();

        let (value, remaining) = if let Some(quoted) = after.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((index, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => {
                        end = index + 1;
                        break;
                    }
                    other => value.push(other),
                }
            }
            (value, &quoted[end.min(quoted.len())..])
        } else {
            match after.find(',') {
                Some(index) => (after[..index].trim().to_string(), &after[index..]),
                None => (after.trim().to_string(), ""),
            }
        };

        params.push((key, value));
        rest = remaining.trim_start().trim_start_matches(',').trim_start();
    }

    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_challenges() {
        let challenge = Challenge::parse(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();

        assert_eq!(challenge.realm, "testrealm@host.com");
        assert_eq!(challenge.nonce, "dcd98b7102dd2f0e8b11d0f600bfb0c093");
        assert_eq!(challenge.qop.as_deref(), Some("auth"));
        assert_eq!(
            challenge.opaque.as_deref(),
            Some("5ccc069c403ebaf9f0171e9517f40e41")
        );
        assert_eq!(challenge.algorithm, "MD5");

        assert!(Challenge::parse("Basic realm=\"x\"").is_none());
        assert!(Challenge::parse("Digest realm=\"x\"").is_none());
    }

    #[test]
    fn it_answers_the_rfc_2617_example() {
        let challenge = Challenge::parse(
            r#"Digest realm="testrealm@host.com", qop="auth", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();

        let header = challenge.authorize_with_cnonce(
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "0a4f113b",
        );

        assert!(
            header.contains(r#"response="6629fae49393a05397450978507c4ef1""#),
            "{header}"
        );
        assert!(header.contains("qop=auth, nc=00000001, cnonce=\"0a4f113b\""));
        assert!(header.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#));
    }

    #[test]
    fn it_supports_sha_256_and_legacy_challenges() {
        let sha = Challenge::parse(r#"Digest realm="r", nonce="n", algorithm=SHA-256"#).unwrap();
        let header = sha.authorize("user", "pass", "GET", "/");
        assert!(header.contains("algorithm=SHA-256"));
        assert!(!header.contains("qop="));

        let session =
            Challenge::parse(r#"Digest realm="r", nonce="n", algorithm=MD5-sess, qop=auth"#)
                .unwrap();
        assert!(
            session
                .authorize("user", "pass", "GET", "/")
                .contains("algorithm=MD5-sess")
        );
    }

    #[test]
    fn it_parses_escaped_parameters() {
        assert_eq!(
            parse_params(r#"a="x\"y", b=plain, c="with, comma""#),
            vec![
                ("a".to_string(), "x\"y".to_string()),
                ("b".to_string(), "plain".to_string()),
                ("c".to_string(), "with, comma".to_string()),
            ]
        );
    }
}
