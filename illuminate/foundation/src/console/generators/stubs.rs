//! The stubs used by the `make:*` commands.
//!
//! Placeholders: `{{ class }}`, `{{ command }}`, `{{ event }}`, `{{ model }}`,
//! `{{ table }}`, `{{ view }}`.
//!
//! Like Laravel, every generator prefers your application's own copy of a
//! stub: run `cargo artisan stub:publish` to copy them all into `stubs/`,
//! then change them however you like.

use crate::application::Application;

/// Every stub the generators use, by the file name `stub:publish` gives it
/// in your application's `stubs` directory (Laravel's names).
pub const ALL: &[(&str, &str)] = &[
    ("channel.stub", CHANNEL),
    ("class.stub", CLASS),
    ("config.stub", CONFIG),
    ("console.stub", COMMAND),
    ("controller.api.stub", CONTROLLER_API),
    ("controller.invokable.stub", CONTROLLER_INVOKABLE),
    ("controller.plain.stub", CONTROLLER),
    ("controller.stub", CONTROLLER_RESOURCE),
    ("enum.stub", ENUM),
    ("event.stub", EVENT),
    ("exception.stub", EXCEPTION),
    ("factory.stub", FACTORY),
    ("interface.stub", TRAIT),
    ("job.middleware.stub", JOB_MIDDLEWARE),
    ("job.queued.stub", JOB),
    ("listener.stub", LISTENER_PLAIN),
    ("listener.typed.stub", LISTENER),
    ("mail.stub", super::messaging::MAIL),
    ("markdown-mail.stub", super::messaging::MARKDOWN),
    ("markdown-notification.stub", super::messaging::MARKDOWN),
    ("middleware.stub", MIDDLEWARE),
    ("migration.create.stub", MIGRATION_CREATE),
    ("migration.stub", MIGRATION),
    ("migration.update.stub", MIGRATION_UPDATE),
    ("model.stub", MODEL),
    ("notification.stub", super::messaging::NOTIFICATION),
    ("observer.stub", OBSERVER),
    ("policy.stub", POLICY),
    ("provider.stub", PROVIDER),
    ("request.stub", REQUEST),
    ("resource-collection.stub", super::resource::COLLECTION),
    ("resource.stub", super::resource::RESOURCE),
    ("rule.stub", RULE),
    ("scope.stub", SCOPE),
    ("seeder.stub", SEEDER),
    ("test.stub", TEST_FEATURE),
    ("test.unit.stub", TEST_UNIT),
    ("trait.stub", TRAIT),
    ("view-component.stub", COMPONENT),
    ("view.stub", VIEW),
];

/// The framework's own copy of a stub.
pub fn default(name: &str) -> Option<&'static str> {
    ALL.iter().find(|(stub, _)| *stub == name).map(|(_, contents)| *contents)
}

/// The stub with the given name: your application's customized
/// `stubs/{name}` when it exists, otherwise the framework's own.
pub fn get(name: &str) -> String {
    if let Some(app) = Application::try_current()
        && let Ok(custom) = std::fs::read_to_string(app.base_path(&format!("stubs/{name}")))
    {
        return custom;
    }
    default(name).unwrap_or_default().to_string()
}

pub const CONTROLLER: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

impl {{ class }} {
    //
}
"#;

pub const CONTROLLER_INVOKABLE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

impl {{ class }} {
    /// Handle the incoming request.
    pub async fn invoke(request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }
}
"#;

