//! `GET /api/runs/:id/source` — workspace-scoped source-file preview.
//!
//! Given a run id and a `?path=` relative to that run's `workspace_path`,
//! returns a windowed slice of the file's lines centered on `?line=` with
//! `?context=` lines either side. This is the read-only "peek at the file a
//! finding/step touched" surface consumed by the web RunDetail source-preview
//! panel.
//!
//! The path-safety approach is copied from
//! [`crate::api::transcript::validate_transcript_path`]: reject `..`
//! outright, canonicalize the deepest existing ancestor (so a
//! not-yet-materialized path still resolves), and require the result to
//! `starts_with` the canonicalized workspace root. The one difference: there
//! is no `.jsonl`-extension requirement here, and (unlike the transcript
//! validator, which accepts several allowed roots) there is exactly one
//! allowed root per request — the resolved run's own `workspace_path`.
//!
//! Local runs (`Global`/`ProjectLocal`) read straight off disk. Remote runs
//! (`Host`, or an explicit `?host=` that isn't `"local"`) and runs with no
//! resolvable workspace (`Unpersisted`) soft-fail with `available: false` and
//! a human-readable `reason` — HTTP 200, not an error, since "no preview
//! available yet" is an expected, common state for the frontend to render
//! around. An out-of-workspace or traversal-attempting `path`, by contrast,
//! is a genuine client error and 400s — see the module doc above and the
//! task brief's security note.

use crate::{
    api::run_resolve::{resolve_run_location, RunLocation},
    api::runs::{run_not_found_or_internal, validate_id},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::runs::RunStore;
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};

/// Files larger than this are not read for preview — return a soft
/// `available: false` instead of loading a potentially huge file into memory.
const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024; // 2 MiB

/// Hard cap on the caller's requested `?context=` (lines either side of the
/// target). Per spec, `context` is clamped to `[0, 200]`, bounding the window
/// to at most `2 * 200 + 1 = 401` lines regardless of what the client asks
/// for.
const MAX_CONTEXT_LINES: usize = 200;

/// The human-readable reason returned for any remote-run request (an
/// explicit `?host=<id>` that isn't `"local"`, or a resolved `RunLocation`
/// that points at a remote host / has no local workspace).
const REMOTE_NOT_SUPPORTED: &str = "Source preview is not available for remote-host runs yet.";

#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceSlice {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<SourceLine>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl SourceSlice {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct SourceLine {
    pub n: usize,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct SourceQuery {
    /// Path relative to the resolved run's `workspace_path`.
    pub path: String,
    /// 1-based target line to center the window on. Defaults to `1` (start
    /// of file) so a caller previewing a file with no specific line still
    /// gets a sensible window.
    #[serde(default = "default_line")]
    pub line: usize,
    /// Lines of context either side of `line`. Defaults to 20.
    #[serde(default = "default_context")]
    pub context: usize,
    /// When present and not `"local"`, this is a remote-host run — see
    /// [`REMOTE_NOT_SUPPORTED`].
    #[serde(default)]
    pub host: Option<String>,
}

fn default_line() -> usize {
    1
}

fn default_context() -> usize {
    20
}

/// Compute a 1-based inclusive `(start, end)` window of `total` lines,
/// centered on `target` with `context` lines either side, clamped to
/// `[1, total]`. `total == 0` (empty/absent file) returns `(1, 1)` — a
/// nominal single-line window rather than an inverted/empty range.
pub fn source_window(total: usize, target: usize, context: usize) -> (usize, usize) {
    if total == 0 {
        return (1, 1);
    }
    let target = target.clamp(1, total);
    let start = target.saturating_sub(context).max(1);
    // `saturating_add`, not `+` — a client-controlled `context` near `usize::MAX`
    // would otherwise overflow (panics in debug, wraps in release).
    let end = target.saturating_add(context).min(total);
    (start, end)
}

