//! The Amazon SES transports.

use std::sync::LazyLock;

use async_trait::async_trait;
use illuminate_support::{Carbon, Map, Result, Value, ValueExt, json};
use regex::Regex;

use illuminate_http_client::aws::{AwsCredentials, SignatureV4};
use super::{Transport, TransportException, api};
use crate::message::{Message, SentMessage};

/// Sends mail through [Amazon SES](https://aws.amazon.com/ses/) (the `ses`
/// and `ses-v2` mailer transports), using the SES v2 `SendEmail` API with
/// the raw MIME message and requests signed with AWS Signature Version 4.
///
/// The credentials and region come from `services.ses` (`key`, `secret`,
/// `region` and, for temporary credentials, `token`), and may be overridden
/// on the mailer. Additional `SendEmail` parameters may be given as the
/// `options`, exactly like Laravel:
///
/// ```
/// use illuminate_mail::{SesTransport, Transport};
/// use illuminate_support::json;
///
/// let transport = SesTransport::from_config(
///     &json!({"transport": "ses", "options": {"ConfigurationSetName": "MyConfigurationSet"}}),
///     &json!({"key": "AKIDEXAMPLE", "secret": "secret", "region": "eu-west-1"}),
/// )
/// .unwrap();
///
/// assert_eq!(transport.name(), "ses");
/// assert_eq!(transport.region(), "eu-west-1");
/// assert_eq!(transport.url(), "https://email.eu-west-1.amazonaws.com/v2/email/outbound-emails");
/// assert_eq!(transport.options()["ConfigurationSetName"], "MyConfigurationSet");
/// ```
///
/// Like Laravel, the message's metadata becomes SES `EmailTags`, the
/// `X-SES-LIST-MANAGEMENT-OPTIONS` header becomes `ListManagementOptions`
/// and the `X-SES-TENANT-NAME` header becomes the `TenantName`. The
/// `MessageId` SES returns becomes the sent message's ID, and is added to
/// the sent message as its `X-Message-ID` and `X-SES-Message-ID` headers.
///
/// Both transports send through the v2 API; `ses` also accepts the v1
/// `Tags` option, sending it as `EmailTags`.
#[derive(Clone)]
pub struct SesTransport {
    credentials: Option<AwsCredentials>,
    region: String,
    endpoint: Option<String>,
    options: Map<String, Value>,
    client: Value,
    v2: bool,
}

impl std::fmt::Debug for SesTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SesTransport")
            .field("name", &self.name())
            .field("credentials", &self.credentials)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// Laravel's pattern for the `X-SES-LIST-MANAGEMENT-OPTIONS` header.
static LIST_MANAGEMENT_OPTIONS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(contactListName=)*(?P<ContactListName>[^;]+)(;\s?topicName=(?P<TopicName>.+))?$",
    )
    .expect("a valid pattern")
});

impl SesTransport {
    /// The region used when none is configured.
    pub const DEFAULT_REGION: &'static str = "us-east-1";

    /// Create an SES transport (the `ses` transport).
    pub fn new(credentials: AwsCredentials, region: impl Into<String>) -> Self {
        Self {
            credentials: Some(credentials),
            region: region.into(),
            endpoint: None,
            options: Map::new(),
            client: Value::Null,
            v2: false,
        }
    }

    /// Create an SES V2 transport (the `ses-v2` transport).
    pub fn v2(credentials: AwsCredentials, region: impl Into<String>) -> Self {
        Self {
            v2: true,
            ..Self::new(credentials, region)
        }
    }

