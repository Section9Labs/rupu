//! Versioned selected-layer editing. Config sidecar locks cover comparison,
//! patching, policy validation and replacement. Registry/assignment evidence is
//! checked twice; those stores do not participate in the config transaction.
use super::*;
use crate::config_write::{lock_config, replace_locked};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;

const MAX_LAYER_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Project,
    Customer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Scope {
    kind: Kind,
    id: String,
    host_id: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Owner {
    scope: Scope,
    target: PathBuf,
    global: PathBuf,
    customer: Option<PathBuf>,
    project_path: Option<PathBuf>,
    customer_slug: Option<String>,
}
impl Owner {
    fn resolve(s: &AppState, kind: Kind, id: &str) -> ApiResult<Self> {
        let global = s.global_dir.join("config.toml");
        let scope = Scope {
            kind,
            id: id.into(),
            host_id: "local",
        };
        match kind {
            Kind::Customer => {
                let store = CustomerStore::new(s.global_dir.clone());
                store.get(id).map_err(customer_api_err)?;
                Ok(Self {
                    scope,
                    target: store.config_path(id),
                    global,
                    customer: None,
                    project_path: None,
                    customer_slug: Some(id.into()),
                })
            }
            Kind::Project => {
                let ws = load_ws(s, id)?;
                if ws.id != id {
                    return Err(ApiError::conflict("project identity changed"));
                }
                let root = ws_root(&ws)?;
                let layers = project_layers(s, &ws)?;
                let resolved_target = layers.project.ok_or_else(|| {
                    ApiError::bad_request(
                        "this project's config is the global config; edit it as the global config",
                    )
                })?;
                let target = project_config_path(s, id)?;
                if target != resolved_target {
                    return Err(ApiError::conflict(
                        "project path changed while resolving configuration",
                    ));
                }
                // Reject a static symlink redirect out of the named project.
                // This is not a hostile-filesystem transaction guarantee.
                if let Some(parent) = target.parent().filter(|p| p.exists()) {
                    if !parent.canonicalize().map_err(io_error)?.starts_with(&root) {
                        return Err(ApiError::bad_request("config path escapes project root"));
                    }
                }
                Ok(Self {
                    scope,
                    target,
                    global,
                    customer: layers.customer,
                    project_path: Some(root),
                    customer_slug: layers.customer_slug,
                })
            }
        }
    }
    fn paths(&self) -> Vec<PathBuf> {
        // Every scoped writer takes global, then customer, then project.
        let mut paths = vec![self.global.clone()];
        if let Some(customer) = &self.customer {
            paths.push(customer.clone());
        }
        paths.push(self.target.clone());
        paths.dedup();
        paths
    }
}
fn io_error(e: std::io::Error) -> ApiError {
    ApiError::internal(format!("cannot read configuration layer: {e}"))
}
fn read_layer(path: &Path) -> ApiResult<Option<String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error(e)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_LAYER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_LAYER_BYTES {
        return Err(ApiError::bad_request("configuration layer exceeds 1 MiB"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| ApiError::bad_request("configuration layer is not UTF-8"))
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Capture {
    owner: Owner,
    global: Option<String>,
    customer: Option<String>,
    selected: Option<String>,
}
impl Capture {
    fn read(s: &AppState, kind: Kind, id: &str) -> ApiResult<Self> {
        let owner = Owner::resolve(s, kind, id)?;
        Ok(Self {
            global: read_layer(&owner.global)?,
            customer: owner
                .customer
                .as_deref()
                .map(read_layer)
                .transpose()?
                .flatten(),
            selected: read_layer(&owner.target)?,
            owner,
        })
    }
    fn revision(&self) -> String {
        let bytes = serde_json::to_vec(&json!({"contract":"config.scoped_revision.v1",
            "scope":self.owner.scope,"target":self.owner.target,"project_path":self.owner.project_path,
            "customer_slug":self.owner.customer_slug,"customer_path":self.owner.customer,
            "global":self.global,"customer":self.customer,"selected":self.selected})).expect("JSON values serialize");
        format!("sha256:{:x}", Sha256::digest(bytes))
    }
    fn response(&self) -> Value {
        json!({"scope":self.owner.scope,"revision":self.revision(),"raw":self.selected.as_deref().unwrap_or(""),
            "exists":self.selected.is_some(),"customer_slug":self.owner.customer_slug,
            "revision_scope":"selected_and_inherited_config_with_resolved_owner",
            "registry_consistency":"rechecked_not_locked"})
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LocalQuery {
    host: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GuardedWrite {
    expected_revision: String,
    raw: Option<String>,
    patch: Option<Value>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/config/scoped/project/:id",
            get(get_project).put(put_project_guarded),
        )
        .route(
            "/api/config/scoped/customer/:slug",
            get(get_customer).put(put_customer_guarded),
        )
}
async fn read(s: AppState, kind: Kind, id: String, query: LocalQuery) -> ApiResult<Json<Value>> {
    super::super::fleet::require_local(query.host.as_deref(), "scoped configuration")?;
    tokio::task::spawn_blocking(move || {
        // GET never creates config/lock files or missing directories. Compare
        // consecutive bounded captures instead of implying an atomic snapshot.
        let first = Capture::read(&s, kind, &id)?;
        let second = Capture::read(&s, kind, &id)?;
        if first != second {
            return Err(ApiError::conflict(
                "configuration changed while reading; reload",
            ));
        }
        Ok(Json(second.response()))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}
async fn get_project(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Query(q): Query<LocalQuery>,
) -> ApiResult<Json<Value>> {
    read(s, Kind::Project, id, q).await
}
async fn get_customer(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Query(q): Query<LocalQuery>,
) -> ApiResult<Json<Value>> {
    read(s, Kind::Customer, id, q).await
}
async fn write(
    s: AppState,
    kind: Kind,
    id: String,
    q: LocalQuery,
    body: GuardedWrite,
) -> ApiResult<Json<Value>> {
    super::super::fleet::require_local(q.host.as_deref(), "scoped configuration")?;
    require_writable(&s)?;
    if body.raw.is_some() == body.patch.is_some() {
        return Err(ApiError::bad_request("provide exactly one of raw or patch"));
    }
    if body.expected_revision.len() != 71
        || !body.expected_revision.starts_with("sha256:")
        || !body.expected_revision[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::bad_request(
            "expected_revision must be the revision from this scope's GET",
        ));
    }
    transact_async(
        s,
        kind,
        id,
        ConfigWriteBody {
            raw: body.raw,
            patch: body.patch,
        },
        Some(body.expected_revision),
    )
    .await
    .map(Json)
}
async fn put_project_guarded(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Query(q): Query<LocalQuery>,
    Json(body): Json<GuardedWrite>,
) -> ApiResult<Json<Value>> {
    write(s, Kind::Project, id, q, body).await
}
async fn put_customer_guarded(
    State(s): State<AppState>,
    AxPath(id): AxPath<String>,
    Query(q): Query<LocalQuery>,
    Json(body): Json<GuardedWrite>,
) -> ApiResult<Json<Value>> {
    write(s, Kind::Customer, id, q, body).await
}
pub(super) async fn legacy_write(
    s: AppState,
    kind: Kind,
    id: String,
    body: ConfigWriteBody,
) -> ApiResult<Value> {
    transact_async(s, kind, id, body, None).await
}
async fn transact_async(
    s: AppState,
    kind: Kind,
    id: String,
    body: ConfigWriteBody,
    expected: Option<String>,
) -> ApiResult<Value> {
    tokio::task::spawn_blocking(move || transact(&s, kind, &id, &body, expected.as_deref()))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}
fn transact(
    s: &AppState,
    kind: Kind,
    id: &str,
    body: &ConfigWriteBody,
    expected: Option<&str>,
) -> ApiResult<Value> {
    let owner = Owner::resolve(s, kind, id)?;
    let _locks: Vec<_> = owner
        .paths()
        .iter()
        .map(|path| lock_config(path).map_err(|e| ApiError::internal(e.to_string())))
        .collect::<ApiResult<_>>()?;
    let before = Capture::read(s, kind, id)?;
    if before.owner != owner {
        return Err(ApiError::conflict("configuration owner changed; reload"));
    }
    if expected.is_some_and(|revision| revision != before.revision()) {
        return Err(ApiError::conflict(
            "configuration or its inherited dependencies changed; reload before saving",
        ));
    }
    let candidate = candidate_toml(body, before.selected.as_deref().unwrap_or(""))?;
    if candidate.len() as u64 > MAX_LAYER_BYTES {
        return Err(ApiError::bad_request("configuration layer exceeds 1 MiB"));
    }
    match kind {
        Kind::Project => reject_locked_project_keys(s, &load_ws(s, id)?, &candidate)?,
        Kind::Customer => reject_globally_locked_keys(s, &candidate)?,
    }
    validate_candidate(&before.owner, &candidate)?;
    // Workspace/customer metadata does not take these config locks. Refuse any
    // observed ownership/dependency change, without claiming registry CAS.
    if Capture::read(s, kind, id)? != before {
        return Err(ApiError::conflict(
            "configuration owner or dependencies changed during validation; reload",
        ));
    }
    replace_locked(&owner.target, &candidate).map_err(|e| ApiError::internal(e.to_string()))?;
    // Canonical receipt of the exact bytes committed, not a later unlocked GET.
    let mut saved = before;
    saved.selected = Some(candidate);
    let mut response = saved.response();
    response["ok"] = json!(true);
    Ok(response)
}
fn validate_candidate(owner: &Owner, candidate: &str) -> ApiResult<()> {
    let inherited = rupu_config::resolve(rupu_config::LayerPaths::new(
        Some(&owner.global),
        owner.customer.as_deref(),
        None,
    ))
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let global =
        rupu_config::layer_files_locked(rupu_config::LayerPaths::global_only(&owner.global))
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
    for key in candidate_keys(candidate)? {
        if global.policy.lock.contains(&key) {
            return Err(ApiError::bad_request(format!(
                "key `{key}` is enforced by global policy"
            )));
        }
        if inherited
            .provenance
            .get(&key)
            .is_some_and(|p| p.locked_by == Some(rupu_config::LockOwner::Customer))
        {
            return Err(ApiError::bad_request(format!(
                "key `{key}` is enforced by customer policy"
            )));
        }
    }
    if owner.scope.kind == Kind::Customer {
        return validate_customer_layer(&owner.global, &owner.target, candidate);
    }
    let tmp = owner
        .target
        .with_file_name(format!("config.toml.candidate-{}", ulid::Ulid::new()));
    struct Remove(PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _remove = Remove(tmp.clone());
    std::fs::write(&tmp, candidate).map_err(io_error)?;
    rupu_config::layer_files_locked(rupu_config::LayerPaths::new(
        Some(&owner.global),
        owner.customer.as_deref(),
        Some(&tmp),
    ))
    .map(|_| ())
    .map_err(|e| {
        ApiError::bad_request(
            e.to_string()
                .replace(&tmp.display().to_string(), "the candidate project config"),
        )
    })
}

#[cfg(test)]
mod tests;
