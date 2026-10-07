//! AWS Signature Version 4: how requests to AWS APIs (Amazon SES, SQS,
//! DynamoDB, ...) are signed.

use std::collections::BTreeMap;
use std::fmt;

use hmac::{Hmac, Mac};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Carbon, Result};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};
use url::Url;

/// The algorithm every Signature Version 4 request is signed with.
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// Credentials for AWS: an access key ID, its secret, and — for
/// [temporary credentials](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_temp_use-resources.html)
/// — a session token.
///
/// ```
/// use illuminate_http_client::aws::AwsCredentials;
///
/// let credentials = AwsCredentials::new("AKIDEXAMPLE", "wJalrXUtnFEMI").with_token("session-token");
///
/// assert_eq!(credentials.token.as_deref(), Some("session-token"));
///
/// // Secrets never show up in debug output...
/// assert!(!format!("{credentials:?}").contains("wJalrXUtnFEMI"));
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct AwsCredentials {
    /// The access key ID (`AWS_ACCESS_KEY_ID`).
    pub key: String,
    /// The secret access key (`AWS_SECRET_ACCESS_KEY`).
    pub secret: String,
    /// The session token of temporary credentials (`AWS_SESSION_TOKEN`).
    pub token: Option<String>,
}

impl AwsCredentials {
    /// Create credentials from an access key ID and its secret.
    pub fn new(key: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            secret: secret.into(),
            token: None,
        }
    }

    /// Use a session token, for temporary credentials. Empty tokens are ignored.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into()).filter(|token| !token.is_empty());
        self
    }
}

impl fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("key", &self.key)
            .field("secret", &"********")
            .field("token", &self.token.as_ref().map(|_| "********"))
            .finish()
    }
}

/// Signs requests with [AWS Signature Version 4](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv.html).
///
/// Signing returns the headers to add to the request — `X-Amz-Date`,
/// `X-Amz-Security-Token` (for temporary credentials) and `Authorization` —
/// so it works with any HTTP client:
///
/// ```
/// use illuminate_http_client::aws::{AwsCredentials, SignatureV4};
/// use illuminate_support::Carbon;
///
/// // The "get-vanilla" request from AWS's Signature Version 4 test suite...
/// let credentials = AwsCredentials::new("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
///
/// let headers = SignatureV4::new("service", "us-east-1")
///     .sign(
///         "GET",
///         "https://example.amazonaws.com/",
///         &[],
///         b"",
///         &credentials,
///         Carbon::from_timestamp(1_440_938_160),
///     )
///     .unwrap();
///
/// assert_eq!(headers[0], ("X-Amz-Date".to_string(), "20150830T123600Z".to_string()));
/// assert_eq!(
///     headers[1].1,
///     "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
///      SignedHeaders=host;x-amz-date, \
///      Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureV4 {
    service: String,
    region: String,
}

