//! The `database` channel.

use async_trait::async_trait;
use illuminate_database::{Connection, DatabaseManager};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};

use super::Channel;
use crate::notifiable::Notifiable;
use crate::notification::Notification;

/// Stores notifications in the `notifications` table, where your
/// application's UI can display them (see
/// [`DatabaseNotification`](crate::DatabaseNotification)).
///
/// The stored data comes from [`to_database`](Notification::to_database)
/// (or [`to_array`](Notification::to_array)).
#[derive(Clone, Debug)]
pub struct DatabaseChannel {
    connection: Option<String>,
    table: String,
}

impl Default for DatabaseChannel {
    fn default() -> Self {
        Self {
            connection: None,
            table: "notifications".into(),
        }
    }
}

impl DatabaseChannel {
    /// Create a channel storing notifications in the `notifications` table
    /// of the default connection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use the given database connection.
    pub fn connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Use the given table.
    pub fn table(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    fn database(&self) -> Connection {
        let manager = DatabaseManager::resolve();
        match &self.connection {
            Some(name) => manager.connection(name),
            None => manager.default_connection(),
        }
    }

    /// Build the record to insert for a notification.
    pub fn build_payload(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
    ) -> Result<Value> {
        let data = notification
            .to_database(notifiable)
            .or_else(|| notification.to_array(notifiable))
            .ok_or_else(|| {
                RuntimeException::new("Notification is missing toDatabase / toArray method.")
            })?;
        let route = notifiable
            .route_notification_for("database", notification)
            .unwrap_or(Value::Null);
        let notifiable_type = route
            .get("notifiable_type")
            .cloned()
            .unwrap_or_else(|| Value::String(notifiable.notifiable_type()));
        let notifiable_id = route
            .get("notifiable_id")
            .cloned()
            .unwrap_or_else(|| notifiable.notifiable_key());
        let read_at = notification
            .initial_database_read_at_value(notifiable)
            .map(|read_at| read_at.to_date_time_string());
        let now = Carbon::now().to_date_time_string();
        Ok(json!({
            "id": id,
            "type": notification.database_type(notifiable),
            "notifiable_type": notifiable_type,
            "notifiable_id": notifiable_id,
            "data": serde_json::to_string(&data)?,
            "read_at": read_at,
            "created_at": now,
            "updated_at": now,
        }))
    }

    async fn insert(&self, record: Value) -> Result<Value> {
        self.database()
            .table(self.table.as_str())
            .insert(record.clone())
            .await?;
        Ok(record)
    }
}

#[async_trait]
impl Channel for DatabaseChannel {
    async fn send(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
    ) -> Result<Value> {
        let record = self.build_payload(notifiable, notification, id)?;
        self.insert(record).await
    }

    async fn prepare(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
    ) -> Result<Option<Value>> {
        self.build_payload(notifiable, notification, id).map(Some)
    }

    fn supports_queueing(&self) -> bool {
        true
    }

    async fn deliver(&self, mut payload: Value) -> Result<Value> {
        // The notification is stored when it is delivered, not when it was queued.
        let now = Value::String(Carbon::now().to_date_time_string());
        if let Value::Object(record) = &mut payload {
            record.insert("created_at".into(), now.clone());
            record.insert("updated_at".into(), now);
        }
        self.insert(payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct User;

    impl Notifiable for User {
        fn notifiable_key(&self) -> Value {
            json!("uuid-1")
        }

        fn notifiable_type(&self) -> String {
            "App\\Models\\User".into()
        }
    }

    #[derive(Serialize)]
    struct Mentioned {
        with_data: bool,
    }

    impl Notification for Mentioned {
        fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
            vec!["database".into()]
        }

        fn to_database(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
            self.with_data.then(|| json!({"post": 1}))
        }

        fn database_type(&self, _notifiable: &dyn Notifiable) -> String {
            "mentioned".into()
        }

        fn initial_database_read_at_value(&self, _notifiable: &dyn Notifiable) -> Option<Carbon> {
            Some(Carbon::parse("2024-01-01 12:00:00").unwrap())
        }
    }

    #[test]
    fn payloads_identify_the_notifiable() {
        let channel = DatabaseChannel::new().connection("sqlite").table("alerts");
        assert_eq!(channel.table, "alerts");
        let payload = channel
            .build_payload(&User, &Mentioned { with_data: true }, "id-1")
            .unwrap();
        assert_eq!(payload["id"], "id-1");
        assert_eq!(payload["type"], "mentioned");
        assert_eq!(payload["notifiable_type"], "App\\Models\\User");
        assert_eq!(payload["notifiable_id"], "uuid-1");
        assert_eq!(payload["data"], "{\"post\":1}");
        assert_eq!(payload["read_at"], "2024-01-01 12:00:00");

        let error = channel
            .build_payload(&User, &Mentioned { with_data: false }, "id-2")
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Notification is missing toDatabase / toArray method."
        );
    }
}
