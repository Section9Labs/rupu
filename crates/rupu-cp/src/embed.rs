//! Embeds the built web UI (`web/dist/`) into the binary at compile time and
//! serves it with an SPA fallback so client-side routes resolve to `index.html`.

use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// Shell flag resolved fresh from the global config on every index.html
/// serve — `rupu config set ui.cp.shell v2` + a browser refresh switches
/// shells without restarting `cp serve` (same per-request-resolve contract
/// as `GET /api/config`).
fn resolve_shell(state: &AppState) -> &'static str {
    let global = state.global_dir.join("config.toml");
    let shell = rupu_config::resolve(Some(&global), None)
        .ok()
        .and_then(|r| r.config.ui.cp.shell);
    match shell.as_deref() {
        Some("v2") => "v2",
        _ => "v1",
    }
}

/// Insert `<meta name="rupu-shell" …>` after the first opening `<head>` tag.
/// Tolerates the build.rs placeholder index.html (has a `<head>`, no `#root`);
/// a document with no `<head>` is served unmodified.
fn inject_shell_meta(html: &str, shell: &str) -> String {
    match html.find("<head>") {
        Some(i) => {
            let at = i + "<head>".len();
            format!(
                "{}<meta name=\"rupu-shell\" content=\"{shell}\">{}",
                &html[..at],
                &html[at..]
            )
        }
        None => html.to_string(),
    }
}

fn serve_index(state: &AppState) -> Response {
    match Assets::get("index.html") {
        Some(content) => {
            let html = String::from_utf8_lossy(&content.data);
            let html = inject_shell_meta(&html, resolve_shell(state));
            ([(header::CONTENT_TYPE, "text/html")], html).into_response()
        }
        None => (StatusCode::NOT_FOUND, "web UI not embedded").into_response(),
    }
}

pub async fn static_handler(State(state): State<AppState>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() || path == "index.html" {
        return serve_index(&state);
    }
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => serve_index(&state),
    }
}

#[cfg(test)]
mod tests {
    use super::inject_shell_meta;

    #[test]
    fn injects_after_head_open() {
        let out = inject_shell_meta("<html><head><title>x</title></head></html>", "v2");
        assert_eq!(
            out,
            "<html><head><meta name=\"rupu-shell\" content=\"v2\"><title>x</title></head></html>"
        );
    }

    #[test]
    fn no_head_serves_unmodified() {
        assert_eq!(inject_shell_meta("<html></html>", "v2"), "<html></html>");
    }
}