impl SignatureV4 {
    /// Create a signer for the given service (its signing name, like `ses`)
    /// and region.
    pub fn new(service: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            region: region.into(),
        }
    }

    /// The service requests are signed for.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// The region requests are signed for.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Sign a request, returning the headers to add to it.
    ///
    /// Every header given is signed, along with `Host` (taken from the URL
    /// unless given), `X-Amz-Date` and, for temporary credentials,
    /// `X-Amz-Security-Token`. Send the signed headers exactly as given.
    pub fn sign(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        payload: &[u8],
        credentials: &AwsCredentials,
        time: Carbon,
    ) -> Result<Vec<(String, String)>> {
        let url = Url::parse(url)
            .map_err(|e| InvalidArgumentException::new(format!("Invalid URL [{url}]: {e}")))?;
        let amz_date = time.utc().inner().format("%Y%m%dT%H%M%SZ").to_string();

        let mut headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"))
        {
            headers.push(("host".into(), host(&url)));
        }
        headers.push(("x-amz-date".into(), amz_date.clone()));
        if let Some(token) = &credentials.token {
            headers.push(("x-amz-security-token".into(), token.clone()));
        }

        let (canonical_request, signed_headers) =
            self.canonical_request(method, &url, &headers, payload);
        let scope = format!(
            "{}/{}/{}/aws4_request",
            &amz_date[..8],
            self.region,
            self.service
        );
        let string_to_sign = format!(
            "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let signature = hex::encode(hmac(
            &self.signing_key(&credentials.secret, &amz_date[..8]),
            string_to_sign.as_bytes(),
        ));

        let mut signed = vec![("X-Amz-Date".to_string(), amz_date)];
        if let Some(token) = &credentials.token {
            signed.push(("X-Amz-Security-Token".to_string(), token.clone()));
        }
        signed.push((
            "Authorization".to_string(),
            format!(
                "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                credentials.key
            ),
        ));
        Ok(signed)
    }

    /// Derive the signing key for the given secret and date (`YYYYMMDD`).
    ///
    /// ```
    /// use illuminate_http_client::aws::SignatureV4;
    ///
    /// let key = SignatureV4::new("iam", "us-east-1")
    ///     .signing_key("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20120215");
    ///
    /// assert_eq!(hex::encode(key), "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d");
    /// ```
    pub fn signing_key(&self, secret: &str, date: &str) -> Vec<u8> {
        [self.region.as_str(), self.service.as_str(), "aws4_request"]
            .iter()
            .fold(
                hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes()),
                |key, part| hmac(&key, part.as_bytes()),
            )
    }

    /// Build the canonical request, returning it with the signed header names.
    fn canonical_request(
        &self,
        method: &str,
        url: &Url,
        headers: &[(String, String)],
        payload: &[u8],
    ) -> (String, String) {
        let mut canonical: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, value) in headers {
            canonical
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push(value.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        let signed_headers = canonical.keys().cloned().collect::<Vec<_>>().join(";");
        let canonical_headers: String = canonical
            .iter()
            .map(|(name, values)| format!("{name}:{}\n", values.join(",")))
            .collect();

        let request = [
            method.to_ascii_uppercase(),
            self.canonical_uri(url),
            canonical_query(url),
            canonical_headers,
            signed_headers.clone(),
            hex::encode(Sha256::digest(payload)),
        ]
        .join("\n");
        (request, signed_headers)
    }

    /// The URI-encoded path: every service but S3 encodes each segment twice.
    fn canonical_uri(&self, url: &Url) -> String {
        let path = url
            .path()
            .split('/')
            .map(|segment| {
                let once = encode(&percent_decode_str(segment).collect::<Vec<u8>>());
                if self.service == "s3" {
                    once
                } else {
                    encode(once.as_bytes())
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        if path.is_empty() { "/".into() } else { path }
    }
}

/// The sorted, URI-encoded query string.
fn canonical_query(url: &Url) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |s: &str| encode(&percent_decode_str(s).collect::<Vec<u8>>());
            (decode(key), decode(value))
        })
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// The `Host` header for a URL (with the port, when it isn't the default).
fn host(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// URI-encode bytes the way AWS does: everything but unreserved characters.
fn encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    /// 2015-08-30T12:36:00Z, the time every request in AWS's test suite is signed at.
    fn test_suite_time() -> Carbon {
        Carbon::from_timestamp(1_440_938_160)
    }

    fn credentials() -> AwsCredentials {
        AwsCredentials::new("AKIDEXAMPLE", SECRET)
    }

    fn signature(headers: &[(String, String)]) -> &str {
        let authorization = &headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .unwrap()
            .1;
        authorization.rsplit("Signature=").next().unwrap()
    }

    fn sign_test_suite_request(
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        payload: &[u8],
    ) -> Vec<(String, String)> {
        SignatureV4::new("service", "us-east-1")
            .sign(
                method,
                url,
                headers,
                payload,
                &credentials(),
                test_suite_time(),
            )
            .unwrap()
    }

    #[test]
    fn the_get_vanilla_request_is_signed() {
        let signer = SignatureV4::new("service", "us-east-1");
        let url = Url::parse("https://example.amazonaws.com/").unwrap();
        let headers = vec![
            ("Host".to_string(), "example.amazonaws.com".to_string()),
            ("X-Amz-Date".to_string(), "20150830T123600Z".to_string()),
        ];
        let (canonical, signed_headers) = signer.canonical_request("GET", &url, &headers, b"");
        assert_eq!(
            canonical,
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(signed_headers, "host;x-amz-date");

        let signed = sign_test_suite_request("GET", "https://example.amazonaws.com/", &[], b"");
        assert_eq!(
            signed,
            vec![
                ("X-Amz-Date".to_string(), "20150830T123600Z".to_string()),
                (
                    "Authorization".to_string(),
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                     SignedHeaders=host;x-amz-date, \
                     Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn the_rest_of_the_test_suite_vectors_are_signed() {
        // post-vanilla
        let signed = sign_test_suite_request("POST", "https://example.amazonaws.com/", &[], b"");
        assert_eq!(
            signature(&signed),
            "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
        );

        // get-vanilla-query-order-key-case: parameters are sorted.
        let signed = sign_test_suite_request(
            "GET",
            "https://example.amazonaws.com/?Param2=value2&Param1=value1",
            &[],
            b"",
        );
        assert_eq!(
            signature(&signed),
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );

        // get-vanilla-empty-query-key / get-vanilla-query: an empty query is ignored.
        let signed = sign_test_suite_request("GET", "https://example.amazonaws.com/?", &[], b"");
        assert_eq!(
            signature(&signed),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );

        // post-x-www-form-urlencoded: the content type and payload are signed.
        let signed = sign_test_suite_request(
            "POST",
            "https://example.amazonaws.com/",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            b"Param1=value1",
        );
        assert_eq!(
            signature(&signed),
            "ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a"
        );
    }

    #[test]
    fn the_iam_list_users_example_is_signed() {
        // The worked example from AWS's "Create a signed request" documentation.
        let signed = SignatureV4::new("iam", "us-east-1")
            .sign(
                "GET",
                "https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08",
                &[(
                    "Content-Type",
                    "application/x-www-form-urlencoded; charset=utf-8",
                )],
                b"",
                &credentials(),
                test_suite_time(),
            )
            .unwrap();
        assert_eq!(
            signed[1].1,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, \
             Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn signing_keys_are_derived() {
        let key = SignatureV4::new("iam", "us-east-1").signing_key(SECRET, "20120215");
        assert_eq!(
            hex::encode(key),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn session_tokens_are_signed_and_sent() {
        let credentials = credentials().with_token("session-token");
        let signed = SignatureV4::new("ses", "eu-west-1")
            .sign(
                "POST",
                "https://email.eu-west-1.amazonaws.com/v2/email/outbound-emails",
                &[("Content-Type", "application/json")],
                br#"{"Content":{}}"#,
                &credentials,
                test_suite_time(),
            )
            .unwrap();
        assert_eq!(
            signed[1],
            (
                "X-Amz-Security-Token".to_string(),
                "session-token".to_string()
            )
        );
        // Verified independently against a reference implementation.
        assert_eq!(
            signed[2].1,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-1/ses/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, \
             Signature=1b914be933dd189b64ce922875844650804132a9e0f9b7dd6532a8527055b611"
        );
        assert!(AwsCredentials::new("a", "b").with_token("").token.is_none());
    }

    #[test]
    fn uris_queries_and_headers_are_canonicalized() {
        let signer = SignatureV4::new("service", "us-east-1");
        let url = Url::parse("http://localhost:4566/a b/c%2Fd?b=2&a=x y&a=1&flag").unwrap();
        assert_eq!(signer.canonical_uri(&url), "/a%2520b/c%252Fd");
        assert_eq!(
            SignatureV4::new("s3", "us-east-1").canonical_uri(&url),
            "/a%20b/c%2Fd"
        );
        assert_eq!(canonical_query(&url), "a=1&a=x%20y&b=2&flag=");
        assert_eq!(host(&url), "localhost:4566");
        assert_eq!(
            host(&Url::parse("https://example.com:443/").unwrap()),
            "example.com"
        );

        let headers = vec![
            ("X-Multi".to_string(), "  a   b  ".to_string()),
            ("x-multi".to_string(), "c".to_string()),
            ("Host".to_string(), "localhost:4566".to_string()),
        ];
        let (canonical, signed) = signer.canonical_request("get", &url, &headers, b"");
        assert!(canonical.starts_with("GET\n"));
        assert!(canonical.contains("host:localhost:4566\nx-multi:a b,c\n\n"));
        assert_eq!(signed, "host;x-multi");

        assert!(
            signer
                .sign(
                    "GET",
                    "not a url",
                    &[],
                    b"",
                    &credentials(),
                    test_suite_time()
                )
                .is_err()
        );
        assert_eq!(signer.service(), "service");
        assert_eq!(signer.region(), "us-east-1");
    }
}