pub const CONTROLLER_RESOURCE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl ResourceController for {{ class }} {
    /// Display a listing of the resource.
    async fn index(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Show the form for creating a new resource.
    async fn create(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Store a newly created resource in storage.
    async fn store(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Display the specified resource.
    async fn show(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Show the form for editing the specified resource.
    async fn edit(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Update the specified resource in storage.
    async fn update(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Remove the specified resource from storage.
    async fn destroy(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }
}
"#;

pub const CONTROLLER_API: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl ResourceController for {{ class }} {
    /// Display a listing of the resource.
    async fn index(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Store a newly created resource in storage.
    async fn store(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Display the specified resource.
    async fn show(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Update the specified resource in storage.
    async fn update(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }

    /// Remove the specified resource from storage.
    async fn destroy(&self, request: Request) -> Result<Response> {
        let _ = request;

        Ok(Response::no_content())
    }
}
"#;

pub const MIDDLEWARE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Middleware for {{ class }} {
    /// Handle an incoming request.
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        Ok(next.run(request).await)
    }
}
"#;

pub const COMMAND: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Command for {{ class }} {
    /// The name and signature of the console command.
    fn signature(&self) -> &str {
        "{{ command }}"
    }

    /// The console command description.
    fn description(&self) -> &str {
        "Command description"
    }

    /// Execute the console command.
    async fn handle(&self, cmd: Console) -> Result<()> {
        let _ = cmd;

        Ok(())
    }
}
"#;

pub const PROVIDER: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

impl ServiceProvider for {{ class }} {
    /// Register services.
    fn register(&self, app: &Container) {
        let _ = app;
    }

    /// Bootstrap services.
    fn boot(&self, app: &Container) {
        let _ = app;
    }
}
"#;

pub const REQUEST: &str = r#"use laravel::prelude::*;

#[derive(Default)]
pub struct {{ class }};

#[async_trait]
impl FormRequest for {{ class }} {
    /// Determine if the user is authorized to make this request.
    async fn authorize(&self, _request: &Request) -> bool {
        false
    }

    /// Get the validation rules that apply to the request.
    fn rules(&self, _request: &Request) -> Rules {
        rules! {
            //
        }
    }
}
"#;

pub const RULE: &str = r#"use laravel::prelude::*;
use laravel::validation::FailCallback;

pub struct {{ class }};

#[async_trait]
impl ValidationRule for {{ class }} {
    /// Run the validation rule.
    async fn validate(&self, attribute: &str, value: &Value, fail: &mut FailCallback<'_>) {
        let _ = (attribute, value, fail);
    }
}
"#;

pub const EVENT: &str = r#"use laravel::prelude::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct {{ class }} {
    //
}

impl {{ class }} {
    /// Create a new event instance.
    pub fn new() -> Self {
        Self {}
    }
}
"#;

pub const LISTENER: &str = r#"use laravel::prelude::*;
use laravel::events::Listener;

use crate::app::events::{{ event }};

pub struct {{ class }};

#[async_trait]
impl Listener<{{ event }}> for {{ class }} {
    /// Handle the event.
    async fn handle(&self, event: &{{ event }}) -> Result<()> {
        let _ = event;

        Ok(())
    }
}
"#;

pub const LISTENER_PLAIN: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

impl {{ class }} {
    /// Handle the event.
    pub async fn handle(&self, event: &Value) -> Result<()> {
        let _ = event;

        Ok(())
    }
}
"#;

pub const EXCEPTION: &str = r#"use std::fmt;

#[derive(Debug)]
pub struct {{ class }};

impl fmt::Display for {{ class }} {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("{{ message }}")
    }
}

impl std::error::Error for {{ class }} {}
"#;

pub const TEST_FEATURE: &str = r#"/// A basic feature test example.
#[tokio::test]
async fn test_example() {
    let mut app = crate::app();

    app.get("/").await.assert_status(200);
}
"#;

pub const TEST_UNIT: &str = r#"/// A basic unit test example.
#[test]
fn test_that_true_is_true() {
    assert!(true);
}
"#;

pub const VIEW: &str = r#"<div>
    <!-- {{ quote }} -->
</div>
"#;

pub const CLASS: &str = r#"#[derive(Default)]
pub struct {{ class }};

impl {{ class }} {
    /// Create a new class instance.
    pub fn new() -> Self {
        Self
    }
}
"#;

pub const ENUM: &str = r#"use laravel::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum {{ class }} {
    //
}
"#;

pub const TRAIT: &str = r#"pub trait {{ class }} {
    //
}
"#;

pub const MIGRATION: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Ok(())
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Ok(())
    }
}
"#;

pub const MIGRATION_CREATE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("{{ table }}", |table| {
            table.id();
            table.timestamps();
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("{{ table }}").await
    }
}
"#;

pub const MIGRATION_UPDATE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::table("{{ table }}", |table| {
            let _ = table;
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::table("{{ table }}", |table| {
            let _ = table;
        })
        .await
    }
}
"#;

pub const SEEDER: &str = r#"use laravel::prelude::*;

#[derive(Default)]
pub struct {{ class }};

#[async_trait]
impl Seeder for {{ class }} {
    /// Run the database seeds.
    async fn run(&self) -> Result<()> {
        Ok(())
    }
}
"#;

pub const JOB: &str = r#"use laravel::prelude::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct {{ class }} {
    //
}

laravel::register_job!({{ class }});

impl {{ class }} {
    /// Create a new job instance.
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ShouldQueue for {{ class }} {
    /// Execute the job.
    async fn handle(&self) -> Result<()> {
        Ok(())
    }
}
"#;

pub const COMPONENT: &str = r#"use laravel::prelude::*;
use laravel::view::{ComponentArgs, ComponentView};

pub struct {{ class }};

impl {{ class }} {
    /// Create a new component instance.
    pub fn new(args: &mut ComponentArgs) -> Result<Self> {
        let _ = args;

        Ok(Self)
    }
}

impl Component for {{ class }} {
    /// Get the view / contents that represent the component.
    fn render(&self) -> ComponentView {
        {{ view }}
    }
}
"#;

pub const MODEL: &str = r#"use laravel::prelude::*;
{{ factory_import }}
#[derive(Debug, Clone, Default, Model)]{{ factory_attribute }}
pub struct {{ class }} {
    pub id: u64,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub original: Original,
}
"#;

pub const FACTORY: &str = r#"use laravel::prelude::*;

use crate::app::models::{{ model }};

#[derive(Default)]
pub struct {{ class }};

impl Factory for {{ class }} {
    type Model = {{ model }};

    /// Define the model's default state.
    fn definition(&self, faker: &mut Faker) -> Value {
        let _ = faker;

        json!({
            //
        })
    }
}
"#;

pub const OBSERVER: &str = r#"use laravel::prelude::*;

use crate::app::models::{{ model }};

pub struct {{ class }};

#[async_trait]
impl Observer<{{ model }}> for {{ class }} {
    /// Handle the {{ model }} "created" event.
    async fn created(&self, {{ variable }}: &mut {{ model }}) -> Result<()> {
        let _ = {{ variable }};

        Ok(())
    }

    /// Handle the {{ model }} "updated" event.
    async fn updated(&self, {{ variable }}: &mut {{ model }}) -> Result<()> {
        let _ = {{ variable }};

        Ok(())
    }

    /// Handle the {{ model }} "deleted" event.
    async fn deleted(&self, {{ variable }}: &mut {{ model }}) -> Result<()> {
        let _ = {{ variable }};

        Ok(())
    }
}
"#;

pub const POLICY: &str = r#"use laravel::prelude::*;

use crate::app::models::{{{ imports }}};

#[derive(Default)]
pub struct {{ class }};

impl Policy<{{ model }}> for {{ class }} {
    type User = User;

    /// Determine whether the user can view any models.
    fn view_any(&self, _user: &User) -> Option<AuthResponse> {
        Some(false.into())
    }

    /// Determine whether the user can view the model.
    fn view(&self, _user: &User, _{{ variable }}: &{{ model }}) -> Option<AuthResponse> {
        Some(false.into())
    }

    /// Determine whether the user can create models.
    fn create(&self, _user: &User) -> Option<AuthResponse> {
        Some(false.into())
    }

    /// Determine whether the user can update the model.
    fn update(&self, _user: &User, _{{ variable }}: &{{ model }}) -> Option<AuthResponse> {
        Some(false.into())
    }

    /// Determine whether the user can delete the model.
    fn delete(&self, _user: &User, _{{ variable }}: &{{ model }}) -> Option<AuthResponse> {
        Some(false.into())
    }

    /// Determine whether the user can restore the model.
    fn restore(&self, _user: &User, _{{ variable }}: &{{ model }}) -> Option<AuthResponse> {
        Some(false.into())
    }

    /// Determine whether the user can permanently delete the model.
    fn force_delete(&self, _user: &User, _{{ variable }}: &{{ model }}) -> Option<AuthResponse> {
        Some(false.into())
    }
}
"#;

/// `make:scope`
pub const SCOPE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

impl<M: Model> Scope<M> for {{ class }} {
    /// Apply the scope to a given Eloquent query builder.
    fn apply(&self, query: Builder<M>) -> Builder<M> {
        query
    }
}
"#;

/// `make:channel`
pub const CHANNEL: &str = r#"{{ import }}

pub struct {{ class }};

impl {{ class }} {
    /// Authenticate the user's access to the channel: register it with
    /// `Broadcast::channel("orders.{order}", {{ class }}::join)`.
    pub async fn join(_user: {{ user }}) -> bool {
        true
    }
}
"#;

/// `make:job-middleware`
pub const JOB_MIDDLEWARE: &str = r#"use laravel::prelude::*;
use laravel::queue::middleware::{JobMiddleware, Next};

pub struct {{ class }};

#[async_trait]
impl JobMiddleware for {{ class }} {
    /// Process the queued job.
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        next.run(job).await
    }
}
"#;

/// `make:config`
pub const CONFIG: &str = r#"use laravel::prelude::*;

pub fn config() -> Value {
    json!({
        //
    })
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stub_has_a_unique_name() {
        let mut names: Vec<&str> = ALL.iter().map(|(name, _)| *name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
        assert!(names.iter().all(|name| name.ends_with(".stub")));
        assert_eq!(default("controller.plain.stub"), Some(CONTROLLER));
        assert_eq!(default("missing.stub"), None);
    }
}
