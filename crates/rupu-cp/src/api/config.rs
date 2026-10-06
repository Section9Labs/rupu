//! `rupu-cp` config read/write API (CP Settings).
//!
//! - `GET /api/config` (+ `?project=<ws_id>` or `?customer=<slug>`, never
//!   both) returns the effective resolved config, per-key provenance, and
//!   the raw global/customer/project TOML text so the settings UI can offer
//!   both a form view and a raw editor.
//! - `PUT /api/config/global` / `PUT /api/config/customer/:slug` /
//!   `PUT /api/config/project/:id` persist an edit
//!   (raw text or a flat form patch) after validating it against the typed
//!   schema, then (for global) reload `AppState.config` so already-running
//!   handlers observe the change without a process restart.
//! - `PUT /api/config/policy` sets the GLOBAL `[policy].lock` list — the
//!   enforced-key allowlist a project layer can never override (see
//!   `rupu_config::resolve`).
//!
//! All writes require an installed [`crate::launcher::RunLauncher`] (the
//! `cp serve` deployment marker) — a read-only `rupu cp` deploy with no
//! launcher returns 501 for every `PUT` here, mirroring the host-add gate.
//!
//! Secrets are never echoed: `Config` has no token/secret field to begin
//! with, and the bearer token `cp serve` was started with is never threaded
//! onto `AppState` at all — only a `token_set: bool` is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxPath, Query, State},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use rupu_workspace::CustomerStore;

use crate::{
    config_write::{apply_form_patch, validate_toml, write_atomic},
    customers::{CustomerLookup, CustomerRef},
    error::{ApiError, ApiResult},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/config", get(get_config))
        .route("/api/config/global", put(put_global))
        .route("/api/config/customer/:slug", put(put_customer))
        .route("/api/config/project/:id", put(put_project))
        .route("/api/config/policy", put(put_policy))
}

/// `?project=<ws_id>` and `?customer=<slug>` select what is layered over
/// global; they are mutually exclusive.
#[derive(Deserialize, Default)]
struct ConfigQuery {
    project: Option<String>,
    customer: Option<String>,
}

#[derive(Serialize)]
pub struct RuntimeStatus {
    pub bind: String,
    pub token_set: bool,
    pub restart_required_keys: Vec<String>,
}

#[derive(Serialize)]
pub struct ConfigView {
    pub effective: serde_json::Value,
    pub provenance: BTreeMap<String, rupu_config::KeyProvenance>,
    pub raw_global: String,
    pub raw_project: Option<String>,
    /// The customer layer's raw text (`?customer=`, or the project's
    /// customer); `None` when there is no customer or it has no file yet.
    pub raw_customer: Option<String>,
    /// The customer in play: the `?customer=` one, or the project's.
    pub customer: Option<CustomerRef>,
    /// The customer layer's `[policy].lock`, as written.
    pub customer_lock: Vec<String>,
    /// Why the layers could not be resolved (a malformed layer). The view is
    /// still served — `effective` is then global only — so the editor opens
    /// and the text can be fixed.
    pub layer_error: Option<String>,
    pub cp: serde_json::Value,
    pub status: RuntimeStatus,
}

/// `GET /api/config` (+ `?project=<ws_id>` | `?customer=<slug>`) — effective
/// config + provenance + raw TOML text for each layer.
async fn get_config(
    State(s): State<AppState>,
    Query(q): Query<ConfigQuery>,
) -> ApiResult<Json<ConfigView>> {
    let global = s.global_dir.join("config.toml");
    let store = CustomerStore::new(s.global_dir.clone());
    let layers = match (&q.project, &q.customer) {
        (Some(_), Some(_)) => {
            return Err(ApiError::bad_request(
                "`project` and `customer` are mutually exclusive",
            ))
        }
        (Some(id), None) => project_layers(&s, &load_ws(&s, id)?)?,
        (None, Some(slug)) => {
            store.get(slug).map_err(customer_api_err)?;
            rupu_workspace::ConfigPaths {
                customer: Some(store.config_path(slug)),
                customer_slug: Some(slug.clone()),
                ..rupu_workspace::ConfigPaths::without_customer(&s.global_dir, None)
            }
        }
        (None, None) => rupu_workspace::ConfigPaths::without_customer(&s.global_dir, None),
    };
    let (resolved, layer_error) = match rupu_config::resolve(layers.layers()) {
        Ok(r) => (r, None),
        // A malformed layer must not lock the operator out of the editor
        // that fixes it: serve what still resolves, with the error beside
        // it. A broken PROJECT layer keeps global + customer (so the
        // customer's values and lock badges still show); only a broken
        // customer layer, or none to keep, falls back to global alone.
        Err(e) if layers.customer.is_some() || layers.project.is_some() => {
            let kept = match layers.customer.as_deref() {
                Some(c) if layers.project.is_some() => {
                    rupu_config::resolve(rupu_config::LayerPaths::new(Some(&global), Some(c), None))
                        .ok()
                }
                _ => None,
            };
            let fallback = match kept {
                Some(r) => r,
                None => rupu_config::resolve(rupu_config::LayerPaths::global_only(&global))
                    .map_err(|e| ApiError::internal(e.to_string()))?,
            };
            (fallback, Some(e.to_string()))
        }
        Err(e) => return Err(ApiError::internal(e.to_string())),
    };
    let read = |p: Option<&Path>| p.and_then(|p| std::fs::read_to_string(p).ok());
    let customer = layers
        .customer_slug
        .as_deref()
        .map(|slug| CustomerLookup::new(store.clone()).customer_ref(slug));
    Ok(Json(ConfigView {
        effective: serde_json::to_value(&resolved.config).unwrap_or(serde_json::Value::Null),
        provenance: resolved.provenance,
        raw_global: std::fs::read_to_string(&global).unwrap_or_default(),
        raw_project: read(layers.project.as_deref()),
        raw_customer: read(layers.customer.as_deref()),
        customer,
        customer_lock: resolved.customer_lock,
        layer_error,
        cp: serde_json::to_value(&resolved.config.cp).unwrap_or(serde_json::Value::Null),
        status: RuntimeStatus {
            bind: s.bind.clone(),
            token_set: s.token_set,
            restart_required_keys: vec!["bind".into(), "token".into()],
        },
    }))
}

#[derive(Deserialize)]
struct ConfigWriteBody {
    raw: Option<String>,
    patch: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct PolicyBody {
    lock: Vec<String>,
}

/// Writes require an installed `RunLauncher` — the same "is this a `cp
/// serve` deployment" marker every other write-path gate in this crate uses
/// (see `api/hosts.rs`'s host-add gate).
pub(crate) fn require_writable(s: &AppState) -> ApiResult<()> {
    require_writable_to(s, "editing config")
}

/// [`require_writable`] for another kind of write: the 501 reads
/// "`<action>` requires `rupu cp serve`".
pub(crate) fn require_writable_to(s: &AppState, action: &str) -> ApiResult<()> {
    s.launcher
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| ApiError::not_available(format!("{action} requires `rupu cp serve`")))
}

