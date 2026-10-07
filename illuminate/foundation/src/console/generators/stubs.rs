//! The stubs used by the `make:*` commands.
//!
//! Placeholders: `{{ class }}`, `{{ command }}`, `{{ event }}`, `{{ model }}`,
//! `{{ table }}`, `{{ view }}`.

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

#[derive(Clone, Debug, Serialize, Deserialize)]
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

pub const CLASS: &str = r#"pub struct {{ class }};

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
