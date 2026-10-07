//! `make:mail` and `make:notification`, which can also create a Markdown
//! template for the message.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Str};

use super::{QualifiedName, Registration, generate, populate, relative};
use crate::application::Application;

const MAIL: &str = r#"use laravel::prelude::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct {{ class }} {
    //
}

impl {{ class }} {
    /// Create a new message instance.
    pub fn new() -> Self {
        Self {}
    }
}

impl Mailable for {{ class }} {
    /// Get the message envelope.
    fn envelope(&self) -> Envelope {
        Envelope::new().subject("{{ subject }}")
    }

    /// Get the message content definition.
    fn content(&self) -> Content {
        {{ content }}
    }

    /// Get the attachments for the message.
    fn attachments(&self) -> Vec<Attachment> {
        Vec::new()
    }
}
"#;

const NOTIFICATION: &str = r#"use laravel::prelude::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct {{ class }} {
    //
}

impl {{ class }} {
    /// Create a new notification instance.
    pub fn new() -> Self {
        Self {}
    }
}

impl Notification for {{ class }} {
    /// Get the notification's delivery channels.
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    /// Get the mail representation of the notification.
    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
        Some({{ mail }})
    }

    /// Get the array representation of the notification.
    fn to_array(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
        Some(json!({
            //
        }))
    }
}
"#;

const MARKDOWN: &str = r#"<x-mail::message>
# Introduction

The body of your message.

<x-mail::button :url="''">
Button Text
</x-mail::button>

Thanks,<br>
{{ config('app.name') }}
</x-mail::message>
"#;

const VIEW: &str = r#"<div>
    <!-- {{ quote }} -->
</div>
"#;

/// Write a view for the message: `mail.orders.shipped` =>
/// `resources/views/mail/orders/shipped.blade.html`.
fn write_view(cmd: &Console, app: &Application, view: &str, contents: &str, force: bool) -> Result<bool> {
    let mut segments: Vec<String> = view.split(['.', '/']).map(str::to_string).collect();
    let class = segments.pop().unwrap_or_default();
    let name = QualifiedName { namespace: segments, class };
    match generate(app, "resources/views", &name, "blade.html", contents, Registration::None, force) {
        Ok(path) => {
            cmd.components()
                .info(format!("View [{}] created successfully.", relative(app, &path)));
            Ok(true)
        }
        Err(error) if error.to_string() == "already exists" => {
            cmd.components().error("View already exists.");
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// `make:mail` — Create a new email class.
pub struct MakeMailCommand;

#[async_trait]
impl Command for MakeMailCommand {
    fn signature(&self) -> &str {
        "make:mail
            {name : The name of the mailable}
            {--f|force : Create the class even if the mailable already exists}
            {--m|markdown= : Create a new Markdown template for the mailable}
            {--view= : Create a new Blade template for the mailable}"
    }

    fn description(&self) -> &str {
        "Create a new email class"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let name = QualifiedName::parse(&cmd.argument("name").unwrap_or_default());
        if name.class.is_empty() {
            return cmd.fail("The name of the mailable is required.");
        }
        let force = cmd.option_bool("force");
        let markdown = cmd.option("markdown").filter(|view| !view.is_empty());
        let view = cmd.option("view").filter(|view| !view.is_empty());

        let content = match (&markdown, &view) {
            (Some(markdown), _) => format!("Content::markdown(\"{markdown}\")"),
            (None, Some(view)) => format!("Content::view(\"{view}\")"),
            (None, None) => "Content::view(\"view.name\")".to_string(),
        };
        let subject = Str::headline(&name.class);
        let contents = populate(MAIL, &[("class", &name.class), ("subject", &subject), ("content", &content)]);

        match generate(&app, "app/mail", &name, "rs", &contents, Registration::ModuleAndExport, force) {
            Ok(path) => cmd
                .components()
                .info(format!("Mailable [{}] created successfully.", relative(&app, &path))),
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error("Mailable already exists.");
                return cmd.exit(1);
            }
            Err(error) => return Err(error),
        }

        if let Some(markdown) = markdown {
            write_view(&cmd, &app, &markdown, MARKDOWN, force)?;
        } else if let Some(view) = view {
            let quote = crate::inspiring::Inspiring::quote();
            write_view(&cmd, &app, &view, &populate(VIEW, &[("quote", &quote)]), force)?;
        }
        Ok(())
    }
}

/// `make:notification` — Create a new notification class.
pub struct MakeNotificationCommand;

#[async_trait]
impl Command for MakeNotificationCommand {
    fn signature(&self) -> &str {
        "make:notification
            {name : The name of the notification}
            {--f|force : Create the class even if the notification already exists}
            {--m|markdown= : Create a new Markdown template for the notification}"
    }

    fn description(&self) -> &str {
        "Create a new notification class"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let name = QualifiedName::parse(&cmd.argument("name").unwrap_or_default());
        if name.class.is_empty() {
            return cmd.fail("The name of the notification is required.");
        }
        let force = cmd.option_bool("force");
        let markdown = cmd.option("markdown").filter(|view| !view.is_empty());

        let mail = match &markdown {
            Some(markdown) => format!("MailMessage::new().markdown(\"{markdown}\")"),
            None => "MailMessage::new()
            .line(\"The introduction to the notification.\")
            .action(\"Notification Action\", url(\"/\"))
            .line(\"Thank you for using our application!\")"
                .to_string(),
        };
        let contents = populate(NOTIFICATION, &[("class", &name.class), ("mail", &mail)]);

        match generate(&app, "app/notifications", &name, "rs", &contents, Registration::ModuleAndExport, force) {
            Ok(path) => cmd
                .components()
                .info(format!("Notification [{}] created successfully.", relative(&app, &path))),
            Err(error) if error.to_string() == "already exists" => {
                cmd.components().error("Notification already exists.");
                return cmd.exit(1);
            }
            Err(error) => return Err(error),
        }

        if let Some(markdown) = markdown {
            write_view(&cmd, &app, &markdown, MARKDOWN, force)?;
        }
        Ok(())
    }
}