    /// Create the transport from a mailer's configuration merged over the
    /// `services.ses` configuration. A `transport` of `ses-v2` creates the
    /// SES V2 transport.
    pub fn from_config(config: &Value, services: &Value) -> Result<Self> {
        let mut merged = services.as_object().cloned().unwrap_or_default();
        if let Some(config) = config.as_object() {
            merged.extend(config.clone());
        }
        let merged = Value::Object(merged);

        let credentials = match (api::filled(&merged, "key"), api::filled(&merged, "secret")) {
            (Some(key), Some(secret)) => Some(
                AwsCredentials::new(key, secret)
                    .with_token(api::filled(&merged, "token").unwrap_or_default()),
            ),
            _ => None,
        };
        let options = match merged.get("options") {
            Some(Value::Object(options)) => options.clone(),
            _ => Map::new(),
        };
        Ok(Self {
            credentials,
            region: api::filled(&merged, "region")
                .unwrap_or_else(|| Self::DEFAULT_REGION.to_string()),
            endpoint: api::filled(&merged, "endpoint"),
            options,
            client: config.get("client").cloned().unwrap_or(Value::Null),
            v2: api::filled(config, "transport").as_deref() == Some("ses-v2"),
        })
    }

    /// Set the additional `SendEmail` parameters, like `ConfigurationSetName`.
    pub fn with_options(mut self, options: Value) -> Self {
        self.options = match options {
            Value::Object(options) => options,
            _ => Map::new(),
        };
        self
    }

    /// Send through a custom endpoint, like `http://localhost:4566`.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into()).filter(|endpoint| !endpoint.is_empty());
        self
    }

    /// Set the HTTP client options (the mailer's `client`, like `{"timeout": 5}`).
    pub fn with_client_options(mut self, options: impl Into<Option<Value>>) -> Self {
        self.client = options.into().unwrap_or(Value::Null);
        self
    }

    /// The additional `SendEmail` parameters.
    pub fn options(&self) -> &Map<String, Value> {
        &self.options
    }

    /// The AWS region.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The AWS credentials, when configured.
    pub fn credentials(&self) -> Option<&AwsCredentials> {
        self.credentials.as_ref()
    }

    /// The URL of the `SendEmail` API.
    pub fn url(&self) -> String {
        let base = match &self.endpoint {
            Some(endpoint) if endpoint.contains("://") => endpoint.clone(),
            Some(endpoint) => format!("https://{endpoint}"),
            None => format!("https://email.{}.amazonaws.com", self.region),
        };
        format!("{}/v2/email/outbound-emails", base.trim_end_matches('/'))
    }

    /// Build the `SendEmail` request for a message.
    pub(crate) fn payload(&self, message: &Message) -> Result<Value> {
        let mut payload = self.options.clone();
        if let Some(tags) = payload.remove("Tags") {
            payload.entry("EmailTags").or_insert(tags);
        }
        if let Some(options) = message
            .get_header("X-SES-LIST-MANAGEMENT-OPTIONS")
            .and_then(list_management_options)
        {
            payload.insert("ListManagementOptions".into(), options);
        }
        if let Some(tenant) = message
            .get_header("X-SES-TENANT-NAME")
            .filter(|tenant| !tenant.is_empty())
        {
            payload.insert("TenantName".into(), Value::String(tenant.to_string()));
        }
        if !message.metadata.is_empty()
            && let Value::Array(tags) = payload.entry("EmailTags").or_insert_with(|| json!([]))
        {
            tags.extend(
                message
                    .metadata
                    .iter()
                    .map(|(key, value)| json!({"Name": key, "Value": value})),
            );
        }
        payload.insert(
            "Destination".into(),
            json!({"ToAddresses": api::recipients(message)}),
        );
        payload.insert(
            "Content".into(),
            json!({"Raw": {"Data": api::base64(&crate::mime::format(message)?)}}),
        );
        Ok(Value::Object(payload))
    }

    /// An error message, worded like Laravel's.
    fn failure(&self, reason: &str) -> String {
        let api = if self.v2 { "AWS SES V2" } else { "AWS SES" };
        format!("Request to {api} API failed. Reason: {reason}.")
    }
}

