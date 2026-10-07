//! `make:model` and its companions: factories, observers, and policies.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Str};

use super::commands::MakeCommand;
use super::{QualifiedName, Registration, generate, populate, relative, stubs};
use crate::application::Application;

/// `make:model` — Create a new Eloquent model class.
pub struct MakeModelCommand;

#[async_trait]
impl Command for MakeModelCommand {
    fn signature(&self) -> &str {
        "make:model
            {name : The name of the model}
            {--a|all : Generate a migration, seeder, factory, policy, and resource controller for the model}
            {--c|controller : Create a new controller for the model}
            {--f|factory : Create a new factory for the model}
            {--force : Create the class even if the model already exists}
            {--m|migration : Create a new migration file for the model}
            {--policy : Create a new policy for the model}
            {--s|seed : Create a new seeder for the model}
            {--r|resource : Indicates if the generated controller should be a resource controller}
            {--api : Indicates if the generated controller should be an API resource controller}"
    }

    fn description(&self) -> &str {
        "Create a new Eloquent model class"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let name = QualifiedName::parse(&cmd.argument("name").unwrap_or_default());
        if name.class.is_empty() {
            return cmd.fail("The name of the model is required.");
        }
        let model = name.class.clone();
        let all = cmd.option_bool("all");
        let factory = all || cmd.option_bool("factory");

        let (factory_import, factory_attribute) = if factory {
            (
                format!("\nuse crate::database::factories::{model}Factory;\n"),
                format!("\n#[use_factory({model}Factory)]"),
            )
        } else {
            (String::new(), String::new())
        };
        let contents = populate(
            stubs::MODEL,
            &[
                ("class", &model),
                ("factory_import", &factory_import),
                ("factory_attribute", &factory_attribute),
            ],
        );

        match generate(&app, "app/models", &name, "rs", &contents, Registration::ModuleAndExport, cmd.option_bool("force")) {
            Ok(path) => cmd
                .components()
                .info(format!("Model [{}] created successfully.", relative(&app, &path))),
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error("Model already exists.");
                return cmd.exit(1);
            }
            Err(error) => return Err(error),
        }

        if factory {
            cmd.call("make:factory", vec![format!("{model}Factory"), format!("--model={model}")]).await?;
        }
        if all || cmd.option_bool("migration") {
            let table = Str::snake(&Str::plural_studly(&model));
            cmd.call("make:migration", vec![format!("create_{table}_table"), format!("--create={table}")]).await?;
        }
        if all || cmd.option_bool("seed") {
            cmd.call("make:seeder", vec![format!("{model}Seeder")]).await?;
        }
        if all || cmd.option_bool("controller") || cmd.option_bool("resource") || cmd.option_bool("api") {
            let mut args = vec![format!("{model}Controller")];
            if cmd.option_bool("api") {
                args.push("--api".into());
            } else if all || cmd.option_bool("resource") {
                args.push("--resource".into());
            }
            cmd.call("make:controller", args).await?;
        }
        if all || cmd.option_bool("policy") {
            cmd.call("make:policy", vec![format!("{model}Policy"), format!("--model={model}")]).await?;
        }
        Ok(())
    }
}

/// The model a generated class is for: `--model=Post`, or guessed from the
/// class name (`PostFactory` => `Post`).
fn model_for(cmd: &Console, name: &QualifiedName, suffix: &str) -> String {
    cmd.option("model")
        .filter(|model| !model.is_empty())
        .map(|model| Str::studly(&model))
        .unwrap_or_else(|| name.class.strip_suffix(suffix).unwrap_or(&name.class).to_string())
}

/// `make:factory`, `make:observer`, and `make:policy`.
pub fn commands() -> Vec<MakeCommand> {
    vec![
        MakeCommand::new(
            "make:factory",
            "Factory",
            "Create a new model factory",
            "database/factories",
            Registration::ModuleAndExport,
            |cmd, name| {
                let model = model_for(cmd, name, "Factory");
                populate(stubs::FACTORY, &[("class", &name.class), ("model", &model)])
            },
        )
        .options("{--m|model= : The name of the model}"),
        MakeCommand::new(
            "make:observer",
            "Observer",
            "Create a new observer class",
            "app/observers",
            Registration::ModuleAndExport,
            |cmd, name| {
                let model = model_for(cmd, name, "Observer");
                let variable = Str::snake(&model);
                populate(stubs::OBSERVER, &[("class", &name.class), ("model", &model), ("variable", &variable)])
            },
        )
        .options("{--m|model= : The model that the observer applies to}"),
        MakeCommand::new(
            "make:policy",
            "Policy",
            "Create a new policy class",
            "app/policies",
            Registration::Discovered,
            |cmd, name| {
                let model = model_for(cmd, name, "Policy");
                let variable = Str::snake(&model);
                let imports = if model == "User" { "User".to_string() } else { format!("{model}, User") };
                populate(
                    stubs::POLICY,
                    &[("class", &name.class), ("model", &model), ("variable", &variable), ("imports", &imports)],
                )
            },
        )
        .options("{--m|model= : The model that the policy applies to}"),
    ]
}
