//! Agentiflow DEFINITIONS (LOCAL only): the `<name>.yaml` files
//! `rupu agentiflow run <name>` loads, in the global layer
//! (`<global>/agentiflows/`) and each registered project's
//! (`<workspace>/.rupu/agentiflows/`).
//!
//! - `GET /api/agentiflow-defs` — every definition, a project's shadowing a
//!   same-named global one.
//! - `GET|PUT|DELETE /api/agentiflow-defs/:name` — one definition.
//! - `POST /api/agentiflow-defs` — create one in the global layer.
//! - `POST /api/agentiflow-defs/validate` — what `agentiflow run` checks
//!   before it starts: parse, engagement profiles, validation.
//!
//! These mirror `api::workflows`' definition CRUD: the same project-first
//! resolution, `?scope_kind=&scope_id=` selector and atomic writes. Files are
//! read with `rupu_agentiflow::AgentiflowDef::parse_str`, the parser the
//! CLI's loader (`rupu_agentiflow::locate_agentiflow_def`) uses; only `.yaml`
//! files are definitions, and the run directories (`af_<ULID>/`) that share
//! the global directory are never read. Definitions have no host transport,
//! so every route answers a remote `?host=` with a 501.

use crate::{
    api::agentiflows::{require_local, LocalHostQuery},
    api::fs_safety,
    api::repo_scope::{distinct_repo_workspaces, scope_name, ScopeKind},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use rupu_agentiflow::AgentiflowDef;
use rupu_workspace::{RepoRegistryStore, WorkspaceStore};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agentiflow-defs", get(list_defs).post(create_def))
        .route(
            "/api/agentiflow-defs/:name",
            get(get_def).put(write_def).delete(delete_def),
        )
        // A literal segment wins over `:name`, so this is not shadowed.
        .route("/api/agentiflow-defs/validate", post(validate_def))
}

/// The query of the routes that act on one definition: which layer's file
/// (`scope_kind` + `scope_id`, as `repo_scope::ScopeQuery`), and `host`,
/// which can only be this one.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct DefQuery {
    /// `global` | `project`: act on exactly this layer's file; without it, the
    /// project-first match.
    #[serde(default)]
    scope_kind: Option<ScopeKind>,
    /// For `project`: the registered workspace's id.
    #[serde(default)]
    scope_id: Option<String>,
    /// Only `local` (or absent): definitions live on this control plane's
    /// host.
    #[serde(default)]
    host: Option<String>,
}

/// The route segments under `/api/agentiflow-defs/` that are not names: a
/// definition named one could never be read, saved or deleted by name.
const RESERVED_NAMES: &[&str] = &["validate"];

/// [`fs_safety::validate_name`], and not a [`RESERVED_NAMES`] segment.
fn writable_name(name: &str) -> ApiResult<()> {
    fs_safety::validate_name(name)?;
    if RESERVED_NAMES.contains(&name) {
        return Err(ApiError::bad_request(format!(
            "`{name}` can't name an agentiflow definition: `/api/agentiflow-defs/{name}` is \
             another route"
        )));
    }
    Ok(())
}

/// The global layer's definitions directory.
fn global_dir(s: &AppState) -> PathBuf {
    rupu_agentiflow::agentiflow_dir(&s.global_dir)
}