/// Resolve `rel` against `workspace` and enforce that the result stays
/// inside `workspace` — the security boundary for this endpoint. Copied from
/// [`crate::api::transcript::validate_transcript_path`]'s traversal +
/// symlink-escape handling, minus the `.jsonl`-extension requirement:
/// - An absolute `rel` is rejected outright (it can never be "relative to
///   the workspace").
/// - A `..` component is rejected outright, before any filesystem access.
/// - The deepest existing ancestor is canonicalized (resolving symlinks) and
///   compared via `starts_with` against the canonicalized workspace root, so
///   a symlink inside the workspace that points outside it is still caught.
pub fn resolve_under_workspace(workspace: &FsPath, rel: &str) -> Result<PathBuf, ApiError> {
    let p = FsPath::new(rel);
    if p.is_absolute() {
        return Err(ApiError::bad_request(
            "path must be relative to the workspace",
        ));
    }
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(ApiError::bad_request("path must not contain .."));
    }
    let joined = workspace.join(p);
    let resolved = if joined.exists() {
        std::fs::canonicalize(&joined).map_err(|_| ApiError::bad_request("cannot resolve path"))?
    } else {
        canonicalize_existing_prefix(&joined)
            .ok_or_else(|| ApiError::bad_request("cannot resolve path"))?
    };
    let root = std::fs::canonicalize(workspace)
        .map_err(|_| ApiError::bad_request("cannot resolve workspace root"))?;
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(ApiError::bad_request("path is outside the workspace"))
    }
}

/// Canonicalize the deepest existing ancestor of `p` (resolving symlinks in
/// the real prefix) and re-append the remaining, not-yet-created
/// components. Returns `None` only if no ancestor exists. Mirrors
/// `transcript::canonicalize_existing_prefix`.
fn canonicalize_existing_prefix(p: &FsPath) -> Option<PathBuf> {
    let mut ancestor = p;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if ancestor.exists() {
            let mut out = std::fs::canonicalize(ancestor).ok()?;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                ancestor = parent;
            }
            _ => return None,
        }
    }
}

/// Map a file extension to a syntax-highlighting language tag. `None` for
/// unrecognized/absent extensions — the frontend falls back to plain text.
pub fn detect_language(path: &FsPath) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "rs" => "rust",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "go" => "go",
        "json" => "json",
        "toml" => "toml",
        "md" => "markdown",
        "yaml" | "yml" => "yaml",
        _ => return None,
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs/:id/source", get(get_source))
        .route("/api/runs/:id/ast", get(get_ast))
}

/// `GET /api/runs/:id/source?path=<rel>[&line=<n>][&context=<n>][&host=<id>]`
///
/// An explicit `?host=<remote-id>` (anything other than `"local"`) always
/// soft-fails — this endpoint doesn't proxy. Otherwise dispatches on
/// [`resolve_run_location`]: `Global`/`ProjectLocal` read the resolved run's
/// `workspace_path` from disk; `Host` and `Unpersisted` (no local workspace
/// to read from) soft-fail the same way; `NotFound` → 404.
///
/// Once a local `workspace_path` is in hand: `path` is validated via
/// [`resolve_under_workspace`] (400 on traversal/escape — see the module
/// doc's security note), then size-guarded (`> 2 MiB` soft-fails), then read
/// and windowed via [`source_window`].
async fn get_source(
    Path(id): Path<String>,
    Query(q): Query<SourceQuery>,
    State(s): State<AppState>,
) -> ApiResult<Json<SourceSlice>> {
    validate_id(&id)?;

    if q.host.as_deref().is_some_and(|h| h != "local") {
        return Ok(Json(SourceSlice::unavailable(REMOTE_NOT_SUPPORTED)));
    }

    let workspace_path = match resolve_run_location(&s, &id).await {
        RunLocation::Global => {
            s.run_store
                .load(&id)
                .map_err(|e| run_not_found_or_internal(&id, e))?
                .workspace_path
        }
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            store
                .load(&id)
                .map_err(|e| run_not_found_or_internal(&id, e))?
                .workspace_path
        }
        RunLocation::Host { .. } => {
            return Ok(Json(SourceSlice::unavailable(REMOTE_NOT_SUPPORTED)));
        }
        RunLocation::Unpersisted { .. } => {
            return Ok(Json(SourceSlice::unavailable(
                "Source preview is not available for this run.",
            )));
        }
        RunLocation::NotFound => {
            return Err(ApiError::not_found(format!("run {id} not found")));
        }
    };

    let resolved = resolve_under_workspace(&workspace_path, &q.path)?;

    if !resolved.is_file() {
        return Ok(Json(SourceSlice::unavailable("file not found")));
    }

    let meta = std::fs::metadata(&resolved).map_err(|e| ApiError::internal(e.to_string()))?;
    if meta.len() > MAX_PREVIEW_BYTES {
        return Ok(Json(SourceSlice::unavailable("File too large to preview")));
    }

    let Ok(content) = std::fs::read_to_string(&resolved) else {
        return Ok(Json(SourceSlice::unavailable(
            "file is not valid UTF-8 text",
        )));
    };

    let all_lines: Vec<&str> = content.lines().collect();
    let total = all_lines.len();

    // A genuinely empty (0-byte / no-newline-only) file is a valid, previewable
    // file — just one with no lines. `source_window(0, ..)` returns `(1, 1)`
    // (correct per spec), but slicing `all_lines[0..1]` on a zero-length Vec
    // would panic, so short-circuit the empty case with a well-formed
    // zero-line slice (`totalLines: 0`, `lines: []`, line markers all 0).
    if total == 0 {
        return Ok(Json(SourceSlice {
            available: true,
            path: Some(q.path),
            language: detect_language(&resolved),
            start_line: Some(0),
            end_line: Some(0),
            target_line: Some(0),
            total_lines: Some(0),
            lines: Some(Vec::new()),
            reason: None,
        }));
    }

    let target = q.line.clamp(1, total);
    // Clamp `context` to the spec's `[0, 200]` before windowing — otherwise a
    // `?context=1000000` returns the entire (≤2 MiB) file as one JSON
    // line-object per line, an unbounded response vs. the stated constraint.
    // `source_window` only clamps the window to file bounds, not the caller's
    // requested context. Default (20) is applied by serde; this only caps it.
    let context = q.context.min(MAX_CONTEXT_LINES);
    let (start, end) = source_window(total, q.line, context);
    let lines: Vec<SourceLine> = all_lines[start.saturating_sub(1)..end]
        .iter()
        .enumerate()
        .map(|(i, text)| SourceLine {
            n: start + i,
            text: (*text).to_string(),
        })
        .collect();

    Ok(Json(SourceSlice {
        available: true,
        path: Some(q.path),
        language: detect_language(&resolved),
        start_line: Some(start),
        end_line: Some(end),
        target_line: Some(target),
        total_lines: Some(total),
        lines: Some(lines),
        reason: None,
    }))
}

