//! The `make:*` Artisan commands.

use std::sync::Arc;

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Str};

use super::stubs;
use super::{QualifiedName, Registration, append_lines, generate, populate, relative};
use crate::application::Application;
use crate::inspiring::Inspiring;

/// Builds a generated file's contents from the command input and name.
pub type StubBuilder = Arc<dyn Fn(&Console, &QualifiedName) -> String + Send + Sync>;

/// Runs after a file has been generated (extra registrations, follow-ups).
pub type AfterGenerate = Arc<dyn Fn(&Console, &QualifiedName, &std::path::Path) -> Result<()> + Send + Sync>;

/// Transforms a parsed name before generating.
type Renamer = Arc<dyn Fn(&QualifiedName) -> QualifiedName + Send + Sync>;

/// A `make:*` command.
pub struct MakeCommand {
    signature: String,
    description: String,
    kind: String,
    base_dir: String,
    extension: String,
    registration: Registration,
    build: StubBuilder,
    after: Option<AfterGenerate>,
    file_name: Option<Renamer>,
}

impl MakeCommand {
    /// Create a generator command, e.g. `make:controller` for a "Controller".
    pub fn new(
        name: &str,
        kind: &str,
        description: &str,
        base_dir: &str,
        registration: Registration,
        build: impl Fn(&Console, &QualifiedName) -> String + Send + Sync + 'static,
    ) -> Self {
        let lower = kind.to_lowercase();
        Self {
            signature: format!(
                "{name} {{name : The name of the {lower}}} {{--f|force : Create the {lower} even if it already exists}}"
            ),
            description: description.to_string(),
            kind: kind.to_string(),
            base_dir: base_dir.to_string(),
            extension: "rs".to_string(),
            registration,
            build: Arc::new(build),
            after: None,
            file_name: None,
        }
    }

    /// Add options to the signature (`"{--r|resource : ...}"`).
    pub fn options(mut self, options: &str) -> Self {
        self.signature.push(' ');
        self.signature.push_str(options);
        self
    }

    /// Use a different file extension.
    pub fn extension(mut self, extension: &str) -> Self {
        self.extension = extension.to_string();
        self
    }

    /// Run a callback after the file is generated.
    pub fn after(
        mut self,
        callback: impl Fn(&Console, &QualifiedName, &std::path::Path) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.after = Some(Arc::new(callback));
        self
    }

    /// Transform the parsed name before generating (e.g. view dot names).
    pub fn rename(mut self, callback: impl Fn(&QualifiedName) -> QualifiedName + Send + Sync + 'static) -> Self {
        self.file_name = Some(Arc::new(callback));
        self
    }
}

#[async_trait]
impl Command for MakeCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let raw = cmd.argument("name").unwrap_or_default();
        let mut name = QualifiedName::parse(&raw);
        if let Some(rename) = &self.file_name {
            name = rename(&name);
        }
        if name.class.is_empty() {
            return cmd.fail(format!("The name of the {} is required.", self.kind.to_lowercase()));
        }

        let contents = (self.build)(&cmd, &name);
        match generate(
            &app,
            &self.base_dir,
            &name,
            &self.extension,
            &contents,
            self.registration,
            cmd.option_bool("force"),
        ) {
            Ok(path) => {
                if let Some(after) = &self.after {
                    after(&cmd, &name, &path)?;
                }
                cmd.components().info(format!(
                    "{} [{}] created successfully.",
                    self.kind,
                    relative(&app, &path)
                ));
                Ok(())
            }
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error(format!("{} already exists.", self.kind));
                cmd.exit(1)
            }
            Err(error) => Err(error),
        }
    }
}

fn class(name: &QualifiedName) -> String {
    name.class.clone()
}

