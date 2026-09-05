use crate::{
    api::fs_safety,
    api::repo_scope::{distinct_repo_workspaces, scope_name, ScopeKind, ScopeQuery},
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    launcher::LaunchError,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use rupu_orchestrator::Workflow;
use rupu_workspace::{RepoRegistryStore, WorkspaceStore};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workflows", get(list_workflows).post(create_workflow))
        .route(
            "/api/workflows/:name",
            get(get_workflow)
                .put(write_workflow)
                .delete(delete_workflow),
        )
        .route("/api/workflows/:name/run", post(launch_run))
        // axum matches static literal segments before dynamic `:name` captures,
        // so this route is reachable and is NOT shadowed by `/api/workflows/:name`.
        .route("/api/workflows/validate", post(validate_workflow))
        .route("/api/workflows/generate", post(generate_workflow))
        .route("/api/generate/models", get(generate_models))
}

/// Directory where global workflow `.yaml` definitions live.
fn workflows_dir(s: &AppState) -> std::path::PathBuf {
    s.global_dir.join("workflows")
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

/// Resolve `name` to a workflow YAML path, the directory that contains it,
/// and the scope layer it resolved from: one representative workspace per
/// distinct repo among the registered projects FIRST (see
/// [`distinct_repo_workspaces`] — many registered workspaces are autoflow
/// run-worktrees of the very same repo, each carrying an identical copy of
/// `.rupu/workflows/`, so this avoids resolving against a stale
/// non-representative worktree), falling back to the global layer
/// (`"global"`, [`ScopeKind::Global`]) only when no registered project
/// defines `name`. First match wins; `None` when `name` resolves in neither
/// layer.
///
/// PROJECT-FIRST is deliberate, not incidental: every list this resolver
/// backs ([`list_workflows`], `api::autoflows::list_autoflow_defs`) shadows
/// a same-named GLOBAL row WITH the project row (`rows.retain(|r|
/// !project_names.contains(...))`), and the CLI runtime resolves the same
/// way (`rupu-cli/src/cmd/workflow.rs`: "we prefer the project shadow if
/// present and fall back to global"). A global-first resolver here — the
/// prior implementation — silently disagreed with what the operator's own
/// list showed them: for a name defined in BOTH layers, the list displayed
/// only the project row, but Delete/Enable/Disable resolved and removed the
/// HIDDEN global file, leaving the visible project row untouched (reading
/// as "delete did nothing" while quietly destroying a definition the
/// operator never saw). Project-first here closes that data-loss bug and
/// makes every consumer of this resolver agree with the list/runtime.
///
/// Using the SAME representative-selection [`list_workflows`] (and
/// `api::autoflows::list_autoflow_defs`) uses to build its rows closes the
/// mismatch a project-scoped row's Delete/Enable/Disable action used to have
/// (both call through here — see [`delete_workflow`] and
/// `api::autoflows::set_autoflow_enabled`): for a repo with several
/// registered worktrees, this resolver now always targets the SAME
/// worktree's copy the list/detail page showed, never a different one.
///
/// One residual ambiguity this does NOT resolve: two DIFFERENT repos that
/// each define a workflow with the same `name` both appear as distinct rows
/// in the list (see `different_repos_same_def_name_both_appear`), but
/// `:name` alone can't disambiguate which one a click meant — this resolver
/// (like `GET`/`PUT /api/workflows/:name` before it) can only ever return
/// the first such repo it finds (deterministic — `distinct_repo_workspaces`
/// sorts by `scope`). A caller that needs to pin down one specific repo's
/// row unambiguously should use [`resolve_workflow_scoped_explicit`]
/// instead, which every mutating web-UI caller now does (see
/// [`delete_workflow`]) by threading the row's `scope_id`.
///
/// No `host`/remote concept here, unlike `launch_run` below: `s.global_dir`
/// and every registered workspace's `path` are local filesystem paths on
/// THIS `rupu cp` process's own host — workflow definitions are never
/// fetched from or proxied to a remote host, so this resolver (and
/// therefore [`delete_workflow`] and `api::autoflows::set_autoflow_enabled`)
/// is correctly host-unaware.
pub(crate) fn resolve_workflow_scoped(
    s: &AppState,
    name: &str,
) -> Option<(
    std::path::PathBuf,
    std::path::PathBuf,
    String,
    ScopeKind,
    Option<String>,
)> {
    let workspaces = store(s).list().unwrap_or_default();
    for r in distinct_repo_workspaces(workspaces, &repo_store(s)) {
        let proj_dir = std::path::Path::new(&r.workspace.path)
            .join(".rupu")
            .join("workflows");
        let candidate = proj_dir.join(format!("{name}.yaml"));
        if candidate.exists() {
            return Some((
                candidate,
                proj_dir,
                r.scope,
                ScopeKind::Project,
                Some(r.workspace.id),
            ));
        }
    }
    let dir = workflows_dir(s);
    let global = dir.join(format!("{name}.yaml"));
    if global.exists() {
        return Some((global, dir, "global".to_string(), ScopeKind::Global, None));
    }
    None
}

/// Path-only convenience wrapper over [`resolve_workflow_scoped`], for
/// callers that only need the resolved path, not which layer it came from.
///
/// `pub(crate)` so `api::autoflows`'s enable/disable endpoint can reuse the
/// same project-aware resolution rather than re-deriving it.
pub(crate) fn resolve_workflow_path(s: &AppState, name: &str) -> Option<std::path::PathBuf> {
    resolve_workflow_scoped(s, name).map(|(path, _, _, _, _)| path)
}

/// Resolve `name` restricted to an EXPLICIT scope — the disambiguating
/// counterpart to [`resolve_workflow_scoped`]'s implicit (project-first,
/// then global) walk.
///
/// - `ScopeKind::Global` looks ONLY in the global workflows dir.
/// - `ScopeKind::Project` requires `scope_id` (the target row's registered
///   workspace `id` — NOT the display `scope` string, which can collide
///   between two different repos whose representative workspace paths share
///   a basename) and looks ONLY at that ONE workspace, found by matching
///   `scope_id` against the FULL registered-workspace list (`store(s).list()`)
///   — deliberately NOT [`distinct_repo_workspaces`]'s output. `/api/projects/
///   :ws_id/*` (see `api::projects`) reports the caller's REQUESTED `ws_id`
///   as `scope_id`, which for a repo with several registered worktrees is
///   often NOT the one worktree `distinct_repo_workspaces` would pick as
///   that repo's representative — matching only against representatives made
///   any such id resolve to `None` unconditionally, 404ing every mutating
///   action built on a non-representative project row. Any registered
///   workspace id is a valid target: if the operator is looking at workspace
///   X's definitions, a mutation must act on X's own files, never on some
///   other worktree's copy that happens to be the representative. A `None`
///   `scope_id` (or one matching no registered workspace) resolves to `None`
///   outright — no fallback to another workspace.
///
/// Returns `None` on ANY mismatch — file absent in the requested scope,
/// `scope_id` not found, etc. — rather than falling back to another layer.
/// Callers must treat `None` as 404, never silently retry with
/// [`resolve_workflow_scoped`] — that would defeat the whole point of an
/// operator picking a specific row to delete/toggle.
pub(crate) fn resolve_workflow_scoped_explicit(
    s: &AppState,
    name: &str,
    scope_kind: ScopeKind,
    scope_id: Option<&str>,
) -> Option<(std::path::PathBuf, std::path::PathBuf, String, ScopeKind)> {
    match scope_kind {
        ScopeKind::Global => {
            let dir = workflows_dir(s);
            let global = dir.join(format!("{name}.yaml"));
            if global.exists() {
                Some((global, dir, "global".to_string(), ScopeKind::Global))
            } else {
                None
            }
        }
        ScopeKind::Project => {
            let scope_id = scope_id?;
            let workspaces = store(s).list().unwrap_or_default();
            let w = workspaces.into_iter().find(|w| w.id == scope_id)?;
            let proj_dir = std::path::Path::new(&w.path)
                .join(".rupu")
                .join("workflows");
            let candidate = proj_dir.join(format!("{name}.yaml"));
            if candidate.exists() {
                Some((candidate, proj_dir, scope_name(&w), ScopeKind::Project))
            } else {
                None
            }
        }
    }
}

/// Path-only convenience wrapper over [`resolve_workflow_scoped_explicit`],
/// mirroring [`resolve_workflow_path`]. Used by
/// `api::autoflows::set_autoflow_enabled` when the request carries explicit
/// scope query params.
pub(crate) fn resolve_workflow_path_explicit(
    s: &AppState,
    name: &str,
    scope_kind: ScopeKind,
    scope_id: Option<&str>,
) -> Option<std::path::PathBuf> {
    resolve_workflow_scoped_explicit(s, name, scope_kind, scope_id).map(|(path, _, _, _)| path)
}

#[derive(Serialize)]
pub(crate) struct WorkflowDto {
    pub(crate) name: String,
    /// DISPLAY ONLY — a workspace path's basename for a project row, and can
    /// therefore legally equal the literal string `"global"`. Destructive
    /// gates must key off `scope_kind`, never this field — see
    /// `repo_scope::ScopeKind`'s doc comment.
    pub(crate) scope: String,
    /// Structured scope discriminator backing every Delete gate.
    pub(crate) scope_kind: ScopeKind,
    /// The underlying registered workspace's unique `id` for a Project row
    /// (`None` for a Global row). Unlike `scope` (a path basename, which can
    /// collide between two different repos), this is the genuinely unique
    /// identifier for "which repo" — pass it back as the `scope_id` query
    /// param on `DELETE`/enable-disable to pin the action to THIS row's file
    /// when another repo defines the same `name`. See
    /// `repo_scope::ScopeQuery`'s doc comment.
    pub(crate) scope_id: Option<String>,
    /// Aggregate token + cost usage across every run attributed to this
    /// workflow name. `RunRecord` records `workflow_name` alone (not which
    /// scope's definition produced the run), so `usage`/`run_count`/
    /// `last_run` are only populated on ONE canonical row per name (the
    /// `scope == "global"` row if one exists, else the first row for that
    /// name in sorted order) — every other same-named row (a different repo
    /// defining the same workflow name) is left zeroed rather than showing
    /// duplicated combined usage. Per-scope usage attribution is a follow-up.
    pub(crate) usage: crate::usage::UsageSummary,
    pub(crate) run_count: u64,
    pub(crate) last_run: Option<String>,
    /// `None` when the workflow YAML has no top-level `autoflow:` block at
    /// all (a plain, manually-launched workflow); `Some(enabled)` mirroring
    /// the on-disk `autoflow.enabled` when it does — enabled AND disabled
    /// autoflows both carry `Some`, so the Workflows list can offer an
    /// Enable/Disable toggle in both directions, the same way
    /// `api::autoflows::AutoflowDefRow::enabled` does for the dedicated
    /// autoflow-only view. Also `None` when the file fails to parse (a
    /// broken definition still gets a row — see `scan_workflow_names`'s doc
    /// comment — it just can't report autoflow state).
    pub(crate) autoflow_enabled: Option<bool>,
}

/// Scan `<dir>/*.yaml` and return one [`WorkflowDto`] per file stem, tagged
/// with `scope`, sorted by name. A missing/unreadable directory yields an
/// empty vec (tolerated, not an error) so the caller can merge layers freely.
///
/// Each file is also read and [`Workflow::parse`]d to populate
/// `autoflow_enabled` — this list previously only read file STEMS (no
/// parsing at all); it now pays the same per-file parse cost
/// `api::autoflows::scan_autoflow_defs` already does. Unlike that scan,
/// though, a file that fails to read or parse is NOT dropped from the
/// list — it still gets a row (`autoflow_enabled: None`, same as a plain
/// workflow), because this is the operator's primary workflow inventory:
/// silently hiding a broken definition here would read as "the file
/// disappeared" rather than "the file is broken."
pub(crate) fn scan_workflow_names(
    dir: &std::path::Path,
    scope: impl Into<String>,
    scope_kind: ScopeKind,
    scope_id: Option<String>,
) -> Vec<WorkflowDto> {
    let scope = scope.into();
    if !dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("workflows: could not read {}: {err}", dir.display());
            return vec![];
        }
    };
    let mut rows: Vec<WorkflowDto> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("yaml"))
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_stem().and_then(|s| s.to_str())?.to_string();
            let autoflow_enabled = std::fs::read_to_string(&path)
                .ok()
                .and_then(|body| Workflow::parse(&body).ok())
                .and_then(|wf| wf.autoflow.map(|a| a.enabled));
            Some(WorkflowDto {
                name,
                scope: scope.clone(),
                scope_kind,
                scope_id: scope_id.clone(),
                usage: crate::usage::UsageSummary::default(),
                run_count: 0,
                last_run: None,
                autoflow_enabled,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// `GET /api/workflows` — global workflow definitions plus one representative
/// workspace per distinct repo among the registered projects'
/// `<path>/.rupu/workflows/*.yaml` (see [`distinct_repo_workspaces`]), sorted
/// by name then scope. Many registered workspaces are autoflow run-worktrees
/// of the same repo; scanning every registered workspace would otherwise emit
/// one duplicate row per worktree.
///
/// Each row is tagged `scope: "global"` or the representative workspace's
/// path basename. A project def shadows a same-named GLOBAL row; two
/// different repos defining the same name both appear (distinguished by
/// `scope`). With no registered projects this is byte-for-byte the prior
/// global-only behavior.
async fn list_workflows(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkflowDto>>> {
    let mut rows = scan_workflow_names(
        &s.global_dir.join("workflows"),
        "global",
        ScopeKind::Global,
        None,
    );

    let workspaces = store(&s).list().unwrap_or_default();
    let repos = distinct_repo_workspaces(workspaces, &repo_store(&s));
    let mut project_rows: Vec<WorkflowDto> = Vec::new();
    for r in repos {
        let dir = std::path::Path::new(&r.workspace.path)
            .join(".rupu")
            .join("workflows");
        let scope_id = Some(r.workspace.id.clone());
        project_rows.extend(scan_workflow_names(
            &dir,
            r.scope,
            ScopeKind::Project,
            scope_id,
        ));
    }

    let project_names: std::collections::BTreeSet<&str> =
        project_rows.iter().map(|r| r.name.as_str()).collect();
    rows.retain(|r| !project_names.contains(r.name.as_str()));
    rows.extend(project_rows);
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.scope.cmp(&b.scope)));

    // Usage is keyed by `workflow_name` alone (`RunRecord` doesn't record
    // which scope's definition produced the run), so attach the rollup to
    // only ONE canonical row per name — preferring `scope == "global"`, else
    // the first row for that name in the already-sorted order — rather than
    // showing the same combined usage on every same-named row across
    // different repos. See the doc comment on `WorkflowDto::usage`.
    let runs = s.run_store.list().unwrap_or_default();
    let rollups = crate::usage::rollup_by(&s.run_store, &runs, &s.pricing, |r| {
        Some(r.workflow_name.clone())
    });
    let mut canonical_row_for_name: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        canonical_row_for_name
            .entry(row.name.clone())
            .and_modify(|idx| {
                if row.scope == "global" {
                    *idx = i;
                }
            })
            .or_insert(i);
    }
    for (name, idx) in canonical_row_for_name {
        if let Some(roll) = rollups.get(&name) {
            rows[idx].usage = roll.usage.clone();
            rows[idx].run_count = roll.run_count;
            rows[idx].last_run = roll.last_active.clone();
        }
    }
    Ok(Json(rows))
}

