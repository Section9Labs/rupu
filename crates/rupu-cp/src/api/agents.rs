use crate::{
    agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher},
    api::fs_safety::{validate_name, validate_within, write_atomic},
    api::repo_scope::{distinct_repo_workspaces, scope_name, ScopeKind, ScopeQuery},
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    session_starter::{SessionStartError, SessionStartRequest, SessionStarter},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use rupu_agent::loader::{load_agent, load_agents, AgentLoadError};
use rupu_workspace::{RepoRegistryStore, WorkspaceStore};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agents", get(list_agents).post(create_agent))
        .route(
            "/api/agents/:name",
            get(get_agent).put(write_agent).delete(delete_agent),
        )
        .route("/api/agents/:name/run", post(run_agent))
        .route("/api/agents/:name/session", post(start_session))
        .route("/api/agents/generate", post(generate_agent))
}

/// Directory where global agent `.md` definitions live.
fn agents_dir(s: &AppState) -> PathBuf {
    s.global_dir.join("agents")
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

/// Resolve agent file-STEM `slug` to its on-disk path, the directory that
/// contains it, and the scope layer it resolved from: one representative
/// workspace per distinct repo among the registered projects FIRST (see
/// [`distinct_repo_workspaces`] — many registered workspaces are autoflow
/// run-worktrees of the very same repo, each carrying an identical copy of
/// `.rupu/agents/`, so this avoids resolving against a stale
/// non-representative worktree), falling back to the global agents dir only
/// when no registered project defines `slug`. First match wins; `None` when
/// `slug` resolves in neither layer.
///
/// PROJECT-FIRST is deliberate: every list this resolver backs
/// ([`list_agents`]) shadows a same-named GLOBAL row WITH the project row,
/// and the CLI runtime resolves the same way. A global-first resolver — the
/// prior implementation — silently disagreed with what the list showed the
/// operator: for a slug present in BOTH layers, Delete would remove the
/// hidden global file while the visible project row survived. See
/// `workflows::resolve_workflow_scoped`'s doc comment for the identical
/// reasoning (and the data-loss bug this fixes).
///
/// Backs `DELETE /api/agents/:name` ([`delete_agent`]) and, as of this
/// change, [`load_detail`]'s fast path too — keyed by file STEM rather than
/// frontmatter `name` (a hand- or CLI-authored `.md` file's stem can differ
/// from its own frontmatter `name`, and resolving "by name" in that case
/// would 404 or remove an unrelated file that happens to share a name — see
/// [`AgentDto::slug`]'s doc comment).
///
/// Two DIFFERENT repos can each define an agent with the same `slug`; this
/// resolver can only ever return the first it finds (deterministic —
/// `distinct_repo_workspaces` sorts by `scope`). A caller that needs to pin
/// down one specific repo's row unambiguously should use
/// [`resolve_agent_scoped_explicit`] instead, which every mutating web-UI
/// caller now does (see [`delete_agent`]) by threading the row's `scope_id`.
///
/// No `host`/remote concept here, unlike `run_agent`/`start_session` below:
/// `s.global_dir` and every registered workspace's `path` are local
/// filesystem paths on THIS `rupu cp` process's own host — agent
/// definitions are never fetched from or proxied to a remote host, so this
/// resolver (and therefore [`delete_agent`]) is correctly host-unaware.
pub(crate) fn resolve_agent_scoped(
    s: &AppState,
    slug: &str,
) -> Option<(PathBuf, PathBuf, String, ScopeKind, Option<String>)> {
    let workspaces = store(s).list().unwrap_or_default();
    for r in distinct_repo_workspaces(workspaces, &repo_store(s)) {
        let proj_dir = std::path::Path::new(&r.workspace.path)
            .join(".rupu")
            .join("agents");
        let candidate = proj_dir.join(format!("{slug}.md"));
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
    let dir = agents_dir(s);
    let global = dir.join(format!("{slug}.md"));
    if global.exists() {
        return Some((global, dir, "global".to_string(), ScopeKind::Global, None));
    }
    None
}

/// Resolve agent file-STEM `slug` restricted to an EXPLICIT scope — the
/// disambiguating counterpart to [`resolve_agent_scoped`]'s implicit
/// (project-first, then global) walk. Mirrors
/// `workflows::resolve_workflow_scoped_explicit` exactly, INCLUDING its
/// `ScopeKind::Project` match against the FULL registered-workspace list
/// (not just [`distinct_repo_workspaces`]'s representatives) — see that
/// function's doc comment for the full contract (global-only vs. one
/// workspace pinned by `scope_id`, `None` on any mismatch rather than a
/// fallback).
pub(crate) fn resolve_agent_scoped_explicit(
    s: &AppState,
    slug: &str,
    scope_kind: ScopeKind,
    scope_id: Option<&str>,
) -> Option<(PathBuf, PathBuf, String, ScopeKind)> {
    match scope_kind {
        ScopeKind::Global => {
            let dir = agents_dir(s);
            let global = dir.join(format!("{slug}.md"));
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
            let proj_dir = std::path::Path::new(&w.path).join(".rupu").join("agents");
            let candidate = proj_dir.join(format!("{slug}.md"));
            if candidate.exists() {
                Some((candidate, proj_dir, scope_name(&w), ScopeKind::Project))
            } else {
                None
            }
        }
    }
}

/// Pure core of the LEGACY (global-only) `PUT /api/agents/:name` path: validate
/// the url name, parse + validate the raw `.md` (no write on parse failure),
/// enforce frontmatter-name == url-name, then atomically write to
/// `<global_dir>/agents/<name>.md`. Returns the written path.
///
/// Scope-aware writes (an explicit `?scope_kind=&scope_id=` selector, or an
/// implicit resolution to an existing PROJECT file) go through
/// [`save_agent_file_at`] instead — see [`write_agent`].
fn save_agent_file(global_dir: &FsPath, url_name: &str, raw: &str) -> Result<PathBuf, ApiError> {
    save_agent_file_at(url_name, raw, &global_dir.join("agents"))
}

/// Core of `PUT /api/agents/:name`, parameterized on the TARGET directory
/// (global agents dir, or one project's `.rupu/agents`) rather than always
/// `<global_dir>/agents`. Validates the url name, parses + validates the raw
/// `.md` (no write on parse failure), enforces frontmatter-name == url-name,
/// then atomically writes `<dir>/<url_name>.md`. Returns the written path.
fn save_agent_file_at(url_name: &str, raw: &str, dir: &FsPath) -> Result<PathBuf, ApiError> {
    validate_name(url_name)?;
    let spec =
        rupu_agent::AgentSpec::parse(raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if spec.name != url_name {
        return Err(ApiError::bad_request(
            "frontmatter name must equal the agent name",
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let target = dir.join(format!("{url_name}.md"));
    write_atomic(&target, raw.as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;
    // Defense in depth, mirroring `delete_agent`'s `validate_within` — run
    // AFTER the write here (rather than before, as delete does) because
    // `target` may not exist yet on disk when this is a genuine create.
    // `target`/`dir` are always constructed from a `validate_name`-checked
    // identifier joined onto a trusted directory, so this can never actually
    // fire today; see `validate_within`'s doc comment.
    validate_within(&target, dir)?;
    Ok(target)
}

/// Pure core of `POST /api/agents`: parse the raw `.md` to derive the name,
/// validate it, refuse to clobber an existing file, then atomically write.
fn create_agent_file(global_dir: &FsPath, raw: &str) -> Result<PathBuf, ApiError> {
    let spec =
        rupu_agent::AgentSpec::parse(raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let name = spec.name.clone();
    validate_name(&name)?;
    let dir = global_dir.join("agents");
    let target = dir.join(format!("{name}.md"));
    if target.exists() {
        return Err(ApiError::conflict("agent already exists"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    write_atomic(&target, raw.as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(target)
}

/// Map each agent's parsed frontmatter `name` to the FILE STEM it was loaded
/// from, by scanning `<dir>/agents/*.md` directly. `delete_agent` removes
/// `<slug>.md` by file stem, not by frontmatter `name` — when the two differ
/// (hand- or CLI-authored files), a row keyed only by `name` would 404 or
/// delete an unrelated file that happens to share that stem. Mirrors
/// `AutoflowDefRow.slug`'s identical fix in
/// `api::autoflows::scan_autoflow_defs`.
///
/// A file that fails to parse, or a missing/unreadable directory, is simply
/// absent from the map rather than erroring — this is a supplementary
/// name→slug lookup alongside the authoritative `AgentSpec` list
/// `load_agents`/`load_agent` already produced; a scan hiccup here should
/// not break the DTO the caller already has in hand (the caller falls back
/// to `name` when a lookup misses).
pub(crate) fn agent_slug_map(dir: &FsPath) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let agents_dir = dir.join("agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return out;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(spec) = rupu_agent::AgentSpec::parse_file(&path) {
            out.insert(spec.name, slug.to_string());
        }
    }
    out
}

/// Build the detail DTO from a loaded spec, tagged with `scope`/`scope_kind`/
/// `scope_id` and `slug`.
fn detail_from_spec(
    spec: rupu_agent::spec::AgentSpec,
    scope: impl Into<String>,
    scope_kind: ScopeKind,
    slug: impl Into<String>,
    scope_id: Option<String>,
) -> AgentDetailDto {
    let system_prompt = spec.system_prompt.clone();
    let raw = spec.raw.clone();
    AgentDetailDto {
        system_prompt,
        raw,
        summary: AgentDto::from_spec(spec, scope, scope_kind, slug, scope_id),
    }
}

/// Load agent `name` and build the full detail DTO. Shared by GET / PUT / POST.
///
/// Resolution has two tiers:
///
/// 1. **Fast/common path** — `name` is checked as a file STEM via
///    [`resolve_agent_scoped`], the EXACT resolver `DELETE /api/agents/:name`
///    uses (project-first, then global — see its doc comment). This covers
///    the overwhelming common case, since every agent created or saved
///    through this API enforces `frontmatter name == url name == file stem`
///    (see [`save_agent_file`]/[`create_agent_file`]).
/// 2. **Fallback** — only when NO file's stem is `name` does this fall back
///    to matching by parsed FRONTMATTER `name` instead, for hand- or
///    CLI-authored files whose stem legitimately differs from their `name:`
///    (see [`AgentDto::slug`]'s doc comment). Also project-first: one
///    representative workspace per distinct repo (via
///    [`distinct_repo_workspaces`] — the SAME representative-selection
///    [`resolve_agent_scoped`] uses), THEN the global layer.
///
/// This used to be two independently-implemented resolvers with a doc
/// comment incorrectly claiming they resolved "the same way": this function
/// matched frontmatter name across EVERY raw registered workspace in
/// whatever order the store listed them, while `resolve_agent_scoped`
/// matched file stem against one deterministic representative per repo —
/// neither the matching key nor the workspace-selection axis actually
/// agreed. Routing the common (name == stem) case through the real
/// `resolve_agent_scoped` resolver, and putting the fallback path onto the
/// same deterministic per-repo selection, unifies both axes except where
/// they must differ by design: `DELETE` always keys on file stem, while
/// `GET`/detail prefers file-stem but still finds a stem-mismatched file by
/// frontmatter name, because switching `GET` to stem-only would break
/// existing `/agents/:name` links built from frontmatter `name`
/// (`Agents.tsx`'s row links, `NewAgentModal`'s post-create navigation) for
/// that rare hand-authored-mismatch case.
///
/// The resolved layer is carried on the returned DTO's `scope`/`scope_kind`/
/// `scope_id`, which the CP detail page shows (scope chip + naming the layer
/// in the delete confirmation) and threads back on its own Delete call when
/// no explicit scope is otherwise available.
fn load_detail(s: &AppState, name: &str) -> ApiResult<AgentDetailDto> {
    if let Some((path, _dir, scope, scope_kind, scope_id)) = resolve_agent_scoped(s, name) {
        let spec = rupu_agent::AgentSpec::parse_file(&path)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        return Ok(detail_from_spec(
            spec,
            scope,
            scope_kind,
            name.to_string(),
            scope_id,
        ));
    }

    let workspaces = store(s).list().unwrap_or_default();
    for r in distinct_repo_workspaces(workspaces, &repo_store(s)) {
        let rupu_dir = std::path::Path::new(&r.workspace.path).join(".rupu");
        if let Ok(spec) = load_agent(&rupu_dir, None, name) {
            let slug = agent_slug_map(&rupu_dir)
                .remove(name)
                .unwrap_or_else(|| name.to_string());
            return Ok(detail_from_spec(
                spec,
                r.scope,
                ScopeKind::Project,
                slug,
                Some(r.workspace.id),
            ));
        }
    }
    match load_agent(&s.global_dir, None, name) {
        Ok(spec) => {
            let slug = agent_slug_map(&s.global_dir)
                .remove(name)
                .unwrap_or_else(|| name.to_string());
            Ok(detail_from_spec(spec, "global", ScopeKind::Global, slug, None))
        }
        Err(AgentLoadError::NotFound(_)) => {
            Err(ApiError::not_found(format!("agent {name} not found")))
        }
        Err(other) => Err(ApiError::internal(other.to_string())),
    }
}

#[derive(Serialize)]
pub(crate) struct AgentDto {
    pub(crate) name: String,
    /// File stem — the identifier `delete_agent` operates on (see
    /// [`agent_slug_map`]'s doc comment for why this can differ from `name`).
    /// Always present on the wire; the row's Delete action must pass THIS,
    /// never `name`.
    pub(crate) slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    /// The agent's frontmatter `tools:` allowlist — the runtime grant a step's
    /// `actions:` narrows (never extends) per the step-`actions:`-enforcement
    /// design. An agent whose frontmatter omits `tools:` (unrestricted — every
    /// catalog tool is available) serializes this as an empty array; the wire
    /// shape doesn't distinguish "no tools" from "unrestricted", but that's
    /// unambiguous for authoring: the workflow-editor picker treats an empty
    /// list here the same as "nothing to flag against" (every step `actions:`
    /// entry is shown, none of them get the "not granted" flag).
    pub(crate) tools: Vec<String>,
    /// `"project"` when the spec was loaded from `<project>/.rupu/agents`,
    /// else `"global"`. Defaults to `"global"` for the global-only endpoints.
    /// DISPLAY ONLY for a project row — it's a workspace path's basename and
    /// can legally collide with the literal string `"global"`. Destructive
    /// gates must key off `scope_kind`, never this field — see
    /// [`ScopeKind`]'s doc comment.
    pub(crate) scope: String,
    /// Structured scope discriminator backing every Delete gate. See
    /// [`ScopeKind`].
    pub(crate) scope_kind: ScopeKind,
    /// The underlying registered workspace's unique `id` for a Project row
    /// (`None` for a Global row). Unlike `scope` (a path basename, which can
    /// collide between two different repos), this is the genuinely unique
    /// identifier for "which repo" — pass it back as the `scope_id` query
    /// param on `DELETE` to pin the action to THIS row's file when another
    /// repo defines the same `name`/`slug`. See `repo_scope::ScopeQuery`'s
    /// doc comment.
    pub(crate) scope_id: Option<String>,
    /// Aggregate token + cost usage across every run attributed to this agent.
    /// Defaults to empty; populated only by the list handler. Usage is
    /// grouped by agent NAME alone (the transcript rows the breakdown is
    /// computed from don't record which scope's definition ran), so only ONE
    /// canonical row per name (the `scope == "global"` row if one exists,
    /// else the first row for that name in sorted order) carries the
    /// aggregate — other same-named rows across different repos are left
    /// zeroed rather than showing duplicated combined usage. Per-scope usage
    /// attribution is a follow-up.
    pub(crate) usage: crate::usage::UsageSummary,
    /// Distinct runs attributed to this agent. Defaults to `0`.
    pub(crate) run_count: u64,
    /// Most-recent run timestamp (ISO-8601) attributed to this agent, or
    /// `None` if it has never run. Same NAME, same wire shape, and same role
    /// as `WorkflowDto::last_run` — the "last activity" signal the list
    /// tables use in place of a non-existent intrinsic status for
    /// definitions (agents/workflows have no status of their own; only their
    /// runs do) — but the two are derived differently, and that difference
    /// is user-visible:
    ///
    /// - `WorkflowDto::last_run` is STRUCTURAL: `rollup_by` keys every
    ///   `RunRecord` by its `workflow_name` field directly, so every run
    ///   counts regardless of whether its transcripts are readable.
    /// - This field is TRANSCRIPT-DERIVED: a `RunRecord` carries no "agent(s)
    ///   this run used" field, so a run only counts here if at least one of
    ///   its transcripts is readable AND contains a `Usage` event naming this
    ///   agent (see the per-run loop in `list_agents`).
    ///
    /// Consequence: an agent whose run died before the first LLM call, or
    /// whose transcripts were pruned or live on an unreachable remote host,
    /// shows `None` here (reading as "never ran") even though an equivalent
    /// workflow with the same run would show a timestamp. Populated by the
    /// same canonical-row rule as `usage` / `run_count` above: only ONE row
    /// per agent name carries it. No `skip_serializing_if` — matches
    /// `WorkflowDto::last_run`, which always serializes (as `null` when
    /// absent) rather than omitting the key.
    #[serde(default)]
    pub(crate) last_run: Option<String>,
}

impl AgentDto {
    /// Map a loaded [`rupu_agent::spec::AgentSpec`] to the wire DTO, tagging
    /// it with the given scope, file-stem slug, and (for a Project row) the
    /// underlying workspace's unique id.
    pub(crate) fn from_spec(
        spec: rupu_agent::spec::AgentSpec,
        scope: impl Into<String>,
        scope_kind: ScopeKind,
        slug: impl Into<String>,
        scope_id: Option<String>,
    ) -> Self {
        AgentDto {
            name: spec.name,
            slug: slug.into(),
            description: spec.description,
            provider: spec.provider,
            model: spec.model,
            effort: spec.effort.map(|e| format!("{e:?}")),
            max_tokens: spec.max_tokens,
            tools: spec.tools.unwrap_or_default(),
            scope: scope.into(),
            scope_kind,
            scope_id,
            usage: crate::usage::UsageSummary::default(),
            run_count: 0,
            last_run: None,
        }
    }
}

#[derive(Serialize)]
struct AgentDetailDto {
    #[serde(flatten)]
    summary: AgentDto,
    system_prompt: String,
    /// Full raw agent definition file (`.md` frontmatter + body), served so the
    /// CP can render it with syntax highlighting.
    raw: String,
}

/// `GET /api/agents` — global agent definitions plus one representative
/// workspace per distinct repo among the registered projects'
/// `<path>/.rupu/agents/*.md` (see [`distinct_repo_workspaces`]), sorted by
/// name then scope. Many registered workspaces are autoflow run-worktrees of
/// the same repo; scanning every registered workspace would otherwise emit
/// one duplicate row per worktree.
///
/// Each row is tagged `scope: "global"` or the representative workspace's
/// path basename. A project def shadows a same-named GLOBAL row; two
/// different repos defining the same name both appear (distinguished by
/// `scope`). With no registered projects this is byte-for-byte the prior
/// global-only behavior.
///
/// A malformed project agent file only drops that project's rows (logged via
/// `tracing::warn!`) rather than failing the whole list; the global scan's
/// error behavior is unchanged.
async fn list_agents(State(s): State<AppState>) -> ApiResult<Json<Vec<AgentDto>>> {
    let specs = load_agents(&s.global_dir, None).map_err(|e| ApiError::internal(e.to_string()))?;
    let global_slugs = agent_slug_map(&s.global_dir);
    let mut dtos: Vec<AgentDto> = specs
        .into_iter()
        .map(|spec| {
            let slug = global_slugs
                .get(&spec.name)
                .cloned()
                .unwrap_or_else(|| spec.name.clone());
            AgentDto::from_spec(spec, "global", ScopeKind::Global, slug, None)
        })
        .collect();

    let workspaces = store(&s).list().unwrap_or_default();
    let repos = distinct_repo_workspaces(workspaces, &repo_store(&s));
    let mut project_dtos: Vec<AgentDto> = Vec::new();
    for r in repos {
        let scope = r.scope;
        let scope_id = r.workspace.id.clone();
        let rupu_dir = std::path::Path::new(&r.workspace.path).join(".rupu");
        match load_agents(&rupu_dir, None) {
            Ok(specs) => {
                let slugs = agent_slug_map(&rupu_dir);
                project_dtos.extend(specs.into_iter().map(|spec| {
                    let slug = slugs
                        .get(&spec.name)
                        .cloned()
                        .unwrap_or_else(|| spec.name.clone());
                    AgentDto::from_spec(
                        spec,
                        scope.clone(),
                        ScopeKind::Project,
                        slug,
                        Some(scope_id.clone()),
                    )
                }));
            }
            Err(err) => {
                tracing::warn!("agents: skipping project {scope}: {err}");
            }
        }
    }
    let project_names: std::collections::BTreeSet<&str> =
        project_dtos.iter().map(|d| d.name.as_str()).collect();
    dtos.retain(|d| !project_names.contains(d.name.as_str()));
    dtos.extend(project_dtos);
    dtos.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.scope.cmp(&b.scope)));

    // Aggregate every run's transcript, grouped by agent, to attach usage.
    // The breakdown is keyed by agent NAME alone (transcript rows don't
    // record which scope's definition ran), so — same as the workflows list
    // — attach it to only ONE canonical row per name (preferring
    // `scope == "global"`, else the first row for that name in the
    // already-sorted order) rather than duplicating it onto every same-named
    // row. See the doc comment on `AgentDto::usage`.
    //
    // Single pass over the run store: each run's own transcripts are
    // aggregated exactly once. The resulting `UsageRow`s feed straight into
    // `all_rows` for `breakdown` (which re-groups by agent name and sums —
    // whether the rows arrive pre-merged from one combined `aggregate` call
    // over every path, or concatenated from one `aggregate` call per run, the
    // per-key sums `breakdown` produces are identical, since token/run counts
    // are strictly additive and every transcript still lands in exactly one
    // run's batch). `last_run` is folded from the SAME per-run rows (their
    // `agent` field names who the run's usage attributes to; `run.started_at`
    // is the candidate timestamp) instead of a second re-aggregation pass
    // that re-walks every run's transcripts a second time over the exact
    // same files `all_rows` had just read.
    let runs = s.run_store.list().unwrap_or_default();
    let mut all_rows: Vec<rupu_transcript::UsageRow> = Vec::new();
    let mut last_runs: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for r in &runs {
        let paths = crate::usage::run_transcript_paths(&s.run_store, &r.id);
        if paths.is_empty() {
            continue;
        }
        let rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        let at = r.started_at.to_rfc3339();
        for row in &rows {
            if row.agent.is_empty() {
                continue;
            }
            last_runs
                .entry(row.agent.clone())
                .and_modify(|cur| {
                    if *cur < at {
                        *cur = at.clone();
                    }
                })
                .or_insert_with(|| at.clone());
        }
        all_rows.extend(rows);
    }
    let breakdown = crate::usage::breakdown(&all_rows, &s.pricing, crate::usage::GroupBy::Agent);

    let mut canonical_dto_for_name: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, dto) in dtos.iter().enumerate() {
        canonical_dto_for_name
            .entry(dto.name.clone())
            .and_modify(|idx| {
                if dto.scope == "global" {
                    *idx = i;
                }
            })
            .or_insert(i);
    }
    for (name, idx) in canonical_dto_for_name {
        if let Some(b) = breakdown.iter().find(|b| b.agent == name) {
            let dto = &mut dtos[idx];
            dto.usage = crate::usage::UsageSummary {
                input_tokens: b.input_tokens,
                output_tokens: b.output_tokens,
                cached_tokens: b.cached_tokens,
                total_tokens: b.total_tokens,
                cost_usd: b.cost_usd,
                priced: b.priced,
                runs: b.runs,
            };
            dto.run_count = b.runs;
            dto.last_run = last_runs.get(&name).cloned();
        }
    }

    Ok(Json(dtos))
}

async fn get_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDetailDto>> {
    Ok(Json(load_detail(&s, &name)?))
}

/// Request body for `PUT /api/agents/:name` and `POST /api/agents`: the full
/// raw `.md` (frontmatter + body) to validate and persist.
#[derive(Deserialize)]
struct AgentWriteBody {
    raw: String,
}

/// `PUT /api/agents/:name` — overwrite (or create) the agent definition
/// `:name` (a file STEM — see [`AgentDto::slug`]). The raw `.md` is validated
/// by [`rupu_agent::AgentSpec::parse`] before any write; the frontmatter
/// `name:` must equal `:name`.
///
/// Accepts optional `?scope_kind=global|project&scope_id=<workspace id>`
/// query params (see [`ScopeQuery`]'s doc comment) — the SAME selector
/// [`delete_agent`] takes.
///
/// - An explicit selector resolves STRICTLY via
///   [`resolve_agent_scoped_explicit`] and writes to that ONE layer; 404 on
///   any mismatch (wrong layer, absent file, unknown `scope_id`) rather than
///   a silent fallback to another layer — exactly like `DELETE`, and nothing
///   is written when it 404s.
/// - No selector (older clients, or a name not yet known to resolve
///   anywhere): resolves via the implicit project-first walk
///   [`resolve_agent_scoped`] — the SAME resolver `DELETE` uses (by file
///   stem). A name that already resolves (in EITHER layer) is overwritten IN
///   PLACE there; a name that resolves NOWHERE is created fresh in the
///   global layer (unchanged behavior for a genuinely new definition).
///
/// This closes the operator-visible bug where the old global-only PUT wrote
/// an edit to a HIDDEN new global file while a same-named PROJECT file (the
/// one the editor was actually showing) was left untouched — reading as
/// "Save did nothing" while quietly leaving a shadow copy the list would
/// never surface. See `workflows::resolve_workflow_scoped`'s doc comment for
/// the identical reasoning `DELETE` already applies.
///
/// Returns the reloaded detail DTO — [`load_detail`] resolves the SAME
/// project-first way, so the response always echoes the file this handler
/// actually just wrote.
async fn write_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ScopeQuery>,
    Json(body): Json<AgentWriteBody>,
) -> ApiResult<Json<AgentDetailDto>> {
    match q.scope_kind {
        Some(kind) => {
            let (_path, dir, _scope, _kind) =
                resolve_agent_scoped_explicit(&s, &name, kind, q.scope_id.as_deref())
                    .ok_or_else(|| {
                        ApiError::not_found(format!(
                            "agent {name} not found in the requested scope"
                        ))
                    })?;
            save_agent_file_at(&name, &body.raw, &dir)?;
        }
        None => match resolve_agent_scoped(&s, &name) {
            Some((_path, dir, _scope, _kind, _scope_id)) => {
                save_agent_file_at(&name, &body.raw, &dir)?;
            }
            None => {
                save_agent_file(&s.global_dir, &name, &body.raw)?;
            }
        },
    }
    Ok(Json(load_detail(&s, &name)?))
}

/// `POST /api/agents` — create a new agent. The name is taken from the parsed
/// frontmatter; fails with 409 if a definition with that name already
/// resolves ANYWHERE (project-first, then global — the SAME resolver
/// `DELETE`/`PUT` use, by file stem), not just the global layer, so create
/// can never silently shadow an existing PROJECT definition with a brand-new
/// global file of the same name. Always writes into the global layer
/// (creating directly into a specific project via this endpoint is not
/// supported). Returns the reloaded detail DTO.
async fn create_agent(
    State(s): State<AppState>,
    Json(body): Json<AgentWriteBody>,
) -> ApiResult<Json<AgentDetailDto>> {
    let spec = rupu_agent::AgentSpec::parse(&body.raw)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    validate_name(&spec.name)?;
    if resolve_agent_scoped(&s, &spec.name).is_some() {
        return Err(ApiError::conflict("agent already exists"));
    }
    create_agent_file(&s.global_dir, &body.raw)?;
    Ok(Json(load_detail(&s, &spec.name)?))
}

/// `DELETE /api/agents/:name` — remove the agent definition whose file STEM
/// is `:name` (see [`AgentDto::slug`]).
///
/// Accepts optional `?scope_kind=global|project&scope_id=<workspace id>`
/// query params (see [`ScopeQuery`]'s doc comment). When present, resolution
/// is EXPLICIT via [`resolve_agent_scoped_explicit`] — restricted to exactly
/// that layer/workspace, 404 on any mismatch rather than falling back to
/// another layer. When absent (older clients), falls back to the implicit
/// project-first resolver [`resolve_agent_scoped`] — a project-scoped
/// agent's Delete row-action therefore removes the actual project file the
/// row/detail page displays, not a same-named global file it shadows — the
/// data-loss bug PR #536 worked around by hiding Delete on non-global rows
/// entirely, and a LATER bug this fixes: the implicit resolver briefly
/// resolved global-first, backwards from what the list/CLI runtime prefer —
/// see [`resolve_agent_scoped`]'s doc comment.
///
/// Returns the resolved `scope`/`scope_kind` alongside `deleted: true` so the
/// caller can confirm which layer's file was actually removed. Local-only,
/// no `?host=` — see [`resolve_agent_scoped`]'s doc comment.
async fn delete_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_name(&name)?;
    let (target, dir, scope, scope_kind) = match q.scope_kind {
        Some(kind) => resolve_agent_scoped_explicit(&s, &name, kind, q.scope_id.as_deref())
            .ok_or_else(|| {
                ApiError::not_found(format!("agent {name} not found in the requested scope"))
            })?,
        None => {
            let (target, dir, scope, scope_kind, _scope_id) = resolve_agent_scoped(&s, &name)
                .ok_or_else(|| ApiError::not_found(format!("agent {name} not found")))?;
            (target, dir, scope, scope_kind)
        }
    };
    // Defense in depth — see `validate_within`'s doc comment.
    validate_within(&target, &dir)?;
    std::fs::remove_file(&target).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "deleted": true,
        "scope": scope,
        "scope_kind": scope_kind,
    })))
}

/// Request body for `POST /api/agents/:name/run`. All fields optional; a
/// bodyless POST launches the agent with no prompt in its default mode.
#[derive(Deserialize, Default)]
struct AgentRunBody {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    /// Optional host id. Absent or `"local"` → local path (including the
    /// existing 501 when no launcher is installed). A remote id proxies via
    /// [`HostConnector::launch_agent`] and returns `{ "run_id", "host_id" }`.
    #[serde(default)]
    host: Option<String>,
}

/// Testable core: map the body + a concrete launcher to a run id.
async fn run_agent_with(
    name: &str,
    body: AgentRunBody,
    launcher: Arc<dyn AgentLauncher>,
) -> Result<String, ApiError> {
    let req = AgentLaunchRequest {
        agent: name.to_string(),
        prompt: body.prompt,
        mode: body.mode,
        target: body.target,
        working_dir: body.working_dir,
    };
    launcher.launch(req).await.map_err(|e| match e {
        AgentLaunchError::Invalid(m) => ApiError::bad_request(m),
        AgentLaunchError::Spawn(m) => ApiError::internal(m),
    })
}

/// Start a fresh run of agent `:name` via the configured [`AgentLauncher`]
/// (local) or by proxying to a remote host. Returns the new run id plus the
/// owning `host_id`. 501 when no launcher is installed and the target is local.
///
/// [`AgentLauncher`]: crate::agent_launcher::AgentLauncher
async fn run_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<AgentRunBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let host = b.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = AgentLaunchRequest {
            agent: name.clone(),
            prompt: b.prompt,
            mode: b.mode,
            target: b.target,
            working_dir: b.working_dir,
        };
        let run_id = conn.launch_agent(req).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": host }),
        ));
    }

    // Local path: unchanged (including the 501 when no launcher is installed).
    let launcher = s
        .agent_launcher
        .clone()
        .ok_or_else(|| ApiError::not_available("launching agents requires `rupu cp serve`"))?;
    let run_id = run_agent_with(&name, b, launcher).await?;
    Ok(Json(
        serde_json::json!({ "run_id": run_id, "host_id": "local" }),
    ))
}