/// Every built-in generator.
pub fn all() -> Vec<MakeCommand> {
    vec![
        MakeCommand::new(
            "make:controller",
            "Controller",
            "Create a new controller class",
            "app/http/controllers",
            Registration::ModuleAndExport,
            |cmd, name| {
                let stub = if cmd.option_bool("api") {
                    "controller.api.stub"
                } else if cmd.option_bool("resource") {
                    "controller.stub"
                } else if cmd.option_bool("invokable") {
                    "controller.invokable.stub"
                } else {
                    "controller.plain.stub"
                };
                populate(&stubs::get(stub), &[("class", &class(name))])
            },
        )
        .options(
            "{--api : Exclude the create and edit methods from the controller}
             {--i|invokable : Generate a single method, invokable controller class}
             {--r|resource : Generate a resource controller class}",
        ),
        MakeCommand::new(
            "make:seeder",
            "Seeder",
            "Create a new seeder class",
            "database/seeders",
            Registration::Discovered,
            |_, name| populate(&stubs::get("seeder.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:job",
            "Job",
            "Create a new job class",
            "app/jobs",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("job.queued.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:middleware",
            "Middleware",
            "Create a new HTTP middleware class",
            "app/http/middleware",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("middleware.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:command",
            "Console command",
            "Create a new Artisan command",
            "app/console/commands",
            Registration::Discovered,
            |cmd, name| {
                let command = cmd.option("command").unwrap_or_else(|| {
                    let base = name.class.strip_suffix("Command").unwrap_or(&name.class);
                    format!("app:{}", Str::kebab(base))
                });
                populate(&stubs::get("console.stub"), &[("class", &class(name)), ("command", &command)])
            },
        )
        .options("{--command= : The terminal command that will be used to invoke the class}"),
        MakeCommand::new(
            "make:provider",
            "Provider",
            "Create a new service provider class",
            "app/providers",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("provider.stub"), &[("class", &class(name))]),
        )
        .after(|_, name, _| register_provider(name)),
        MakeCommand::new(
            "make:request",
            "Request",
            "Create a new form request class",
            "app/http/requests",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("request.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:rule",
            "Rule",
            "Create a new validation rule",
            "app/rules",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("rule.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:event",
            "Event",
            "Create a new event class",
            "app/events",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("event.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:listener",
            "Listener",
            "Create a new event listener class",
            "app/listeners",
            Registration::ModuleAndExport,
            |cmd, name| match cmd.option("event") {
                Some(event) => populate(
                    &stubs::get("listener.typed.stub"),
                    &[("class", &class(name)), ("event", &Str::studly(&event))],
                ),
                None => populate(&stubs::get("listener.stub"), &[("class", &class(name))]),
            },
        )
        .options("{--e|event= : The event class being listened for}"),
        MakeCommand::new(
            "make:exception",
            "Exception",
            "Create a new custom exception class",
            "app/exceptions",
            Registration::ModuleAndExport,
            |_, name| {
                let message = Str::ucfirst(&Str::snake_with(&name.class, " ").replace(" exception", ""));
                populate(&stubs::get("exception.stub"), &[("class", &class(name)), ("message", &format!("{message}."))])
            },
        ),
        MakeCommand::new(
            "make:test",
            "Test",
            "Create a new test class",
            "tests/feature",
            Registration::None,
            |cmd, _| {
                if cmd.option_bool("unit") {
                    stubs::get("test.unit.stub")
                } else {
                    stubs::get("test.stub")
                }
            },
        )
        .options("{--u|unit : Create a unit test}")
        .after(|cmd, name, path| {
            // Unit tests live in tests/unit; move the file if needed.
            let app = Application::current();
            let dir = if cmd.option_bool("unit") { "tests/unit" } else { "tests/feature" };
            let target = std::path::PathBuf::from(app.base_path(dir)).join(path.file_name().unwrap_or_default());
            if target != path {
                std::fs::create_dir_all(target.parent().unwrap_or(path))?;
                std::fs::rename(path, &target)?;
            }
            let main = std::path::PathBuf::from(app.base_path(dir)).join("main.rs");
            append_lines(&main, &[format!("mod {};", name.file_stem())])
        }),
        MakeCommand::new(
            "make:view",
            "View",
            "Create a new view",
            "resources/views",
            Registration::None,
            |_, _| populate(&stubs::get("view.stub"), &[("quote", &Inspiring::quote())]),
        )
        .extension("blade.html")
        .rename(|name| {
            // `users.index` => resources/views/users/index.blade.html
            let raw: Vec<String> = name
                .namespace
                .iter()
                .cloned()
                .chain(std::iter::once(name.class.clone()))
                .collect::<Vec<_>>()
                .join(".")
                .split('.')
                .map(Str::kebab)
                .collect();
            let mut segments = raw;
            let class = segments.pop().unwrap_or_default();
            QualifiedName { namespace: segments, class }
        }),
        MakeCommand::new(
            "make:scope",
            "Scope",
            "Create a new scope class",
            "app/models/scopes",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("scope.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:channel",
            "Channel",
            "Create a new channel class",
            "app/broadcasting",
            Registration::ModuleAndExport,
            |_, name| {
                let has_user = std::path::Path::new(&Application::current().base_path("app/models/user.rs")).exists();
                let (import, user) = if has_user {
                    ("use crate::app::models::User;", "User")
                } else {
                    ("use laravel::prelude::*;", "AuthUser")
                };
                populate(&stubs::get("channel.stub"), &[("class", &class(name)), ("import", import), ("user", user)])
            },
        ),
        MakeCommand::new(
            "make:job-middleware",
            "Job middleware",
            "Create a new job middleware class",
            "app/jobs/middleware",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("job.middleware.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:config",
            "Config",
            "Create a new configuration file",
            "config",
            Registration::None,
            |_, _| stubs::get("config.stub"),
        )
        .rename(|name| QualifiedName { namespace: Vec::new(), class: Str::snake(&name.class) })
        .after(|_, name, _| {
            let app = Application::current();
            crate::console::commands::install::register_config(&app.config_path("mod.rs"), &name.class)
        }),
        MakeCommand::new(
            "make:class",
            "Class",
            "Create a new class",
            "app",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("class.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:enum",
            "Enum",
            "Create a new enum",
            "app",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("enum.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:trait",
            "Trait",
            "Create a new trait",
            "app",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("trait.stub"), &[("class", &class(name))]),
        ),
        MakeCommand::new(
            "make:interface",
            "Interface",
            "Create a new interface (a Rust trait)",
            "app",
            Registration::ModuleAndExport,
            |_, name| populate(&stubs::get("interface.stub"), &[("class", &class(name))]),
        ),
    ]
}

/// Add a provider to `bootstrap/providers.rs`.
fn register_provider(name: &QualifiedName) -> Result<()> {
    let app = Application::current();
    let path = std::path::PathBuf::from(app.bootstrap_path("providers.rs"));
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let module = if name.namespace.is_empty() {
        format!("crate::app::providers::{}", name.class)
    } else {
        format!("crate::app::providers::{}::{}", name.namespace.join("::"), name.class)
    };
    let entry = format!("Box::new({module}),");
    if contents.contains(&entry) {
        return Ok(());
    }
    if let Some(position) = contents.rfind(']') {
        let (before, after) = contents.split_at(position);
        let before = before.trim_end();
        let separator = if before.ends_with('[') || before.ends_with(',') { "" } else { "," };
        let updated = format!("{before}{separator}\n        {entry}\n    {after}");
        std::fs::write(&path, updated)?;
    }
    Ok(())
}
