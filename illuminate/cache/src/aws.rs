//! A small client for AWS's JSON APIs (DynamoDB, SQS, ...).
//!
//! Every request is a `POST` of a JSON document to the service's endpoint,
//! naming the operation in the `X-Amz-Target` header and signed with
//! [AWS Signature Version 4](SignatureV4). Requests go through the
//! [`Http`] facade, so `Http::fake()` intercepts them in tests:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_cache::aws::AwsClient;
//! use illuminate_container::Container;
//! use illuminate_http_client::Http;
//! use illuminate_http_client::aws::AwsCredentials;
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! # let _guard = Container::set_local_instance(Arc::new(Container::new()));
//! Http::fake_using(|_| Http::response(json!({"TableNames": ["cache"]}), 200, &[]));
//!
//! let dynamo = AwsClient::new("dynamodb", "DynamoDB_20120810", "eu-west-1")
//!     .with_credentials(AwsCredentials::new("AKIDEXAMPLE", "secret"));
//!
//! let result = dynamo.call("ListTables", json!({})).await.unwrap();
//!
//! assert_eq!(result["TableNames"][0], "cache");
//! Http::assert_sent(|request| {
//!     request.url() == "https://dynamodb.eu-west-1.amazonaws.com/"
//!         && request.has_header_value("X-Amz-Target", "DynamoDB_20120810.ListTables")
//! });
//! # });
//! ```

use std::fmt;

use illuminate_http_client::Http;
use illuminate_http_client::aws::{AwsCredentials, SignatureV4};
use illuminate_support::{Carbon, Map, Result, Value, ValueExt};

/// The error returned when an AWS API rejects a request (the SDK's
/// `AwsException`): the error code (like `ConditionalCheckFailedException`),
/// its message and the HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct AwsException {
    /// The operation that failed, like `PutItem`.
    pub operation: String,
    /// The URL the request was sent to.
    pub url: String,
    /// The HTTP status code of the response.
    pub status: u16,
    /// The AWS error code, like `ConditionalCheckFailedException`.
    pub code: String,
    /// The error message AWS returned.
    pub message: String,
}

impl fmt::Display for AwsException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Error executing \"{}\" on \"{}\"; AWS HTTP error: {} {}",
            self.operation, self.url, self.status, self.code
        )?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

impl AwsException {
    /// Determine if the error has the given AWS error code.
    pub fn is(&self, code: &str) -> bool {
        self.code == code
    }

    /// Determine if the given error is an [`AwsException`] with the given code.
    ///
    /// ```
    /// use illuminate_cache::aws::AwsException;
    ///
    /// let error: illuminate_support::Error = AwsException {
    ///     operation: "PutItem".into(),
    ///     url: "https://dynamodb.us-east-1.amazonaws.com/".into(),
    ///     status: 400,
    ///     code: "ConditionalCheckFailedException".into(),
    ///     message: "The conditional request failed".into(),
    /// }
    /// .into();
    ///
    /// assert!(AwsException::has_code(&error, "ConditionalCheckFailedException"));
    /// assert!(!AwsException::has_code(&error, "ResourceNotFoundException"));
    /// ```
    pub fn has_code(error: &illuminate_support::Error, code: &str) -> bool {
        error
            .downcast_ref::<AwsException>()
            .is_some_and(|exception| exception.is(code))
    }
}

/// A client for one of AWS's JSON APIs.
///
/// The endpoint is `https://{service}.{region}.amazonaws.com/` unless an
/// `endpoint` is configured (handy for LocalStack or DynamoDB Local).
#[derive(Clone)]
pub struct AwsClient {
    service: String,
    target_prefix: String,
    json_version: String,
    region: String,
    endpoint: Option<String>,
    credentials: Option<AwsCredentials>,
    timeout: Option<f64>,
}

impl fmt::Debug for AwsClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsClient")
            .field("service", &self.service)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

/// A non-blank configuration value, as a string.
pub(crate) fn filled(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .filter(|value| !value.is_blank())
        .map(ValueExt::to_string_lossy)
}

impl AwsClient {
    /// The region used when none is configured.
    pub const DEFAULT_REGION: &'static str = "us-east-1";