/// Request body for `POST /api/agents/:name/session`. All fields optional; a
/// bodyless POST starts the agent session with no prompt in its default mode.
#[derive(Deserialize, Default)]
struct SessionStartBody {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    /// Optional host id. Absent or `"local"` → local path (including the
    /// existing 501 when no starter is installed). A remote id proxies via
    /// [`HostConnector::start_session`] and returns `{ "session_id", "host_id" }`.
    #[serde(default)]
    host: Option<String>,
}

/// Testable core: map the body + a concrete starter to a session id.
async fn start_session_with(
    name: &str,
    body: SessionStartBody,
    starter: Arc<dyn SessionStarter>,
) -> Result<String, ApiError> {
    let req = SessionStartRequest {
        agent: name.to_string(),
        prompt: body.prompt,
        mode: body.mode,
        target: body.target,
        working_dir: body.working_dir,
    };
    starter.start(req).await.map_err(|e| match e {
        SessionStartError::Invalid(m) => ApiError::bad_request(m),
        SessionStartError::Spawn(m) => ApiError::internal(m),
    })
}

/// Start a fresh session of agent `:name` via the configured [`SessionStarter`]
/// (local) or by proxying to a remote host. Returns the new session id plus the
/// owning `host_id`. 501 when no starter is installed and the target is local.
///
/// [`SessionStarter`]: crate::session_starter::SessionStarter
async fn start_session(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<SessionStartBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let host = b.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = SessionStartRequest {
            agent: name.clone(),
            prompt: b.prompt,
            mode: b.mode,
            target: b.target,
            working_dir: b.working_dir,
        };
        let session_id = conn.start_session(req).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(
            serde_json::json!({ "session_id": session_id, "host_id": host }),
        ));
    }

    // Local path: unchanged (including the 501 when no starter is installed).
    let starter = s
        .session_starter
        .clone()
        .ok_or_else(|| ApiError::not_available("starting sessions requires `rupu cp serve`"))?;
    let session_id = start_session_with(&name, b, starter).await?;
    Ok(Json(
        serde_json::json!({ "session_id": session_id, "host_id": "local" }),
    ))
}