/// A workspace's definitions directory, or `None` when its `.rupu/` is the
/// global dir (R5, `rupu_workspace::project_layer_dir`): a global-layer
/// launch starts in the global dir's parent and registers it, and its
/// "project" definitions would be the global ones a second time.
fn project_dir(s: &AppState, workspace: &str) -> Option<PathBuf> {
    rupu_workspace::project_layer_dir(&s.global_dir, Some(FsPath::new(workspace)))
        .map(|d| d.join("agentiflows"))
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

fn repo_store(s: &AppState) -> RepoRegistryStore {
    RepoRegistryStore {
        root: s.global_dir.join("repos"),
    }
}

/// `<dir>/<name>.yaml` when it is a file: the loader reads only `.yaml`.
fn definition_file(dir: &FsPath, name: &str) -> Option<PathBuf> {
    Some(dir.join(format!("{name}.yaml"))).filter(|p| p.is_file())
}

/// Where `name` resolved: its file, the directory holding it, the display
/// scope, the layer and (for a project) the workspace id.
struct Resolved {
    path: PathBuf,
    dir: PathBuf,
    scope: String,
    scope_kind: ScopeKind,
    scope_id: Option<String>,
}

/// Resolve `name` project-first (one representative workspace per distinct
/// repo, as the list shows them), falling back to the global layer — the
/// rule `api::workflows::resolve_workflow_scoped` documents, and the one
/// the list's shadowing and the CLI's loader follow.
fn resolve(s: &AppState, name: &str) -> Option<Resolved> {
    let workspaces = store(s).list().unwrap_or_default();
    for r in distinct_repo_workspaces(workspaces, &repo_store(s)) {
        let Some(dir) = project_dir(s, &r.workspace.path) else {
            continue;
        };
        if let Some(path) = definition_file(&dir, name) {
            return Some(Resolved {
                path,
                dir,
                scope: r.scope,
                scope_kind: ScopeKind::Project,
                scope_id: Some(r.workspace.id),
            });
        }
    }
    let dir = global_dir(s);
    let path = definition_file(&dir, name)?;
    Some(Resolved {
        path,
        dir,
        scope: "global".into(),
        scope_kind: ScopeKind::Global,
        scope_id: None,
    })
}

/// Resolve `name` in exactly one layer: the global one, or the registered
/// workspace `scope_id` names (any registered workspace, not only a
/// representative). `None` on any mismatch — never a fallback to another
/// layer (`api::workflows::resolve_workflow_scoped_explicit`'s rule).
fn resolve_explicit(
    s: &AppState,
    name: &str,
    kind: ScopeKind,
    scope_id: Option<&str>,
) -> Option<Resolved> {
    match kind {
        ScopeKind::Global => {
            let dir = global_dir(s);
            let path = definition_file(&dir, name)?;
            Some(Resolved {
                path,
                dir,
                scope: "global".into(),
                scope_kind: ScopeKind::Global,
                scope_id: None,
            })
        }
        ScopeKind::Project => {
            let scope_id = scope_id?;
            let w = store(s)
                .list()
                .unwrap_or_default()
                .into_iter()
                .find(|w| w.id == scope_id)?;
            let dir = project_dir(s, &w.path)?;
            let path = definition_file(&dir, name)?;
            Some(Resolved {
                path,
                dir,
                scope: scope_name(&w),
                scope_kind: ScopeKind::Project,
                scope_id: Some(w.id),
            })
        }
    }
}

/// The resolution a request's selector asks for: strict with `scope_kind`,
/// project-first without.
fn resolve_for(s: &AppState, name: &str, q: &DefQuery) -> ApiResult<Resolved> {
    match q.scope_kind {
        Some(kind) => resolve_explicit(s, name, kind, q.scope_id.as_deref()).ok_or_else(|| {
            ApiError::not_found(format!(
                "agentiflow definition {name} not found in the requested scope"
            ))
        }),
        None => resolve(s, name)
            .ok_or_else(|| ApiError::not_found(format!("agentiflow definition {name} not found"))),
    }
}

/// The launch directory a scope-pinned agentiflow launch starts in, so the
/// child's own lookup finds the definition `scope_kind`/`scope_id` picked
/// (`repo_scope::launch_dir_for`). 404 when `name` is not in that scope.
pub(crate) fn launch_dir(
    s: &AppState,
    name: &str,
    kind: ScopeKind,
    scope_id: Option<&str>,
) -> ApiResult<PathBuf> {
    let r = resolve_explicit(s, name, kind, scope_id).ok_or_else(|| {
        ApiError::not_found(format!(
            "agentiflow definition {name} not found in the requested scope"
        ))
    })?;
    crate::api::repo_scope::launch_dir_for(&r.dir, r.scope_kind)?
        .ok_or_else(|| ApiError::internal("the definition's directory has no workspace root"))
}

/// One row of `GET /api/agentiflow-defs`.
#[derive(Serialize)]
struct DefRow {
    name: String,
    /// DISPLAY ONLY: `global`, or the workspace path's basename.
    scope: String,
    scope_kind: ScopeKind,
    /// The registered workspace's id for a project row; pass it back as
    /// `scope_id` to act on this row's file.
    scope_id: Option<String>,
    description: Option<String>,
    lead: Option<String>,
    engagement_profiles: Vec<String>,
    /// Why the file does not parse; the row is still listed so a broken
    /// definition doesn't read as a missing one.
    error: Option<String>,
}

/// `<dir>/*.yaml` as rows, sorted by name; an absent directory is empty.
fn scan(dir: &FsPath, scope: &str, scope_kind: ScopeKind, scope_id: Option<&str>) -> Vec<DefRow> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<DefRow> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "yaml"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_string();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|raw| AgentiflowDef::parse_str(&raw).map_err(|e| e.to_string()));
            let (description, lead, engagement_profiles, error) = match parsed {
                Ok(d) => (d.description, Some(d.lead), d.engagement_profiles, None),
                Err(e) => (None, None, Vec::new(), Some(e)),
            };
            Some(DefRow {
                name,
                scope: scope.to_string(),
                scope_kind,
                scope_id: scope_id.map(str::to_string),
                description,
                lead,
                engagement_profiles,
                error,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// `GET /api/agentiflow-defs` — the global definitions plus one
/// representative workspace per distinct repo's, sorted by name then scope.
/// A project definition hides a same-named global one (it is the one
/// `agentiflow run` loads there); two repos defining the same name both
/// appear.
async fn list_defs(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
) -> ApiResult<Json<Vec<DefRow>>> {
    q.require_local("listing agentiflow definitions")?;
    let mut rows = scan(&global_dir(&s), "global", ScopeKind::Global, None);
    let mut project_rows = Vec::new();
    for r in distinct_repo_workspaces(store(&s).list().unwrap_or_default(), &repo_store(&s)) {
        let Some(dir) = project_dir(&s, &r.workspace.path) else {
            continue;
        };
        project_rows.extend(scan(
            &dir,
            &r.scope,
            ScopeKind::Project,
            Some(&r.workspace.id),
        ));
    }
    let shadowed: std::collections::BTreeSet<String> =
        project_rows.iter().map(|r| r.name.clone()).collect();
    rows.retain(|r| !shadowed.contains(&r.name));
    rows.extend(project_rows);
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.scope.cmp(&b.scope)));
    Ok(Json(rows))
}

