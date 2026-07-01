//! `rupu-cp` config read/write API (CP Settings).
//!
//! - `GET /api/config` (+ `?project=<ws_id>`) returns the effective resolved
//!   config, per-key provenance, and the raw global/project TOML text so the
//!   settings UI can offer both a form view and a raw editor.
//! - `PUT /api/config/global` / `PUT /api/config/project/:id` persist an edit
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

use crate::{
    config_write::{apply_form_patch, validate_toml, write_atomic},
    error::{ApiError, ApiResult},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/config", get(get_config))
        .route("/api/config/global", put(put_global))
        .route("/api/config/project/:id", put(put_project))
        .route("/api/config/policy", put(put_policy))
}

#[derive(Deserialize)]
struct ProjectQuery {
    project: Option<String>,
}

#[derive(Serialize)]
struct RuntimeStatus {
    bind: String,
    token_set: bool,
    restart_required_keys: Vec<String>,
}

#[derive(Serialize)]
struct ConfigView {
    effective: serde_json::Value,
    provenance: BTreeMap<String, rupu_config::KeyProvenance>,
    raw_global: String,
    raw_project: Option<String>,
    cp: serde_json::Value,
    status: RuntimeStatus,
}

/// `GET /api/config` (+ `?project=<ws_id>`) — effective config + provenance +
/// raw TOML text for both layers.
async fn get_config(
    State(s): State<AppState>,
    Query(q): Query<ProjectQuery>,
) -> ApiResult<Json<ConfigView>> {
    let global = s.global_dir.join("config.toml");
    let project_path = match &q.project {
        Some(id) => Some(project_config_path(&s, id)?),
        None => None,
    };
    let resolved = rupu_config::resolve(Some(&global), project_path.as_deref(), &BTreeMap::new())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let raw_global = std::fs::read_to_string(&global).unwrap_or_default();
    let raw_project = project_path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok());
    Ok(Json(ConfigView {
        effective: serde_json::to_value(&resolved.config).unwrap_or(serde_json::Value::Null),
        provenance: resolved.provenance,
        raw_global,
        raw_project,
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
fn require_writable(s: &AppState) -> ApiResult<()> {
    s.launcher
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| ApiError::not_available("editing config requires `rupu cp serve`"))
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

/// `PUT /api/config/project/:id` — persist a project-layer config edit under
/// `<workspace path>/.rupu/config.toml`. Rejects an edit that would set a key
/// enforced by the GLOBAL `[policy].lock` list (a project layer can never
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
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cand = candidate_toml(&body, &existing)?;
    reject_locked_project_keys(&s, &cand)?;
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
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!("invalid project id `{id}`")))
    }
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
/// defense-in-depth, not the primary guard: `validate_ws_id` below is what
/// actually stops a traversal id, since `root_canon` here is always the
/// canonicalized base that `candidate` was just joined onto (the
/// `starts_with` alone would be vacuous against a hostile `id` — the real
/// stop is refusing to load the record for a malformed id at all).
fn project_config_path(s: &AppState, id: &str) -> ApiResult<PathBuf> {
    validate_ws_id(id)?;
    let store = rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    };
    let ws = match store.load(id) {
        Ok(Some(w)) => w,
        Ok(None) => return Err(ApiError::not_found(format!("project {id} not found"))),
        Err(e) => return Err(ApiError::internal(e.to_string())),
    };
    let root = Path::new(&ws.path);
    let root_canon = root
        .canonicalize()
        .map_err(|e| ApiError::bad_request(format!("project path invalid: {e}")))?;
    let candidate = root_canon.join(".rupu").join("config.toml");
    if !candidate.starts_with(&root_canon) {
        return Err(ApiError::bad_request("config path escapes project root"));
    }
    Ok(candidate)
}

/// Flatten a parsed TOML value to dotted leaf-key paths (tables recurse;
/// scalars and arrays are leaves). Mirrors `rupu_config::resolve`'s private
/// `flatten` helper, duplicated here because that one isn't exported — used
/// only to check candidate project keys against the global lock list, not
/// for the actual layered-merge semantics.
fn flatten_toml_keys(v: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    if let toml::Value::Table(t) = v {
        for (k, vv) in t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match vv {
                toml::Value::Table(_) => flatten_toml_keys(vv, &key, out),
                _ => out.push(key),
            }
        }
    }
}

/// Reject a project-layer candidate that sets a key enforced by the GLOBAL
/// `[policy].lock` list. Reads the lock list from `AppState.config` — the
/// global-only resolved snapshot (`resolve(global, None, ..)`), which is
/// exactly where locks are sourced from.
fn reject_locked_project_keys(s: &AppState, candidate_toml: &str) -> ApiResult<()> {
    // `unwrap_or_default()` fails OPEN on a poisoned RwLock (empty lock list,
    // so this pre-write check would let the candidate through). That's safe,
    // not a bypass: `rupu_config::resolve` re-enforces the lock list at
    // RESOLUTION time from the global layer regardless of what a project
    // file contains, so a project key that slips past this check on a
    // poisoned lock is merely an inert value on disk — resolution still
    // ignores it in favor of the locked global value. This check exists only
    // to give the operator an early, clear write-time error; it is not the
    // enforcement boundary.
    let lock = s
        .config
        .read()
        .map(|c| c.policy.lock.clone())
        .unwrap_or_default();
    if lock.is_empty() {
        return Ok(());
    }
    let value: toml::Value =
        toml::from_str(candidate_toml).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut keys = Vec::new();
    flatten_toml_keys(&value, "", &mut keys);
    for key in &keys {
        if lock.iter().any(|l| l == key) {
            return Err(ApiError::bad_request(format!(
                "key `{key}` is enforced by global policy"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

        let view = get_config(State(s), Query(ProjectQuery { project: None }))
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

        let view = get_config(
            State(s),
            Query(ProjectQuery {
                project: Some("ws_proj".into()),
            }),
        )
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
                Query(ProjectQuery {
                    project: Some(traversal_id.into()),
                }),
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
        let view = get_config(State(s.clone()), Query(ProjectQuery { project: None }))
            .await
            .expect("get_config ok")
            .0;
        assert_eq!(view.effective["default_model"], "sonnet");

        // ── 3. A fresh, independent on-disk resolve() also sees it ──────────
        // This is the "took effect" assertion: it does not touch `s` at all,
        // it just re-reads the file the handler wrote, the same way any
        // other process (a `rupu` CLI invocation, a fresh `cp serve`) would.
        let resolved = rupu_config::resolve(Some(&global_path), None, &BTreeMap::new())
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

        let view2 = get_config(State(s), Query(ProjectQuery { project: None }))
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
        let resolved =
            rupu_config::resolve(Some(&global_path), Some(&project_path), &BTreeMap::new())
                .expect("resolve ok");
        assert_eq!(resolved.config.permission_mode.as_deref(), Some("ask"));
        let prov = resolved
            .provenance
            .get("permission_mode")
            .expect("provenance for permission_mode");
        assert!(matches!(prov.source, rupu_config::KeySource::Global));
        assert!(prov.locked);

        // ── 2. Same enforcement visible through the read API ────────────────
        let view = get_config(
            State(s.clone()),
            Query(ProjectQuery {
                project: Some("ws_e2e_lock".into()),
            }),
        )
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
