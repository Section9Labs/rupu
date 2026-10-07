//! Asset inventory — `GET /api/assets`.
//!
//! The asset model (`rupu_coverage::asset`) is what the finding/coverage layer
//! records for a NON-code engagement: hosts and services for `network`, sites
//! and routes for `web`, and so on. Each asset is a profile-namespaced `kind`
//! (`network:service`), a typed `locator` (host + port), a coverage-depth rung,
//! and a parent link, folded last-line-wins in each target's `assets.jsonl`.
//!
//! This endpoint walks every registered workspace the same way `/api/coverage`
//! does (`discover_targets` → `CoveragePaths` → `read_assets`) and returns a
//! flat, web-friendly list — coordinates flattened to `{host, port, url, …}`
//! and the `kind` split into `profile` + `sub_kind`. It is the read side of
//! "how do we display non-code assets": the Security → Assets page and the
//! agentiflow detail's Assets tab both consume it, the latter scoped by
//! `?ws_id=` / `?target=`.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::asset::{read_assets, Coordinate, Locator};
use rupu_coverage::{discover_targets, CoveragePaths};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::ApiResult;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/assets", get(list_assets))
}

#[derive(Debug, Deserialize)]
struct AssetQuery {
    /// Restrict to a single workspace (as listed on each row).
    ws_id: Option<String>,
    /// Restrict to a single coverage target within the workspace(s).
    target: Option<String>,
}

#[derive(Serialize)]
struct AssetRow {
    /// Stable asset id (`<kind>:<16-hex>`).
    id: String,
    /// Owning workspace id — target_ids can collide across workspaces.
    ws_id: String,
    /// Workspace path basename, for display.
    project: String,
    /// Coverage target the asset lives under.
    target_id: String,
    /// Full profile-namespaced kind, e.g. `network:service`.
    kind: String,
    /// The owning profile (`network`) — `kind` before the `:`.
    profile: String,
    /// The bare kind (`service`) — `kind` after the `:`.
    sub_kind: String,
    /// Parent asset id in the graph (`network:host` for a `network:service`).
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    /// The rendered label (`1.2.3.4:443 https/nginx`).
    label: String,
    /// The coverage-depth rung reached (`enumerated`, `tested`, …), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<String>,
    /// Flattened locator coordinates: `{host, port, proto, url, path, …}`.
    coords: Value,
}

#[derive(Serialize)]
struct AssetListResponse {
    assets: Vec<AssetRow>,
}

fn store(s: &AppState) -> rupu_workspace::WorkspaceStore {
    rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Flatten a typed locator into a plain `{coordinate: value}` object the web
/// can render without knowing the Rust enum shape.
fn coords_json(loc: &Locator) -> Value {
    let mut m = Map::new();
    for c in &loc.0 {
        match c {
            Coordinate::Host(h) => {
                m.insert("host".into(), json!(h));
            }
            Coordinate::Port { number, proto } => {
                m.insert("port".into(), json!(number));
                m.insert("proto".into(), json!(format!("{proto:?}").to_lowercase()));
            }
            Coordinate::Url(u) => {
                m.insert("url".into(), json!(u));
            }
            Coordinate::Path(p) => {
                m.insert("path".into(), json!(p));
            }
            Coordinate::Symbol(sym) => {
                m.insert("symbol".into(), json!(sym));
            }
            Coordinate::LineRange { start, end } => {
                m.insert("line_range".into(), json!(format!("{start}-{end}")));
            }
            Coordinate::HttpRoute { method, path } => {
                m.insert("http_route".into(), json!(format!("{method} {path}")));
            }
            Coordinate::Param(p) => {
                m.insert("param".into(), json!(p));
            }
            Coordinate::ResourceId { scheme, id } => {
                m.insert("resource_id".into(), json!(format!("{scheme}:{id}")));
            }
            Coordinate::Commit(c) => {
                m.insert("commit".into(), json!(c));
            }
            Coordinate::Sha256(h) => {
                m.insert("sha256".into(), json!(h));
            }
            Coordinate::Offset(o) => {
                m.insert("offset".into(), json!(o));
            }
            Coordinate::Address(a) => {
                m.insert("address".into(), json!(a));
            }
        }
    }
    Value::Object(m)
}

fn scoped_workspaces(s: &AppState, ws_id: &Option<String>) -> Vec<rupu_workspace::Workspace> {
    let workspaces = store(s).list().unwrap_or_default();
    match ws_id {
        Some(id) => workspaces.into_iter().filter(|w| &w.id == id).collect(),
        None => workspaces,
    }
}

/// `GET /api/assets?ws_id=&target=` — every asset across the registered
/// workspaces (or the one named), newest workspaces first. A workspace or
/// target whose ledger can't be read is skipped with a warning, never a 500.
async fn list_assets(
    State(s): State<AppState>,
    Query(q): Query<AssetQuery>,
) -> ApiResult<Json<AssetListResponse>> {
    let mut assets = Vec::new();
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        let project = project_name(&w.path);
        let targets = match discover_targets(wp) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(ws_id = %w.id, path = %w.path, error = %e, "discover_targets failed; skipping");
                continue;
            }
        };
        for t in targets {
            if let Some(want) = &q.target {
                if &t.target_id != want {
                    continue;
                }
            }
            let paths = CoveragePaths::new(wp, &t.target_id);
            let found = match read_assets(&paths.assets) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(ws_id = %w.id, target_id = %t.target_id, error = %e, "read_assets failed; skipping target");
                    continue;
                }
            };
            for a in found {
                let (profile, sub_kind) = a.kind.split_once(':').unwrap_or(("", a.kind.as_str()));
                assets.push(AssetRow {
                    id: a.id.clone(),
                    ws_id: w.id.clone(),
                    project: project.clone(),
                    target_id: t.target_id.clone(),
                    profile: profile.to_string(),
                    sub_kind: sub_kind.to_string(),
                    kind: a.kind.clone(),
                    parent: a.parent.clone(),
                    label: a.label.clone(),
                    depth: a.depth.clone(),
                    coords: coords_json(&a.locator),
                });
            }
        }
    }
    Ok(Json(AssetListResponse { assets }))
}