/// The detail body: the parsed definition (`null` when it does not parse,
/// with `error` saying why), the raw YAML and where it resolved.
fn detail(name: &str, r: &Resolved) -> ApiResult<Json<serde_json::Value>> {
    let yaml = std::fs::read_to_string(&r.path).map_err(|e| ApiError::internal(e.to_string()))?;
    let (def, error) = match AgentiflowDef::parse_str(&yaml) {
        Ok(d) => (Some(d), None),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(Json(serde_json::json!({
        "name": name,
        "def": def,
        "yaml": yaml,
        "error": error,
        "scope": r.scope,
        "scope_kind": r.scope_kind,
        "scope_id": r.scope_id,
    })))
}

/// `GET /api/agentiflow-defs/:name` — project-first, else global (or the
/// layer `scope_kind`/`scope_id` pick).
async fn get_def(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<DefQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_local(q.host.as_deref(), "reading an agentiflow definition")?;
    fs_safety::validate_stem(&name)?;
    let r = resolve_for(&s, &name, &q)?;
    detail(&name, &r)
}

/// Request body of `PUT /api/agentiflow-defs/:name` and
/// `POST /api/agentiflow-defs`: the whole definition file.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DefWriteBody {
    raw: String,
}

/// Parse `raw` as a definition: a 400 with the parser's message if it does
/// not.
fn parse(raw: &str) -> ApiResult<AgentiflowDef> {
    AgentiflowDef::parse_str(raw).map_err(|e| ApiError::bad_request(e.to_string()))
}

/// Write `raw` to `target` in `dir` atomically, then confirm it landed inside
/// `dir` (defense in depth; see `fs_safety::validate_within`).
fn write(target: &FsPath, dir: &FsPath, raw: &str) -> ApiResult<()> {
    std::fs::create_dir_all(dir).map_err(|e| ApiError::internal(e.to_string()))?;
    fs_safety::write_atomic(target, raw.as_bytes())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    fs_safety::validate_within(target, dir)
}

