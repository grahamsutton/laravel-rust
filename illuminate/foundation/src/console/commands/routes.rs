//! `route:list` — List all registered routes.

use illuminate_console::{Command, Console, async_trait};
use illuminate_routing::{RouteListing, Router};
use illuminate_support::{Result, Str, json};

use crate::application::Application;

/// List all registered routes, styled exactly like Laravel's `route:list`.
pub struct RouteListCommand;

#[async_trait]
impl Command for RouteListCommand {
    fn signature(&self) -> &str {
        "route:list
            {--json : Output the route list as JSON}
            {--method= : Filter the routes by method}
            {--action= : Filter the routes by action}
            {--name= : Filter the routes by name}
            {--domain= : Filter the routes by domain}
            {--middleware= : Filter the routes by middleware}
            {--path= : Only show routes matching the given path pattern}
            {--except-path= : Do not display the routes matching the given path pattern}
            {--r|reverse : Reverse the ordering of the routes}
            {--sort=uri : The column (domain, method, uri, name, action, middleware) to sort by}"
    }

    fn description(&self) -> &str {
        "List all registered routes"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let router = app.make::<Router>();
        let mut routes: Vec<RouteListing> = router.route_list();

        let contains = |haystack: &str, needle: &Option<String>| {
            needle
                .as_ref()
                .is_none_or(|n| haystack.to_lowercase().contains(&n.to_lowercase()))
        };
        let method = cmd.option("method");
        let action = cmd.option("action");
        let name = cmd.option("name");
        let domain = cmd.option("domain");
        let middleware = cmd.option("middleware");
        let path = cmd.option("path");
        let except = cmd.option("except-path");

        routes.retain(|route| {
            contains(&route.methods.join("|"), &method)
                && contains(&route.action, &action)
                && contains(route.name.as_deref().unwrap_or(""), &name)
                && contains(route.domain.as_deref().unwrap_or(""), &domain)
                && contains(&route.middleware.join(","), &middleware)
                && path.as_ref().is_none_or(|p| route.uri.starts_with(p.trim_matches('/')))
                && except.as_ref().is_none_or(|e| {
                    !e.split(',').any(|e| route.uri.starts_with(e.trim().trim_matches('/')))
                })
        });

        match cmd.option("sort").as_deref() {
            Some("name") => routes.sort_by(|a, b| a.name.cmp(&b.name)),
            Some("action") => routes.sort_by(|a, b| a.action.cmp(&b.action)),
            Some("method") => routes.sort_by_key(|a| a.methods.join("|")),
            Some("domain") => routes.sort_by(|a, b| a.domain.cmp(&b.domain)),
            Some("middleware") => routes.sort_by_key(|a| a.middleware.join(",")),
            Some("precedence") => {}
            _ => routes.sort_by(|a, b| a.uri.cmp(&b.uri)),
        }
        if cmd.option_bool("reverse") {
            routes.reverse();
        }

        if routes.is_empty() {
            return cmd.fail(if router.count() == 0 {
                "Your application doesn't have any routes."
            } else {
                "Your application doesn't have any routes matching the given criteria."
            });
        }

        if cmd.option_bool("json") {
            let json: Vec<_> = routes
                .iter()
                .map(|route| {
                    json!({
                        "domain": route.domain,
                        "method": route.methods.join("|"),
                        "uri": route.uri,
                        "name": route.name,
                        "action": route.action,
                        "middleware": route.middleware,
                    })
                })
                .collect();
            cmd.line(serde_json::to_string(&json)?);
            return Ok(());
        }

        let width = terminal_width();
        let max_method = routes
            .iter()
            .map(|r| r.methods.join("|").len())
            .max()
            .unwrap_or(0);

        cmd.new_line(1);
        for route in &routes {
            let methods = route.methods.join("|");
            let spaces = " ".repeat((max_method + 6).saturating_sub(methods.len()));
            let uri = match &route.domain {
                Some(domain) => format!("{domain}/{}", route.uri.trim_start_matches('/')),
                None => route.uri.clone(),
            };
            let action = route.action.clone();
            let action = if action == "Closure" { String::new() } else { action };
            let name = route.name.clone().unwrap_or_default();
            let right = match (name.is_empty(), action.is_empty()) {
                (true, true) => String::new(),
                (false, true) => name.clone(),
                (true, false) => action.clone(),
                (false, false) => format!("{name} › {action}"),
            };
            let used = methods.len() + spaces.len() + uri.len() + right.len() + 6 + usize::from(!right.is_empty());
            let dots = ".".repeat(width.saturating_sub(used));
            let colored_methods = methods
                .split('|')
                .map(|m| format!("<fg={}>{m}</>", method_color(m)))
                .collect::<Vec<_>>()
                .join("<fg=#6C7280>|</>");
            let uri = highlight_parameters(&uri);
            let right = if right.is_empty() {
                String::new()
            } else {
                format!(" <fg=#6C7280>{right}</>")
            };
            cmd.line(format!("  {colored_methods}{spaces}{uri}<fg=#6C7280> {dots}</>{right}"));

            if cmd.is_verbose() {
                for middleware in &route.middleware {
                    cmd.line(format!("  {}<fg=#6C7280>⇂ {}</>", " ".repeat(max_method + 6), Str::class_basename(middleware)));
                }
            }
        }

        cmd.new_line(1);
        let footer = format!("Showing [{}] routes", routes.len());
        let padding = width.saturating_sub(footer.len() + 2);
        cmd.line(format!("{}<fg=blue;options=bold>{footer}</>", " ".repeat(padding)));
        cmd.new_line(1);
        Ok(())
    }
}

fn method_color(method: &str) -> &'static str {
    match method {
        "GET" | "HEAD" => "blue",
        "POST" | "PUT" | "PATCH" => "yellow",
        "DELETE" => "red",
        "ANY" => "red",
        _ => "default",
    }
}

fn highlight_parameters(uri: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for c in uri.chars() {
        match c {
            '{' => {
                in_param = true;
                out.push_str("<fg=yellow>{");
            }
            '}' if in_param => {
                in_param = false;
                out.push_str("}</>");
            }
            c => out.push(c),
        }
    }
    out
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(120)
        .clamp(60, 200)
}
