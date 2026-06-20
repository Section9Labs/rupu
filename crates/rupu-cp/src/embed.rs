//! Embeds the built web UI (`web/dist/`) into the binary at compile time and
//! serves it with an SPA fallback so client-side routes resolve to `index.html`.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// Serve an embedded asset by path; fall back to `index.html` for unknown
/// paths so the SPA's client-side router can handle them.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => match Assets::get("index.html") {
            // SPA fallback for client-side routes (e.g. `/runs/abc`).
            Some(c) => ([(header::CONTENT_TYPE, "text/html")], c.data).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                "web UI not built: run `npm run build` in crates/rupu-cp/web",
            )
                .into_response(),
        },
    }
}
