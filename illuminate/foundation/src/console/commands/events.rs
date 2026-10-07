//! `event:list` and `cache:forget`.

use illuminate_console::{Command, Console, async_trait};
use illuminate_events::Event;
use illuminate_support::{Result, json};

/// `event:list` — List the application's events and listeners.
pub struct EventListCommand;

#[async_trait]
impl Command for EventListCommand {
    fn signature(&self) -> &str {
        "event:list
            {--event= : Filter the events by name}
            {--json : Output the events and listeners as JSON}"
    }

    fn description(&self) -> &str {
        "List the application's events and listeners"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let filter = cmd.option("event").filter(|event| !event.is_empty());
        let mut events: Vec<(String, Vec<&'static str>)> = Event::dispatcher()
            .get_raw_listeners()
            .into_iter()
            .filter(|(_, listeners)| !listeners.is_empty())
            .filter(|(event, _)| {
                filter
                    .as_ref()
                    .is_none_or(|filter| event.to_lowercase().contains(&filter.to_lowercase()))
            })
            .collect();
        events.sort_by(|(a, _), (b, _)| a.cmp(b));

        if cmd.option_bool("json") {
            let data: Vec<_> = events
                .iter()
                .map(|(event, listeners)| json!({"event": event, "listeners": listeners}))
                .collect();
            cmd.line(serde_json::to_string(&data)?);
            return Ok(());
        }

        if events.is_empty() {
            cmd.components()
                .info("Your application doesn't have any events matching the given criteria.");
            return Ok(());
        }

        cmd.new_line(1);
        for (event, listeners) in &events {
            cmd.components().two_column_detail(event, "");
            cmd.components().bullet_list(listeners.iter());
        }
        cmd.new_line(1);
        Ok(())
    }
}

/// `cache:forget` — Remove an item from the cache.
pub struct CacheForgetCommand;

#[async_trait]
impl Command for CacheForgetCommand {
    fn signature(&self) -> &str {
        "cache:forget
            {key : The key to remove}
            {store? : The store to remove the key from}"
    }

    fn description(&self) -> &str {
        "Remove an item from the cache"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let key = cmd.argument("key").unwrap_or_default();
        let store = match cmd.argument("store").filter(|store| !store.is_empty()) {
            Some(name) => illuminate_cache::Cache::store(&name)?,
            None => illuminate_cache::Cache::default_store()?,
        };
        store.forget(&key).await?;
        cmd.components()
            .info(format!("The [{key}] key has been removed from the cache."));
        Ok(())
    }
}
