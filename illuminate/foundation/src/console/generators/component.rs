//! `make:component` — a Blade component class and its view.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Str};

use super::{QualifiedName, Registration, generate, populate, relative, stubs};
use crate::application::Application;
use crate::inspiring::Inspiring;

/// `make:component` — Create a new view component class.
pub struct MakeComponentCommand;

#[async_trait]
impl Command for MakeComponentCommand {
    fn signature(&self) -> &str {
        "make:component
            {name : The name of the component}
            {--inline : Create a component that renders an inline view}
            {--view : Create an anonymous component with only a view}
            {--f|force : Create the class even if the component already exists}"
    }

    fn description(&self) -> &str {
        "Create a new view component class"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let raw = cmd.argument("name").unwrap_or_default();
        let name = QualifiedName::parse(&raw);
        if name.class.is_empty() {
            return cmd.fail("The name of the component is required.");
        }
        let force = cmd.option_bool("force");

        // `Forms/Input` => components/forms/input.blade.html, <x-forms.input>
        let view_segments: Vec<String> = name
            .namespace
            .iter()
            .map(|segment| Str::kebab(segment))
            .chain(std::iter::once(Str::kebab(&name.class)))
            .collect();
        let view_name = view_segments.join(".");
        let view_file = QualifiedName {
            namespace: std::iter::once("components".to_string())
                .chain(view_segments[..view_segments.len() - 1].iter().cloned())
                .collect(),
            class: view_segments[view_segments.len() - 1].clone(),
        };
        let quote = Inspiring::quote();
        let template = populate(stubs::VIEW, &[("quote", &quote)]);

        if cmd.option_bool("view") {
            return self.write_view(&cmd, &app, &view_file, &template, force);
        }

        if !name.namespace.is_empty() {
            return cmd.fail("Component classes are discovered from app/view/components; nested namespaces aren't supported. Use --view for nested anonymous components.");
        }

        let render = if cmd.option_bool("inline") {
            format!("ComponentView::inline(r#\"{}\"#)", template.trim_end())
        } else {
            format!("ComponentView::view(\"components.{view_name}\")")
        };
        let contents = populate(stubs::COMPONENT, &[("class", &name.class), ("view", &render)]);

        match generate(&app, "app/view/components", &name, "rs", &contents, Registration::Discovered, force) {
            Ok(path) => {
                cmd.components()
                    .info(format!("Component [{}] created successfully.", relative(&app, &path)));
            }
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error("Component already exists.");
                return cmd.exit(1);
            }
            Err(error) => return Err(error),
        }

        if cmd.option_bool("inline") {
            return Ok(());
        }
        self.write_view(&cmd, &app, &view_file, &template, force)
    }
}

impl MakeComponentCommand {
    fn write_view(
        &self,
        cmd: &Console,
        app: &Application,
        view: &QualifiedName,
        template: &str,
        force: bool,
    ) -> Result<()> {
        match generate(app, "resources/views", view, "blade.html", template, Registration::None, force) {
            Ok(path) => {
                cmd.components()
                    .info(format!("View [{}] created successfully.", relative(app, &path)));
                Ok(())
            }
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error("View already exists.");
                cmd.exit(1)
            }
            Err(error) => Err(error),
        }
    }
}