    /// Create a client for a service: its signing name (`dynamodb`, `sqs`),
    /// the prefix of its `X-Amz-Target` header (`DynamoDB_20120810`,
    /// `AmazonSQS`) and the region.
    pub fn new(
        service: impl Into<String>,
        target_prefix: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            service: service.into(),
            target_prefix: target_prefix.into(),
            json_version: "1.0".to_string(),
            region: region.into(),
            endpoint: None,
            credentials: None,
            timeout: None,
        }
    }

    /// Create a client from a connection's configuration, the way Laravel
    /// builds its AWS clients: `region` (default `us-east-1`), `endpoint`,
    /// and the `key`, `secret` and `token` credentials (used when both the
    /// key and the secret are filled).
    pub fn from_config(
        service: impl Into<String>,
        target_prefix: impl Into<String>,
        config: &Value,
    ) -> Self {
        let mut client = Self::new(
            service,
            target_prefix,
            filled(config, "region").unwrap_or_else(|| Self::DEFAULT_REGION.to_string()),
        );
        if let (Some(key), Some(secret)) = (filled(config, "key"), filled(config, "secret")) {
            client.credentials = Some(
                AwsCredentials::new(key, secret)
                    .with_token(filled(config, "token").unwrap_or_default()),
            );
        }
        if let Some(endpoint) = filled(config, "endpoint") {
            client = client.with_endpoint(endpoint);
        }
        client
    }

    /// Sign requests with the given credentials.
    pub fn with_credentials(mut self, credentials: AwsCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Send requests to a custom endpoint, like `http://localhost:8000`.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into()).filter(|endpoint| !endpoint.is_empty());
        self
    }

    /// Give up on requests after the given number of seconds.
    pub fn with_timeout(mut self, seconds: impl Into<f64>) -> Self {
        self.timeout = Some(seconds.into());
        self
    }

    /// Use another version of the JSON protocol (`1.0` by default; some
    /// services speak `1.1`).
    pub fn with_json_version(mut self, version: impl Into<String>) -> Self {
        self.json_version = version.into();
        self
    }

    /// The service's signing name.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// The region requests are sent to.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The credentials requests are signed with, when configured.
    pub fn credentials(&self) -> Option<&AwsCredentials> {
        self.credentials.as_ref()
    }

    /// The URL requests are sent to.
    pub fn endpoint(&self) -> String {
        let endpoint = match &self.endpoint {
            Some(endpoint) if endpoint.contains("://") => endpoint.clone(),
            Some(endpoint) => format!("https://{endpoint}"),
            None => format!("https://{}.{}.amazonaws.com", self.service, self.region),
        };
        match endpoint.split_once("://") {
            Some((_, rest)) if !rest.contains('/') => format!("{endpoint}/"),
            _ => endpoint,
        }
    }

    /// Call an operation (like `GetItem`) with the given input, returning
    /// the decoded output. Errors reported by AWS become an [`AwsException`].
    pub async fn call(&self, operation: &str, input: Value) -> Result<Value> {
        let url = self.endpoint();
        let exception = |status: u16, code: String, message: String| AwsException {
            operation: operation.to_string(),
            url: url.clone(),
            status,
            code,
            message,
        };
        let Some(credentials) = &self.credentials else {
            return Err(exception(
                0,
                "CredentialsException".to_string(),
                "No AWS credentials are configured; set the [key] and [secret] options".to_string(),
            )
            .into());
        };

        let input = match input {
            Value::Null => Value::Object(Map::new()),
            input => input,
        };
        let body = serde_json::to_vec(&input)?;
        let content_type = format!("application/x-amz-json-{}", self.json_version);
        let target = format!("{}.{operation}", self.target_prefix);
        let signed = SignatureV4::new(&self.service, &self.region).sign(
            "POST",
            &url,
            &[
                ("Content-Type", content_type.as_str()),
                ("X-Amz-Target", target.as_str()),
            ],
            &body,
            credentials,
            Carbon::now(),
        )?;

        let mut request = Http::new_request()
            .with_headers(signed)
            .with_header("X-Amz-Target", target)
            .accept(content_type.as_str())
            .with_body(body, &content_type);
        if let Some(timeout) = self.timeout {
            request = request.timeout(timeout);
        }
        let response = request.post(url.clone(), ()).await?;

        let output = response.json();
        if response.successful() {
            return Ok(match output {
                Value::Null => Value::Object(Map::new()),
                output => output,
            });
        }

        // `{"__type": "com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException", "message": "..."}`
        let kind = output
            .get("__type")
            .or_else(|| output.get("code"))
            .map(ValueExt::to_string_lossy)
            .filter(|kind| !kind.is_empty())
            .unwrap_or_else(|| {
                let header = response.header("x-amzn-ErrorType");
                header.split(':').next().unwrap_or_default().to_string()
            });
        let code = kind.rsplit('#').next().unwrap_or_default().to_string();
        let message = ["message", "Message"]
            .iter()
            .find_map(|key| output.get(*key).filter(|message| !message.is_blank()))
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| response.body().trim().to_string());
        let code = if code.is_empty() {
            response.reason().to_string()
        } else {
            code
        };
        Err(exception(response.status(), code, message).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn endpoints_follow_the_region_unless_configured() {
        let client = AwsClient::new("sqs", "AmazonSQS", "eu-west-1");
        assert_eq!(client.endpoint(), "https://sqs.eu-west-1.amazonaws.com/");
        assert_eq!(
            client.clone().with_endpoint("localhost:4566").endpoint(),
            "https://localhost:4566/"
        );
        assert_eq!(
            client
                .clone()
                .with_endpoint("http://localhost:4566")
                .endpoint(),
            "http://localhost:4566/"
        );
        assert_eq!(
            client.with_endpoint("http://localhost:4566/sqs").endpoint(),
            "http://localhost:4566/sqs"
        );
    }

    #[test]
    fn clients_are_configured_like_laravel() {
        let client = AwsClient::from_config(
            "dynamodb",
            "DynamoDB_20120810",
            &json!({"key": "AKID", "secret": "secret", "token": "token", "region": "eu-central-1", "endpoint": null}),
        );
        assert_eq!(client.region(), "eu-central-1");
        assert_eq!(client.service(), "dynamodb");
        let credentials = client.credentials().unwrap();
        assert_eq!(credentials.key, "AKID");
        assert_eq!(credentials.token.as_deref(), Some("token"));
        assert!(!format!("{client:?}").contains("secret\""));

        let client = AwsClient::from_config(
            "dynamodb",
            "DynamoDB_20120810",
            &json!({"key": "", "secret": "secret"}),
        );
        assert!(client.credentials().is_none());
        assert_eq!(client.region(), "us-east-1");
    }

    #[test]
    fn exceptions_describe_the_failure() {
        let exception = AwsException {
            operation: "GetItem".into(),
            url: "https://dynamodb.us-east-1.amazonaws.com/".into(),
            status: 400,
            code: "ResourceNotFoundException".into(),
            message: "Requested resource not found".into(),
        };
        assert_eq!(
            exception.to_string(),
            "Error executing \"GetItem\" on \"https://dynamodb.us-east-1.amazonaws.com/\"; AWS HTTP error: 400 ResourceNotFoundException: Requested resource not found"
        );
        assert!(exception.is("ResourceNotFoundException"));
    }
}