/// Materialize the write body into candidate TOML text (form patch merged
/// onto `existing`, or the raw text verbatim) and validate it against the
/// typed schema. Does not touch disk.
fn candidate_toml(body: &ConfigWriteBody, existing: &str) -> ApiResult<String> {
    let cand = match (&body.raw, &body.patch) {
        (Some(raw), _) => raw.clone(),
        (None, Some(patch)) => {
            apply_form_patch(existing, patch).map_err(|e| ApiError::bad_request(e.to_string()))?
        }
        (None, None) => return Err(ApiError::bad_request("body needs `raw` or `patch`")),
    };
    validate_toml(&cand).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(cand)
}

/// Run `write_atomic` on a blocking worker thread. `write_atomic` takes an
/// `fs2` exclusive file lock (`lock_exclusive`, which BLOCKS) — calling it
/// directly from an async handler would stall the Tokio worker it runs on
/// for as long as the lock is held. `spawn_blocking` moves the write onto the
/// blocking thread pool; the handler `.await`s the join and maps a panicked
/// task to `ApiError::internal`.
async fn write_atomic_blocking(path: PathBuf, contents: String) -> ApiResult<()> {
    tokio::task::spawn_blocking(move || write_atomic(&path, &contents))
        .await
        .map_err(|e| ApiError::internal(format!("config write task panicked: {e}")))?
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// `PUT /api/config/global` — persist a global config edit, then reload
/// `AppState.config` so already-running handlers observe the update without
/// a process restart.
async fn put_global(
    State(s): State<AppState>,
    Json(body): Json<ConfigWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let path = s.global_dir.join("config.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cand = candidate_toml(&body, &existing)?;
    write_atomic_blocking(path, cand).await?;
    s.reload_config();
    Ok(Json(
        serde_json::json!({ "ok": true, "restart_required": [] }),
    ))
}

/// `PUT /api/config/customer/:slug` — persist a customer layer
/// (`customers/<slug>/config.toml`). The candidate is validated as a layer
/// ON TOP OF GLOBAL before anything is written: a layer that is valid alone
/// but breaks the merged config is refused, so an invalid layer never
/// reaches disk. Keys the GLOBAL `[policy].lock` enforces are refused, as
/// for a project. The customer's own lock list is written through this
/// endpoint too, with `patch: {"policy.lock": [...]}`.
async fn put_customer(
    State(s): State<AppState>,
    AxPath(slug): AxPath<String>,
    Json(body): Json<ConfigWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let store = CustomerStore::new(s.global_dir.clone());
    store.get(&slug).map_err(customer_api_err)?;
    let path = store.config_path(&slug);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cand = candidate_toml(&body, &existing)?;
    reject_globally_locked_keys(&s, &cand)?;
    validate_customer_layer(&s.global_dir.join("config.toml"), &path, &cand)?;
    write_atomic_blocking(path, cand).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Resolve `candidate` as the customer layer over the global config, via a
/// temp file next to `target` (removed before returning). 400 on failure.
fn validate_customer_layer(global: &Path, target: &Path, candidate: &str) -> ApiResult<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = target.with_file_name(format!(
        "config.toml.candidate-{}-{nanos}",
        std::process::id()
    ));
    std::fs::write(&tmp, candidate)
        .map_err(|e| ApiError::internal(format!("stage customer config candidate: {e}")))?;
    let result = rupu_config::layer_files_locked(rupu_config::LayerPaths::new(
        Some(global),
        Some(&tmp),
        None,
    ));
    let _ = std::fs::remove_file(&tmp);
    result.map(|_| ()).map_err(|e| {
        ApiError::bad_request(
            e.to_string()
                .replace(&tmp.display().to_string(), "the candidate customer config"),
        )
    })
}

/// `PUT /api/config/project/:id` — persist a project-layer config edit under
/// `<workspace path>/.rupu/config.toml`. Rejects an edit that would set a key
/// enforced by the GLOBAL `[policy].lock` list or by the project's CUSTOMER
/// layer's `[policy].lock` (a project layer can never
/// override a locked key at resolution time anyway; rejecting the write up
/// front gives the operator a clear error instead of a silently-ignored
/// setting).
async fn put_project(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Json(body): Json<ConfigWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let path = project_config_path(&s, &id)?;
    // The shared rule (`rupu_workspace::ConfigPaths`): a project whose
    // `.rupu/` is the global dir has no project layer of its own.
    let ws = load_ws(&s, &id)?;
    let root = ws_root(&ws)?;
    if rupu_workspace::ConfigPaths::without_customer(&s.global_dir, Some(&root))
        .project
        .is_none()
    {
        return Err(ApiError::bad_request(format!(
            "this project's config is the global config ({}); edit it as the global config",
            path.display()
        )));
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cand = candidate_toml(&body, &existing)?;
    reject_locked_project_keys(&s, &ws, &cand)?;
    write_atomic_blocking(path, cand).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `PUT /api/config/policy` — set the GLOBAL `[policy].lock` enforced-key
/// list. Always operates on the global layer: locks are only ever read from
/// there (see `rupu_config::resolve`'s doc comment).
async fn put_policy(
    State(s): State<AppState>,
    Json(body): Json<PolicyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_writable(&s)?;
    let path = s.global_dir.join("config.toml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let patch = serde_json::json!({ "policy.lock": body.lock });
    let cand =
        apply_form_patch(&existing, &patch).map_err(|e| ApiError::bad_request(e.to_string()))?;
    validate_toml(&cand).map_err(|e| ApiError::bad_request(e.to_string()))?;
    write_atomic_blocking(path, cand).await?;
    s.reload_config();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Validate a user-controlled workspace id (from the `:id` path segment or
/// `?project=` query param) BEFORE it is ever used to build a filesystem
/// path. `WorkspaceStore::load` joins the id verbatim as
/// `<global_dir>/workspaces/<id>.toml` (see `rupu_workspace::store`'s
/// `record_path`), so an unvalidated id is a straightforward path-traversal
/// vector (`../foo`, `..`, an absolute path, a percent-decoded `..%2Ffoo`
/// that axum's extractors already decoded to a literal `/` by the time it
/// reaches us, embedded NULs, etc.) — any of these could make `load` read (or
/// a future write path persist) a `.toml` outside `<global_dir>/workspaces`.
///
/// Real workspace ids are ULID-like tokens (see `api/findings.rs` /
/// `api/coverage.rs` usage), so a conservative allowlist — non-empty ASCII
/// alphanumerics plus `-`/`_` only — is sufficient and rejects every
/// traversal shape above without needing to special-case `..` or separators.
fn validate_ws_id(id: &str) -> ApiResult<()> {
    rupu_workspace::validate_ws_id(id)
        .map_err(|_| ApiError::bad_request(format!("invalid project id `{id}`")))
}

/// Resolve a project's `.rupu/config.toml` path from its workspace id and
/// confine it under the workspace's own recorded root.
///
/// The workspace's `path` field is documented as "canonical absolute path"
/// (set via `Path::canonicalize` at workspace-registration time), but this
/// loads it back off disk as plain TOML — a corrupted or hand-edited record
/// could point anywhere. Canonicalizing it here and checking the joined
/// `.rupu/config.toml` still starts with that canonical root is defense in
/// depth against a workspace record steering a config write outside the
/// project tree, mirroring `host::workspace_stage::confine`'s guard for
/// staged workspace dirs. Note this `starts_with` check is ALSO
/// defense-in-depth, not the primary guard: `validate_ws_id` (in
/// [`load_ws`]) is what actually stops a traversal id, since `root_canon` here is always the
/// canonicalized base that `candidate` was just joined onto (the
/// `starts_with` alone would be vacuous against a hostile `id` — the real
/// stop is refusing to load the record for a malformed id at all).
fn project_config_path(s: &AppState, id: &str) -> ApiResult<PathBuf> {
    project_config_path_of(&load_ws(s, id)?)
}

/// [`project_config_path`] for an already-loaded workspace record.
fn project_config_path_of(ws: &rupu_workspace::Workspace) -> ApiResult<PathBuf> {
    let root_canon = ws_root(ws)?;
    let candidate = root_canon.join(".rupu").join("config.toml");
    if !candidate.starts_with(&root_canon) {
        return Err(ApiError::bad_request("config path escapes project root"));
    }
    Ok(candidate)
}

/// The canonicalized root of project `ws`; 400 when its path is invalid.
fn ws_root(ws: &rupu_workspace::Workspace) -> ApiResult<PathBuf> {
    Path::new(&ws.path)
        .canonicalize()
        .map_err(|e| ApiError::bad_request(format!("project path invalid: {e}")))
}

/// The workspace record of project `id`: the id is validated first (the
/// traversal guard), then loaded; 404 when there is no such project.
fn load_ws(s: &AppState, id: &str) -> ApiResult<rupu_workspace::Workspace> {
    validate_ws_id(id)?;
    let store = rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    };
    match store.load(id) {
        Ok(Some(w)) => Ok(w),
        Ok(None) => Err(ApiError::not_found(format!("project {id} not found"))),
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

fn customer_api_err(e: rupu_workspace::CustomerError) -> ApiError {
    super::customers::api_err(e)
}

/// The layers a run in project `ws` would load, through the one resolver
/// the CLI uses (`rupu_workspace::config_paths`): global, the project's
/// customer and its `.rupu/config.toml` (absent when that is the global
/// dir). A dangling customer assignment is an error (500), matching the
/// run, which would fail too.
fn project_layers(
    s: &AppState,
    ws: &rupu_workspace::Workspace,
) -> ApiResult<rupu_workspace::ConfigPaths> {
    let root = ws_root(ws)?;
    rupu_workspace::config_paths(&s.global_dir, Some(&root), Path::new(&ws.path))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// Quote a single key segment using the same canonical dotted-key encoding
/// as `rupu_config::resolve`'s private `dotted()` (and the frontend's
/// `quoteSegment` in `ConfigEditor.tsx`): a segment containing a `.` or a
/// `"` is wrapped in double quotes with embedded `"` escaped as `\"`;
/// anything else is emitted bare. Must stay in lockstep with those two so a
/// pricing model key like `/raid/models/zai-org/GLM-5.2-FP8` produces the
/// exact same dotted string here as it does in the resolved provenance map
/// (and hence in `[policy].lock`, which the CP Settings pricing tab's
/// per-field lock toggle can populate with a dotted/quoted key — see
/// `resolve::dotted`'s doc comment).
fn quote_key_segment(k: &str) -> String {
    if k.contains('.') || k.contains('"') {
        format!("\"{}\"", k.replace('"', "\\\""))
    } else {
        k.to_string()
    }
}

/// Flatten a parsed TOML value to dotted leaf-key paths (tables recurse;
/// scalars and arrays are leaves). Mirrors `rupu_config::resolve`'s private
/// `flatten` + `dotted` helpers, duplicated here because those aren't
/// exported — used only to check candidate project keys against the global
/// lock list, not for the actual layered-merge semantics.
fn flatten_toml_keys(v: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    if let toml::Value::Table(t) = v {
        for (k, vv) in t {
            let qk = quote_key_segment(k);
            let key = if prefix.is_empty() {
                qk
            } else {
                format!("{prefix}.{qk}")
            };
            match vv {
                toml::Value::Table(_) => flatten_toml_keys(vv, &key, out),
                _ => out.push(key),
            }
        }
    }
}

/// The GLOBAL `[policy].lock` list, from `AppState.config` — the
/// global-only resolved snapshot, which is exactly where locks are sourced.
///
/// `unwrap_or_default()` fails OPEN on a poisoned RwLock (empty lock list,
/// so the pre-write check would let the candidate through). That's safe,
/// not a bypass: `rupu_config::resolve` re-enforces the lock list at
/// RESOLUTION time from the global layer regardless of what a lower layer
/// contains, so a key that slips past this check on a poisoned lock is
/// merely an inert value on disk. This check exists only to give the
/// operator an early, clear write-time error; it is not the enforcement
/// boundary.
fn global_lock(s: &AppState) -> Vec<String> {
    s.config
        .read()
        .map(|c| c.policy.lock.clone())
        .unwrap_or_default()
}

/// The dotted leaf keys of a candidate layer.
fn candidate_keys(candidate_toml: &str) -> ApiResult<Vec<String>> {
    let value: toml::Value =
        toml::from_str(candidate_toml).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut keys = Vec::new();
    flatten_toml_keys(&value, "", &mut keys);
    Ok(keys)
}

/// Reject a candidate layer that sets a key enforced by the GLOBAL
/// `[policy].lock` list.
fn reject_globally_locked_keys(s: &AppState, candidate_toml: &str) -> ApiResult<()> {
    let lock = global_lock(s);
    if lock.is_empty() {
        return Ok(());
    }
    for key in &candidate_keys(candidate_toml)? {
        if lock.iter().any(|l| l == key) {
            return Err(ApiError::bad_request(format!(
                "key `{key}` is enforced by global policy"
            )));
        }
    }
    Ok(())
}

/// The keys `slug`'s customer layer locks AND sets — a customer lock on a
/// key its layer does not set locks nothing (`rupu_config::resolve`), so it
/// must not block a project from setting it. Read through `resolve` over
/// global + customer so the rule is the resolver's own. An unreadable layer
/// is an error: fail closed, never "no locks".
fn customer_locked_keys(s: &AppState, slug: &str) -> ApiResult<Vec<String>> {
    let global = s.global_dir.join("config.toml");
    let customer = CustomerStore::new(s.global_dir.clone()).config_path(slug);
    let resolved = rupu_config::resolve(rupu_config::LayerPaths::new(
        Some(&global),
        Some(&customer),
        None,
    ))
    .map_err(|e| {
        ApiError::internal(format!(
            "cannot check customer `{slug}`'s policy locks: {e}; fix its config first"
        ))
    })?;
    Ok(resolved
        .provenance
        .into_iter()
        .filter(|(_, p)| p.locked_by == Some(rupu_config::LockOwner::Customer))
        .map(|(k, _)| k)
        .collect())
}

/// Reject a project-layer candidate that sets a key enforced by the GLOBAL
/// `[policy].lock` list, or by the `[policy].lock` of the project's customer
/// (the error names which layer).
fn reject_locked_project_keys(
    s: &AppState,
    ws: &rupu_workspace::Workspace,
    candidate_toml: &str,
) -> ApiResult<()> {
    reject_globally_locked_keys(s, candidate_toml)?;
    let layers = project_layers(s, ws)?;
    let Some(slug) = layers.customer_slug.as_deref() else {
        return Ok(());
    };
    let locked = customer_locked_keys(s, slug)?;
    if locked.is_empty() {
        return Ok(());
    }
    for key in &candidate_keys(candidate_toml)? {
        if locked.iter().any(|l| l == key) {
            return Err(ApiError::bad_request(format!(
                "key `{key}` is enforced by customer `{slug}` policy"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    impl ConfigQuery {
        fn project(id: &str) -> Self {
            Self {
                project: Some(id.into()),
                customer: None,
            }
        }
        fn customer(slug: &str) -> Self {
            Self {
                project: None,
                customer: Some(slug.into()),
            }
        }
    }

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
    }

    /// Never actually invoked in these tests — `require_writable` only
    /// checks `launcher.is_some()`. Its presence marks the deployment as a
    /// writable `cp serve`, mirroring how `api/workflows.rs`'s tests inject a
    /// `MockLauncher`.
    struct DummyLauncher;

    #[async_trait::async_trait]
    impl crate::launcher::RunLauncher for DummyLauncher {
        async fn launch(
            &self,
            _req: crate::launcher::LaunchRequest,
        ) -> Result<String, crate::launcher::LaunchError> {
            Ok("run_dummy".into())
        }
    }

    fn writable_state(tmp: &tempfile::TempDir) -> AppState {
        test_state(tmp).with_launcher(Some(Arc::new(DummyLauncher)))
    }

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root`.
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &Path) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn get_config_returns_effective_and_masks_token() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::default()))
            .await
            .expect("get_config ok")
            .0;

        assert_eq!(view.effective["default_model"], "opus");
        let prov = view
            .provenance
            .get("default_model")
            .expect("provenance for default_model");
        assert!(matches!(prov.source, rupu_config::KeySource::Global));
        assert!(!prov.locked);

        // Runtime status masks the token to a bool; no launcher/token was
        // installed on this test AppState, so token_set is false.
        assert!(!view.status.token_set);
        assert_eq!(view.status.bind, "127.0.0.1:7878");

        // No secret VALUE anywhere in the serialized view — Config has no
        // token/secret field to begin with, and status only ever carries the
        // bool.
        let rendered = serde_json::to_string(&view).unwrap();
        assert!(!rendered.contains("\"token\":\""), "{rendered}");
    }

    #[tokio::test]
    async fn get_config_with_project_merges_project_layer() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = test_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_proj", proj.path());
        std::fs::create_dir_all(proj.path().join(".rupu")).unwrap();
        std::fs::write(
            proj.path().join(".rupu/config.toml"),
            "default_model = \"sonnet\"\n",
        )
        .unwrap();

        let view = get_config(State(s), Query(ConfigQuery::project("ws_proj")))
            .await
            .expect("get_config ok")
            .0;

        assert_eq!(view.effective["default_model"], "sonnet");
        assert_eq!(
            view.raw_project.as_deref(),
            Some("default_model = \"sonnet\"\n")
        );
        let prov = view.provenance.get("default_model").unwrap();
        assert!(matches!(prov.source, rupu_config::KeySource::Project));
    }

    #[tokio::test]
    async fn get_config_never_loads_the_global_file_as_the_project_layer() {
        // A workspace rooted at the global dir's parent (`$HOME`): its
        // `.rupu/config.toml` IS the global config.
        let root = tempfile::TempDir::new().unwrap();
        let home = root.path().join(".rupu");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = AppState::new(home.clone(), rupu_config::PricingConfig::default());
        std::fs::create_dir_all(home.join("workspaces")).unwrap();
        std::fs::write(
            home.join("workspaces/ws_home.toml"),
            format!(
                "id = \"ws_home\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                root.path().display()
            ),
        )
        .unwrap();
        let customers = rupu_workspace::CustomerStore::new(&home);
        customers
            .create(
                "acme",
                &rupu_workspace::NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        std::fs::write(
            customers.config_path("acme"),
            "default_model = \"acme-model\"\n",
        )
        .unwrap();
        customers
            .assign("acme", rupu_workspace::ProjectRef::Id("ws_home"))
            .unwrap();

        let view = get_config(State(s), Query(ConfigQuery::project("ws_home")))
            .await
            .expect("get_config ok")
            .0;

        assert_eq!(view.effective["default_model"], "acme-model");
        assert_eq!(view.raw_project, None);
    }

    #[tokio::test]
    async fn put_project_refuses_when_the_project_config_is_the_global_config() {
        let root = tempfile::TempDir::new().unwrap();
        let home = root.path().join(".rupu");
        std::fs::create_dir_all(home.join("workspaces")).unwrap();
        let original = "default_model = \"opus\"\n";
        std::fs::write(home.join("config.toml"), original).unwrap();
        std::fs::write(
            home.join("workspaces/ws_home.toml"),
            format!(
                "id = \"ws_home\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                root.path().display()
            ),
        )
        .unwrap();
        let s = AppState::new(home.clone(), rupu_config::PricingConfig::default())
            .with_launcher(Some(Arc::new(DummyLauncher)));

        let body = ConfigWriteBody {
            raw: Some("default_model = \"x\"\n".into()),
            patch: None,
        };
        let err = put_project(State(s), AxPath("ws_home".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            original,
            "the global config must not be written through the project endpoint"
        );
    }

    #[tokio::test]
    async fn get_config_with_project_reports_customer_provenance() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = test_state(&tmp);

        // A project with no project-layer config, assigned to customer
        // `acme` whose layer sets the model.
        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_cust", proj.path());
        std::fs::create_dir_all(proj.path().join(".rupu")).unwrap();
        let customers = rupu_workspace::CustomerStore::new(tmp.path());
        customers
            .create(
                "acme",
                &rupu_workspace::NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        std::fs::write(
            customers.config_path("acme"),
            "default_model = \"acme-model\"\n",
        )
        .unwrap();
        customers
            .assign("acme", rupu_workspace::ProjectRef::Id("ws_cust"))
            .unwrap();

        let view = get_config(State(s), Query(ConfigQuery::project("ws_cust")))
            .await
            .expect("get_config ok")
            .0;

        assert_eq!(view.effective["default_model"], "acme-model");
        let prov = view.provenance.get("default_model").unwrap();
        assert!(matches!(prov.source, rupu_config::KeySource::Customer));
        let rendered = serde_json::to_value(prov).unwrap();
        assert_eq!(rendered["source"], "customer");
    }

    #[tokio::test]
    async fn get_config_with_a_dangling_customer_is_a_500_not_a_global_view() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = test_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_dangling", proj.path());
        let customers = rupu_workspace::CustomerStore::new(tmp.path());
        customers
            .create(
                "acme",
                &rupu_workspace::NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        customers
            .assign("acme", rupu_workspace::ProjectRef::Id("ws_dangling"))
            .unwrap();
        std::fs::remove_dir_all(tmp.path().join("customers/acme")).unwrap();

        let err = match get_config(State(s), Query(ConfigQuery::project("ws_dangling"))).await {
            Ok(_) => panic!("a dangling customer must not serve a global-only view"),
            Err(e) => e,
        };
        assert_eq!(err.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn put_global_persists_and_reloads() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = writable_state(&tmp);

        let body = ConfigWriteBody {
            raw: Some("default_model = \"sonnet\"\n".into()),
            patch: None,
        };
        let resp = put_global(State(s.clone()), Json(body))
            .await
            .expect("put_global ok");
        assert_eq!(resp.0["ok"], true);

        let on_disk = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
        assert!(on_disk.contains("sonnet"), "{on_disk}");

        // Reloaded in place — no restart needed to observe the new value.
        assert_eq!(
            s.config.read().unwrap().default_model.as_deref(),
            Some("sonnet")
        );
    }

    #[tokio::test]
    async fn put_global_rejects_unknown_key() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = writable_state(&tmp);

        let body = ConfigWriteBody {
            raw: Some("bogus_key = 1\n".into()),
            patch: None,
        };
        let err = put_global(State(s), Json(body)).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);

        let on_disk = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
        assert!(
            on_disk.contains("opus"),
            "file must be unchanged: {on_disk}"
        );
    }

    #[tokio::test]
    async fn put_without_launcher_is_501() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = test_state(&tmp); // no launcher installed

        let body = ConfigWriteBody {
            raw: Some("default_model = \"sonnet\"\n".into()),
            patch: None,
        };
        let err = put_global(State(s), Json(body)).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn put_project_rejects_locked_key() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "permission_mode = \"ask\"\n[policy]\nlock = [\"permission_mode\"]\n",
        )
        .unwrap();
        let s = writable_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_locked", proj.path());

        let body = ConfigWriteBody {
            raw: Some("permission_mode = \"bypass\"\n".into()),
            patch: None,
        };
        let err = put_project(State(s), AxPath("ws_locked".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.contains("enforced by global policy"), "{}", err.1);

        // Nothing was written.
        assert!(!proj.path().join(".rupu/config.toml").exists());
    }

    #[tokio::test]
    async fn put_project_rejects_locked_dotted_pricing_key() {
        // The lock entry is a dotted key with a quoted model-id segment —
        // exactly what the CP Settings pricing tab's per-field lock toggle
        // would write (see `rupu_config::resolve::dotted`'s doc comment).
        // `flatten_toml_keys` must quote the candidate's model segment the
        // same way, or this early write-time rejection silently never fires
        // for a locked pricing field (resolution-time enforcement would
        // still hold, but the operator loses the clear write-time error).
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            r#"[pricing.oracle."/raid/models/zai-org/GLM-5.2-FP8"]
input_per_mtok = 1.42
output_per_mtok = 1.42
cached_input_per_mtok = 0.82

[policy]
lock = ["pricing.oracle.\"/raid/models/zai-org/GLM-5.2-FP8\".input_per_mtok"]
"#,
        )
        .unwrap();
        let s = writable_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_locked_pricing", proj.path());

        let body = ConfigWriteBody {
            raw: Some(
                r#"[pricing.oracle."/raid/models/zai-org/GLM-5.2-FP8"]
input_per_mtok = 5.0
"#
                .into(),
            ),
            patch: None,
        };
        let err = put_project(State(s), AxPath("ws_locked_pricing".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.contains("enforced by global policy"), "{}", err.1);
        assert!(!proj.path().join(".rupu/config.toml").exists());
    }

    #[tokio::test]
    async fn put_project_confines_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = writable_state(&tmp);

        // A workspace record whose `path` doesn't resolve to a real,
        // canonicalizable directory — simulating a corrupted/malicious
        // record trying to steer the write somewhere unexpected. The
        // confinement guard in `project_config_path` must reject this before
        // any write is attempted, not 500 or silently write elsewhere.
        register_workspace(&tmp, "ws_missing", &tmp.path().join("does-not-exist"));

        let body = ConfigWriteBody {
            raw: Some("default_model = \"x\"\n".into()),
            patch: None,
        };
        let err = put_project(State(s), AxPath("ws_missing".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    /// Regression for the vacuous `candidate.starts_with(&root_canon)` guard
    /// that used to be the only confinement check in `project_config_path`:
    /// since `candidate` is always built by joining onto `root_canon`, that
    /// check could never fail, so a traversal `:id` (`../evil`, `..`, an
    /// absolute path) was never rejected before reaching
    /// `WorkspaceStore::load`. `validate_ws_id` must reject these ids
    /// up front — this test would fail if that validation were removed,
    /// regardless of what any real workspace record on disk says.
    #[tokio::test]
    async fn project_id_traversal_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();

        // A real record at `<global>/workspaces/evil.toml` that a traversal
        // id `../evil` would resolve to (`store.record_path` joins
        // `format!("{id}.toml")` onto its root) if id validation were
        // skipped. Its presence proves any rejection is due to the id
        // format, not merely a missing file.
        let evil_root = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "evil", evil_root.path());
        std::fs::create_dir_all(evil_root.path().join(".rupu")).unwrap();
        std::fs::write(evil_root.path().join(".rupu/config.toml"), "x = 1\n").unwrap();

        for traversal_id in ["../evil", "..", "/etc/evil", "a/../../evil", "a/b"] {
            // GET ?project=<traversal id>
            let err = match get_config(
                State(test_state(&tmp)),
                Query(ConfigQuery::project(traversal_id)),
            )
            .await
            {
                Err(e) => e,
                Ok(_) => panic!("GET must reject id `{traversal_id}`"),
            };
            assert_eq!(
                err.0,
                axum::http::StatusCode::BAD_REQUEST,
                "id `{traversal_id}`: {}",
                err.1
            );

            // PUT /project/<traversal id>
            let body = ConfigWriteBody {
                raw: Some("default_model = \"x\"\n".into()),
                patch: None,
            };
            let err = match put_project(
                State(writable_state(&tmp)),
                AxPath(traversal_id.into()),
                Json(body),
            )
            .await
            {
                Err(e) => e,
                Ok(_) => panic!("PUT must reject id `{traversal_id}`"),
            };
            assert_eq!(
                err.0,
                axum::http::StatusCode::BAD_REQUEST,
                "id `{traversal_id}`: {}",
                err.1
            );
        }

        // Nothing was ever written to the escaped/legitimate-looking target.
        assert_eq!(
            std::fs::read_to_string(evil_root.path().join(".rupu/config.toml")).unwrap(),
            "x = 1\n",
            "traversal must not reach the file a `../evil`-style id resolves to"
        );
    }

    #[tokio::test]
    async fn put_project_unknown_id_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = writable_state(&tmp);

        let body = ConfigWriteBody {
            raw: Some("default_model = \"x\"\n".into()),
            patch: None,
        };
        let err = put_project(State(s), AxPath("ws_nope".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_policy_sets_global_lock_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let s = writable_state(&tmp);

        let body = PolicyBody {
            lock: vec!["permission_mode".to_string()],
        };
        let resp = put_policy(State(s.clone()), Json(body))
            .await
            .expect("put_policy ok");
        assert_eq!(resp.0["ok"], true);
        assert_eq!(
            s.config.read().unwrap().policy.lock,
            vec!["permission_mode".to_string()]
        );
    }

    // ── Customer config layer ──────────────────────────────────────────────

    fn make_customer(tmp: &tempfile::TempDir, slug: &str, layer: Option<&str>) {
        let customers = rupu_workspace::CustomerStore::new(tmp.path());
        customers
            .create(
                slug,
                &rupu_workspace::NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        if let Some(layer) = layer {
            std::fs::write(customers.config_path(slug), layer).unwrap();
        }
    }

    fn customer_put(raw: &str) -> ConfigWriteBody {
        ConfigWriteBody {
            raw: Some(raw.into()),
            patch: None,
        }
    }

    #[tokio::test]
    async fn get_config_for_a_customer_layers_it_over_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        make_customer(&tmp, "acme", Some("default_model = \"acme-model\"\n"));
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::customer("acme")))
            .await
            .expect("get_config ok")
            .0;
        assert_eq!(view.effective["default_model"], "acme-model");
        assert_eq!(
            view.raw_customer.as_deref(),
            Some("default_model = \"acme-model\"\n")
        );
        assert_eq!(view.customer.as_ref().unwrap().slug, "acme");
        assert_eq!(view.raw_project, None);
        assert_eq!(view.layer_error, None);
        let prov = serde_json::to_value(view.provenance.get("default_model").unwrap()).unwrap();
        assert_eq!(prov["source"], "customer");
    }

    #[tokio::test]
    async fn get_config_project_and_customer_together_is_a_400() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_customer(&tmp, "acme", None);
        let s = test_state(&tmp);
        let err = match get_config(
            State(s),
            Query(ConfigQuery {
                project: Some("ws_x".into()),
                customer: Some("acme".into()),
            }),
        )
        .await
        {
            Ok(_) => panic!("both selectors must be refused"),
            Err(e) => e,
        };
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_config_unknown_customer_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = match get_config(State(s), Query(ConfigQuery::customer("nope"))).await {
            Ok(_) => panic!("unknown customer"),
            Err(e) => e,
        };
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_config_malformed_customer_layer_is_200_with_layer_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        make_customer(&tmp, "acme", Some("default_model = = broken\n"));
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::customer("acme")))
            .await
            .expect("the editor must still open")
            .0;
        assert!(view.layer_error.is_some());
        // Effective is global only; the broken text is still served to fix.
        assert_eq!(view.effective["default_model"], "opus");
        assert_eq!(
            view.raw_customer.as_deref(),
            Some("default_model = = broken\n")
        );
    }

    #[tokio::test]
    async fn get_config_with_project_fills_the_projects_customer() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        make_customer(
            &tmp,
            "acme",
            Some("default_model = \"acme-model\"\n[policy]\nlock = [\"default_model\"]\n"),
        );
        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_cust2", proj.path());
        rupu_workspace::CustomerStore::new(tmp.path())
            .assign("acme", rupu_workspace::ProjectRef::Id("ws_cust2"))
            .unwrap();
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::project("ws_cust2")))
            .await
            .expect("get_config ok")
            .0;
        assert_eq!(view.customer.as_ref().unwrap().slug, "acme");
        assert_eq!(view.customer_lock, vec!["default_model".to_string()]);
    }

    #[tokio::test]
    async fn put_customer_writes_and_the_next_get_reflects_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        make_customer(&tmp, "acme", None);
        let s = writable_state(&tmp);

        let resp = put_customer(
            State(s.clone()),
            AxPath("acme".into()),
            Json(customer_put("default_model = \"acme-model\"\n")),
        )
        .await
        .expect("put ok");
        assert_eq!(resp.0["ok"], true);

        let view = get_config(State(s), Query(ConfigQuery::customer("acme")))
            .await
            .unwrap()
            .0;
        assert_eq!(view.effective["default_model"], "acme-model");
    }

    #[tokio::test]
    async fn put_customer_invalid_toml_is_400_and_the_file_is_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_customer(&tmp, "acme", Some("default_model = \"keep\"\n"));
        let s = writable_state(&tmp);
        let path = rupu_workspace::CustomerStore::new(tmp.path()).config_path("acme");

        for bad in ["bogus_key = 1\n", "default_model = = x\n"] {
            let err = put_customer(
                State(s.clone()),
                AxPath("acme".into()),
                Json(customer_put(bad)),
            )
            .await
            .unwrap_err();
            assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "default_model = \"keep\"\n"
            );
        }
        // No candidate temp file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("candidate"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[tokio::test]
    async fn put_customer_rejects_a_layer_that_is_invalid_on_top_of_global() {
        // Each file is valid alone; layered, the provider loses its
        // required default_model, so the layer must be refused.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[providers.corp]\nkind = \"openai-compatible\"\nbase_url = \"http://corp.example\"\ndefault_model = \"m\"\n",
        )
        .unwrap();
        make_customer(&tmp, "acme", Some("default_model = \"keep\"\n"));
        let s = writable_state(&tmp);
        let path = rupu_workspace::CustomerStore::new(tmp.path()).config_path("acme");

        let err = put_customer(
            State(s),
            AxPath("acme".into()),
            Json(customer_put("[providers.corp]\ndefault_model = \"\"\n")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "default_model = \"keep\"\n"
        );
    }

    #[tokio::test]
    async fn put_customer_unknown_slug_is_404_and_unwritable_is_501() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_customer(&tmp, "acme", None);
        let err = put_customer(
            State(writable_state(&tmp)),
            AxPath("nope".into()),
            Json(customer_put("default_model = \"x\"\n")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);

        let err = put_customer(
            State(test_state(&tmp)),
            AxPath("acme".into()),
            Json(customer_put("default_model = \"x\"\n")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn put_customer_patch_sets_the_customer_lock_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_customer(&tmp, "acme", Some("default_provider = \"anthropic\"\n"));
        let s = writable_state(&tmp);

        let _ = put_customer(
            State(s.clone()),
            AxPath("acme".into()),
            Json(ConfigWriteBody {
                raw: None,
                patch: Some(serde_json::json!({ "policy.lock": ["default_provider"] })),
            }),
        )
        .await
        .expect("put ok");

        let view = get_config(State(s), Query(ConfigQuery::customer("acme")))
            .await
            .unwrap()
            .0;
        assert_eq!(view.customer_lock, vec!["default_provider".to_string()]);
        let prov = serde_json::to_value(view.provenance.get("default_provider").unwrap()).unwrap();
        assert_eq!(prov["locked_by"], "customer");
    }

    #[tokio::test]
    async fn put_customer_rejects_a_globally_locked_key() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "permission_mode = \"ask\"\n[policy]\nlock = [\"permission_mode\"]\n",
        )
        .unwrap();
        make_customer(&tmp, "acme", None);
        let err = put_customer(
            State(writable_state(&tmp)),
            AxPath("acme".into()),
            Json(customer_put("permission_mode = \"bypass\"\n")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.contains("enforced by global policy"), "{}", err.1);
    }

    fn customer_with_project(tmp: &tempfile::TempDir, layer: &str) -> tempfile::TempDir {
        make_customer(tmp, "acme", Some(layer));
        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(tmp, "ws_acme", proj.path());
        rupu_workspace::CustomerStore::new(tmp.path())
            .assign("acme", rupu_workspace::ProjectRef::Id("ws_acme"))
            .unwrap();
        proj
    }

    #[tokio::test]
    async fn put_project_rejects_a_key_its_customer_locks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = customer_with_project(
            &tmp,
            "permission_mode = \"ask\"\n[policy]\nlock = [\"permission_mode\"]\n",
        );
        let s = writable_state(&tmp);

        let err = put_project(
            State(s),
            AxPath("ws_acme".into()),
            Json(customer_put("permission_mode = \"bypass\"\n")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.contains("customer"), "{}", err.1);
        assert!(err.1.contains("acme"), "{}", err.1);
        assert!(!proj.path().join(".rupu/config.toml").exists());
    }

    #[tokio::test]
    async fn put_project_allows_a_key_its_customer_lists_but_does_not_set() {
        // A lock on a key the customer layer does not set locks nothing
        // (`resolve` says so too), so the project may set it.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = customer_with_project(&tmp, "[policy]\nlock = [\"permission_mode\"]\n");
        let s = writable_state(&tmp);

        let _ = put_project(
            State(s),
            AxPath("ws_acme".into()),
            Json(customer_put("permission_mode = \"bypass\"\n")),
        )
        .await
        .expect("an inert lock must not block the write");
        assert!(proj.path().join(".rupu/config.toml").exists());
    }

    #[tokio::test]
    async fn put_project_with_an_unreadable_customer_layer_fails_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = customer_with_project(&tmp, "default_model = = broken\n");
        let s = writable_state(&tmp);

        let err = put_project(
            State(s),
            AxPath("ws_acme".into()),
            Json(customer_put("permission_mode = \"bypass\"\n")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!proj.path().join(".rupu/config.toml").exists());
    }

    #[tokio::test]
    async fn get_config_malformed_project_layer_is_200_with_layer_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_bad", proj.path());
        std::fs::create_dir_all(proj.path().join(".rupu")).unwrap();
        std::fs::write(
            proj.path().join(".rupu/config.toml"),
            "default_model = = x\n",
        )
        .unwrap();
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::project("ws_bad")))
            .await
            .expect("the editor must still open")
            .0;
        assert!(view.layer_error.is_some());
        assert_eq!(view.effective["default_model"], "opus");
        assert_eq!(view.raw_project.as_deref(), Some("default_model = = x\n"));
    }

    #[tokio::test]
    async fn get_config_broken_project_layer_keeps_the_customer_layer() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let proj = customer_with_project(
            &tmp,
            "default_model = \"acme-model\"\n[policy]\nlock = [\"default_model\"]\n",
        );
        std::fs::create_dir_all(proj.path().join(".rupu")).unwrap();
        std::fs::write(
            proj.path().join(".rupu/config.toml"),
            "default_model = = x\n",
        )
        .unwrap();
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::project("ws_acme")))
            .await
            .expect("the editor must still open")
            .0;
        assert!(view.layer_error.is_some());
        assert_eq!(view.effective["default_model"], "acme-model");
        let prov = serde_json::to_value(view.provenance.get("default_model").unwrap()).unwrap();
        assert_eq!(prov["source"], "customer");
        assert_eq!(prov["locked_by"], "customer");
        assert_eq!(view.customer_lock, vec!["default_model".to_string()]);
        assert_eq!(view.customer.as_ref().unwrap().slug, "acme");
        assert_eq!(view.raw_project.as_deref(), Some("default_model = = x\n"));
    }

    #[tokio::test]
    async fn get_config_broken_customer_layer_with_a_project_falls_back_to_global() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "default_model = \"opus\"\n").unwrap();
        let proj = customer_with_project(&tmp, "default_model = = broken\n");
        std::fs::create_dir_all(proj.path().join(".rupu")).unwrap();
        std::fs::write(
            proj.path().join(".rupu/config.toml"),
            "default_model = \"proj\"\n",
        )
        .unwrap();
        let s = test_state(&tmp);

        let view = get_config(State(s), Query(ConfigQuery::project("ws_acme")))
            .await
            .expect("the editor must still open")
            .0;
        assert!(view.layer_error.is_some());
        assert_eq!(view.effective["default_model"], "opus");
        assert!(view.customer_lock.is_empty());
    }

    // ── Task 7: end-to-end — round-trip + lock enforcement ─────────────────
    //
    // These two tests exercise the FULL write→reload→resolve chain (not just
    // one handler in isolation, as the tests above do): an edit made through
    // the API must (a) persist to disk, (b) be visible through a follow-up
    // `GET` without a process restart, AND (c) be visible to a fresh,
    // independent `rupu_config::resolve()` call reading the same file off
    // disk — proving the write actually took effect for any consumer, not
    // just the one `AppState` handle that made it.
    //
    // Handlers here (`get_config`/`put_global`/`put_project`) are private to
    // this module, so — per the harness note above this `mod tests` block —
    // these live in-module rather than in `tests/config_e2e.rs`, reusing
    // `test_state`/`writable_state`/`register_workspace` exactly as the unit
    // tests above do.

    /// Full round trip for a global edit: raw PUT → 200 → GET reflects the
    /// reload → a fresh on-disk `resolve()` (independent of the `AppState`
    /// that made the write) also sees the new value. Then a form-patch PUT
    /// on top of a hand-commented file: the patched key persists AND the
    /// pre-existing comment survives (`toml_edit`'s comment/layout
    /// preservation, exercised end-to-end through the real handler + real
    /// file on disk, not just the `config_write` unit test).
    #[tokio::test]
    async fn edit_persists_reloads_and_takes_effect() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global_path = tmp.path().join("config.toml");
        std::fs::write(&global_path, "default_model = \"opus\"\n").unwrap();
        let s = writable_state(&tmp);

        // ── 1. Raw PUT: default_model opus -> sonnet ────────────────────────
        let body = ConfigWriteBody {
            raw: Some("default_model = \"sonnet\"\n".into()),
            patch: None,
        };
        let resp = put_global(State(s.clone()), Json(body))
            .await
            .expect("put_global ok");
        assert_eq!(resp.0["ok"], true);

        // ── 2. GET reflects the reload without a restart ────────────────────
        let view = get_config(State(s.clone()), Query(ConfigQuery::default()))
            .await
            .expect("get_config ok")
            .0;
        assert_eq!(view.effective["default_model"], "sonnet");

        // ── 3. A fresh, independent on-disk resolve() also sees it ──────────
        // This is the "took effect" assertion: it does not touch `s` at all,
        // it just re-reads the file the handler wrote, the same way any
        // other process (a `rupu` CLI invocation, a fresh `cp serve`) would.
        let resolved = rupu_config::resolve(rupu_config::LayerPaths::global_only(&global_path))
            .expect("on-disk resolve ok");
        assert_eq!(resolved.config.default_model.as_deref(), Some("sonnet"));

        // ── 4. Hand-add a comment, then form-patch a DIFFERENT key ──────────
        // Simulates an operator who has hand-edited the file with their own
        // comment; the settings UI's form editor must not clobber it.
        let commented = format!(
            "# operator note: do not remove\n{}",
            std::fs::read_to_string(&global_path).unwrap()
        );
        std::fs::write(&global_path, &commented).unwrap();

        let patch_body = ConfigWriteBody {
            raw: None,
            patch: Some(serde_json::json!({ "log_level": "debug" })),
        };
        let resp2 = put_global(State(s.clone()), Json(patch_body))
            .await
            .expect("put_global patch ok");
        assert_eq!(resp2.0["ok"], true);

        let on_disk = std::fs::read_to_string(&global_path).unwrap();
        assert!(
            on_disk.contains("# operator note: do not remove"),
            "comment must survive a form-patch write: {on_disk}"
        );
        assert!(on_disk.contains("sonnet"), "{on_disk}");
        assert!(on_disk.contains("log_level"), "{on_disk}");

        let view2 = get_config(State(s), Query(ConfigQuery::default()))
            .await
            .expect("get_config ok")
            .0;
        assert_eq!(view2.effective["log_level"], "debug");
        assert_eq!(view2.effective["default_model"], "sonnet");
    }

    /// A key in the GLOBAL `[policy].lock` list wins over a project's
    /// override AT RESOLUTION — both via a direct `rupu_config::resolve()`
    /// call over the two on-disk files, and via the read API (`GET
    /// /api/config?project=`). Attempting to persist the override through
    /// the WRITE API (`PUT /api/config/project/:id`) is rejected up front
    /// with a message naming the enforcing policy, and nothing is written.
    #[tokio::test]
    async fn global_lock_overrides_project_at_resolution() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global_path = tmp.path().join("config.toml");
        std::fs::write(
            &global_path,
            "permission_mode = \"ask\"\n[policy]\nlock = [\"permission_mode\"]\n",
        )
        .unwrap();
        let s = writable_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        register_workspace(&tmp, "ws_e2e_lock", proj.path());
        std::fs::create_dir_all(proj.path().join(".rupu")).unwrap();
        let project_path = proj.path().join(".rupu/config.toml");
        std::fs::write(&project_path, "permission_mode = \"bypass\"\n").unwrap();

        // ── 1. Direct resolve(): locked global wins, provenance says so ─────
        let resolved = rupu_config::resolve(rupu_config::LayerPaths::new(
            Some(&global_path),
            None,
            Some(&project_path),
        ))
        .expect("resolve ok");
        assert_eq!(resolved.config.permission_mode.as_deref(), Some("ask"));
        let prov = resolved
            .provenance
            .get("permission_mode")
            .expect("provenance for permission_mode");
        assert!(matches!(prov.source, rupu_config::KeySource::Global));
        assert!(prov.locked);

        // ── 2. Same enforcement visible through the read API ────────────────
        let view = get_config(State(s.clone()), Query(ConfigQuery::project("ws_e2e_lock")))
            .await
            .expect("get_config ok")
            .0;
        assert_eq!(view.effective["permission_mode"], "ask");
        let view_prov = view.provenance.get("permission_mode").unwrap();
        assert!(matches!(view_prov.source, rupu_config::KeySource::Global));
        assert!(view_prov.locked);

        // ── 3. The write API refuses to persist the (moot) override ─────────
        let body = ConfigWriteBody {
            raw: Some("permission_mode = \"bypass\"\n".into()),
            patch: None,
        };
        let err = put_project(State(s), AxPath("ws_e2e_lock".into()), Json(body))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.contains("enforced by global policy"), "{}", err.1);

        // Nothing was written — the on-disk project file is unchanged.
        assert_eq!(
            std::fs::read_to_string(&project_path).unwrap(),
            "permission_mode = \"bypass\"\n"
        );
    }
}