/// `PUT /api/agentiflow-defs/:name` — overwrite (or create) `:name`. The
/// file must parse and its `name:` must equal `:name`. With
/// `scope_kind`/`scope_id` it writes exactly that layer's existing file (404
/// otherwise); without, the project-first match is overwritten in place, and
/// a name that resolves nowhere is created in the global layer. Answers the
/// saved definition as `GET` does.
async fn write_def(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<DefQuery>,
    Json(body): Json<DefWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_local(q.host.as_deref(), "editing an agentiflow definition")?;
    writable_name(&name)?;
    crate::api::config::require_writable_to(&s, "editing an agentiflow definition")?;
    let def = parse(&body.raw)?;
    if def.name != name {
        return Err(ApiError::bad_request(format!(
            "the definition's name `{}` must equal the file name `{name}`",
            def.name
        )));
    }
    let r = match (q.scope_kind, resolve(&s, &name)) {
        (Some(_), _) => resolve_for(&s, &name, &q)?,
        (None, Some(r)) => r,
        (None, None) => {
            let dir = global_dir(&s);
            Resolved {
                path: dir.join(format!("{name}.yaml")),
                dir,
                scope: "global".into(),
                scope_kind: ScopeKind::Global,
                scope_id: None,
            }
        }
    };
    write(&r.path, &r.dir, &body.raw)?;
    detail(&name, &r)
}

/// `POST /api/agentiflow-defs` — create a definition in the global layer,
/// named by its `name:`. 409 when that name already resolves in any layer,
/// so a create never shadows (or is shadowed by) an existing definition.
async fn create_def(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
    Json(body): Json<DefWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    q.require_local("creating an agentiflow definition")?;
    crate::api::config::require_writable_to(&s, "creating an agentiflow definition")?;
    let name = parse(&body.raw)?.name;
    writable_name(&name)?;
    if resolve(&s, &name).is_some() {
        return Err(ApiError::conflict(format!(
            "agentiflow definition {name} already exists"
        )));
    }
    let dir = global_dir(&s);
    let r = Resolved {
        path: dir.join(format!("{name}.yaml")),
        dir,
        scope: "global".into(),
        scope_kind: ScopeKind::Global,
        scope_id: None,
    };
    write(&r.path, &r.dir, &body.raw)?;
    detail(&name, &r)
}

/// `DELETE /api/agentiflow-defs/:name` — remove the definition file the
/// selector (or, without one, the project-first match) names. Answers
/// `{deleted, scope, scope_kind}`.
async fn delete_def(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<DefQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_local(q.host.as_deref(), "deleting an agentiflow definition")?;
    fs_safety::validate_name(&name)?;
    crate::api::config::require_writable_to(&s, "deleting an agentiflow definition")?;
    let r = resolve_for(&s, &name, &q)?;
    fs_safety::validate_within(&r.path, &r.dir)?;
    std::fs::remove_file(&r.path).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "deleted": true,
        "scope": r.scope,
        "scope_kind": r.scope_kind,
    })))
}

/// Request body of `POST /api/agentiflow-defs/validate`.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ValidateBody {
    /// The whole definition file.
    raw: String,
    /// A registered workspace id: its `.rupu/profiles` join the engagement
    /// profiles the definition is checked against, as for a run started
    /// there. Without it, the built-in and global profiles only.
    #[serde(default)]
    scope_id: Option<String>,
}

/// `POST /api/agentiflow-defs/validate` — what `rupu agentiflow run` checks
/// before it starts anything (`AgentiflowDef::check`): the file parses, its
/// engagement profiles resolve, and the definition validates against them.
/// Writes nothing. Answers `{ok: true, name, lead, engagement_profiles}`, or
/// a 400 naming the first problem (404 for an unknown `scope_id`).
async fn validate_def(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
    Json(body): Json<ValidateBody>,
) -> ApiResult<Json<serde_json::Value>> {
    q.require_local("validating an agentiflow definition")?;
    let def = parse(&body.raw)?;
    let workspace = match body.scope_id.as_deref() {
        Some(id) => {
            let w = store(&s)
                .list()
                .unwrap_or_default()
                .into_iter()
                .find(|w| w.id == id)
                .ok_or_else(|| ApiError::not_found(format!("no registered project `{id}`")))?;
            Some(PathBuf::from(w.path))
        }
        None => None,
    };
    def.check(&s.global_dir, workspace.as_deref())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "name": def.name,
        "lead": def.lead,
        "engagement_profiles": def.engagement_profiles,
    })))
}