/// Parse the `X-SES-LIST-MANAGEMENT-OPTIONS` header
/// (`contactListName=MyContactList;topicName=MyTopic`).
fn list_management_options(header: &str) -> Option<Value> {
    let captures = LIST_MANAGEMENT_OPTIONS.captures(header)?;
    Some(Value::Object(
        ["ContactListName", "TopicName"]
            .into_iter()
            .filter_map(|name| {
                captures
                    .name(name)
                    .map(|value| (name.to_string(), Value::String(value.as_str().to_string())))
            })
            .collect(),
    ))
}

#[async_trait]
impl Transport for SesTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let credentials = self.credentials.as_ref().ok_or_else(|| {
            TransportException::new(self.failure(
                "No AWS credentials are configured; set [services.ses.key] and [services.ses.secret]",
            ))
        })?;
        let body = serde_json::to_vec(&self.payload(message)?)?;
        let url = self.url();
        let signed = SignatureV4::new("ses", &self.region).sign(
            "POST",
            &url,
            &[("Content-Type", "application/json")],
            &body,
            credentials,
            Carbon::now(),
        )?;

        let response = api::request(&self.client)
            .with_headers(signed)
            .with_body(body, "application/json")
            .post(url, ())
            .await
            .map_err(|e| {
                let reason = e.to_string();
                api::unreachable(e, self.failure(&reason))
            })?;

        let status = response.status();
        let result = response.json();
        if !response.successful() {
            let reason = ["message", "Message"]
                .iter()
                .find_map(|key| result.get(*key).filter(|reason| !reason.is_blank()))
                .map(ValueExt::to_string_lossy)
                .or_else(|| {
                    let kind = response.header("x-amzn-ErrorType");
                    let kind = kind.split(':').next().unwrap_or_default();
                    (!kind.is_empty()).then(|| kind.to_string())
                })
                .unwrap_or_else(|| match response.body().trim() {
                    "" => format!("{status} {}", response.reason()),
                    body => body.to_string(),
                });
            return Err(api::failed(self.failure(&reason), status.into(), response));
        }

        let id = result["MessageId"].to_string_lossy();
        let mut sent = SentMessage::new(message.clone());
        sent.message
            .header("X-Message-ID", id.clone())
            .header("X-SES-Message-ID", id.clone());
        sent.message_id = id;
        sent.debug = response.body().into_owned();
        Ok(sent)
    }

    fn name(&self) -> String {
        if self.v2 { "ses-v2" } else { "ses" }.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::testing;
    use base64::Engine;
    use illuminate_http_client::{Http, Request};

    /// Freeze "now" on this thread for the lifetime of the guard.
    struct FrozenNow;

    impl FrozenNow {
        fn at(timestamp: i64) -> Self {
            Carbon::set_thread_test_now(Some(Carbon::from_timestamp(timestamp)));
            Self
        }
    }

    impl Drop for FrozenNow {
        fn drop(&mut self) {
            Carbon::set_thread_test_now(None);
        }
    }

    fn transport() -> SesTransport {
        SesTransport::new(AwsCredentials::new("AKIDEXAMPLE", "secret"), "eu-west-1")
    }

    fn accepted() -> illuminate_http_client::FakeResponse {
        Http::response(json!({"MessageId": "0102018c-ses-message-id"}), 200, &[])
    }

    fn raw_message(request: &Request) -> String {
        let data = request.data()["Content"]["Raw"]["Data"].as_str().unwrap();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn messages_are_sent_through_the_ses_v2_api() {
        let _app = testing::container();
        let _now = FrozenNow::at(1_440_938_160);
        Http::fake_using(|_| accepted());
        let mut message = testing::message();
        message.metadata("plan", "pro");

        let sent = transport()
            .with_options(json!({
                "ConfigurationSetName": "MyConfigurationSet",
                "EmailTags": [{"Name": "foo", "Value": "bar"}],
            }))
            .send(&message)
            .await
            .unwrap();

        assert_eq!(sent.message_id(), "0102018c-ses-message-id");
        assert_eq!(
            sent.message.get_header("X-Message-ID"),
            Some("0102018c-ses-message-id")
        );
        assert_eq!(
            sent.message.get_header("X-SES-Message-ID"),
            Some("0102018c-ses-message-id")
        );

        let request = testing::sent_request();
        assert_eq!(request.method(), "POST");
        assert_eq!(
            request.url(),
            "https://email.eu-west-1.amazonaws.com/v2/email/outbound-emails"
        );
        assert!(request.has_header_value("Content-Type", "application/json"));
        assert!(request.has_header_value("X-Amz-Date", "20150830T123600Z"));
        assert!(!request.has_header("X-Amz-Security-Token"));

        let data = request.data();
        assert_eq!(data["ConfigurationSetName"], "MyConfigurationSet");
        assert_eq!(
            data["EmailTags"],
            json!([
                {"Name": "foo", "Value": "bar"},
                {"Name": "order_id", "Value": "1"},
                {"Name": "plan", "Value": "pro"},
            ])
        );
        assert_eq!(
            data["Destination"],
            json!({"ToAddresses": [
                "taylor@example.com",
                "abigail@example.com",
                "james@example.com",
                "secret@example.com",
            ]})
        );
        for key in ["ListManagementOptions", "TenantName", "FromEmailAddress"] {
            assert!(data.get(key).is_none(), "{key} should not be sent");
        }

        let raw = raw_message(&request);
        assert!(raw.contains("From: Example <hello@example.com>"));
        assert!(raw.contains("Subject: Order Shipped"));
        assert!(raw.contains("Message-ID: <abc@example.com>"));
        assert!(raw.contains("X-Tag: shipment"));
        assert!(raw.contains(&format!("Content-ID: <{}>", testing::content_id(&message))));
        assert!(
            !raw.contains("secret@example.com"),
            "Bcc must not be in the raw message"
        );

        // The request is signed exactly as it was sent...
        let authorization = request.header("Authorization").remove(0);
        assert!(authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-1/ses/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, Signature="
        ));
        let expected = SignatureV4::new("ses", "eu-west-1")
            .sign(
                "POST",
                request.url(),
                &[("Content-Type", "application/json")],
                request.bytes(),
                &AwsCredentials::new("AKIDEXAMPLE", "secret"),
                Carbon::from_timestamp(1_440_938_160),
            )
            .unwrap();
        assert_eq!(authorization, expected[1].1);
    }

    #[tokio::test]
    async fn temporary_credentials_send_their_session_token() {
        let _app = testing::container();
        Http::fake_using(|_| accepted());

        SesTransport::v2(
            AwsCredentials::new("AKIDEXAMPLE", "secret").with_token("session-token"),
            "us-east-1",
        )
        .send(&testing::message())
        .await
        .unwrap();

        let request = testing::sent_request();
        assert!(request.has_header_value("X-Amz-Security-Token", "session-token"));
        assert!(
            request.header("Authorization")[0]
                .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token,")
        );
    }

    #[test]
    fn ses_headers_become_send_email_parameters() {
        let mut message = testing::message();
        message
            .header(
                "X-Ses-List-Management-Options",
                "contactListName=MyContactList;topicName=MyTopic",
            )
            .header("X-SES-TENANT-NAME", "tenant-id");
        let payload = transport()
            .with_options(json!({"Tags": [{"Name": "foo", "Value": "bar"}]}))
            .payload(&message)
            .unwrap();

        assert_eq!(
            payload["ListManagementOptions"],
            json!({"ContactListName": "MyContactList", "TopicName": "MyTopic"})
        );
        assert_eq!(payload["TenantName"], "tenant-id");
        assert!(payload.get("Tags").is_none());
        assert_eq!(
            payload["EmailTags"],
            json!([{"Name": "foo", "Value": "bar"}, {"Name": "order_id", "Value": "1"}])
        );

        assert_eq!(
            list_management_options("MyContactList"),
            Some(json!({"ContactListName": "MyContactList"}))
        );
        assert_eq!(
            list_management_options("CONTACTLISTNAME=List; topicName=Topic"),
            Some(json!({"ContactListName": "List", "TopicName": "Topic"}))
        );
        assert_eq!(list_management_options(""), None);
    }

    #[tokio::test]
    async fn api_errors_are_reported_like_laravel() {
        let _app = testing::container();
        Http::fake_sequence("email.*")
            .push(json!({"message": "Email address is not verified."}), 400)
            .push_with_headers(
                "",
                403,
                &[(
                    "x-amzn-ErrorType",
                    "AccessDeniedException:http://internal.amazon.com/",
                )],
            )
            .push("", 503)
            .push_failed_connection_with("Connection refused");
        let message = testing::message();
        let v2 = SesTransport::v2(AwsCredentials::new("key", "secret"), "us-east-1");

        let error = transport().send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Request to AWS SES API failed. Reason: Email address is not verified.."
        );
        let exception = error.downcast_ref::<TransportException>().unwrap();
        assert_eq!(exception.code, 400);
        assert!(exception.response.is_some());

        let error = v2.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Request to AWS SES V2 API failed. Reason: AccessDeniedException."
        );

        let error = v2.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Request to AWS SES V2 API failed. Reason: 503 Service Unavailable."
        );

        let error = v2.send(&message).await.unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Request to AWS SES V2 API failed. Reason: ")
        );
        assert!(error.to_string().contains("Connection refused"));
    }

    #[tokio::test]
    async fn missing_credentials_fail_before_anything_is_sent() {
        let _app = testing::container();
        Http::fake();
        let transport =
            SesTransport::from_config(&json!({"transport": "ses"}), &Value::Null).unwrap();
        assert!(transport.credentials().is_none());

        let error = transport.send(&testing::message()).await.unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("Request to AWS SES API failed. Reason: No AWS credentials")
        );
        Http::assert_nothing_sent();
    }

    #[test]
    fn configuration_is_merged_over_the_services_configuration() {
        let services = json!({
            "key": "services-key",
            "secret": "services-secret",
            "region": "eu-west-1",
            "token": "services-token",
            "options": {"ConfigurationSetName": "Services"},
        });

        let transport = SesTransport::from_config(&json!({"transport": "ses"}), &services).unwrap();
        assert_eq!(transport.name(), "ses");
        assert_eq!(
            transport.credentials(),
            Some(
                &AwsCredentials::new("services-key", "services-secret")
                    .with_token("services-token")
            )
        );
        assert_eq!(transport.region(), "eu-west-1");
        assert_eq!(transport.options()["ConfigurationSetName"], "Services");

        let transport = SesTransport::from_config(
            &json!({
                "transport": "ses-v2",
                "key": "mailer-key",
                "secret": "mailer-secret",
                "region": "ap-southeast-2",
                "endpoint": "http://localhost:4566/",
                "options": {"ConfigurationSetName": "Mailer"},
            }),
            &services,
        )
        .unwrap();
        assert_eq!(transport.name(), "ses-v2");
        assert_eq!(transport.credentials().unwrap().key, "mailer-key");
        assert_eq!(transport.region(), "ap-southeast-2");
        assert_eq!(
            transport.url(),
            "http://localhost:4566/v2/email/outbound-emails"
        );
        assert_eq!(transport.options()["ConfigurationSetName"], "Mailer");
        assert!(!format!("{transport:?}").contains("mailer-secret"));

        let transport =
            SesTransport::from_config(&json!({}), &json!({"key": "only-a-key"})).unwrap();
        assert!(transport.credentials().is_none());
        assert_eq!(transport.region(), "us-east-1");
        assert_eq!(
            transport.with_endpoint("email.example.com").url(),
            "https://email.example.com/v2/email/outbound-emails"
        );
    }
}
