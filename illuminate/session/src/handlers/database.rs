use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use illuminate_database::{Builder, Connection, QueryException};
use illuminate_http::{Request, async_trait};
use illuminate_support::{Carbon, Map, Result, Value, ValueExt};

use super::SessionHandler;

/// The longest user agent recorded with a session, in characters.
const USER_AGENT_LENGTH: usize = 500;

/// Stores sessions in a database table — Laravel's `DatabaseSessionHandler`.
///
/// Each session is a row of the `session.table` table (`sessions` by
/// default), created by the skeleton's users migration:
///
/// ```php
/// Schema::create('sessions', function (Blueprint $table) {
///     $table->string('id')->primary();
///     $table->foreignId('user_id')->nullable()->index();
///     $table->string('ip_address', 45)->nullable();
///     $table->text('user_agent')->nullable();
///     $table->longText('payload');
///     $table->integer('last_activity')->index();
/// });
/// ```
///
/// The payload is stored base64 encoded. When the [`StartSession`]
/// middleware hands the handler the current request, every write also
/// records the client's IP address, its user agent, and the authenticated
/// user's ID (the request's `_auth_id` attribute).
///
/// [`StartSession`]: crate::StartSession
///
/// ```
/// use illuminate_database::Connection;
/// use illuminate_session::{DatabaseSessionHandler, SessionHandler};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let connection = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
/// connection.get_schema_builder().create("sessions", |table| {
///     table.string("id").primary();
///     table.foreign_id("user_id").nullable().index();
///     table.string_len("ip_address", 45).nullable();
///     table.text("user_agent").nullable();
///     table.long_text("payload");
///     table.integer("last_activity").index();
/// }).await.unwrap();
///
/// let handler = DatabaseSessionHandler::new(connection, "sessions", 120);
/// handler.write("abc", r#"{"name":"Taylor"}"#).await.unwrap();
///
/// assert_eq!(handler.read("abc").await.unwrap(), r#"{"name":"Taylor"}"#);
/// assert_eq!(handler.read("missing").await.unwrap(), "");
/// # });
/// ```
#[derive(Debug)]
pub struct DatabaseSessionHandler {
    connection: Connection,
    table: String,
    minutes: i64,
    exists: AtomicBool,
    request: RwLock<Option<Request>>,
}

impl DatabaseSessionHandler {
    /// Create a handler storing sessions in `table`, valid for `minutes`.
    pub fn new(connection: Connection, table: impl Into<String>, minutes: i64) -> Self {
        Self {
            connection,
            table: table.into(),
            minutes,
            exists: AtomicBool::new(false),
            request: RwLock::new(None),
        }
    }

    /// The database connection sessions are stored on.
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }

    /// The name of the sessions table.
    pub fn get_table(&self) -> &str {
        &self.table
    }

    /// Determine if the session is known to exist in the table.
    pub fn exists(&self) -> bool {
        self.exists.load(Ordering::SeqCst)
    }

    /// Determine if a (possibly expired) row exists for the session ID.
    pub async fn validate_id(&self, session_id: &str) -> Result<bool> {
        Ok(self.query().find(session_id).await?.is_some())
    }

    /// A fresh query builder for the sessions table.
    fn query(&self) -> Builder {
        self.connection.table(self.table.as_str()).use_write_pdo()
    }

    fn current_time() -> i64 {
        Carbon::now().timestamp()
    }

    /// Determine if the session row has been idle longer than the lifetime.
    fn expired(&self, session: &Value) -> bool {
        match session
            .get("last_activity")
            .and_then(ValueExt::to_i64_lossy)
        {
            Some(last_activity) => {
                last_activity < Self::current_time() - self.minutes.saturating_mul(60)
            }
            None => false,
        }
    }

    /// The columns written for the session: the payload, the time, and —
    /// when the request is known — the user, IP address and user agent.
    fn default_payload(&self, data: &str) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("payload".into(), Value::String(BASE64.encode(data)));
        payload.insert("last_activity".into(), Value::from(Self::current_time()));

        if let Some(request) = self.request.read().unwrap().clone() {
            self.add_user_information(&mut payload, &request);
            self.add_request_information(&mut payload, &request);
        }
        payload
    }

    /// Record the authenticated user's ID (the `_auth_id` request attribute).
    ///
    /// The attribute is only known once a guard has resolved the user, so a
    /// missing ID never clears the one already recorded — invalidating the
    /// session on logout (as Laravel's docs recommend) starts a fresh row.
    fn add_user_information(&self, payload: &mut Map<String, Value>, request: &Request) {
        let id = request.attribute("_auth_id");
        if !id.is_null() {
            payload.insert("user_id".into(), id);
        }
    }

    /// Record the client's IP address and user agent.
    fn add_request_information(&self, payload: &mut Map<String, Value>, request: &Request) {
        payload.insert(
            "ip_address".into(),
            request.ip().map(Value::String).unwrap_or(Value::Null),
        );
        payload.insert(
            "user_agent".into(),
            Value::String(
                request
                    .user_agent()
                    .unwrap_or_default()
                    .chars()
                    .take(USER_AGENT_LENGTH)
                    .collect(),
            ),
        );
    }

    async fn perform_insert(
        &self,
        session_id: &str,
        mut payload: Map<String, Value>,
    ) -> Result<()> {
        payload.insert("id".into(), Value::String(session_id.to_string()));
        match self.query().insert(Value::Object(payload.clone())).await {
            Ok(_) => Ok(()),
            // Another request created the row in the meantime: update it instead.
            Err(error) if error.downcast_ref::<QueryException>().is_some() => {
                payload.remove("id");
                self.perform_update(session_id, payload).await
            }
            Err(error) => Err(error),
        }
    }

    async fn perform_update(&self, session_id: &str, payload: Map<String, Value>) -> Result<()> {
        self.query()
            .where_("id", session_id)
            .update(Value::Object(payload))
            .await?;
        Ok(())
    }
}