#[derive(Deserialize)]
struct GenerateAgentBody {
    description: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct GeneratedDefDto {
    raw: String,
    provider: String,
    model: String,
    attempts: u8,
}

async fn generate_agent(
    State(s): State<AppState>,
    Json(body): Json<GenerateAgentBody>,
) -> ApiResult<Json<GeneratedDefDto>> {
    use crate::definition_generator::{DefKind, GenDefError, GenerateDefRequest};
    let gen = s
        .generator
        .clone()
        .ok_or_else(|| ApiError::not_available("AI generation requires `rupu cp serve`"))?;
    let out = gen
        .generate(GenerateDefRequest {
            kind: DefKind::Agent,
            description: body.description,
            provider: body.provider,
            model: body.model,
        })
        .await
        .map_err(|e| match e {
            GenDefError::NoCredentials => ApiError::bad_request(e.to_string()),
            GenDefError::Failed(m) => ApiError::internal(m),
        })?;
    Ok(Json(GeneratedDefDto {
        raw: out.raw,
        provider: out.provider,
        model: out.model,
        attempts: out.attempts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
    use crate::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};
    use std::sync::{Arc, Mutex};

    struct MockAgent {
        last: Mutex<Option<AgentLaunchRequest>>,
    }

    #[async_trait::async_trait]
    impl AgentLauncher for MockAgent {
        async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("run_A".into())
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
    async fn run_agent_forwards_request() {
        let mock = Arc::new(MockAgent {
            last: Mutex::new(None),
        });
        let body = AgentRunBody {
            prompt: Some("do it".into()),
            mode: Some("bypass".into()),
            target: None,
            working_dir: Some("/tmp/p".into()),
            host: None,
        };
        let run_id = run_agent_with("triage", body, mock.clone())
            .await
            .expect("ok");
        assert_eq!(run_id, "run_A");
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.agent, "triage");
        assert_eq!(got.prompt.as_deref(), Some("do it"));
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/p"));
    }

    #[tokio::test]
    async fn missing_launcher_is_not_available() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // agent_launcher: None

        let err = run_agent(State(s), Path("triage".into()), None)
            .await
            .expect_err("no launcher should error");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    const VALID_MD: &str = "---\nname: code-reviewer\nmodel: opus\n---\nReview code carefully.\n";

    #[test]
    fn save_writes_exact_bytes_at_named_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = save_agent_file(tmp.path(), "code-reviewer", VALID_MD).expect("save ok");
        assert_eq!(path, tmp.path().join("agents").join("code-reviewer.md"));
        let back = std::fs::read_to_string(&path).unwrap();
        assert_eq!(back, VALID_MD);
        // No leftover temp file.
        assert!(!path.with_extension("md.tmp").exists());
    }

    #[test]
    fn save_unparseable_is_bad_request_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = save_agent_file(tmp.path(), "code-reviewer", "no frontmatter here")
            .expect_err("should reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!tmp.path().join("agents").join("code-reviewer.md").exists());
    }

    #[test]
    fn save_name_mismatch_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = save_agent_file(tmp.path(), "other-name", VALID_MD).expect_err("mismatch");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!tmp.path().join("agents").join("other-name.md").exists());
    }

