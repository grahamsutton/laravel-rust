//! The HTML pages shown for errors.

use illuminate_http::Request;
use illuminate_support::{Error, e};

/// Laravel's minimal error page: "404 | Not Found".
pub fn render_minimal_page(status: u16, message: &str) -> String {
    let code = status;
    let message = e(message);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
    <head>
        <meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">

        <title>{message}</title>

        <style>
            html {{ line-height: 1.15; -webkit-text-size-adjust: 100%; }}
            body {{ margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans", sans-serif; -webkit-font-smoothing: antialiased; background: #fff; color: #000; }}
            .container {{ position: relative; display: flex; justify-content: center; align-items: center; min-height: 100vh; }}
            .content {{ display: flex; align-items: center; max-width: 36rem; margin: 0 auto; padding: 0 1.5rem; }}
            .code {{ padding: 0 1rem; font-size: 1.125rem; border-right: 1px solid #cbd5e0; letter-spacing: .05em; }}
            .message {{ margin-left: 1rem; font-size: 1.125rem; text-transform: uppercase; letter-spacing: .05em; }}
            @media (prefers-color-scheme: dark) {{ body {{ background: #111827; color: #e5e7eb; }} .code {{ border-color: #4b5563; }} }}
        </style>
    </head>
    <body>
        <div class="container" role="main">
            <div class="content">
                <h1 class="code">{code}</h1>
                <div class="message">{message}</div>
            </div>
        </div>
    </body>
</html>
"#
    )
}

/// The detailed error page shown when `APP_DEBUG` is enabled.
pub fn render_debug_page(error: &Error, request: &Request, environment: &str) -> String {
    let message = e(error.to_string());
    let chain: Vec<String> = error.chain().skip(1).map(|cause| e(cause.to_string())).collect();
    let debug = e(format!("{error:?}"));

    let causes = if chain.is_empty() {
        String::new()
    } else {
        let items: String = chain
            .iter()
            .map(|cause| format!("<li>{cause}</li>"))
            .collect();
        format!("<section><h2>Caused by</h2><ol class=\"causes\">{items}</ol></section>")
    };

    let headers: String = request
        .headers()
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "cookie" | "authorization"))
        .map(|(name, value)| {
            format!(
                "<tr><th>{}</th><td>{}</td></tr>",
                e(name.as_str()),
                e(value.to_str().unwrap_or("[binary]"))
            )
        })
        .collect();

    let route = request
        .route_name()
        .map(|name| format!("<tr><th>Route</th><td>{}</td></tr>", e(name)))
        .unwrap_or_default();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{message}</title>
    <style>
        * {{ box-sizing: border-box; }}
        body {{ margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; background: #f9fafb; color: #111827; }}
        header {{ background: #fff; border-bottom: 1px solid #e5e7eb; padding: 2.5rem 3rem; }}
        .badge {{ display: inline-block; font-size: .75rem; font-weight: 600; letter-spacing: .05em; text-transform: uppercase; color: #fff; background: #ef4444; padding: .25rem .6rem; border-radius: 9999px; }}
        h1 {{ font-size: 1.5rem; line-height: 1.4; margin: 1rem 0 .5rem; word-break: break-word; }}
        .meta {{ color: #6b7280; font-size: .875rem; }}
        main {{ padding: 2rem 3rem; display: grid; gap: 1.5rem; }}
        section {{ background: #fff; border: 1px solid #e5e7eb; border-radius: .75rem; padding: 1.5rem; overflow: auto; }}
        h2 {{ font-size: .75rem; letter-spacing: .08em; text-transform: uppercase; color: #6b7280; margin: 0 0 1rem; }}
        pre {{ margin: 0; font-family: ui-monospace, Menlo, Monaco, Consolas, monospace; font-size: .8rem; line-height: 1.6; white-space: pre-wrap; }}
        table {{ border-collapse: collapse; width: 100%; font-size: .85rem; }}
        th {{ text-align: left; color: #6b7280; font-weight: 500; padding: .35rem 1rem .35rem 0; vertical-align: top; white-space: nowrap; }}
        td {{ font-family: ui-monospace, Menlo, monospace; padding: .35rem 0; word-break: break-all; }}
        .causes li {{ margin-bottom: .5rem; }}
        @media (prefers-color-scheme: dark) {{
            body {{ background: #030712; color: #f3f4f6; }}
            header, section {{ background: #111827; border-color: #1f2937; }}
        }}
    </style>
</head>
<body>
    <header>
        <span class="badge">Internal Server Error</span>
        <h1>{message}</h1>
        <div class="meta">{method} {url} &middot; Environment: {environment}</div>
    </header>
    <main>
        {causes}
        <section>
            <h2>Details</h2>
            <pre>{debug}</pre>
        </section>
        <section>
            <h2>Request</h2>
            <table>
                <tr><th>Method</th><td>{method}</td></tr>
                <tr><th>URL</th><td>{url}</td></tr>
                {route}
                {headers}
            </table>
        </section>
    </main>
</body>
</html>
"#,
        method = e(request.method().as_str()),
        url = e(request.full_url()),
        environment = e(environment),
    )
}