/// Load workflow `name` and build the full detail DTO (`workflow` + raw `yaml` +
/// aggregate `usage` + resolved `scope`/`scope_kind`/`scope_id`). Shared by
/// GET / PUT / POST.
///
/// Project-aware: resolves `name` project-first, falling back to the global
/// layer (see [`resolve_workflow_scoped`]'s doc comment for why) so a
/// project-only workflow's detail route doesn't 404. The resolved layer is
/// carried on the response's `scope`/`scope_kind`/`scope_id`, which the CP
/// detail page shows (scope chip + naming the layer in the delete
/// confirmation) and threads back on its own Delete/toggle calls —
/// `DELETE /api/workflows/:name` independently resolves the SAME way (see
/// [`resolve_workflow_scoped`]), so Delete always removes the file this page
/// is actually showing when no explicit scope is passed.
fn load_detail(s: &AppState, name: &str) -> ApiResult<Json<serde_json::Value>> {
    let (path, _dir, scope, scope_kind, scope_id) = resolve_workflow_scoped(s, name)
        .ok_or_else(|| ApiError::not_found(format!("workflow {name} not found")))?;
    let yaml = std::fs::read_to_string(&path).map_err(|e| ApiError::internal(e.to_string()))?;
    let workflow = Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;

    let runs = s.run_store.list().unwrap_or_default();
    let usage =
        crate::usage::rollup(runs.iter().filter(|r| r.workflow_name == name).map(|r| {
            crate::usage::summarize_run_resolved(&s.run_store, &s.hosts, &r.id, &s.pricing)
        }));

    Ok(Json(serde_json::json!({
        "workflow": workflow,
        "yaml": yaml,
        "usage": usage,
        "scope": scope,
        "scope_kind": scope_kind,
        "scope_id": scope_id,
    })))
}

async fn get_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    load_detail(&s, &name)
}

/// Request body for `PUT /api/workflows/:name` and `POST /api/workflows`: the
/// full raw `.yaml` to validate and persist.
#[derive(Deserialize)]
struct WorkflowWriteBody {
    raw: String,
}

/// `PUT /api/workflows/:name` — overwrite (or create) the workflow definition
/// `:name`. The raw `.yaml` is validated by [`Workflow::parse`] before any
/// write; the parsed `name:` must equal `:name`.
///
/// Accepts optional `?scope_kind=global|project&scope_id=<workspace id>`
/// query params (see [`ScopeQuery`]'s doc comment) — the SAME selector
/// [`delete_workflow`] takes.
///
/// - An explicit selector resolves STRICTLY via
///   [`resolve_workflow_scoped_explicit`] and writes to that ONE layer; 404
///   on any mismatch (wrong layer, absent file, unknown `scope_id`) rather
///   than a silent fallback to another layer — exactly like `DELETE`, and
///   nothing is written when it 404s.
/// - No selector (older clients, or a name not yet known to resolve
///   anywhere): resolves via the implicit project-first walk
///   [`resolve_workflow_scoped`] — the SAME resolver `GET`/`DELETE` use. A
///   name that already resolves (in EITHER layer) is overwritten IN PLACE
///   there; a name that resolves NOWHERE is created fresh in the global
///   layer (unchanged behavior for a genuinely new definition).
///
/// This closes the operator-visible bug where the old global-only PUT wrote
/// an edit to a HIDDEN new global file while a same-named PROJECT file (the
/// one the editor was actually showing) was left untouched — reading as
/// "Save did nothing" while quietly leaving a shadow copy the list would
/// never surface (project shadows global). See [`resolve_workflow_scoped`]'s
/// doc comment for the identical reasoning `DELETE` already applies.
///
/// Returns the reloaded detail DTO — [`load_detail`] resolves the SAME
/// project-first way, so the response always echoes the file this handler
/// actually just wrote.
async fn write_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ScopeQuery>,
    Json(body): Json<WorkflowWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    fs_safety::validate_name(&name)?;
    let wf = Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if wf.name != name {
        return Err(ApiError::bad_request(
            "workflow name must equal the workflow file name",
        ));
    }

    let (target, dir) = match q.scope_kind {
        Some(kind) => {
            let (path, dir, _scope, _kind) =
                resolve_workflow_scoped_explicit(&s, &name, kind, q.scope_id.as_deref())
                    .ok_or_else(|| {
                        ApiError::not_found(format!(
                            "workflow {name} not found in the requested scope"
                        ))
                    })?;
            (path, dir)
        }
        None => match resolve_workflow_scoped(&s, &name) {
            Some((path, dir, _scope, _kind, _scope_id)) => (path, dir),
            None => {
                let dir = workflows_dir(&s);
                (dir.join(format!("{name}.yaml")), dir)
            }
        },
    };
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    fs_safety::write_atomic(&target, body.raw.as_bytes())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // Defense in depth, mirroring `delete_workflow`'s `validate_within` — run
    // AFTER the write here (rather than before, as delete does) because
    // `target` may not exist yet on disk when this is a genuine create.
    // `target/dir` above are always constructed from a `validate_name`-
    // checked identifier joined onto a trusted directory, so this can never
    // actually fire today; see `fs_safety::validate_within`'s doc comment.
    fs_safety::validate_within(&target, &dir)?;
    load_detail(&s, &name)
}

