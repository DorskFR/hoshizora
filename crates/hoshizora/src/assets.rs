//! Embedded viewer build (`viewer/dist`), with a placeholder when it was not built.

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../viewer/dist"]
#[allow_missing = true]
struct Assets;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>hoshizora</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem;color:#ddd;background:#0b0e14}code{background:#1c2130;padding:.1em .3em;border-radius:3px}</style>
</head><body>
<h1>hoshizora</h1>
<p>The viewer is not built into this binary. Build it, then rebuild <code>hoshizora</code>:</p>
<pre><code>cd viewer &amp;&amp; npm ci &amp;&amp; npm run build</code></pre>
<p>The API is up: <a href="/api/graph">/api/graph</a>, <a href="/api/runtime/stats">/api/runtime/stats</a>.</p>
</body></html>
"#;

pub fn serve(path: &str) -> Response {
    let path = path.trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Assets::get(candidate) {
        return file_response(candidate, file.data.into_owned());
    }
    if path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    match Assets::get("index.html") {
        Some(file) => file_response("index.html", file.data.into_owned()),
        None => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            PLACEHOLDER,
        )
            .into_response(),
    }
}

fn file_response(path: &str, data: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
