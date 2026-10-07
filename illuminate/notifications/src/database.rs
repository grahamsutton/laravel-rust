//! Database notifications: reading the `notifications` table.

use std::future::Future;

use illuminate_database::{Builder, DB, Migration, Schema, async_trait};
use illuminate_support::{Carbon, Result, Value, ValueExt, json};
use serde::{Deserialize, Serialize};

use crate::notifiable::Notifiable;

/// The table database notifications are stored in.
pub const NOTIFICATIONS_TABLE: &str = "notifications";

/// A notification stored by the `database` channel (Laravel's
/// `DatabaseNotification` model).
///
/// ```no_run
/// # async fn example(user: &impl illuminate_notifications::Notifiable) -> illuminate_support::Result<()> {
/// use illuminate_notifications::HasDatabaseNotifications;
///
/// for mut notification in user.unread_notifications().await? {
///     println!("{}: {}", notification.notification_type, notification.data["invoice_id"]);
///     notification.mark_as_read().await?;
/// }
/// # Ok(()) }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatabaseNotification {
    /// The notification's ID (a UUID).
    pub id: String,
    /// The notification's type.
    #[serde(rename = "type")]
    pub notification_type: String,
    /// The notifiable's type.
    pub notifiable_type: String,
    /// The notifiable's key.
    pub notifiable_id: Value,
    /// The notification's data (decoded from JSON).
    pub data: Value,
    /// When the notification was read.
    pub read_at: Option<String>,
    /// When the notification was created.
    pub created_at: Option<String>,
    /// When the notification was last updated.
    pub updated_at: Option<String>,
}

fn optional_string(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .filter(|value| !value.is_null())
        .map(ValueExt::to_string_lossy)
}

impl DatabaseNotification {
    /// Build a notification from a database row.
    pub fn from_row(row: &Value) -> Result<Self> {
        let data = match row.get("data") {
            Some(Value::String(json)) => serde_json::from_str(json)?,
            Some(other) => other.clone(),
            None => Value::Null,
        };
        Ok(Self {
            id: row
                .get("id")
                .map(ValueExt::to_string_lossy)
                .unwrap_or_default(),
            notification_type: row
                .get("type")
                .map(ValueExt::to_string_lossy)
                .unwrap_or_default(),
            notifiable_type: row
                .get("notifiable_type")
                .map(ValueExt::to_string_lossy)
                .unwrap_or_default(),
            notifiable_id: row.get("notifiable_id").cloned().unwrap_or(Value::Null),
            data,
            read_at: optional_string(row, "read_at"),
            created_at: optional_string(row, "created_at"),
            updated_at: optional_string(row, "updated_at"),
        })
    }

    /// Begin a query against the `notifications` table.
    pub fn query() -> Builder {
        DB::table(NOTIFICATIONS_TABLE)
    }

    /// Query the notifications of the given notifiable, newest first.
    pub fn for_notifiable(notifiable: &dyn Notifiable) -> Builder {
        Self::query()
            .where_("notifiable_type", notifiable.notifiable_type())
            .where_("notifiable_id", notifiable.notifiable_key())
            .latest()
    }

    /// Scope a query to only include read notifications.
    pub fn scope_read(query: Builder) -> Builder {
        query.where_not_null("read_at")
    }

    /// Scope a query to only include unread notifications.
    pub fn scope_unread(query: Builder) -> Builder {
        query.where_null("read_at")
    }

    /// Run a notifications query.
    pub async fn get(query: Builder) -> Result<Vec<DatabaseNotification>> {
        query
            .get()
            .await?
            .all()
            .iter()
            .map(DatabaseNotification::from_row)
            .collect()
    }

    /// Find a notification by its ID.
    pub async fn find(id: &str) -> Result<Option<DatabaseNotification>> {
        match Self::query().where_("id", id).first().await? {
            Some(row) => Ok(Some(Self::from_row(&row)?)),
            None => Ok(None),
        }
    }

    /// Determine if a notification has been read.
    pub fn read(&self) -> bool {
        self.read_at.is_some()
    }

    /// Determine if a notification has not been read.
    pub fn unread(&self) -> bool {
        self.read_at.is_none()
    }