/// `POST /api/workflows` — create a new workflow. The name is taken from the
/// parsed `name:`; fails with 409 if a definition with that name already
/// resolves ANYWHERE (project-first, then global — the SAME resolver
/// `GET`/`PUT`/`DELETE` use), not just the global layer, so create can never
/// silently shadow an existing PROJECT definition with a brand-new global
/// file of the same name. Always writes into the global layer (creating
/// directly into a specific project via this endpoint is not supported).
/// Returns the reloaded detail DTO.
async fn create_workflow(
    State(s): State<AppState>,
    Json(body): Json<WorkflowWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let wf = Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let name = wf.name;
    fs_safety::validate_name(&name)?;
    if resolve_workflow_scoped(&s, &name).is_some() {
        return Err(ApiError::conflict("workflow already exists"));
    }
    let dir = workflows_dir(&s);
    let target = dir.join(format!("{name}.yaml"));
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    fs_safety::write_atomic(&target, body.raw.as_bytes())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    load_detail(&s, &name)
}

/// `POST /api/workflows/validate` — stateless parse-check of a raw workflow
/// `.yaml`. Takes no [`State`] and touches no filesystem: it only runs
/// [`Workflow::parse`] and reports `{ "ok": true }` on success or a 400 with the
/// parse error message on failure. Backs the editor's live valid/invalid badge.
async fn validate_workflow(
    Json(body): Json<WorkflowWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `DELETE /api/workflows/:name` — remove the workflow definition `:name`.
///
/// Accepts optional `?scope_kind=global|project&scope_id=<workspace id>`
/// query params (see [`ScopeQuery`]'s doc comment). When present, resolution
/// is EXPLICIT via [`resolve_workflow_scoped_explicit`] — restricted to
/// exactly that layer/workspace, 404 on any mismatch (wrong layer, absent
/// file, unknown `scope_id`) rather than falling back to another layer. When
/// absent (older clients), falls back to the implicit project-first resolver
/// [`resolve_workflow_scoped`] — the SAME resolver `GET`/`PUT
/// /api/workflows/:name` use — so a project-scoped workflow's Delete
/// row-action still removes the actual project file the row/detail page
/// displays, not a same-named global file it shadows (the data-loss bug PR
/// #536 worked around by hiding Delete on non-global rows entirely, and a
/// LATER bug this fixes: the implicit resolver briefly resolved
/// global-first, backwards from what the list/CLI runtime prefer — see
/// [`resolve_workflow_scoped`]'s doc comment).
///
/// Returns the resolved `scope`/`scope_kind` alongside `deleted: true` so the
/// caller can confirm which layer's file was actually removed. Local-only,
/// no `?host=` — see [`resolve_workflow_scoped`]'s doc comment.
async fn delete_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    fs_safety::validate_name(&name)?;
    let (target, dir, scope, scope_kind) = match q.scope_kind {
        Some(kind) => resolve_workflow_scoped_explicit(&s, &name, kind, q.scope_id.as_deref())
            .ok_or_else(|| {
                ApiError::not_found(format!("workflow {name} not found in the requested scope"))
            })?,
        None => {
            let (target, dir, scope, scope_kind, _scope_id) = resolve_workflow_scoped(&s, &name)
                .ok_or_else(|| ApiError::not_found(format!("workflow {name} not found")))?;
            (target, dir, scope, scope_kind)
        }
    };
    // Defense in depth — see `fs_safety::validate_within`'s doc comment.
    fs_safety::validate_within(&target, &dir)?;
    std::fs::remove_file(&target).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "deleted": true,
        "scope": scope,
        "scope_kind": scope_kind,
    })))
}

/// Request body for `POST /api/workflows/:name/run`. All fields optional; a
/// bodyless POST launches the workflow with no inputs in its default mode.
#[derive(Deserialize, Default)]
struct LaunchBody {
    #[serde(default)]
    inputs: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    /// Optional host id. Absent or `"local"` → local path (including the
    /// existing 501 when no launcher is installed). A remote id proxies via
    /// [`HostConnector::launch_run`] and returns `{ "run_id", "host_id" }`.
    #[serde(default)]
    host: Option<String>,
    /// Optional scope selector — same shape/semantics as
    /// `api::agents::AgentRunBody::scope_kind`/`scope_id`; see
    /// [`resolve_launch_scope`].
    #[serde(default)]
    scope_kind: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
}

/// Resolve an optional `scope_kind`/`scope_id` selector on [`LaunchBody`] to
/// a working-dir override. The workflow-launch twin of
/// `api::agents::resolve_launch_scope` — see that function's doc comment for
/// the full contract: back-compat when both are absent, 400 on an
/// unrecognized `scope_kind`, 404 naming the scope on any resolution
/// mismatch, LOCAL-ONLY (only ever called from [`launch_run`]'s local
/// branch), `Project` returns the workspace ROOT so `rupu workflow run`'s
/// cwd-based discovery finds the SAME `.rupu/workflows/<name>.yaml` the
/// picker resolved, `Global` applies the identical trick one level higher
/// (pointing cwd at `AppState::global_dir`'s PARENT so `project_root_for`'s
/// walk hits the global dir itself first) — GUARDED: only when `global_dir`
/// is literally named `.rupu`; otherwise 400 (loud beats wrong) rather than
/// silently proceeding without an override under a non-default `$RUPU_HOME`.
/// Applies here to [`resolve_workflow_scoped_explicit`] instead of the agent
/// resolver.
fn resolve_launch_scope(
    s: &AppState,
    name: &str,
    scope_kind: Option<&str>,
    scope_id: Option<&str>,
) -> Result<Option<std::path::PathBuf>, ApiError> {
    let Some(raw_kind) = scope_kind else {
        return Ok(None);
    };
    let kind: ScopeKind = raw_kind.parse().map_err(|_| {
        ApiError::bad_request(format!(
            "unknown scope_kind {raw_kind:?}; expected \"global\" or \"project\""
        ))
    })?;
    let (_path, dir, _scope, resolved_kind) =
        resolve_workflow_scoped_explicit(s, name, kind, scope_id).ok_or_else(|| {
            ApiError::not_found(format!("workflow {name} not found in the requested scope"))
        })?;
    match resolved_kind {
        // dir == <workspace root>/.rupu/workflows; strip both segments to
        // reach the workspace root the picker's Project row targets.
        ScopeKind::Project => Ok(dir
            .parent()
            .and_then(|p| p.parent())
            .map(std::path::PathBuf::from)),
        // dir == <global_dir>/workflows; dir.parent() == global_dir itself.
        ScopeKind::Global => {
            let global_dir = dir
                .parent()
                .ok_or_else(|| ApiError::internal("resolved global workflows dir has no parent"))?;
            if global_dir.file_name() != Some(std::ffi::OsStr::new(".rupu")) {
                return Err(ApiError::bad_request(
                    "global-scope launch pinning isn't available: RUPU_HOME is set to a \
                     directory not named \".rupu\", so the cwd-based trick that forces \
                     global-only resolution can't be applied here",
                ));
            }
            let global_parent = global_dir
                .parent()
                .ok_or_else(|| ApiError::internal("global dir has no parent"))?;
            Ok(Some(global_parent.to_path_buf()))
        }
    }
}

/// Start a fresh run of `:name` via the configured [`RunLauncher`] (local) or
/// by proxying to a remote host. Returns the new run id plus the owning
/// `host_id`. 501 when no launcher is installed and the target is local.
///
/// Scope (`scope_kind`/`scope_id`) is a LOCAL-ONLY affordance (ruled): a
/// remote-targeted launch skips scope validation and the working_dir
/// override entirely and behaves exactly as before — see
/// `api::agents::run_agent`'s identical doc comment for the full rationale.
///
/// [`RunLauncher`]: crate::launcher::RunLauncher
async fn launch_run(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<LaunchBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut b = body.map(|j| j.0).unwrap_or_default();
    let host = b.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        // Remote: scope fields, if present, are silently ignored — see this
        // function's doc comment.
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = crate::launcher::LaunchRequest {
            workflow: name,
            inputs: b.inputs,
            mode: b.mode,
            target: b.target,
            working_dir: b.working_dir,
        };
        let run_id = conn.launch_run(req).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": host }),
        ));
    }

    // Local path from here. `scope_kind` and a body-supplied `working_dir`
    // are mutually exclusive — see `api::agents::run_agent`'s identical
    // comment for the rationale (no existing caller sends both).
    if b.scope_kind.is_some() && b.working_dir.is_some() {
        return Err(ApiError::bad_request(
            "scope_kind and working_dir are mutually exclusive — the scope selector determines the working directory",
        ));
    }
    let scope_working_dir =
        resolve_launch_scope(&s, &name, b.scope_kind.as_deref(), b.scope_id.as_deref())?;
    // Unchanged local behavior (including the 501 when no launcher is
    // installed) except for the scope-driven working_dir override computed
    // above, which only ever fills in a working_dir the body left unset.
    if b.working_dir.is_none() {
        b.working_dir = scope_working_dir.map(|p| p.display().to_string());
    }
    let launcher = s
        .launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("launching runs requires `rupu cp serve`"))?;
    let req = crate::launcher::LaunchRequest {
        workflow: name,
        inputs: b.inputs,
        mode: b.mode,
        target: b.target,
        working_dir: b.working_dir,
    };
    match launcher.launch(req).await {
        Ok(run_id) => Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": "local" }),
        )),
        Err(LaunchError::Invalid(m)) => Err(ApiError::bad_request(m)),
        Err(LaunchError::Spawn(m)) => Err(ApiError::internal(m)),
    }
}

