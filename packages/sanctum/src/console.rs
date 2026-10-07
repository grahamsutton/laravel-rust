//! `sanctum:prune-expired`: delete tokens that expired a while ago.

use illuminate_console::{Command, Console, async_trait};
use illuminate_database::eloquent::Model;
use illuminate_support::{Carbon, Result};

use crate::config;
use crate::personal_access_token::PersonalAccessToken;

/// Prune tokens that have been expired for more than the given number of
/// hours — both tokens past their own `expires_at` and, when
/// `sanctum.expiration` is configured, tokens older than that.
///
/// ```ignore
/// Schedule::command("sanctum:prune-expired --hours=24").daily();
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct PruneExpired;

impl PruneExpired {
    /// Delete tokens whose `expires_at` passed more than `hours` ago,
    /// returning how many were deleted.
    pub async fn prune_expired_tokens(hours: i64) -> Result<u64> {
        let cutoff = Carbon::now().sub_hours(hours).to_date_time_string();
        PersonalAccessToken::where_op("expires_at", "<", cutoff)
            .delete()
            .await
    }

    /// Delete tokens created more than `expiration` minutes plus `hours`
    /// ago, returning how many were deleted.
    pub async fn prune_tokens_older_than(expiration: f64, hours: i64) -> Result<u64> {
        let seconds = ((expiration + (hours as f64) * 60.0) * 60.0).round() as i64;
        let cutoff = Carbon::now().sub_seconds(seconds).to_date_time_string();
        PersonalAccessToken::where_op("created_at", "<", cutoff)
            .delete()
            .await
    }
}

#[async_trait]
impl Command for PruneExpired {
    fn signature(&self) -> &str {
        "sanctum:prune-expired {--hours=24 : The number of hours to retain expired Sanctum tokens}"
    }

    fn description(&self) -> &str {
        "Prune tokens expired for more than specified number of hours"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let option = cmd.option("hours").unwrap_or_else(|| "24".into());
        let Ok(hours) = option.trim().parse::<i64>() else {
            return cmd.fail(format!(
                "The [hours] option must be a whole number, [{option}] given."
            ));
        };
        let components = cmd.components();

        components
            .task(
                "Pruning tokens with expired expires_at timestamps",
                || async { Self::prune_expired_tokens(hours).await.map(|_| ()) },
            )
            .await?;

        match config::expiration() {
            Some(expiration) => {
                components
                    .task(
                        "Pruning tokens with expired expiration value based on configuration file",
                        || async {
                            Self::prune_tokens_older_than(expiration, hours)
                                .await
                                .map(|_| ())
                        },
                    )
                    .await?;
            }
            None => components.warn("Expiration value not specified in configuration file."),
        }

        components.info(format!(
            "Tokens expired for more than [{hours} hours] pruned successfully."
        ));
        Ok(())
    }
}
