//! `model:show` and `model:prune`.

use std::sync::{Arc, Mutex};

use illuminate_console::{Command, Console, async_trait};
use illuminate_database::eloquent::registry;
use illuminate_support::Result;

/// `model:show` — Show information about an Eloquent model.
pub struct ShowModelCommand;

#[async_trait]
impl Command for ShowModelCommand {
    fn signature(&self) -> &str {
        "model:show
            {model : The model to show}
            {--database= : The database connection to use}
            {--json : Output the model as JSON}"
    }

    fn description(&self) -> &str {
        "Show information about an Eloquent model"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let name = cmd.argument("model").unwrap_or_default();
        let Some(model) = registry::find(&name) else {
            cmd.components().error(format!("Model [{name}] not found."));
            return cmd.exit(1);
        };
        let database = cmd.option("database").filter(|database| !database.is_empty());
        let info = model.inspect(database.as_deref()).await?;

        if cmd.option_bool("json") {
            cmd.line(serde_json::to_string(&info)?);
            return Ok(());
        }

        let components = cmd.components();
        cmd.new_line(1);
        components.two_column_detail(format!("<fg=green;options=bold>{}</>", info.class), "");
        components.two_column_detail("Database", &info.database);
        components.two_column_detail("Table", &info.table);
        if let Some(policy) = &info.policy {
            components.two_column_detail("Policy", policy);
        }
        cmd.new_line(1);

        components.two_column_detail(
            "<fg=green;options=bold>Attributes</>",
            "type <fg=gray>/</> <fg=yellow;options=bold>cast</>",
        );
        for attribute in &info.attributes {
            let properties: Vec<String> = [
                ("increments", attribute.increments),
                ("unique", attribute.unique.unwrap_or(false)),
                ("nullable", attribute.nullable.unwrap_or(false)),
                ("fillable", attribute.fillable),
                ("hidden", attribute.hidden),
                ("appended", attribute.appended.unwrap_or(false)),
            ]
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(property, _)| format!("<fg=gray>{property}</>"))
            .collect();
            let first = format!("{} {}", attribute.name, properties.join("<fg=gray>,</> "));
            let second: Vec<String> = [
                attribute.type_.clone(),
                attribute.cast.as_ref().map(|cast| format!("<fg=yellow;options=bold>{cast}</>")),
            ]
            .into_iter()
            .flatten()
            .collect();
            components.two_column_detail(first.trim(), second.join(" <fg=gray>/</> "));
            if cmd.is_verbose()
                && let Some(default) = &attribute.default
            {
                components.bullet_list([format!("default: {default}")]);
            }
        }
        cmd.new_line(1);

        components.two_column_detail("<fg=green;options=bold>Relations</>", "");
        for relation in &info.relations {
            components.two_column_detail(
                format!("{} <fg=gray>{}</>", relation.name, relation.type_),
                &relation.related,
            );
        }
        cmd.new_line(1);

        components.two_column_detail("<fg=green;options=bold>Observers</>", "");
        for observer in &info.observers {
            components.two_column_detail(&observer.event, observer.observer.join(", "));
        }
        cmd.new_line(1);
        Ok(())
    }
}

/// `model:prune` — Prune models that are no longer needed.
pub struct PruneCommand;

#[async_trait]
impl Command for PruneCommand {
    fn signature(&self) -> &str {
        "model:prune
            {--model=* : Class names of the models to be pruned}
            {--except=* : Class names of the models to be excluded from pruning}
            {--chunk=1000 : The number of models to retrieve per chunk of models to be deleted}
            {--pretend : Display the number of prunable records found instead of deleting them}"
    }

    fn description(&self) -> &str {
        "Prune models that are no longer needed"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let models = match registry::models_to_prune(&cmd.option_list("model"), &cmd.option_list("except")) {
            Ok(models) => models,
            Err(error) => return cmd.fail(error.to_string()),
        };
        if models.is_empty() {
            cmd.components().info("No prunable models found.");
            return Ok(());
        }

        if cmd.option_bool("pretend") {
            for model in models {
                let count = model.pretend_to_prune().await?;
                let name = model.class_name();
                if count == 0 {
                    cmd.components().info(format!("No prunable [{name}] records found."));
                } else {
                    cmd.components().info(format!("{count} [{name}] records will be pruned."));
                }
            }
            return Ok(());
        }

        let chunk = cmd.option("chunk").and_then(|chunk| chunk.parse().ok()).unwrap_or(1000);
        for model in models {
            let name = model.class_name();
            let started = Arc::new(Mutex::new(false));
            let output = cmd.clone();
            let total = model
                .prune_with_progress(chunk, move |count| {
                    let mut started = started.lock().unwrap();
                    if !*started {
                        *started = true;
                        output.new_line(1);
                        output.components().info(format!("Pruning [{name}] records."));
                    }
                    output.components().two_column_detail(name, format!("{count} records"));
                })
                .await?;
            if total == 0 {
                cmd.components().info(format!("No prunable [{name}] records found."));
            }
        }
        Ok(())
    }
}
