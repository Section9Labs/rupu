//! The stock security-assessment fleet and project init: what `rupu fleet
//! list|install` and `rupu init` do, through the same `rupu_templates`
//! functions. Local only — no host transport carries them.
//!
//! - `GET /api/fleet[?ws_id=]` — every fleet entry, and how the global root
//!   (and that project's `.rupu/`) has it.
//! - `POST /api/fleet/install {force?, project?}` — `rupu fleet install`.
//! - `POST /api/projects/init {path, with_samples?, force?}` — `rupu init`.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use rupu_templates::fleet::{self, Installed};
use rupu_templates::WriteReport;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/fleet", get(get_fleet))
        .route("/api/fleet/install", post(install_fleet))
        .route("/api/projects/init", post(init_project))
}

/// 501 for a `host` other than this one: `what` has no host transport.
pub(crate) fn require_local(host: Option<&str>, what: &str) -> ApiResult<()> {
    match host {
        None | Some("local") => Ok(()),
        Some(h) => Err(ApiError::not_available(format!(
            "{what} is local-only: no host transport carries it (host {h})"
        ))),
    }
}

/// The `.rupu/` directory of the registered project `ws_id`: 404 for an
/// unknown project, 409 when it has none (run `projects.init` first) or its
/// `.rupu/` is the global root.
pub(crate) fn project_rupu_dir(s: &AppState, ws_id: &str) -> ApiResult<PathBuf> {
    let w = crate::api::code::load_workspace(s, ws_id)?;
    let dir = rupu_workspace::project_layer_dir(&s.global_dir, Some(Path::new(&w.path)))
        .ok_or_else(|| {
            ApiError::conflict(format!(
                "project {ws_id}'s .rupu/ is the global rupu root; it has no project layer"
            ))
        })?;
    if !dir.is_dir() {
        return Err(ApiError::conflict(format!(
            "project {ws_id} has no .rupu/ directory ({}); initialise it first",
            dir.display()
        )));
    }
    Ok(dir)
}

#[derive(Deserialize, Default, schemars::JsonSchema)]
pub(crate) struct FleetQuery {
    /// A project id: also report how that project's `.rupu/` has each entry.
    #[serde(default)]
    ws_id: Option<String>,
    /// Must be this host: the fleet is local-only.
    #[serde(default)]
    host: Option<String>,
}

#[derive(Serialize)]
struct FleetRow {
    kind: fleet::EntryKind,
    name: &'static str,
    /// Where it installs, relative to a rupu root.
    path: &'static str,
    global: Installed,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<Installed>,
}

/// `GET /api/fleet[?ws_id=]`: the stock fleet's entries (`rupu fleet list`)
/// with each one's installed status — `missing`, `current` (the shipped
/// file), `modified` or `unreadable` — in the global root and, with
/// `ws_id`, that project's `.rupu/`.
async fn get_fleet(
    State(s): State<AppState>,
    Query(q): Query<FleetQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_local(q.host.as_deref(), "the stock fleet")?;
    let project = q
        .ws_id
        .as_deref()
        .map(|id| project_rupu_dir(&s, id))
        .transpose()?;
    let global = s.global_dir.clone();
    let (rows, project) = blocking(move || {
        let in_global = fleet::status(&global);
        let in_project = project.as_deref().map(fleet::status);
        let rows: Vec<FleetRow> = fleet::entries()
            .into_iter()
            .enumerate()
            .map(|(i, e)| FleetRow {
                kind: e.kind,
                name: e.name,
                path: e.path,
                global: in_global[i],
                project: in_project.as_ref().map(|p| p[i]),
            })
            .collect();
        Ok((rows, project))
    })
    .await?;
    Ok(Json(serde_json::json!({
        "entries": rows,
        "global_root": s.global_dir,
        "project_root": project,
    })))
}

#[derive(Deserialize, Default, schemars::JsonSchema)]
pub(crate) struct FleetInstallBody {
    /// Overwrite fleet files that already exist (default: keep them).
    #[serde(default)]
    force: bool,
    /// A project id: install into that project's `.rupu/` instead of the
    /// global root.
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize, Default, schemars::JsonSchema)]
pub(crate) struct LocalQuery {
    /// Must be this host: the operation is local-only.
    #[serde(default)]
    host: Option<String>,
}

fn report_json(root: &Path, report: &WriteReport) -> serde_json::Value {
    serde_json::json!({
        "root": root,
        "created": report.created(),
        "skipped": report.skipped(),
        "overwrote": report.overwrote(),
        "files": report.files,
    })
}

/// `POST /api/fleet/install {force?, project?}`: `rupu fleet install
/// [--force]` into the global root or the project's `.rupu/`. Existing files
/// are kept unless `force`. Answers `{root, created, skipped, overwrote,
/// files}` once the files are written.
async fn install_fleet(
    State(s): State<AppState>,
    Query(q): Query<LocalQuery>,
    crate::api::runs::OptionalJson(body): crate::api::runs::OptionalJson<FleetInstallBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_local(q.host.as_deref(), "installing the stock fleet")?;
    crate::api::config::require_writable_to(&s, "installing the stock fleet")?;
    let body = body.unwrap_or_default();
    let root = match body.project.as_deref() {
        Some(id) => project_rupu_dir(&s, id)?,
        None => s.global_dir.clone(),
    };
    let at = root.clone();
    let report = blocking(move || {
        fleet::install_into(&at, body.force)
            .map_err(|e| ApiError::internal(format!("fleet install: {e}")))
    })
    .await?;
    Ok(Json(report_json(&root, &report)))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct InitBody {
    /// The project directory: absolute, and it must exist.
    path: String,
    /// Include the curated agent + workflow samples.
    #[serde(default)]
    with_samples: bool,
    /// Re-run on an initialised project, overwriting samples that exist
    /// (`config.toml` is never overwritten). Without it, an initialised
    /// project is `HTTP 409`.
    #[serde(default)]
    force: bool,
}

/// `POST /api/projects/init {path, with_samples?, force?}`: `rupu init
/// <path> [--with-samples] [--force]`. 400 unless `path` is an absolute,
/// existing directory; 409 when it already has `.rupu/config.toml` and
/// `force` is not set. Answers `{root, created, skipped, overwrote, files}`.
async fn init_project(
    State(s): State<AppState>,
    Query(q): Query<LocalQuery>,
    Json(body): Json<InitBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_local(q.host.as_deref(), "initialising a project")?;
    crate::api::config::require_writable_to(&s, "initialising a project")?;
    let root = PathBuf::from(&body.path);
    if !root.is_absolute() {
        return Err(ApiError::bad_request(format!(
            "path must be absolute: {}",
            body.path
        )));
    }
    if !root.is_dir() {
        return Err(ApiError::bad_request(format!(
            "path is not an existing directory: {}",
            body.path
        )));
    }
    if rupu_templates::init::is_initialised(&root) && !body.force {
        return Err(ApiError::conflict(format!(
            "{} is already a rupu project (.rupu/config.toml exists); pass force to re-run",
            root.display()
        )));
    }
    let at = root.clone();
    let report = blocking(move || {
        rupu_templates::init::init_project(&at, body.with_samples, body.force)
            .map_err(|e| ApiError::internal(format!("init: {e}")))
    })
    .await?;
    Ok(Json(report_json(&root, &report)))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}
