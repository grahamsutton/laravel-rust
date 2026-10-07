//! `cache:prune-stale-tags`.

use illuminate_cache::{CacheManager, RedisStore};
use illuminate_console::{Command, Console, async_trait};
use illuminate_support::Result;

use crate::application::Application;

/// `cache:prune-stale-tags` — Prune stale cache tags from the cache (Redis
/// only): the items orphaned when a tag was flushed, and the records of
/// namespaces that no longer hold any items.
pub struct PruneStaleTagsCommand;

#[async_trait]
impl Command for PruneStaleTagsCommand {
    fn signature(&self) -> &str {
        "cache:prune-stale-tags {store? : The name of the store you would like to prune tags from}"
    }

    fn description(&self) -> &str {
        "Prune stale cache tags from the cache (Redis only)"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let manager = Application::current().make::<CacheManager>();
        let store = cmd.argument("store").filter(|store| !store.is_empty());
        let cache = manager.driver(store.as_deref())?;

        if let Some(redis) = RedisStore::of(&cache.get_store()) {
            redis.flush_stale_tags().await?;
        }

        cmd.components().info("Stale cache tags pruned successfully.");
        Ok(())
    }
}
