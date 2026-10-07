//! `serve` — Serve the application on the development server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use illuminate_console::{Command, Console, async_trait};
use illuminate_http::{BoxFuture, Request, Response};
use illuminate_support::{Carbon, Result};

use crate::application::Application;

/// Serve the application, logging each request like `php artisan serve`.
pub struct ServeCommand;

#[async_trait]
impl Command for ServeCommand {
    fn signature(&self) -> &str {
        "serve
            {--host=127.0.0.1 : The host address to serve the application on}
            {--port=8000 : The port to serve the application on}
            {--tries=10 : The max number of ports to attempt to serve from}
            {--no-reload : Do not reload the development server on .env file changes}"
    }

    fn description(&self) -> &str {
        "Serve the application on the development server"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        app.set_running_in_console(false);
        let kernel = app.http_kernel()?;

        let host = cmd.option("host").unwrap_or_else(|| "127.0.0.1".into());
        let base_port: u16 = cmd
            .option("port")
            .and_then(|p| p.parse().ok())
            .unwrap_or(8000);
        let tries: u16 = cmd.option("tries").and_then(|t| t.parse().ok()).unwrap_or(10);

        // Find an available port, like Laravel does.
        let mut listener = None;
        let mut port = base_port;
        for offset in 0..=tries {
            port = base_port.saturating_add(offset);
            let addr: SocketAddr = format!("{host}:{port}")
                .parse()
                .map_err(|_| illuminate_support::error::error!("Invalid host [{host}]."))?;
            if let Ok(bound) = tokio::net::TcpListener::bind(addr).await {
                listener = Some(bound);
                break;
            }
        }
        let Some(listener) = listener else {
            return cmd.fail(format!("Failed to listen on {host}:{base_port} (reason: Address already in use)."));
        };

        cmd.new_line(1);
        cmd.components()
            .info(format!("Server running on [http://{host}:{port}]."));
        cmd.line("  <fg=yellow;options=bold>Press Ctrl+C to stop the server</>");
        cmd.new_line(1);

        let output = cmd.output().clone();
        let inner = kernel.into_handler();
        let handler: illuminate_http::server::Handler = Arc::new(move |request: Request| {
            let inner = inner.clone();
            let output = output.clone();
            Box::pin(async move {
                let started = Instant::now();
                let method = request.method().to_string();
                let path = request.uri().path().to_string();
                let response: Response = inner(request).await;
                let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                let status = response.status_code();
                let color = match status {
                    500.. => "red",
                    400..=499 => "yellow",
                    300..=399 => "blue",
                    _ => "green",
                };
                let left = format!(
                    "  <fg=gray>{}</> {method} {path}",
                    Carbon::now().to_date_time_string()
                );
                let right = format!("<fg={color}>{status}</> <fg=gray>~ {elapsed:.2}ms</>");
                let visible = Carbon::now().to_date_time_string().len() + method.len() + path.len() + 4
                    + status.to_string().len() + format!("~ {elapsed:.2}ms").len() + 2;
                let width = 100usize;
                let dots = ".".repeat(width.saturating_sub(visible).max(1));
                output.writeln(&format!("{left} <fg=gray>{dots}</> {right}"));
                response
            }) as BoxFuture<'static, Response>
        });

        illuminate_http::server::serve_listener(listener, handler, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

        cmd.new_line(1);
        Ok(())
    }
}