/// The human-readable reason returned for any remote-run `?ast` request —
/// same policy as [`REMOTE_NOT_SUPPORTED`], worded for the AST endpoint.
const AST_REMOTE_NOT_SUPPORTED: &str = "AST view is not available for remote-host runs yet.";

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AstResponse {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<rupu_ast::AstNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AstResponse {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AstQuery {
    /// Path relative to the resolved run's `workspace_path`.
    pub path: String,
    /// 1-based target line. Defaults to `1`.
    #[serde(default = "default_line")]
    pub line: usize,
    /// 1-based target column (byte offset within the line, matching
    /// tree-sitter's `Point.column` convention — see
    /// [`rupu_ast::parse_slice`]'s doc comment). Defaults to `1`.
    #[serde(default = "default_col")]
    pub col: usize,
    /// When present and not `"local"`, this is a remote-host run — see
    /// [`AST_REMOTE_NOT_SUPPORTED`].
    #[serde(default)]
    pub host: Option<String>,
}

fn default_col() -> usize {
    1
}

/// `GET /api/runs/:id/ast?path=<rel>&line=<1-based>&col=<1-based>[&host=<id>]`
///
/// Mirrors [`get_source`]'s run-resolution / local-remote branch and
/// [`resolve_under_workspace`] path guard exactly (same security boundary,
/// same soft-fail-vs-400 split), then adds two more soft-fail steps specific
/// to parsing: no [`rupu_ast::Lang`] mapped for the file's extension, or
/// [`rupu_ast::parse_slice`] itself erroring (tree-sitter language-set
/// failure or an empty parse). On success the response is filled from the
/// returned [`rupu_ast::AstSubtree`].
async fn get_ast(
    Path(id): Path<String>,
    Query(q): Query<AstQuery>,
    State(s): State<AppState>,
) -> ApiResult<Json<AstResponse>> {
    validate_id(&id)?;

    if q.host.as_deref().is_some_and(|h| h != "local") {
        return Ok(Json(AstResponse::unavailable(AST_REMOTE_NOT_SUPPORTED)));
    }

    let workspace_path = match resolve_run_location(&s, &id).await {
        RunLocation::Global => {
            s.run_store
                .load(&id)
                .map_err(|e| run_not_found_or_internal(&id, e))?
                .workspace_path
        }
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            store
                .load(&id)
                .map_err(|e| run_not_found_or_internal(&id, e))?
                .workspace_path
        }
        RunLocation::Host { .. } => {
            return Ok(Json(AstResponse::unavailable(AST_REMOTE_NOT_SUPPORTED)));
        }
        RunLocation::Unpersisted { .. } => {
            return Ok(Json(AstResponse::unavailable(
                "AST view is not available for this run.",
            )));
        }
        RunLocation::NotFound => {
            return Err(ApiError::not_found(format!("run {id} not found")));
        }
    };

    let resolved = resolve_under_workspace(&workspace_path, &q.path)?;

    if !resolved.is_file() {
        return Ok(Json(AstResponse::unavailable("file not found")));
    }

    let meta = std::fs::metadata(&resolved).map_err(|e| ApiError::internal(e.to_string()))?;
    if meta.len() > MAX_PREVIEW_BYTES {
        return Ok(Json(AstResponse::unavailable("File too large to parse")));
    }

    let Some(lang) = rupu_ast::Lang::from_path(&resolved) else {
        return Ok(Json(AstResponse::unavailable(
            "No syntax grammar for this file type.",
        )));
    };

    let Ok(content) = std::fs::read_to_string(&resolved) else {
        return Ok(Json(AstResponse::unavailable(
            "file is not valid UTF-8 text",
        )));
    };

    // `as u32` truncates rather than saturates on a huge client-supplied
    // `usize` — clamp into u32 range first so an absurd `?line=` doesn't wrap
    // around to an arbitrary small value (same class of concern as
    // `source_window`'s `saturating_add` guard against a huge `?context=`).
    let line = q.line.max(1).min(u32::MAX as usize) as u32;
    let col = q.col.max(1).min(u32::MAX as usize) as u32;

    match rupu_ast::parse_slice(&content, lang, line, col) {
        Ok(sub) => Ok(Json(AstResponse {
            available: true,
            language: Some(sub.language),
            root: Some(sub.root),
            truncated: Some(sub.truncated),
            reason: None,
        })),
        Err(_) => Ok(Json(AstResponse::unavailable("Could not parse file."))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_clamps_at_bounds() {
        assert_eq!(source_window(219, 46, 20), (26, 66));
        assert_eq!(source_window(219, 1, 20), (1, 21)); // clamp at start
        assert_eq!(source_window(219, 219, 20), (199, 219)); // clamp at end
        assert_eq!(source_window(5, 3, 20), (1, 5)); // window > file
        assert_eq!(source_window(0, 1, 20), (1, 1)); // empty file guard
    }

    #[test]
    fn window_does_not_overflow_on_a_huge_context() {
        // A client-controlled `context` near `usize::MAX` must saturate, not
        // panic (debug overflow-check) or wrap (release).
        assert_eq!(source_window(219, 46, usize::MAX), (1, 219));
        assert_eq!(source_window(5, 3, usize::MAX), (1, 5));
    }

    #[test]
    fn rejects_path_traversal() {
        let root = std::path::Path::new("/tmp/ws-does-not-matter");
        assert!(resolve_under_workspace(root, "../etc/passwd").is_err());
        assert!(resolve_under_workspace(root, "/etc/passwd").is_err());
    }

    #[test]
    fn rejects_traversal_that_lands_back_inside_the_workspace() {
        // A `..` component is rejected outright, even one that nets out
        // inside the workspace root (`foo/../bar` == `bar`) — same
        // "no ParentDir component at all" policy as the transcript
        // validator.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bar.rs"), "fn main() {}\n").unwrap();
        assert!(resolve_under_workspace(tmp.path(), "foo/../bar.rs").is_err());
    }

    #[test]
    fn resolves_a_plain_in_workspace_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("src").join("lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let resolved = resolve_under_workspace(tmp.path(), "src/lib.rs").unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn rejects_symlink_escaping_the_workspace() {
        #[cfg(unix)]
        {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.txt"), "shh").unwrap();
            let link = tmp.path().join("escape.txt");
            std::os::unix::fs::symlink(outside.path().join("secret.txt"), &link).unwrap();

            assert!(resolve_under_workspace(tmp.path(), "escape.txt").is_err());
        }
    }

    #[test]
    fn detect_language_maps_known_extensions() {
        assert_eq!(detect_language(std::path::Path::new("a.rs")), Some("rust"));
        assert_eq!(
            detect_language(std::path::Path::new("a.py")),
            Some("python")
        );
        assert_eq!(
            detect_language(std::path::Path::new("a.tsx")),
            Some("typescript")
        );
        assert_eq!(
            detect_language(std::path::Path::new("a.jsx")),
            Some("javascript")
        );
        assert_eq!(detect_language(std::path::Path::new("a.go")), Some("go"));
        assert_eq!(detect_language(std::path::Path::new("a.yml")), Some("yaml"));
        assert_eq!(detect_language(std::path::Path::new("a.bin")), None);
        assert_eq!(detect_language(std::path::Path::new("noext")), None);
    }

    #[test]
    fn source_slice_default_is_unavailable_with_no_extra_fields_serialized() {
        let s = SourceSlice::unavailable("nope");
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "available": false, "reason": "nope" })
        );
    }
}