#[derive(serde::Deserialize)]
struct GenerateWorkflowBody {
    description: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct GeneratedWfDto {
    raw: String,
    provider: String,
    model: String,
    attempts: u8,
}

#[derive(serde::Serialize)]
struct ProviderModelsDto {
    provider: String,
    models: Vec<String>,
    is_default: bool,
}

async fn generate_workflow(
    State(s): State<AppState>,
    Json(body): Json<GenerateWorkflowBody>,
) -> ApiResult<Json<GeneratedWfDto>> {
    use crate::definition_generator::{DefKind, GenDefError, GenerateDefRequest};
    let gen = s
        .generator
        .clone()
        .ok_or_else(|| ApiError::not_available("AI generation requires `rupu cp serve`"))?;
    let out = gen
        .generate(GenerateDefRequest {
            kind: DefKind::Workflow,
            description: body.description,
            provider: body.provider,
            model: body.model,
        })
        .await
        .map_err(|e| match e {
            GenDefError::NoCredentials => ApiError::bad_request(e.to_string()),
            GenDefError::Failed(m) => ApiError::internal(m),
        })?;
    Ok(Json(GeneratedWfDto {
        raw: out.raw,
        provider: out.provider,
        model: out.model,
        attempts: out.attempts,
    }))
}

async fn generate_models(State(s): State<AppState>) -> Json<Vec<ProviderModelsDto>> {
    let list = match &s.generator {
        Some(g) => g
            .available_models()
            .await
            .into_iter()
            .map(|p| ProviderModelsDto {
                provider: p.provider,
                models: p.models,
                is_default: p.is_default,
            })
            .collect(),
        None => Vec::new(),
    };
    Json(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{LaunchError, LaunchRequest, RunLauncher};
    use std::sync::{Arc, Mutex};

    /// Captures the last `LaunchRequest` and returns a canned run id.
    struct MockLauncher {
        last: Mutex<Option<LaunchRequest>>,
        run_id: String,
    }

    #[async_trait::async_trait]
    impl RunLauncher for MockLauncher {
        async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError> {
            *self.last.lock().unwrap() = Some(req);
            Ok(self.run_id.clone())
        }
    }

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
    }

    #[tokio::test]
    async fn launch_run_invokes_launcher_and_returns_run_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_xyz".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        let mut inputs = std::collections::BTreeMap::new();
        inputs.insert("repo".to_string(), "acme/widgets".to_string());
        let body = LaunchBody {
            inputs: inputs.clone(),
            mode: Some("bypass".into()),
            target: None,
            working_dir: None,
            host: None,
            scope_kind: None,
            scope_id: None,
        };

        let resp = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect("launch should succeed");
        assert_eq!(resp.0["run_id"], "run_xyz");

        let captured = mock.last.lock().unwrap().clone().expect("request captured");
        assert_eq!(captured.workflow, "nightly");
        assert_eq!(captured.inputs, inputs);
        assert_eq!(captured.mode.as_deref(), Some("bypass"));
        assert_eq!(captured.target, None);
    }