    /// Mark the notification as read.
    pub async fn mark_as_read(&mut self) -> Result<()> {
        if self.read_at.is_none() {
            let now = Carbon::now().to_date_time_string();
            Self::query()
                .where_("id", self.id.as_str())
                .update(json!({"read_at": now, "updated_at": now}))
                .await?;
            self.read_at = Some(now.clone());
            self.updated_at = Some(now);
        }
        Ok(())
    }

    /// Mark the notification as unread.
    pub async fn mark_as_unread(&mut self) -> Result<()> {
        if self.read_at.is_some() {
            let now = Carbon::now().to_date_time_string();
            Self::query()
                .where_("id", self.id.as_str())
                .update(json!({"read_at": null, "updated_at": now}))
                .await?;
            self.read_at = None;
            self.updated_at = Some(now);
        }
        Ok(())
    }

    /// Mark every given notification as read.
    pub async fn mark_all_as_read(notifications: &mut [DatabaseNotification]) -> Result<()> {
        for notification in notifications {
            notification.mark_as_read().await?;
        }
        Ok(())
    }

    /// Mark every given notification as unread.
    pub async fn mark_all_as_unread(notifications: &mut [DatabaseNotification]) -> Result<()> {
        for notification in notifications {
            notification.mark_as_unread().await?;
        }
        Ok(())
    }

    /// Delete the notification.
    pub async fn delete(&self) -> Result<()> {
        Self::query()
            .where_("id", self.id.as_str())
            .delete()
            .await?;
        Ok(())
    }
}

/// Database notification helpers for every notifiable (Laravel's
/// `HasDatabaseNotifications` trait).
pub trait HasDatabaseNotifications: Notifiable + Sized {
    /// A query for the notifiable's notifications, newest first.
    fn notifications_query(&self) -> Builder {
        DatabaseNotification::for_notifiable(self)
    }

    /// Get the notifiable's notifications, newest first.
    fn notifications(&self) -> impl Future<Output = Result<Vec<DatabaseNotification>>> + Send {
        let query = self.notifications_query();
        async move { DatabaseNotification::get(query).await }
    }

    /// Get the notifiable's read notifications.
    fn read_notifications(&self) -> impl Future<Output = Result<Vec<DatabaseNotification>>> + Send {
        let query = DatabaseNotification::scope_read(self.notifications_query());
        async move { DatabaseNotification::get(query).await }
    }

    /// Get the notifiable's unread notifications.
    fn unread_notifications(
        &self,
    ) -> impl Future<Output = Result<Vec<DatabaseNotification>>> + Send {
        let query = DatabaseNotification::scope_unread(self.notifications_query());
        async move { DatabaseNotification::get(query).await }
    }

    /// Mark all of the notifiable's unread notifications as read.
    fn mark_notifications_as_read(&self) -> impl Future<Output = Result<u64>> + Send {
        let query = DatabaseNotification::scope_unread(
            DatabaseNotification::query()
                .where_("notifiable_type", self.notifiable_type())
                .where_("notifiable_id", self.notifiable_key()),
        );
        async move {
            let now = Carbon::now().to_date_time_string();
            query
                .update(json!({"read_at": now, "updated_at": now}))
                .await
        }
    }
}

impl<T: Notifiable + Sized> HasDatabaseNotifications for T {}

/// The migration for the `notifications` table (Laravel's
/// `notifications.stub`, from `php artisan make:notifications-table`).
///
/// ```no_run
/// use illuminate_database::{Migrator, migrations};
/// use illuminate_notifications::CreateNotificationsTable;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let migrator = Migrator::resolve(migrations![
///     "2024_01_01_000000_create_notifications_table" => CreateNotificationsTable,
/// ]);
/// migrator.run(Default::default()).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct CreateNotificationsTable;

#[async_trait]
impl Migration for CreateNotificationsTable {
    async fn up(&self) -> Result<()> {
        Schema::create(NOTIFICATIONS_TABLE, |table| {
            table.uuid("id").primary();
            table.string("type");
            table.morphs("notifiable");
            table.text("data");
            table.timestamp("read_at").nullable();
            table.timestamps();
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists(NOTIFICATIONS_TABLE).await
    }
}