#[async_trait]
impl SessionHandler for DatabaseSessionHandler {
    async fn read(&self, session_id: &str) -> Result<String> {
        let Some(session) = self.query().find(session_id).await? else {
            return Ok(String::new());
        };

        // The row exists either way, so the next write is an update.
        self.exists.store(true, Ordering::SeqCst);
        if self.expired(&session) {
            return Ok(String::new());
        }

        Ok(match session.get("payload") {
            Some(Value::String(payload)) => BASE64
                .decode(payload.trim())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default(),
            _ => String::new(),
        })
    }

    async fn write(&self, session_id: &str, data: &str) -> Result<()> {
        let payload = self.default_payload(data);

        if !self.exists() {
            self.read(session_id).await?;
        }

        if self.exists() {
            self.perform_update(session_id, payload).await?;
        } else {
            self.perform_insert(session_id, payload).await?;
        }

        self.exists.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn destroy(&self, session_id: &str) -> Result<()> {
        self.query().where_("id", session_id).delete().await?;
        Ok(())
    }

    async fn gc(&self, lifetime: u64) -> Result<usize> {
        let lifetime = i64::try_from(lifetime).unwrap_or(i64::MAX);
        let deleted = self
            .query()
            .where_op(
                "last_activity",
                "<=",
                Self::current_time().saturating_sub(lifetime),
            )
            .delete()
            .await?;
        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }

    fn set_exists(&self, exists: bool) {
        self.exists.store(exists, Ordering::SeqCst);
    }

    fn needs_request(&self) -> bool {
        true
    }

    fn set_request(&self, request: &Request) {
        *self.request.write().unwrap() = Some(request.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_http::{HeaderMap, HeaderValue};
    use illuminate_support::json;

    async fn connection() -> Connection {
        let connection = Connection::new(
            "sqlite",
            json!({"driver": "sqlite", "database": ":memory:"}),
        );
        connection
            .get_schema_builder()
            .create("sessions", |table| {
                table.string("id").primary();
                table.foreign_id("user_id").nullable().index();
                table.string_len("ip_address", 45).nullable();
                table.text("user_agent").nullable();
                table.long_text("payload");
                table.integer("last_activity").index();
            })
            .await
            .unwrap();
        connection
    }

    async fn row(connection: &Connection, id: &str) -> Value {
        connection
            .table("sessions")
            .find(id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn age(connection: &Connection, id: &str, seconds: i64) {
        connection
            .table("sessions")
            .where_("id", id)
            .update(json!({"last_activity": Carbon::now().timestamp() - seconds}))
            .await
            .unwrap();
    }

    fn request(user_agent: &str) -> Request {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_str(user_agent).unwrap());
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        let request = Request::create_with("/", "GET", json!({}), headers);
        request.set_trust_proxies(true);
        request
    }

    #[tokio::test]
    async fn sessions_are_stored_base64_encoded() {
        let connection = connection().await;
        let handler = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);
        assert_eq!(handler.get_table(), "sessions");
        assert_eq!(handler.get_connection().get_name(), "sqlite");
        assert!(handler.needs_request());

        assert_eq!(handler.read("abc").await.unwrap(), "");
        assert!(!handler.exists());
        assert!(!handler.validate_id("abc").await.unwrap());

        let before = Carbon::now().timestamp();
        handler.write("abc", r#"{"name":"Taylor"}"#).await.unwrap();
        assert!(handler.exists());
        assert!(handler.validate_id("abc").await.unwrap());

        let stored = row(&connection, "abc").await;
        assert_eq!(stored["payload"], BASE64.encode(r#"{"name":"Taylor"}"#));
        assert_eq!(stored["user_id"], Value::Null);
        assert_eq!(stored["ip_address"], Value::Null);
        assert_eq!(stored["user_agent"], Value::Null);
        let last_activity = stored["last_activity"].as_i64().unwrap();
        assert!((before..=Carbon::now().timestamp()).contains(&last_activity));

        // A second write updates the same row.
        handler.write("abc", "second").await.unwrap();
        assert_eq!(handler.read("abc").await.unwrap(), "second");
        assert_eq!(connection.table("sessions").count().await.unwrap(), 1);

        // A fresh handler (the next request) reads it back too.
        let next = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);
        assert_eq!(next.read("abc").await.unwrap(), "second");
        assert!(next.exists());

        handler.destroy("abc").await.unwrap();
        handler.destroy("abc").await.unwrap();
        assert_eq!(handler.read("abc").await.unwrap(), "");
        assert_eq!(connection.table("sessions").count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_existence_flag_chooses_between_insert_and_update() {
        let connection = connection().await;
        let handler = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);

        // Marked as existing, but the row is gone: the update touches nothing.
        handler.set_exists(true);
        handler.write("ghost", "data").await.unwrap();
        assert_eq!(connection.table("sessions").count().await.unwrap(), 0);

        // Marked as new, but another request created the row: it is updated.
        let other = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);
        other.write("shared", "first").await.unwrap();
        handler.set_exists(false);
        handler.write("shared", "second").await.unwrap();
        assert_eq!(connection.table("sessions").count().await.unwrap(), 1);
        assert_eq!(other.read("shared").await.unwrap(), "second");
    }

    #[tokio::test]
    async fn expired_sessions_read_as_empty_but_still_exist() {
        let connection = connection().await;
        let handler = DatabaseSessionHandler::new(connection.clone(), "sessions", 1);
        handler.write("old", "data").await.unwrap();
        age(&connection, "old", 59).await;
        assert_eq!(handler.read("old").await.unwrap(), "data");
        age(&connection, "old", 61).await;

        let next = DatabaseSessionHandler::new(connection.clone(), "sessions", 1);
        assert_eq!(next.read("old").await.unwrap(), "");
        assert!(next.exists(), "the expired row is updated, not inserted");
        next.write("old", "fresh").await.unwrap();
        assert_eq!(next.read("old").await.unwrap(), "fresh");
        assert_eq!(connection.table("sessions").count().await.unwrap(), 1);

        // Payloads that aren't base64 read as empty sessions.
        connection
            .table("sessions")
            .where_("id", "old")
            .update(json!({"payload": "not base64!"}))
            .await
            .unwrap();
        assert_eq!(next.read("old").await.unwrap(), "");
    }

    #[tokio::test]
    async fn garbage_collection_removes_idle_sessions() {
        let connection = connection().await;
        let handler = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);
        for id in ["old", "older", "new"] {
            handler.set_exists(false);
            handler.write(id, "data").await.unwrap();
        }
        age(&connection, "old", 3600).await;
        age(&connection, "older", 7200).await;

        assert_eq!(handler.gc(1800).await.unwrap(), 2);
        assert_eq!(
            connection
                .table("sessions")
                .pluck("id")
                .await
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![json!("new")]
        );
        assert_eq!(handler.gc(1800).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_request_and_user_are_recorded() {
        let connection = connection().await;
        let handler = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);
        let request = request(&"a".repeat(600));
        handler.set_request(&request);

        handler.write("abc", "guest").await.unwrap();
        let stored = row(&connection, "abc").await;
        assert_eq!(stored["ip_address"], "203.0.113.9");
        assert_eq!(stored["user_agent"], "a".repeat(500));
        assert_eq!(stored["user_id"], Value::Null);

        // Logging in during the request is picked up when the session is saved.
        request.set_attribute("_auth_id", 42);
        handler.write("abc", "user").await.unwrap();
        assert_eq!(row(&connection, "abc").await["user_id"], 42);

        // A request that never resolved the user keeps the recorded ID.
        let next = DatabaseSessionHandler::new(connection.clone(), "sessions", 120);
        next.set_request(&self::request("Mozilla/5.0"));
        next.write("abc", "later").await.unwrap();
        let stored = row(&connection, "abc").await;
        assert_eq!(stored["user_id"], 42);
        assert_eq!(stored["user_agent"], "Mozilla/5.0");
    }

    #[tokio::test]
    async fn database_errors_are_reported() {
        let connection = connection().await;
        let handler = DatabaseSessionHandler::new(connection, "missing", 120);
        assert!(handler.read("abc").await.is_err());
        assert!(handler.write("abc", "data").await.is_err());
        assert!(handler.destroy("abc").await.is_err());
        assert!(handler.gc(10).await.is_err());
    }
}