    #[tokio::test]
    async fn launch_forwards_working_dir() {
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_X".into(),
        });
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));
        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: Some("/tmp/projX".into()),
            host: None,
            scope_kind: None,
            scope_id: None,
        };
        let _ = launch_run(State(s), Path("nightly".into()), Some(Json(body))).await;
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/projX"));
    }

    // ── Scope-aware launch (Phase 5A Task 2) ────────────────────────────────
    //
    // `launch_run` used to resolve `:name` purely as a side effect of the
    // spawned subprocess's cwd — completely independent of any scope a
    // picker resolved. For a name defined BOTH globally AND in a project,
    // that meant the WRONG definition could launch. See
    // `resolve_launch_scope`'s doc comment for the full contract; mirrors
    // `api::agents`'s identical fix for `run_agent`/`start_session`.

    const VALID_NIGHTLY_YAML: &str =
        "name: nightly\nsteps:\n  - id: one\n    agent: x\n    prompt: hi\n";

    /// A global `nightly` AND a project `nightly` both exist. Passing the
    /// project's explicit scope resolves to that ONE definition: proven by
    /// the launcher receiving the PROJECT ROOT as `working_dir` (not
    /// `.rupu/workflows` itself — `rupu workflow run` discovers
    /// `.rupu/workflows/<name>.yaml` by walking UP from cwd).
    #[tokio::test]
    async fn launch_run_scope_kind_project_resolves_project_definition_over_global_collision() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_scoped".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(workflows_dir(&s).join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None, // NOT set — the scope override must fill this in.
            host: None,
            scope_kind: Some("project".into()),
            scope_id: Some("ws_a".into()),
        };
        let _ = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect("project-scoped launch should resolve");

        let got = mock.last.lock().unwrap().clone().expect("request captured");
        assert_eq!(
            got.working_dir.as_deref(),
            Some(proj.path().display().to_string().as_str())
        );
    }

    /// No `scope_kind`/`scope_id` on the body → byte-for-byte unchanged
    /// behavior: no working_dir is synthesized, even though a project
    /// definition of the same name exists and could have been resolved.
    #[tokio::test]
    async fn launch_run_absent_scope_leaves_working_dir_untouched_back_compat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_unscoped".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None,
            host: None,
            scope_kind: None,
            scope_id: None,
        };
        let _ = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect("no-scope launch should proceed exactly as before");

        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(
            got.working_dir, None,
            "absent scope must not synthesize a working_dir"
        );
    }

    /// An unrecognized `scope_kind` string is a 400 the handler controls,
    /// and nothing launches.
    #[tokio::test]
    async fn launch_run_scope_kind_unrecognized_value_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "unused".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None,
            host: None,
            scope_kind: Some("bogus".into()),
            scope_id: None,
        };
        let err = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect_err("unrecognized scope_kind must 400");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            mock.last.lock().unwrap().is_none(),
            "must not launch on an invalid scope selector"
        );
    }

    /// `scope_kind: project` with a `scope_id` naming no registered
    /// workspace resolves to nothing → 404, never a silent fallback to
    /// another layer, and nothing launches.
    #[tokio::test]
    async fn launch_run_scope_project_unresolvable_combo_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "unused".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None,
            host: None,
            scope_kind: Some("project".into()),
            scope_id: Some("ws_missing".into()),
        };
        let err = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect_err("unresolvable scope must 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
        assert!(mock.last.lock().unwrap().is_none());
    }

    /// `scope_kind: global` launches the GLOBAL definition even when the
    /// spawned subprocess's cwd (absent an override) would otherwise sit
    /// inside a SHADOWING project of the same name — same mechanism as
    /// `api::agents::run_agent`'s identical test, applied to workflows.
    #[tokio::test]
    async fn launch_run_scope_kind_global_resolves_global_definition_over_project_collision() {
        let tmp = tempfile::TempDir::new().unwrap();
        // global_dir MUST be literally named `.rupu` for the cwd trick to
        // apply — build AppState directly on a `.rupu`-named child.
        let global_dir = tmp.path().join(".rupu");
        std::fs::create_dir_all(&global_dir).unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_global".into(),
        });
        let s = AppState::new(global_dir.clone(), rupu_config::PricingConfig::default())
            .with_workspace_dir(global_dir.clone())
            .with_launcher(Some(mock.clone()));

        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(workflows_dir(&s).join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None,
            host: None,
            scope_kind: Some("global".into()),
            scope_id: None,
        };
        let _ = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect("global-scoped launch should resolve");

        let got = mock.last.lock().unwrap().clone().expect("request captured");
        assert_eq!(
            got.working_dir.as_deref(),
            Some(global_dir.parent().unwrap().display().to_string().as_str()),
            "working_dir must be global_dir's PARENT, not global_dir itself"
        );
    }

    /// `scope_kind: global` under a `$RUPU_HOME`-style global dir NOT named
    /// `.rupu` can't be pinned by the cwd trick — 400, never a silent no-op.
    #[tokio::test]
    async fn launch_run_scope_kind_global_under_non_dot_rupu_home_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global_dir = tmp.path().join("custom-home");
        std::fs::create_dir_all(&global_dir).unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "unused".into(),
        });
        let s = AppState::new(global_dir.clone(), rupu_config::PricingConfig::default())
            .with_workspace_dir(global_dir.clone())
            .with_launcher(Some(mock.clone()));

        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(workflows_dir(&s).join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None,
            host: None,
            scope_kind: Some("global".into()),
            scope_id: None,
        };
        let err = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect_err("global pinning under a non-\".rupu\" RUPU_HOME must 400");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(mock.last.lock().unwrap().is_none());
    }

    /// `scope_kind` and a body-supplied `working_dir` are mutually
    /// exclusive — same rationale as `api::agents::run_agent`'s identical
    /// test. Nothing launches.
    #[tokio::test]
    async fn launch_run_scope_kind_with_explicit_working_dir_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "unused".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("nightly.yaml"), VALID_NIGHTLY_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: Some("/explicit/caller/dir".into()),
            host: None,
            scope_kind: Some("project".into()),
            scope_id: Some("ws_a".into()),
        };
        let err = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect_err("scope_kind + working_dir together must 400");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(mock.last.lock().unwrap().is_none());
    }

    /// A REMOTE-targeted launch ignores scope fields entirely — scope is a
    /// LOCAL-ONLY affordance (ruled). A project unknown to THIS process's
    /// local workspace registry must not 404 a remote launch.
    #[tokio::test]
    async fn launch_run_remote_host_ignores_scope_fields_and_proxies_unmodified() {
        let remote = httpmock::MockServer::start_async().await;
        let m = remote.mock(|when, then| {
            when.method("POST").path("/api/workflows/nightly/run");
            then.status(200)
                .json_body(serde_json::json!({ "run_id": "remote_1", "host_id": "local" }));
        });

        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let host = s
            .hosts
            .add_host("test-remote", &remote.base_url(), None)
            .expect("add_host should succeed");

        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: None,
            host: Some(host.id.clone()),
            scope_kind: Some("project".into()),
            scope_id: Some("ws_unknown_locally".into()),
        };
        let resp = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect("remote launch must succeed exactly like a scope-less remote launch");
        assert_eq!(resp.0["run_id"], "remote_1");
        assert_eq!(resp.0["host_id"], host.id);
        m.assert();
    }

    #[tokio::test]
    async fn launch_run_without_launcher_is_not_implemented() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // launcher: None

        let err = launch_run(State(s), Path("nightly".into()), None)
            .await
            .expect_err("no launcher should error");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    const VALID_YAML: &str = "name: demo\nsteps:\n  - id: one\n    agent: x\n    prompt: hi\n";

    fn wf_path(s: &AppState, name: &str) -> std::path::PathBuf {
        workflows_dir(s).join(format!("{name}.yaml"))
    }

    #[tokio::test]
    async fn put_valid_writes_and_reloads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let resp = write_workflow(
            State(s.clone()),
            Path("demo".into()),
            Query(ScopeQuery::default()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect("put ok");
        assert_eq!(resp.0["yaml"], serde_json::json!(VALID_YAML));
        assert_eq!(
            std::fs::read_to_string(wf_path(&s, "demo")).unwrap(),
            VALID_YAML
        );

        // Re-reading via get_workflow returns the new yaml.
        let got = get_workflow(State(s.clone()), Path("demo".into()))
            .await
            .expect("get ok");
        assert_eq!(got.0["yaml"], serde_json::json!(VALID_YAML));
    }

    #[tokio::test]
    async fn put_unparseable_is_bad_request_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let err = write_workflow(
            State(s.clone()),
            Path("demo".into()),
            Query(ScopeQuery::default()),
            Json(WorkflowWriteBody { raw: "".into() }),
        )
        .await
        .expect_err("should reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!wf_path(&s, "demo").exists());
    }

    #[tokio::test]
    async fn put_name_mismatch_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let err = write_workflow(
            State(s.clone()),
            Path("other".into()),
            Query(ScopeQuery::default()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect_err("mismatch");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!wf_path(&s, "other").exists());
    }

    #[tokio::test]
    async fn post_creates_then_conflicts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let resp = create_workflow(
            State(s.clone()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect("create ok");
        assert_eq!(resp.0["yaml"], serde_json::json!(VALID_YAML));
        assert_eq!(
            std::fs::read_to_string(wf_path(&s, "demo")).unwrap(),
            VALID_YAML
        );

        let err = create_workflow(
            State(s.clone()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect_err("conflict");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    /// A name that already resolves in a PROJECT must not be silently
    /// shadowed by a brand-new global file of the same name — `POST` must
    /// 409, project-aware, not just "absent from global."
    #[tokio::test]
    async fn post_conflicts_on_project_only_name_without_creating_a_global_shadow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("demo.yaml"), VALID_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let err = create_workflow(
            State(s.clone()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect_err("must conflict against the project-only definition");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
        assert!(
            !wf_path(&s, "demo").exists(),
            "no hidden global shadow must be created"
        );
    }

    // ── PUT scope-awareness (write-path parity with DELETE) ───────────────
    //
    // `write_workflow` used to unconditionally write to the GLOBAL layer,
    // then return `load_detail`, which (like `GET`/`DELETE`) resolves
    // project-first. For a project-only workflow this meant: PUT silently
    // created a HIDDEN new global file holding the edit, while `load_detail`
    // echoed back the UNCHANGED project file — reading as "Save did
    // nothing" while quietly leaving a shadow copy the list would never
    // show. These tests are the seeded-collision regression suite for the
    // fix: an explicit `?scope_kind=&scope_id=` selector (the SAME shape
    // `DELETE` takes) pins the write to exactly one layer, 404ing rather
    // than falling back, and — even with NO selector — a name that already
    // resolves somewhere is edited IN PLACE there rather than shadowed.

    /// (a) Saving a PROJECT-scoped def with an explicit project selector
    /// writes the PROJECT file and creates NO global file.
    #[tokio::test]
    async fn put_explicit_project_selector_writes_project_creates_no_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("nightly-sweep.yaml"),
            VALID_YAML.replace("demo", "nightly-sweep"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());
        let ws_id = "ws_a".to_string();

        let edited = VALID_YAML
            .replace("demo", "nightly-sweep")
            .replace("hi", "edited");
        let resp = write_workflow(
            State(s.clone()),
            Path("nightly-sweep".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some(ws_id),
            }),
            Json(WorkflowWriteBody {
                raw: edited.clone(),
            }),
        )
        .await
        .expect("explicit project-scoped put should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert_eq!(
            std::fs::read_to_string(proj_workflows.join("nightly-sweep.yaml")).unwrap(),
            edited,
            "the PROJECT file must hold the edit"
        );
        assert!(
            !wf_path(&s, "nightly-sweep").exists(),
            "no hidden GLOBAL file must be created"
        );
    }

    /// (b) Same name present in BOTH layers, explicit project selector → the
    /// project file changes, the global file is byte-unchanged.
    #[tokio::test]
    async fn put_explicit_project_selector_with_name_in_both_layers_leaves_global_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        let global_yaml = VALID_YAML.replace("demo", "shared-name");
        std::fs::write(wf_path(&s, "shared-name"), &global_yaml).unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("shared-name.yaml"),
            VALID_YAML.replace("demo", "shared-name"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());
        let ws_id = "ws_a".to_string();

        let edited = VALID_YAML
            .replace("demo", "shared-name")
            .replace("hi", "edited-in-project");
        let resp = write_workflow(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some(ws_id),
            }),
            Json(WorkflowWriteBody {
                raw: edited.clone(),
            }),
        )
        .await
        .expect("explicit project-scoped put should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert_eq!(
            std::fs::read_to_string(proj_workflows.join("shared-name.yaml")).unwrap(),
            edited,
            "the PROJECT file must hold the edit"
        );
        assert_eq!(
            std::fs::read_to_string(wf_path(&s, "shared-name")).unwrap(),
            global_yaml,
            "the GLOBAL file must be byte-unchanged"
        );
    }

    /// (c) An explicit selector pointing at a layer that doesn't hold the
    /// def → 404, and NOTHING is written anywhere.
    #[tokio::test]
    async fn put_explicit_scope_mismatch_404s_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        let global_yaml = VALID_YAML.replace("demo", "shared-name");
        std::fs::write(wf_path(&s, "shared-name"), &global_yaml).unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        let proj_yaml = VALID_YAML.replace("demo", "shared-name");
        std::fs::write(proj_workflows.join("shared-name.yaml"), &proj_yaml).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let err = write_workflow(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("no-such-workspace".to_string()),
            }),
            Json(WorkflowWriteBody {
                raw: VALID_YAML
                    .replace("demo", "shared-name")
                    .replace("hi", "should-not-land"),
            }),
        )
        .await
        .expect_err("mismatched explicit scope must 404, never fall back");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            std::fs::read_to_string(wf_path(&s, "shared-name")).unwrap(),
            global_yaml,
            "global file untouched"
        );
        assert_eq!(
            std::fs::read_to_string(proj_workflows.join("shared-name.yaml")).unwrap(),
            proj_yaml,
            "project file untouched"
        );
    }

    /// (d) No selector + name resolves only in a project → writes the
    /// PROJECT file, still no global shadow created.
    #[tokio::test]
    async fn put_no_selector_project_only_name_writes_project_no_global_shadow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("nightly-sweep.yaml"),
            VALID_YAML.replace("demo", "nightly-sweep"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let edited = VALID_YAML
            .replace("demo", "nightly-sweep")
            .replace("hi", "edited");
        let resp = write_workflow(
            State(s.clone()),
            Path("nightly-sweep".into()),
            Query(ScopeQuery::default()),
            Json(WorkflowWriteBody {
                raw: edited.clone(),
            }),
        )
        .await
        .expect("implicit project-first put should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert_eq!(
            std::fs::read_to_string(proj_workflows.join("nightly-sweep.yaml")).unwrap(),
            edited,
            "the PROJECT file must hold the edit"
        );
        assert!(
            !wf_path(&s, "nightly-sweep").exists(),
            "no hidden GLOBAL shadow must be created"
        );
    }

    // (e) "no selector + name resolves nowhere → creates in global" is
    // already covered by `put_valid_writes_and_reloads` above (an empty
    // fixture with no pre-seeded file in either layer) — unchanged behavior.

    // `validate_workflow` is stateless: it takes no `State` and touches no
    // filesystem — it only parse-checks the raw YAML.
    #[tokio::test]
    async fn validate_valid_yaml_is_ok() {
        let resp = validate_workflow(Json(WorkflowWriteBody {
            raw: VALID_YAML.into(),
        }))
        .await
        .expect("valid yaml should validate");
        assert_eq!(resp.0["ok"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn validate_unparseable_is_bad_request() {
        // An empty/invalid workflow (no steps) fails `Workflow::parse`.
        let err = validate_workflow(Json(WorkflowWriteBody {
            raw: "steps: []".into(),
        }))
        .await
        .expect_err("invalid workflow should reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_present_then_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "demo"), VALID_YAML).unwrap();

        let resp = delete_workflow(
            State(s.clone()),
            Path("demo".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("delete ok");
        assert_eq!(resp.0["deleted"], serde_json::json!(true));
        assert_eq!(resp.0["scope"], serde_json::json!("global"));
        assert_eq!(resp.0["scope_kind"], serde_json::json!("global"));
        assert!(!wf_path(&s, "demo").exists());

        let err = delete_workflow(
            State(s.clone()),
            Path("demo".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect_err("absent");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// Regression for the operator complaint this PR fixes: a project-scoped
    /// workflow's Delete row-action must remove the actual PROJECT file, and
    /// leave an unrelated global definition byte-for-byte untouched.
    #[tokio::test]
    async fn delete_removes_project_file_leaves_unrelated_global_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "keeper"), VALID_YAML.replace("demo", "keeper")).unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("nightly-sweep.yaml"),
            VALID_YAML.replace("demo", "nightly-sweep"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let resp = delete_workflow(
            State(s.clone()),
            Path("nightly-sweep".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("project-scoped delete should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert!(
            !proj_workflows.join("nightly-sweep.yaml").exists(),
            "the PROJECT file must be removed"
        );
        assert!(
            wf_path(&s, "keeper").exists(),
            "an unrelated GLOBAL workflow must be left untouched"
        );
    }

    /// Vice-versa: deleting a global workflow must leave a differently-named
    /// project workflow untouched.
    #[tokio::test]
    async fn delete_removes_global_file_leaves_unrelated_project_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(
            wf_path(&s, "nightly-sweep"),
            VALID_YAML.replace("demo", "nightly-sweep"),
        )
        .unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("other.yaml"),
            VALID_YAML.replace("demo", "other"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let resp = delete_workflow(
            State(s.clone()),
            Path("nightly-sweep".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("global delete should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("global"));
        assert!(!wf_path(&s, "nightly-sweep").exists());
        assert!(
            proj_workflows.join("other.yaml").exists(),
            "an unrelated PROJECT workflow must be left untouched"
        );
    }

    /// A traversal-y `:name` must be rejected before any disk access —
    /// mirrors `api::autoflows`'s identical guard.
    #[tokio::test]
    async fn delete_rejects_traversal_name_before_any_disk_access() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let outside_target = tmp.path().join("evil.yaml");
        std::fs::write(&outside_target, VALID_YAML).unwrap();
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();

        let err = delete_workflow(
            State(s),
            Path("../evil".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect_err("traversal name must be rejected");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            outside_target.exists(),
            "the out-of-tree file must be byte-for-byte untouched"
        );
    }

    // ── Critical 1: project-first precedence for a name shared by BOTH
    //    layers ─────────────────────────────────────────────────────────
    //
    // The list always shadows a same-named GLOBAL row WITH the project row
    // (`rows.retain(|r| !project_names.contains(...))` in `list_workflows`),
    // so Delete on the row the operator sees must remove the PROJECT file,
    // never the hidden global one. Regression for the inverted-precedence
    // data-loss bug: a global-first resolver would delete the global file
    // here while the project row (the one actually shown) survived —
    // silently destroying a definition the operator never saw, while
    // reading as "delete did nothing."

    #[tokio::test]
    async fn delete_with_name_in_both_layers_removes_project_leaves_global_intact() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(
            wf_path(&s, "shared-name"),
            VALID_YAML.replace("demo", "shared-name"),
        )
        .unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("shared-name.yaml"),
            VALID_YAML.replace("demo", "shared-name"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // The list shadows the global row with the project row — confirm
        // that's really what's shown before deleting it.
        let Json(rows) = list_workflows(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 1, "the project row shadows the global one");
        assert_eq!(rows[0].scope_kind, ScopeKind::Project);

        // No explicit scope query — the implicit (project-first) resolver
        // must match what the list showed.
        let resp = delete_workflow(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("delete should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert!(
            !proj_workflows.join("shared-name.yaml").exists(),
            "the PROJECT file (the one the list/operator saw) must be removed"
        );
        assert!(
            wf_path(&s, "shared-name").exists(),
            "the shadowed GLOBAL file must survive untouched"
        );
    }

    // ── Critical 2: `scope_id` disambiguates a name shared by TWO repos ──

    #[tokio::test]
    async fn delete_with_explicit_scope_id_targets_one_repo_of_two_with_the_same_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj_x = tmp.path().join("proj-x");
        let workflows_x = proj_x.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_x).unwrap();
        std::fs::write(
            workflows_x.join("foo.yaml"),
            VALID_YAML.replace("demo", "foo"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let workflows_y = proj_y.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_y).unwrap();
        std::fs::write(
            workflows_y.join("foo.yaml"),
            VALID_YAML.replace("demo", "foo"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        // Both rows appear in the list — confirm the ambiguity is real
        // before resolving it with an explicit scope_id.
        let Json(rows) = list_workflows(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 2);

        // Delete repo Y's row specifically, by its workspace id.
        let resp = delete_workflow(
            State(s.clone()),
            Path("foo".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("ws_y".to_string()),
            }),
        )
        .await
        .expect("explicit-scope delete should succeed");
        assert_eq!(resp.0["scope"], serde_json::json!("proj-y"));
        assert!(
            !workflows_y.join("foo.yaml").exists(),
            "repo Y's file must be removed"
        );
        assert!(
            workflows_x.join("foo.yaml").exists(),
            "repo X's same-named file must survive untouched"
        );
    }

    #[tokio::test]
    async fn delete_explicit_scope_mismatch_404s_and_leaves_every_file_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(
            wf_path(&s, "shared-name"),
            VALID_YAML.replace("demo", "shared-name"),
        )
        .unwrap();

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("shared-name.yaml"),
            VALID_YAML.replace("demo", "shared-name"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // Ask explicitly for the GLOBAL layer's row's Delete to instead
        // target Project scope with an unknown workspace id — must 404,
        // NOT silently fall back to deleting the global (or project) file.
        let err = delete_workflow(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("no-such-workspace".to_string()),
            }),
        )
        .await
        .expect_err("mismatched explicit scope must 404, never fall back");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
        assert!(wf_path(&s, "shared-name").exists(), "global file untouched");
        assert!(
            proj_workflows.join("shared-name.yaml").exists(),
            "project file untouched"
        );
    }

    // ── resolve_workflow_scoped_explicit: any registered workspace id,
    // not just a distinct_repo_workspaces representative ────────────────
    //
    // `/api/projects/:ws_id/*` reports the caller's REQUESTED `ws_id`
    // (`projects::project_workflows` et al.), which for a repo with several
    // registered worktrees is often NOT the one `distinct_repo_workspaces`
    // would pick as that repo's representative. Matching only against
    // representatives made such an id resolve to `None` unconditionally.

    /// (a) An explicit `scope_id` naming a NON-representative worktree of a
    /// multi-worktree repo resolves to THAT worktree's own file, not `None`
    /// and not the representative's file.
    #[tokio::test]
    async fn explicit_project_scope_resolves_non_representative_worktree_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        // proj-a sorts first -> distinct_repo_workspaces picks it as the
        // representative when there's no tracked-repo preferred_path.
        let proj_a = tmp.path().join("proj-a");
        std::fs::create_dir_all(proj_a.join(".rupu").join("workflows")).unwrap();
        register_workspace_with_remote(&tmp, "ws_a", &proj_a, Some("git@github.com:acme/x.git"));

        let proj_b = tmp.path().join("proj-b");
        let workflows_b = proj_b.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_b).unwrap();
        std::fs::write(
            workflows_b.join("nightly.yaml"),
            VALID_YAML.replace("demo", "nightly"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_b", &proj_b, Some("git@github.com:acme/x.git"));

        // Confirm proj-a really is the representative (and thus that a
        // representative-only resolver could never have found ws_b's file).
        let workspaces = store(&s).list().unwrap();
        let reps = distinct_repo_workspaces(workspaces, &repo_store(&s));
        assert_eq!(reps.len(), 1);
        assert_eq!(reps[0].workspace.id, "ws_a");

        let resolved =
            resolve_workflow_scoped_explicit(&s, "nightly", ScopeKind::Project, Some("ws_b"))
                .expect("must resolve against the non-representative worktree");
        assert_eq!(resolved.0, workflows_b.join("nightly.yaml"));
        assert_eq!(resolved.2, "proj-b");
        assert_eq!(resolved.3, ScopeKind::Project);
    }

    /// (b) A representative's own id still resolves exactly as before.
    #[tokio::test]
    async fn explicit_project_scope_representative_id_still_resolves() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let proj_a = tmp.path().join("proj-a");
        let workflows_a = proj_a.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_a).unwrap();
        std::fs::write(
            workflows_a.join("nightly.yaml"),
            VALID_YAML.replace("demo", "nightly"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_a", &proj_a, Some("git@github.com:acme/x.git"));

        let proj_b = tmp.path().join("proj-b");
        std::fs::create_dir_all(proj_b.join(".rupu").join("workflows")).unwrap();
        register_workspace_with_remote(&tmp, "ws_b", &proj_b, Some("git@github.com:acme/x.git"));

        let resolved =
            resolve_workflow_scoped_explicit(&s, "nightly", ScopeKind::Project, Some("ws_a"))
                .expect("representative id must still resolve");
        assert_eq!(resolved.0, workflows_a.join("nightly.yaml"));
        assert_eq!(resolved.2, "proj-a");
    }

    /// (c) An id matching no registered workspace at all still returns
    /// `None` (⇒ the existing 404 path).
    #[tokio::test]
    async fn explicit_project_scope_unknown_id_returns_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();

        assert!(resolve_workflow_scoped_explicit(
            &s,
            "nightly",
            ScopeKind::Project,
            Some("no-such-workspace"),
        )
        .is_none());
    }

    /// (d) A KNOWN id whose own workspace does not contain the definition
    /// returns `None` — no fallback to another workspace that does.
    #[tokio::test]
    async fn explicit_project_scope_known_id_without_def_returns_none_no_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let proj_a = tmp.path().join("proj-a");
        let workflows_a = proj_a.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_a).unwrap();
        std::fs::write(
            workflows_a.join("nightly.yaml"),
            VALID_YAML.replace("demo", "nightly"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_a", &proj_a, Some("git@github.com:acme/x.git"));

        // ws_b is a registered worktree of the SAME repo but has no
        // `nightly.yaml` of its own.
        let proj_b = tmp.path().join("proj-b");
        std::fs::create_dir_all(proj_b.join(".rupu").join("workflows")).unwrap();
        register_workspace_with_remote(&tmp, "ws_b", &proj_b, Some("git@github.com:acme/x.git"));

        assert!(
            resolve_workflow_scoped_explicit(&s, "nightly", ScopeKind::Project, Some("ws_b"))
                .is_none(),
            "must not fall back to ws_a's copy of the same-named def"
        );
        assert!(
            workflows_a.join("nightly.yaml").exists(),
            "ws_a's file must be untouched (never even inspected as a fallback target)"
        );
    }

    /// (e) The `scope` string an explicit representative-id lookup returns
    /// matches exactly what the aggregate list endpoint shows for that same
    /// project row (both ultimately derive from `repo_scope::scope_name`).
    #[tokio::test]
    async fn explicit_project_scope_display_string_matches_list_endpoint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(
            proj_workflows.join("nightly.yaml"),
            VALID_YAML.replace("demo", "nightly"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let Json(rows) = list_workflows(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope_id.as_deref(), Some("ws_a"));

        let resolved =
            resolve_workflow_scoped_explicit(&s, "nightly", ScopeKind::Project, Some("ws_a"))
                .expect("must resolve");
        assert_eq!(resolved.2, rows[0].scope);
    }

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root`.
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &std::path::Path) {
        register_workspace_with_remote(tmp, id, project_root, None);
    }

    /// Same as [`register_workspace`], optionally tagging the record with a
    /// `repo_remote` (simulating autoflow run-worktrees of the same repo).
    fn register_workspace_with_remote(
        tmp: &tempfile::TempDir,
        id: &str,
        project_root: &std::path::Path,
        repo_remote: Option<&str>,
    ) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        let remote_line = repo_remote
            .map(|u| format!("repo_remote = \"{u}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\n{remote_line}created_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn list_no_projects_is_global_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "demo"), VALID_YAML).unwrap();

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "demo");
        assert_eq!(rows[0].scope, "global");
    }

    #[tokio::test]
    async fn list_includes_project_defs_tagged_with_project_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("proj-only.yaml"), VALID_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(rows[0].name, "proj-only");
        assert_eq!(rows[0].scope, expected_scope);
    }

    #[tokio::test]
    async fn workflow_detail_resolves_project_def() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("demo.yaml"), VALID_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // Absent from global, present only in the project — must resolve, not 404.
        let resp = get_workflow(State(s), Path("demo".into()))
            .await
            .expect("project-only workflow should resolve via detail");
        assert_eq!(resp.0["yaml"], serde_json::json!(VALID_YAML));
        // Resolved layer must be surfaced — the detail page's Delete gate and
        // scope indicator both key off these.
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resp.0["scope"], serde_json::json!(expected_scope));
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
    }

    /// A GLOBAL workflow's detail response is tagged `scope_kind: "global"` —
    /// the counterpart to `workflow_detail_resolves_project_def`'s project
    /// case. The CP detail page's Delete gate keys off this field.
    #[tokio::test]
    async fn workflow_detail_global_is_scope_kind_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "demo"), VALID_YAML).unwrap();

        let resp = get_workflow(State(s), Path("demo".into()))
            .await
            .expect("global workflow should resolve");
        assert_eq!(resp.0["scope"], serde_json::json!("global"));
        assert_eq!(resp.0["scope_kind"], serde_json::json!("global"));
    }

    #[tokio::test]
    async fn same_repo_worktrees_dedupe_to_one_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        // Three registered workspaces = run-worktrees of the SAME repo, each
        // carrying its own copy of `.rupu/workflows/issue-triage.yaml`.
        let remote = "git@github.com:acme/widgets.git";
        for (id, name) in [
            ("ws_a", "worktree-a"),
            ("ws_b", "worktree-b"),
            ("ws_c", "worktree-c"),
        ] {
            let root = tmp.path().join(name);
            let workflows = root.join(".rupu").join("workflows");
            std::fs::create_dir_all(&workflows).unwrap();
            std::fs::write(
                workflows.join("issue-triage.yaml"),
                VALID_YAML.replace("demo", "issue-triage"),
            )
            .unwrap();
            register_workspace_with_remote(&tmp, id, &root, Some(remote));
        }

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            1,
            "issue-triage must appear exactly once despite 3 worktrees of the same repo"
        );
        assert_eq!(rows[0].name, "issue-triage");
        // No tracked-repo record was seeded, so the tie-break is the
        // deterministic path sort: "worktree-a" sorts first.
        assert_eq!(rows[0].scope, "worktree-a");
    }

    #[tokio::test]
    async fn different_repos_same_def_name_both_appear() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj_x = tmp.path().join("proj-x");
        let workflows_x = proj_x.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_x).unwrap();
        std::fs::write(
            workflows_x.join("foo.yaml"),
            VALID_YAML.replace("demo", "foo"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let workflows_y = proj_y.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_y).unwrap();
        std::fs::write(
            workflows_y.join("foo.yaml"),
            VALID_YAML.replace("demo", "foo"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2, "different repos are distinct groups");
        let scopes: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["proj-x", "proj-y"])
        );
        assert!(rows.iter().all(|r| r.name == "foo"));
    }

    #[tokio::test]
    async fn no_repo_remote_scans_every_standalone_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj_a = tmp.path().join("standalone-a");
        let workflows_a = proj_a.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_a).unwrap();
        std::fs::write(
            workflows_a.join("alpha.yaml"),
            VALID_YAML.replace("demo", "alpha"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", &proj_a);

        let proj_b = tmp.path().join("standalone-b");
        let workflows_b = proj_b.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_b).unwrap();
        std::fs::write(
            workflows_b.join("beta.yaml"),
            VALID_YAML.replace("demo", "beta"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_b", &proj_b);

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            2,
            "both standalone (no repo_remote) dirs are scanned"
        );
        let names: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, std::collections::BTreeSet::from(["alpha", "beta"]));
    }

    /// Minimal `RunRecord` for usage-rollup tests: workflow `name`, unique
    /// `id`, arbitrary workspace binding (the usage join only reads
    /// `workflow_name` today — see the doc comment on `WorkflowDto::usage`).
    fn run_record(
        id: &str,
        workflow_name: &str,
        workspace_id: &str,
    ) -> rupu_orchestrator::RunRecord {
        rupu_orchestrator::RunRecord {
            id: id.into(),
            workflow_name: workflow_name.into(),
            status: rupu_orchestrator::RunStatus::Completed,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: workspace_id.into(),
            workspace_path: std::path::PathBuf::from("/tmp/proj"),
            transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
        }
    }

    #[tokio::test]
    async fn same_named_rows_from_different_repos_do_not_both_show_combined_usage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj_x = tmp.path().join("proj-x");
        let workflows_x = proj_x.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_x).unwrap();
        std::fs::write(
            workflows_x.join("foo.yaml"),
            VALID_YAML.replace("demo", "foo"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let workflows_y = proj_y.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_y).unwrap();
        std::fs::write(
            workflows_y.join("foo.yaml"),
            VALID_YAML.replace("demo", "foo"),
        )
        .unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        // Two runs of a workflow named "foo" — as far as `RunRecord` is
        // concerned they're indistinguishable by scope (only `workflow_name`
        // is recorded), so the rollup key "foo" accrues both.
        s.run_store
            .create(run_record("run_1", "foo", "ws_x"), "name: foo\n")
            .unwrap();
        s.run_store
            .create(run_record("run_2", "foo", "ws_y"), "name: foo\n")
            .unwrap();

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.name == "foo"));

        let run_counts: Vec<u64> = rows.iter().map(|r| r.run_count).collect();
        // Both runs must be attributed to exactly ONE canonical row (whichever
        // row won the tie-break), not duplicated across both same-named rows.
        assert_eq!(
            run_counts.iter().sum::<u64>(),
            2,
            "the 2 runs are counted exactly once between the two rows combined"
        );
        assert_eq!(
            run_counts.iter().filter(|&&c| c == 2).count(),
            1,
            "exactly one row carries the combined run_count"
        );
        assert_eq!(
            run_counts.iter().filter(|&&c| c == 0).count(),
            1,
            "the other same-named row stays zeroed rather than duplicating usage"
        );
    }

    // ── `autoflow_enabled` on the Workflows list DTO ─────────────────────
    // Autoflows are just workflows with an `autoflow:` block — the list now
    // surfaces that so the operator can tell them apart at a glance and
    // toggle them right from the Workflows page (Part B).

    #[tokio::test]
    async fn list_workflows_autoflow_enabled_is_none_for_a_plain_workflow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "demo"), VALID_YAML).unwrap();

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].autoflow_enabled, None);
    }

    #[tokio::test]
    async fn list_workflows_autoflow_enabled_reflects_on_disk_enabled_and_disabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(
            wf_path(&s, "nightly"),
            "name: nightly\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    prompt: p\n",
        )
        .unwrap();
        std::fs::write(
            wf_path(&s, "stale-cleanup"),
            "name: stale-cleanup\nautoflow:\n  enabled: false\nsteps:\n  - id: s1\n    agent: ag\n    prompt: p\n",
        )
        .unwrap();

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2);
        let nightly = rows.iter().find(|r| r.name == "nightly").unwrap();
        let stale = rows.iter().find(|r| r.name == "stale-cleanup").unwrap();
        assert_eq!(nightly.autoflow_enabled, Some(true));
        assert_eq!(stale.autoflow_enabled, Some(false));
    }

    /// A file that fails to parse still gets a row (unlike
    /// `scan_autoflow_defs`, which drops it) — it just can't report
    /// autoflow state, so `autoflow_enabled` is `None` rather than the row
    /// disappearing from the operator's workflow inventory.
    #[tokio::test]
    async fn list_workflows_unparseable_file_still_listed_with_autoflow_enabled_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "broken"), "not: [valid").unwrap();

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1, "the broken file still gets a row");
        assert_eq!(rows[0].name, "broken");
        assert_eq!(rows[0].autoflow_enabled, None);
    }

    /// Proves the toggle-scope-caveat fix: `resolve_workflow_scoped` now
    /// uses the SAME representative-worktree selection
    /// (`distinct_repo_workspaces`) `list_workflows` uses, so a
    /// project-scoped Enable/Disable (or Delete) always lands on the exact
    /// worktree copy the list row shows — never a different, non-
    /// representative worktree of the same repo.
    #[tokio::test]
    async fn resolve_scoped_targets_the_same_representative_worktree_the_list_uses() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        // Three worktrees of the SAME repo, each with its own copy of
        // `issue-triage.yaml`. No tracked-repo record → deterministic
        // path-sort tie-break picks "worktree-a" (see
        // `repo_scope::distinct_repo_workspaces`'s tests).
        let remote = "git@github.com:acme/widgets.git";
        for name in ["worktree-a", "worktree-b", "worktree-c"] {
            let root = tmp.path().join(name);
            let workflows = root.join(".rupu").join("workflows");
            std::fs::create_dir_all(&workflows).unwrap();
            std::fs::write(
                workflows.join("issue-triage.yaml"),
                "name: issue-triage\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    prompt: p\n",
            )
            .unwrap();
            register_workspace_with_remote(&tmp, &format!("ws_{name}"), &root, Some(remote));
        }

        // The list's row scope names the representative worktree...
        let Json(rows) = list_workflows(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "worktree-a");

        // ...and the resolver used by both delete and the autoflow toggle
        // must resolve to that SAME worktree's file.
        let (path, _dir, scope, _kind, _scope_id) = resolve_workflow_scoped(&s, "issue-triage")
            .expect("should resolve against the representative worktree");
        assert_eq!(scope, "worktree-a");
        assert_eq!(
            path,
            tmp.path()
                .join("worktree-a")
                .join(".rupu")
                .join("workflows")
                .join("issue-triage.yaml")
        );
    }

    // ── macOS golden fixtures (apps/rupu-macos/Fixtures/) ─────────────────
    //
    // `WorkflowDto`/`LaunchBody`/`WorkflowWriteBody` are private to this
    // module — the integration test (`tests/macos_fixtures.rs`) can't build
    // them, so their fixtures live here instead. Same `check_fixture`
    // contract as that file (duplicated: a unit test can't share code with
    // an integration test without a public module) — see `api/host_info.rs`'s
    // test module for the established pattern.

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/rupu-macos/Fixtures")
    }

    fn check_fixture(name: &str, value: &impl serde::Serialize) {
        let path = fixtures_dir().join(name);
        let rendered = serde_json::to_string_pretty(value).expect("serialize fixture");
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, rendered + "\n").expect("write fixture");
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture {name}; run `make macos-fixtures`"));
        assert_eq!(
            on_disk.trim_end(),
            rendered,
            "fixture {name} drifted from the Rust types; run `make macos-fixtures`"
        );
    }

    /// Reads the checked-in `apps/rupu-macos/Fixtures/requests/<name>` and
    /// returns its contents. `REGEN_FIXTURES=1` first (re)writes `raw` (the
    /// hand-authored canonical JSON — these body types are Deserialize-only,
    /// so there's no `Serialize` impl to render from) verbatim to disk, so
    /// this always ends up reading back exactly what's checked in.
    fn check_request_fixture(name: &str, raw: &str) -> String {
        let path = fixtures_dir().join("requests").join(name);
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, raw).expect("write request fixture");
        }
        std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing request fixture {name}; run `make macos-fixtures`"))
    }

    #[test]
    fn workflow_defs_fixture_is_current() {
        let global = WorkflowDto {
            name: "nightly-health".into(),
            scope: "global".into(),
            scope_kind: ScopeKind::Global,
            scope_id: None,
            usage: crate::usage::UsageSummary {
                input_tokens: 5000,
                output_tokens: 1200,
                cached_tokens: 300,
                total_tokens: 6500,
                cost_usd: Some(0.42),
                priced: true,
                runs: 5,
            },
            run_count: 5,
            last_run: Some("2026-08-20T12:00:00+00:00".into()),
            autoflow_enabled: Some(true),
        };
        let project = WorkflowDto {
            name: "adhoc-build".into(),
            scope: "widgets".into(),
            scope_kind: ScopeKind::Project,
            scope_id: Some("ws_a".into()),
            usage: crate::usage::UsageSummary::default(),
            run_count: 0,
            last_run: None,
            autoflow_enabled: None,
        };
        check_fixture("workflow_defs.json", &vec![global, project]);
    }

    /// Declares two inputs — one `required: true` with `enum` values (no
    /// default), one defaulted (`required: false`, implicit) — to lock the
    /// `InputDef` wire shape the Launcher's declared-input rows source.
    const SAMPLE_WORKFLOW_YAML: &str = "name: nightly-health\nsteps:\n  - id: one\n    agent: x\n    prompt: hi\ninputs:\n  branch:\n    type: string\n    required: true\n    enum: [main, staging]\n  target:\n    type: string\n    default: production\n";

    #[test]
    fn workflow_detail_fixture_is_current() {
        let workflow =
            Workflow::parse(SAMPLE_WORKFLOW_YAML).expect("SAMPLE_WORKFLOW_YAML should parse");
        let usage = crate::usage::UsageSummary {
            input_tokens: 5000,
            output_tokens: 1200,
            cached_tokens: 300,
            total_tokens: 6500,
            cost_usd: Some(0.42),
            priced: true,
            runs: 5,
        };
        // Mirrors `load_detail`'s `json!` shape exactly.
        let value = serde_json::json!({
            "workflow": workflow,
            "yaml": SAMPLE_WORKFLOW_YAML,
            "usage": usage,
            "scope": "global",
            "scope_kind": ScopeKind::Global,
            "scope_id": Option::<String>::None,
        });
        check_fixture("workflow_detail.json", &value);
    }

    #[test]
    fn workflow_launch_body_request_fixture_roundtrips() {
        // Locks `LaunchBody`'s wire shape (`POST /api/workflows/:name/run`) —
        // `inputs` defaults to `{}`, `host` in the BODY (unlike the
        // run-control routes, which take it as a query param). No
        // `scope_kind`/`scope_id` in this payload — proves the Phase 5A
        // Task 2 selector (see `resolve_launch_scope`) is absent-tolerant
        // (defaults to `None`); `api::agents::agent_run_body_request_fixture_roundtrips`
        // carries the "present" case for the same field pair.
        let raw = check_request_fixture(
            "workflow_launch_body.json",
            "{\n  \"inputs\": {\n    \"branch\": \"main\"\n  },\n  \"mode\": \"ask\",\n  \"host\": \"mini\"\n}\n",
        );
        let body: LaunchBody = serde_json::from_str(&raw).expect("deserialize LaunchBody");
        assert_eq!(body.inputs.get("branch").map(String::as_str), Some("main"));
        assert_eq!(body.mode.as_deref(), Some("ask"));
        assert_eq!(body.target, None);
        assert_eq!(body.working_dir, None);
        assert_eq!(body.host.as_deref(), Some("mini"));
        assert_eq!(body.scope_kind, None);
        assert_eq!(body.scope_id, None);
    }

    #[test]
    fn validate_body_request_fixture_roundtrips() {
        // `POST /api/workflows/validate` shares `WorkflowWriteBody`'s shape
        // (`{"raw": String}`) with `PUT /api/workflows/:name` — same type,
        // separate fixture since the two are distinct call sites the app
        // encodes independently.
        let raw = check_request_fixture(
            "validate_body.json",
            "{\n  \"raw\": \"name: demo\\nsteps:\\n  - id: one\\n    agent: x\\n    prompt: hi\\n\"\n}\n",
        );
        let body: WorkflowWriteBody =
            serde_json::from_str(&raw).expect("deserialize WorkflowWriteBody");
        assert!(body.raw.contains("name: demo"));
        assert!(body.raw.contains("agent: x"));
    }
}
