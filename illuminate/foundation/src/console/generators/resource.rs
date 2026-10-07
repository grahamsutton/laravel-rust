//! `make:resource` — an Eloquent API resource or resource collection.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Str};

use super::{QualifiedName, Registration, generate, populate, relative};
use crate::application::Application;

const RESOURCE: &str = r#"use laravel::prelude::*;
{{ import }}
pub struct {{ class }}(pub {{ model }});

impl JsonResource for {{ class }} {
    type Model = {{ model }};

    fn from_model(model: {{ model }}) -> Self {
        Self(model)
    }

    fn model(&self) -> &{{ model }} {
        &self.0
    }

    /// Transform the resource into an array.
    fn to_array(&self, _request: &Request) -> Value {
        to_value(&self.0)
    }
}
"#;

const COLLECTION: &str = r#"use laravel::prelude::*;

use super::{{ collects }};

pub struct {{ class }}(pub Resources<{{ collects }}>);

impl ResourceCollection for {{ class }} {
    type Collects = {{ collects }};

    fn from_collection(collection: Resources<{{ collects }}>) -> Self {
        Self(collection)
    }

    fn collection(&self) -> &Resources<{{ collects }}> {
        &self.0
    }

    /// Transform the resource collection into an array.
    fn to_array(&self, _request: &Request) -> Value {
        to_value(&self.0)
    }
}
"#;

/// `make:resource` — Create a new resource.
pub struct MakeResourceCommand;

#[async_trait]
impl Command for MakeResourceCommand {
    fn signature(&self) -> &str {
        "make:resource
            {name : The name of the resource}
            {--f|force : Create the class even if the resource already exists}
            {--c|collection : Create a resource collection}
            {--m|model= : The model the resource transforms}"
    }

    fn description(&self) -> &str {
        "Create a new resource"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let name = QualifiedName::parse(&cmd.argument("name").unwrap_or_default());
        if name.class.is_empty() {
            return cmd.fail("The name of the resource is required.");
        }
        let collection = cmd.option_bool("collection") || name.class.ends_with("Collection");

        let contents = if collection {
            // `UserCollection` collects `UserResource`.
            let base = name.class.strip_suffix("Collection").unwrap_or(&name.class);
            let collects = format!("{base}Resource");
            populate(COLLECTION, &[("class", &name.class), ("collects", &collects)])
        } else {
            let model = cmd
                .option("model")
                .filter(|model| !model.is_empty())
                .map(|model| Str::studly(&model))
                .unwrap_or_else(|| name.class.strip_suffix("Resource").unwrap_or(&name.class).to_string());
            // Point at the model when it exists; otherwise transform raw values.
            let model_file = app.base_path(&format!("app/models/{}.rs", Str::snake(&model)));
            if std::path::Path::new(&model_file).exists() {
                let import = format!("\nuse crate::app::models::{model};\n");
                populate(RESOURCE, &[("class", &name.class), ("model", &model), ("import", &import)])
            } else {
                populate(RESOURCE, &[("class", &name.class), ("model", "Value"), ("import", "")])
            }
        };

        match generate(
            &app,
            "app/http/resources",
            &name,
            "rs",
            &contents,
            Registration::ModuleAndExport,
            cmd.option_bool("force"),
        ) {
            Ok(path) => {
                cmd.components()
                    .info(format!("Resource [{}] created successfully.", relative(&app, &path)));
                Ok(())
            }
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error("Resource already exists.");
                cmd.exit(1)
            }
            Err(error) => Err(error),
        }
    }
}