    #[test]
    fn create_conflicts_then_writes_using_frontmatter_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Absent → writes at the frontmatter-derived name.
        let path = create_agent_file(tmp.path(), VALID_MD).expect("create ok");
        assert_eq!(path, tmp.path().join("agents").join("code-reviewer.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID_MD);
        // Present → conflict.
        let err = create_agent_file(tmp.path(), VALID_MD).expect_err("conflict");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    /// A slug that already resolves in a PROJECT must not be silently
    /// shadowed by a brand-new global file of the same name — `POST` must
    /// 409, project-aware, not just "absent from global."
    #[tokio::test]
    async fn post_conflicts_on_project_only_name_without_creating_a_global_shadow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // `AgentDetailDto` (the `Ok` payload) doesn't implement `Debug`, so
        // `expect_err` isn't usable here — match explicitly instead.
        let err = match create_agent(State(s.clone()), Json(AgentWriteBody { raw: VALID_MD.into() })).await {
            Err(e) => e,
            Ok(_) => panic!("must conflict against the project-only definition"),
        };
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
        assert!(
            !agents_dir(&s).join("code-reviewer.md").exists(),
            "no hidden global shadow must be created"
        );
    }

    // ── PUT scope-awareness (write-path parity with DELETE) ───────────────
    //
    // `write_agent` used to unconditionally write to the GLOBAL layer, then
    // return `load_detail`, which (like `DELETE`) resolves project-first. For
    // a project-only agent this meant: PUT silently created a HIDDEN new
    // global file holding the edit, while `load_detail` echoed back the
    // UNCHANGED project file — reading as "Save did nothing" while quietly
    // leaving a shadow copy the list would never show. These tests are the
    // seeded-collision regression suite for the fix: an explicit
    // `?scope_kind=&scope_id=` selector (the SAME shape `DELETE` takes) pins
    // the write to exactly one layer, 404ing rather than falling back, and —
    // even with NO selector — a name that already resolves somewhere is
    // edited IN PLACE there rather than shadowed.

    /// (a) Saving a PROJECT-scoped def with an explicit project selector
    /// writes the PROJECT file and creates NO global file.
    #[tokio::test]
    async fn put_explicit_project_selector_writes_project_creates_no_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let edited = format!("{VALID_MD}\nEdited.\n");
        let resp = write_agent(
            State(s.clone()),
            Path("code-reviewer".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("ws_a".to_string()),
            }),
            Json(AgentWriteBody { raw: edited.clone() }),
        )
        .await
        .expect("explicit project-scoped put should succeed");
        assert_eq!(
            resp.0.summary.scope_kind,
            ScopeKind::Project
        );
        assert_eq!(
            std::fs::read_to_string(proj_agents.join("code-reviewer.md")).unwrap(),
            edited,
            "the PROJECT file must hold the edit"
        );
        assert!(
            !agents_dir(&s).join("code-reviewer.md").exists(),
            "no hidden GLOBAL file must be created"
        );
    }

    /// (b) Same slug present in BOTH layers, explicit project selector → the
    /// project file changes, the global file is byte-unchanged.
    #[tokio::test]
    async fn put_explicit_project_selector_with_slug_in_both_layers_leaves_global_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        const GLOBAL_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nGlobal copy.\n";
        save_agent_file(&s.global_dir, "shared-name", GLOBAL_MD).expect("seed global");

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        const PROJECT_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nProject copy.\n";
        std::fs::write(proj_agents.join("shared-name.md"), PROJECT_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let edited = "---\nname: shared-name\nmodel: opus\n---\nProject copy, edited.\n";
        let resp = write_agent(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("ws_a".to_string()),
            }),
            Json(AgentWriteBody { raw: edited.to_string() }),
        )
        .await
        .expect("explicit project-scoped put should succeed");
        assert_eq!(resp.0.summary.scope_kind, ScopeKind::Project);
        assert_eq!(
            std::fs::read_to_string(proj_agents.join("shared-name.md")).unwrap(),
            edited,
            "the PROJECT file must hold the edit"
        );
        assert_eq!(
            std::fs::read_to_string(agents_dir(&s).join("shared-name.md")).unwrap(),
            GLOBAL_MD,
            "the GLOBAL file must be byte-unchanged"
        );
    }

    /// (c) An explicit selector pointing at a layer that doesn't hold the
    /// def → 404, and NOTHING is written anywhere.
    #[tokio::test]
    async fn put_explicit_scope_mismatch_404s_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        const GLOBAL_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nGlobal copy.\n";
        save_agent_file(&s.global_dir, "shared-name", GLOBAL_MD).expect("seed global");

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        const PROJECT_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nProject copy.\n";
        std::fs::write(proj_agents.join("shared-name.md"), PROJECT_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // `AgentDetailDto` (the `Ok` payload) doesn't implement `Debug`, so
        // `expect_err` isn't usable here — match explicitly instead.
        let err = match write_agent(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("no-such-workspace".to_string()),
            }),
            Json(AgentWriteBody {
                raw: "---\nname: shared-name\nmodel: opus\n---\nShould not land.\n".to_string(),
            }),
        )
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("mismatched explicit scope must 404, never fall back"),
        };
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            std::fs::read_to_string(agents_dir(&s).join("shared-name.md")).unwrap(),
            GLOBAL_MD,
            "global file untouched"
        );
        assert_eq!(
            std::fs::read_to_string(proj_agents.join("shared-name.md")).unwrap(),
            PROJECT_MD,
            "project file untouched"
        );
    }

    /// (d) No selector + slug resolves only in a project → writes the
    /// PROJECT file, still no global shadow created.
    #[tokio::test]
    async fn put_no_selector_project_only_slug_writes_project_no_global_shadow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let edited = format!("{VALID_MD}\nEdited.\n");
        let resp = write_agent(
            State(s.clone()),
            Path("code-reviewer".into()),
            Query(ScopeQuery::default()),
            Json(AgentWriteBody { raw: edited.clone() }),
        )
        .await
        .expect("implicit project-first put should succeed");
        assert_eq!(resp.0.summary.scope_kind, ScopeKind::Project);
        assert_eq!(
            std::fs::read_to_string(proj_agents.join("code-reviewer.md")).unwrap(),
            edited,
            "the PROJECT file must hold the edit"
        );
        assert!(
            !agents_dir(&s).join("code-reviewer.md").exists(),
            "no hidden GLOBAL shadow must be created"
        );
    }

    /// (e) No selector + slug resolves nowhere → creates in global
    /// (unchanged creation behavior).
    #[tokio::test]
    async fn put_no_selector_unresolved_slug_creates_in_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let resp = write_agent(
            State(s.clone()),
            Path("code-reviewer".into()),
            Query(ScopeQuery::default()),
            Json(AgentWriteBody { raw: VALID_MD.to_string() }),
        )
        .await
        .expect("put ok");
        assert_eq!(resp.0.summary.scope_kind, ScopeKind::Global);
        assert_eq!(
            std::fs::read_to_string(agents_dir(&s).join("code-reviewer.md")).unwrap(),
            VALID_MD
        );
    }

    #[test]
    fn delete_present_then_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");

        let body = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(delete_agent(
                State(s.clone()),
                Path("code-reviewer".into()),
                Query(ScopeQuery::default()),
            ))
            .expect("delete ok");
        assert_eq!(body.0["deleted"], serde_json::json!(true));
        assert_eq!(body.0["scope"], serde_json::json!("global"));
        assert_eq!(body.0["scope_kind"], serde_json::json!("global"));
        assert!(!agents_dir(&s).join("code-reviewer.md").exists());

        // Second delete → not found.
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(delete_agent(
                State(s.clone()),
                Path("code-reviewer".into()),
                Query(ScopeQuery::default()),
            ))
            .expect_err("absent");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// Regression for the operator complaint this PR fixes: a project-scoped
    /// agent's Delete row-action must remove the actual PROJECT file, not a
    /// same-named global one (or, if there is no name collision, an
    /// unrelated global definition must be left byte-for-byte untouched).
    #[tokio::test]
    async fn delete_removes_project_file_leaves_unrelated_global_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        // An unrelated global agent — must survive the project-scoped delete.
        const KEEPER_MD: &str = "---\nname: keeper\nmodel: opus\n---\nStays put.\n";
        save_agent_file(&s.global_dir, "keeper", KEEPER_MD).expect("seed global");

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        const REVIEWER_MD: &str = "---\nname: reviewer\nmodel: opus\n---\nProject reviewer.\n";
        std::fs::write(proj_agents.join("reviewer.md"), REVIEWER_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let resp = delete_agent(
            State(s.clone()),
            Path("reviewer".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("project-scoped delete should succeed");
        assert_eq!(resp.0["deleted"], serde_json::json!(true));
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert!(
            !proj_agents.join("reviewer.md").exists(),
            "the PROJECT file must be removed"
        );
        assert!(
            agents_dir(&s).join("keeper.md").exists(),
            "an unrelated GLOBAL agent must be left untouched"
        );
    }

    /// Vice-versa of the above: deleting a global agent must leave a
    /// differently-named project agent untouched.
    #[tokio::test]
    async fn delete_removes_global_file_leaves_unrelated_project_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        const REVIEWER_MD: &str = "---\nname: reviewer\nmodel: opus\n---\nGlobal reviewer.\n";
        save_agent_file(&s.global_dir, "reviewer", REVIEWER_MD).expect("seed global");

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        const OTHER_MD: &str = "---\nname: other\nmodel: opus\n---\nProject-only agent.\n";
        std::fs::write(proj_agents.join("other.md"), OTHER_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let resp = delete_agent(
            State(s.clone()),
            Path("reviewer".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("global delete should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("global"));
        assert!(!agents_dir(&s).join("reviewer.md").exists());
        assert!(
            proj_agents.join("other.md").exists(),
            "an unrelated PROJECT agent must be left untouched"
        );
    }

    /// A traversal-y `:name` must be rejected by `validate_name` before any
    /// disk access — mirrors `api::autoflows`'s identical guard.
    #[tokio::test]
    async fn delete_rejects_traversal_name_before_any_disk_access() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        // A file a successful traversal *would* reach: sibling of the global
        // agents dir, i.e. `<global_dir>/evil.md` via `../evil` from inside
        // `<global_dir>/agents/`.
        let outside_target = tmp.path().join("evil.md");
        std::fs::write(&outside_target, VALID_MD).unwrap();

        let err = delete_agent(
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

    // ── Critical 1: project-first precedence for a slug shared by BOTH
    //    layers ─────────────────────────────────────────────────────────
    //
    // The list always shadows a same-named GLOBAL row WITH the project row,
    // so Delete on the row the operator sees must remove the PROJECT file,
    // never the hidden global one.

    #[tokio::test]
    async fn delete_with_slug_in_both_layers_removes_project_leaves_global_intact() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        const GLOBAL_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nGlobal copy.\n";
        save_agent_file(&s.global_dir, "shared-name", GLOBAL_MD).expect("seed global");

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        const PROJECT_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nProject copy.\n";
        std::fs::write(proj_agents.join("shared-name.md"), PROJECT_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // The list shadows the global row with the project row.
        let Json(rows) = list_agents(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 1, "the project row shadows the global one");
        assert_eq!(rows[0].scope_kind, ScopeKind::Project);

        // No explicit scope query — the implicit (project-first) resolver
        // must match what the list showed.
        let resp = delete_agent(
            State(s.clone()),
            Path("shared-name".into()),
            Query(ScopeQuery::default()),
        )
        .await
        .expect("delete should succeed");
        assert_eq!(resp.0["scope_kind"], serde_json::json!("project"));
        assert!(
            !proj_agents.join("shared-name.md").exists(),
            "the PROJECT file (the one the list/operator saw) must be removed"
        );
        assert!(
            agents_dir(&s).join("shared-name.md").exists(),
            "the shadowed GLOBAL file must survive untouched"
        );
    }

    // ── Critical 2: `scope_id` disambiguates a slug shared by TWO repos ──

    #[tokio::test]
    async fn delete_with_explicit_scope_id_targets_one_repo_of_two_with_the_same_slug() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj_x = tmp.path().join("proj-x");
        let agents_x = proj_x.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_x).unwrap();
        std::fs::write(agents_x.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let agents_y = proj_y.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_y).unwrap();
        std::fs::write(agents_y.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        // Both rows appear — confirm the ambiguity is real before resolving
        // it with an explicit scope_id.
        let Json(rows) = list_agents(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 2);

        let resp = delete_agent(
            State(s.clone()),
            Path("code-reviewer".into()),
            Query(ScopeQuery {
                scope_kind: Some(ScopeKind::Project),
                scope_id: Some("ws_y".to_string()),
            }),
        )
        .await
        .expect("explicit-scope delete should succeed");
        assert_eq!(resp.0["scope"], serde_json::json!("proj-y"));
        assert!(
            !agents_y.join("code-reviewer.md").exists(),
            "repo Y's file must be removed"
        );
        assert!(
            agents_x.join("code-reviewer.md").exists(),
            "repo X's same-named file must survive untouched"
        );
    }

    #[tokio::test]
    async fn delete_explicit_scope_mismatch_404s_and_leaves_every_file_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        const GLOBAL_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nGlobal copy.\n";
        save_agent_file(&s.global_dir, "shared-name", GLOBAL_MD).expect("seed global");

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        const PROJECT_MD: &str = "---\nname: shared-name\nmodel: opus\n---\nProject copy.\n";
        std::fs::write(proj_agents.join("shared-name.md"), PROJECT_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let err = delete_agent(
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
        assert!(agents_dir(&s).join("shared-name.md").exists(), "global file untouched");
        assert!(
            proj_agents.join("shared-name.md").exists(),
            "project file untouched"
        );
    }

    // ── resolve_agent_scoped_explicit: any registered workspace id, not
    // just a distinct_repo_workspaces representative ─────────────────────
    //
    // Mirrors the workflows-side suite in `workflows.rs` exactly — see its
    // doc comments for the full rationale (`/api/projects/:ws_id/*` reports
    // the caller's REQUESTED `ws_id`, which for a multi-worktree repo is
    // often not the representative `distinct_repo_workspaces` would pick).

    /// (a) An explicit `scope_id` naming a NON-representative worktree of a
    /// multi-worktree repo resolves to THAT worktree's own file.
    #[tokio::test]
    async fn explicit_project_scope_resolves_non_representative_worktree_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        // proj-a sorts first -> distinct_repo_workspaces picks it as the
        // representative when there's no tracked-repo preferred_path.
        let proj_a = tmp.path().join("proj-a");
        std::fs::create_dir_all(proj_a.join(".rupu").join("agents")).unwrap();
        register_workspace_with_remote(&tmp, "ws_a", &proj_a, Some("git@github.com:acme/x.git"));

        let proj_b = tmp.path().join("proj-b");
        let agents_b = proj_b.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_b).unwrap();
        std::fs::write(agents_b.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_b", &proj_b, Some("git@github.com:acme/x.git"));

        // Confirm proj-a really is the representative.
        let workspaces = store(&s).list().unwrap();
        let reps = distinct_repo_workspaces(workspaces, &repo_store(&s));
        assert_eq!(reps.len(), 1);
        assert_eq!(reps[0].workspace.id, "ws_a");

        let resolved =
            resolve_agent_scoped_explicit(&s, "code-reviewer", ScopeKind::Project, Some("ws_b"))
                .expect("must resolve against the non-representative worktree");
        assert_eq!(resolved.0, agents_b.join("code-reviewer.md"));
        assert_eq!(resolved.2, "proj-b");
        assert_eq!(resolved.3, ScopeKind::Project);
    }

    /// (b) A representative's own id still resolves exactly as before.
    #[tokio::test]
    async fn explicit_project_scope_representative_id_still_resolves() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let proj_a = tmp.path().join("proj-a");
        let agents_a = proj_a.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_a).unwrap();
        std::fs::write(agents_a.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_a", &proj_a, Some("git@github.com:acme/x.git"));

        let proj_b = tmp.path().join("proj-b");
        std::fs::create_dir_all(proj_b.join(".rupu").join("agents")).unwrap();
        register_workspace_with_remote(&tmp, "ws_b", &proj_b, Some("git@github.com:acme/x.git"));

        let resolved =
            resolve_agent_scoped_explicit(&s, "code-reviewer", ScopeKind::Project, Some("ws_a"))
                .expect("representative id must still resolve");
        assert_eq!(resolved.0, agents_a.join("code-reviewer.md"));
        assert_eq!(resolved.2, "proj-a");
    }

    /// (c) An id matching no registered workspace at all still returns
    /// `None` (⇒ the existing 404 path).
    #[tokio::test]
    async fn explicit_project_scope_unknown_id_returns_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(agents_dir(&s)).unwrap();

        assert!(resolve_agent_scoped_explicit(
            &s,
            "code-reviewer",
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
        let agents_a = proj_a.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_a).unwrap();
        std::fs::write(agents_a.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_a", &proj_a, Some("git@github.com:acme/x.git"));

        // ws_b is a registered worktree of the SAME repo but has no
        // `code-reviewer.md` of its own.
        let proj_b = tmp.path().join("proj-b");
        std::fs::create_dir_all(proj_b.join(".rupu").join("agents")).unwrap();
        register_workspace_with_remote(&tmp, "ws_b", &proj_b, Some("git@github.com:acme/x.git"));

        assert!(
            resolve_agent_scoped_explicit(&s, "code-reviewer", ScopeKind::Project, Some("ws_b"))
                .is_none(),
            "must not fall back to ws_a's copy of the same-named def"
        );
        assert!(
            agents_a.join("code-reviewer.md").exists(),
            "ws_a's file must be untouched (never even inspected as a fallback target)"
        );
    }

    /// (e) The `scope` string an explicit representative-id lookup returns
    /// matches exactly what the aggregate list endpoint shows for that same
    /// project row (both ultimately derive from `repo_scope::scope_name`).
    #[tokio::test]
    async fn explicit_project_scope_display_string_matches_list_endpoint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let Json(rows) = list_agents(State(s.clone())).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope_id.as_deref(), Some("ws_a"));

        let resolved =
            resolve_agent_scoped_explicit(&s, "code-reviewer", ScopeKind::Project, Some("ws_a"))
                .expect("must resolve");
        assert_eq!(resolved.2, rows[0].scope);
    }

    struct MockStarter {
        last: Mutex<Option<SessionStartRequest>>,
    }

    #[async_trait::async_trait]
    impl SessionStarter for MockStarter {
        async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("ses_TEST".into())
        }
    }

    #[tokio::test]
    async fn start_session_forwards_request() {
        let mock = Arc::new(MockStarter {
            last: Mutex::new(None),
        });
        let body = SessionStartBody {
            prompt: Some("hi".into()),
            mode: Some("ask".into()),
            target: None,
            working_dir: Some("/tmp/p".into()),
            host: None,
        };
        let id = start_session_with("triage", body, mock.clone())
            .await
            .expect("ok");
        assert_eq!(id, "ses_TEST");
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.agent, "triage");
        assert_eq!(got.prompt.as_deref(), Some("hi"));
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/p"));
    }

    #[tokio::test]
    async fn start_session_without_starter_is_not_available() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // session_starter: None
        let err = start_session(State(s), Path("triage".into()), None)
            .await
            .expect_err("no starter");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    use crate::definition_generator::{
        DefKind, DefinitionGenerator, GenDefError, GenerateDefRequest, GeneratedDef, ProviderModels,
    };

    struct StubGen;
    #[async_trait::async_trait]
    impl DefinitionGenerator for StubGen {
        async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
            assert_eq!(req.kind, DefKind::Agent);
            Ok(GeneratedDef {
                raw: VALID_MD.to_string(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                attempts: 1,
            })
        }
        async fn available_models(&self) -> Vec<ProviderModels> {
            vec![ProviderModels {
                provider: "anthropic".into(),
                models: vec!["claude-sonnet-4-6".into()],
                is_default: true,
            }]
        }
    }

    #[tokio::test]
    async fn generate_agent_returns_content_without_writing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp).with_generator(Some(std::sync::Arc::new(StubGen)));
        let body = GenerateAgentBody {
            description: "x".into(),
            provider: None,
            model: None,
        };
        let Json(out) = generate_agent(State(state), Json(body)).await.expect("ok");
        assert!(out.raw.contains("name:"));
        // Nothing persisted by generate.
        assert!(
            !tmp.path().join("agents").exists()
                || std::fs::read_dir(tmp.path().join("agents"))
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[tokio::test]
    async fn generate_agent_without_adapter_is_not_available() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp); // generator = None
        let body = GenerateAgentBody {
            description: "x".into(),
            provider: None,
            model: None,
        };
        let err = generate_agent(State(state), Json(body)).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
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

    /// `GET /api/agents` exposes the agent's frontmatter `tools:` allowlist —
    /// additive DTO field consumed by the workflow-editor `actions:` picker
    /// (step-actions-enforcement design §3d) to flag a step action the
    /// selected agent doesn't grant.
    #[tokio::test]
    async fn list_agents_exposes_frontmatter_tools() {
        const WITH_TOOLS_MD: &str = "---\nname: issue-reporter\ntools: [issues.list, issues.comment, issues.create]\n---\nTriage issues.\n";
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "issue-reporter", WITH_TOOLS_MD).expect("seed");

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].tools,
            vec!["issues.list".to_string(), "issues.comment".to_string(), "issues.create".to_string()]
        );
    }

    /// An agent whose frontmatter omits `tools:` entirely (unrestricted) still
    /// serializes a (empty) `tools` array rather than erroring or omitting the
    /// field — the DTO field is unconditional, not `Option`.
    #[tokio::test]
    async fn list_agents_without_tools_frontmatter_is_empty_array() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tools, Vec::<String>::new());
    }

    #[tokio::test]
    async fn list_no_projects_is_global_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "code-reviewer");
        assert_eq!(rows[0].scope, "global");
        assert_eq!(rows[0].scope_kind, crate::api::repo_scope::ScopeKind::Global);
    }

    #[tokio::test]
    async fn list_includes_project_defs_tagged_with_project_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(rows[0].name, "code-reviewer");
        assert_eq!(rows[0].scope, expected_scope);
        assert_eq!(
            rows[0].scope_kind,
            crate::api::repo_scope::ScopeKind::Project
        );
    }

    #[tokio::test]
    async fn agent_detail_resolves_project_def() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // Absent from global, present only in the project — must resolve, not 404.
        let resp = get_agent(State(s), Path("code-reviewer".into()))
            .await
            .expect("project-only agent should resolve via detail");
        assert_eq!(resp.0.summary.name, "code-reviewer");
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resp.0.summary.scope, expected_scope);
        // Project-only detail must be tagged `scope_kind: Project` — the CP
        // detail page's Delete gate keys off this, never the display `scope`.
        assert_eq!(
            resp.0.summary.scope_kind,
            crate::api::repo_scope::ScopeKind::Project
        );
    }

    /// A GLOBAL agent's detail DTO is tagged `scope_kind: Global` — the
    /// counterpart to `agent_detail_resolves_project_def`'s Project case.
    /// The CP detail page's Delete gate keys off this field.
    #[tokio::test]
    async fn agent_detail_global_is_scope_kind_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");

        let resp = get_agent(State(s), Path("code-reviewer".into()))
            .await
            .expect("global agent should resolve");
        assert_eq!(resp.0.summary.scope, "global");
        assert_eq!(
            resp.0.summary.scope_kind,
            crate::api::repo_scope::ScopeKind::Global
        );
    }

    #[tokio::test]
    async fn same_repo_worktrees_dedupe_to_one_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        // Three registered workspaces = run-worktrees of the SAME repo, each
        // carrying its own copy of `.rupu/agents/code-reviewer.md`.
        let remote = "git@github.com:acme/widgets.git";
        for (id, name) in [
            ("ws_a", "worktree-a"),
            ("ws_b", "worktree-b"),
            ("ws_c", "worktree-c"),
        ] {
            let root = tmp.path().join(name);
            let proj_agents = root.join(".rupu").join("agents");
            std::fs::create_dir_all(&proj_agents).unwrap();
            std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
            register_workspace_with_remote(&tmp, id, &root, Some(remote));
        }

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            1,
            "code-reviewer must appear exactly once despite 3 worktrees of the same repo"
        );
        assert_eq!(rows[0].name, "code-reviewer");
        // No tracked-repo record was seeded, so the tie-break is the
        // deterministic path sort: "worktree-a" sorts first.
        assert_eq!(rows[0].scope, "worktree-a");
    }

    #[tokio::test]
    async fn different_repos_same_def_name_both_appear() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj_x = tmp.path().join("proj-x");
        let agents_x = proj_x.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_x).unwrap();
        std::fs::write(agents_x.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let agents_y = proj_y.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_y).unwrap();
        std::fs::write(agents_y.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2, "different repos are distinct groups");
        let scopes: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["proj-x", "proj-y"])
        );
        assert!(rows.iter().all(|r| r.name == "code-reviewer"));
    }

    #[tokio::test]
    async fn no_repo_remote_scans_every_standalone_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        const OTHER_MD: &str = "---\nname: doc-writer\nmodel: opus\n---\nWrite docs.\n";

        let proj_a = tmp.path().join("standalone-a");
        let agents_a = proj_a.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_a).unwrap();
        std::fs::write(agents_a.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", &proj_a);

        let proj_b = tmp.path().join("standalone-b");
        let agents_b = proj_b.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_b).unwrap();
        std::fs::write(agents_b.join("doc-writer.md"), OTHER_MD).unwrap();
        register_workspace(&tmp, "ws_b", &proj_b);

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            2,
            "both standalone (no repo_remote) dirs are scanned"
        );
        let names: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["code-reviewer", "doc-writer"])
        );
    }

    /// Write a single-event transcript (`RunStart` only, no `Usage` events) so
    /// `rupu_transcript::aggregate` emits exactly one zero-token row for
    /// `agent`, bumping that row's `runs` by 1.
    fn write_agent_transcript(path: &std::path::Path, agent: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ev = rupu_transcript::Event::RunStart {
            run_id: "r1".into(),
            workspace_id: "ws1".into(),
            agent: agent.into(),
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            started_at: chrono::Utc::now(),
            mode: rupu_transcript::RunMode::Ask,
        };
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        std::fs::write(path, &line).unwrap();
    }

    /// Register a run of `workflow_name` bound to `workspace_id`, with one
    /// completed step whose transcript attributes usage to `agent`.
    fn seed_run_with_agent_usage(
        s: &AppState,
        run_id: &str,
        workspace_id: &str,
        agent: &str,
        transcript_path: &std::path::Path,
    ) {
        let record = rupu_orchestrator::RunRecord {
            id: run_id.into(),
            workflow_name: "wf".into(),
            status: rupu_orchestrator::RunStatus::Completed,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: workspace_id.into(),
            workspace_path: PathBuf::from("/tmp/proj"),
            transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
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
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
        };
        s.run_store.create(record, "name: wf\n").unwrap();
        write_agent_transcript(transcript_path, agent);
        s.run_store
            .append_step_result(
                run_id,
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "s1".into(),
                    run_id: run_id.into(),
                    transcript_path: transcript_path.to_path_buf(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: rupu_orchestrator::runs::StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                },
            )
            .unwrap();
    }

    #[tokio::test]
    async fn same_named_rows_from_different_repos_do_not_both_show_combined_usage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj_x = tmp.path().join("proj-x");
        let agents_x = proj_x.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_x).unwrap();
        std::fs::write(agents_x.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let agents_y = proj_y.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_y).unwrap();
        std::fs::write(agents_y.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        // Two runs, both attributing usage to agent "code-reviewer" — as far
        // as the transcript breakdown is concerned they're indistinguishable
        // by scope (grouped by agent name alone).
        seed_run_with_agent_usage(
            &s,
            "run_1",
            "ws_x",
            "code-reviewer",
            &tmp.path().join("t1.jsonl"),
        );
        seed_run_with_agent_usage(
            &s,
            "run_2",
            "ws_y",
            "code-reviewer",
            &tmp.path().join("t2.jsonl"),
        );

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.name == "code-reviewer"));

        let run_counts: Vec<u64> = rows.iter().map(|r| r.run_count).collect();
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

        // The canonical-row rule must hold for `last_run` too: exactly one of
        // the two same-named rows carries a timestamp, the other stays `None`
        // rather than both showing the same (duplicated) last-run signal.
        let last_runs: Vec<Option<String>> = rows.iter().map(|r| r.last_run.clone()).collect();
        assert_eq!(
            last_runs.iter().filter(|r| r.is_some()).count(),
            1,
            "exactly one row carries last_run"
        );
        assert_eq!(
            last_runs.iter().filter(|r| r.is_none()).count(),
            1,
            "the other same-named row stays None rather than duplicating last_run"
        );
    }

    #[tokio::test]
    async fn list_agents_last_run_reflects_run_and_none_when_never_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");
        const GHOST_MD: &str = "---\nname: ghost\nmodel: opus\n---\nNever runs.\n";
        save_agent_file(&s.global_dir, "ghost", GHOST_MD).expect("seed");

        seed_run_with_agent_usage(
            &s,
            "run_1",
            "ws_x",
            "code-reviewer",
            &tmp.path().join("t1.jsonl"),
        );

        let Json(rows) = list_agents(State(s)).await.expect("ok");

        let reviewer = rows
            .iter()
            .find(|r| r.name == "code-reviewer")
            .expect("code-reviewer present");
        assert!(
            reviewer.last_run.is_some(),
            "an agent that ran must carry a last_run timestamp"
        );

        let ghost = rows
            .iter()
            .find(|r| r.name == "ghost")
            .expect("ghost present");
        assert_eq!(
            ghost.last_run, None,
            "an agent that never ran must have last_run == None"
        );
    }

    /// The Delete row-action must key off the file STEM, not the
    /// frontmatter `name` — `delete_agent` removes `<slug>.md` by file
    /// stem. When a `.md` file's stem differs from its frontmatter `name`
    /// (hand- or CLI-authored files), the DTO must still report the
    /// correct `slug` rather than falling back to `name`. Mirrors
    /// `api::autoflows`'s `slug_is_file_stem_distinct_from_parsed_name`.
    #[tokio::test]
    async fn list_agents_slug_reflects_file_stem_when_it_differs_from_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let dir = agents_dir(&s);
        std::fs::create_dir_all(&dir).unwrap();
        // File stem ("my-file-stem") deliberately differs from the parsed
        // frontmatter `name` ("code-reviewer") in VALID_MD.
        std::fs::write(dir.join("my-file-stem.md"), VALID_MD).unwrap();

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "code-reviewer");
        assert_eq!(rows[0].slug, "my-file-stem");
    }
}
