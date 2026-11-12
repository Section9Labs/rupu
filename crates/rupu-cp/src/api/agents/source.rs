//! Exact local raw agent-file reads and cooperative revision-bound repair.
mod disk;
use super::*;
use axum::http::StatusCode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;

pub const FEATURE: &str = "agent.source.repair.v1";
const MAX_SOURCE: usize = 256 * 1024;
const MAX_REGISTRY: usize = 64 * 1024;
const MAX_ENVELOPE: usize = 2 * 1024 * 1024;
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceQuery {
    pub host: Option<String>,
    pub scope_kind: ScopeKind,
    pub scope_id: Option<String>,
    pub path: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepairBody {
    pub raw: String,
    pub expected_revision: String,
    pub request_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Owner {
    global: PathBuf,
    global_identity: disk::Identity,
    scope_kind: ScopeKind,
    scope_id: Option<String>,
    dir: PathBuf,
    directory_identity: disk::Identity,
    filename: String,
    stem: String,
    path: PathBuf,
    #[serde(skip)]
    requested_path: String,
    registry: Option<(PathBuf, String, disk::Stamp)>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Fault {
    None,
    BeforeRename,
    AfterRename,
}
fn large(s: &str) -> ApiError {
    ApiError(StatusCode::PAYLOAD_TOO_LARGE, s.into())
}
fn storage(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(format!("agent source storage failure: {e}"))
}
fn changed() -> ApiError {
    ApiError::conflict("agent source owner or revision changed; reload exact selected source")
}
fn unknown() -> ApiError {
    ApiError::internal(
        "agent_source_save_unknown; reload exact selected source before considering another save",
    )
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn safe_id(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/agent-files/source",
            get(read_source).put(repair_source),
        )
        .layer(axum::extract::DefaultBodyLimit::max(MAX_ENVELOPE))
}
impl Owner {
    fn resolve(s: &AppState, q: &SourceQuery) -> ApiResult<Self> {
        super::super::agentiflows::require_local(q.host.as_deref(), "agent source repair")?;
        if q.path.len() > 4096 || q.path.chars().any(char::is_control) {
            return Err(ApiError::bad_request("invalid exact agent path"));
        }
        let requested = FsPath::new(&q.path);
        if !requested.is_absolute()
            || requested.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(ApiError::bad_request(
                "agent source requires an absolute plain path",
            ));
        }
        let filename = requested
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ApiError::bad_request("agent source filename unavailable"))?
            .to_string();
        let stem = filename
            .strip_suffix(".md")
            .ok_or_else(|| ApiError::bad_request("agent source must be one .md file"))?
            .to_string();
        if stem.len() > 128 {
            return Err(ApiError::bad_request("agent file stem exceeds limit"));
        }
        validate_stem(&stem)?;
        let global = s.global_dir.canonicalize().map_err(storage)?;
        let global_dir = disk::directory(&global)?;
        let global_identity = disk::Identity::of(&global_dir.metadata().map_err(storage)?);
        let (dir, lexical, registry) = match q.scope_kind {
            ScopeKind::Global => {
                if q.scope_id.is_some() {
                    return Err(ApiError::bad_request(
                        "global agent source cannot have a project selector",
                    ));
                }
                (global.join("agents"), s.global_dir.join("agents"), None)
            }
            ScopeKind::Project => {
                let id = q
                    .scope_id
                    .as_deref()
                    .filter(|s| safe_id(s))
                    .ok_or_else(|| {
                        ApiError::bad_request("project agent source requires exact workspace ID")
                    })?;
                let path = global.join("workspaces").join(format!("{id}.toml"));
                let (bytes, stamp) = disk::read(&path, MAX_REGISTRY)?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| ApiError::conflict("project registration is not UTF-8"))?;
                let workspace: rupu_workspace::Workspace = toml::from_str(text)
                    .map_err(|_| ApiError::conflict("project registration is malformed"))?;
                if workspace.id != id || !FsPath::new(&workspace.path).is_absolute() {
                    return Err(changed());
                }
                let project = FsPath::new(&workspace.path)
                    .canonicalize()
                    .map_err(storage)?;
                let layer = project.join(".rupu");
                let layer_fd = disk::directory(&layer)?;
                if disk::Identity::of(&layer_fd.metadata().map_err(storage)?) == global_identity {
                    return Err(ApiError::conflict(
                        "project storage aliases global; select global explicitly",
                    ));
                }
                (
                    layer.join("agents"),
                    FsPath::new(&workspace.path).join(".rupu/agents"),
                    Some((path, digest(&bytes), stamp)),
                )
            }
        };
        let path = dir.join(&filename);
        if requested != path && requested != lexical.join(&filename) {
            return Err(ApiError::bad_request(
                "diagnostic path does not identify a direct file in the selected agent layer",
            ));
        }
        let directory = disk::directory(&dir)?;
        let directory_identity = disk::Identity::of(&directory.metadata().map_err(storage)?);
        Ok(Self {
            global,
            global_identity,
            scope_kind: q.scope_kind,
            scope_id: q.scope_id.clone(),
            dir,
            directory_identity,
            filename,
            stem,
            path,
            requested_path: q.path.clone(),
            registry,
        })
    }
    fn revision(&self, bytes: &[u8], stamp: &disk::Stamp) -> ApiResult<String> {
        Ok(digest(
            &serde_json::to_vec(&(self, stamp, digest(bytes))).map_err(storage)?,
        ))
    }
    fn check(&self, s: &AppState, q: &SourceQuery) -> ApiResult<()> {
        if Self::resolve(s, q)? != *self {
            Err(changed())
        } else {
            Ok(())
        }
    }
}
fn capped_error(s: String) -> String {
    let mut end = s.len().min(8192);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    if end < s.len() {
        format!("{}…", &s[..end])
    } else {
        s
    }
}
fn envelope(
    owner: &Owner,
    bytes: Vec<u8>,
    stamp: disk::Stamp,
    saved: bool,
    request_id: Option<&str>,
    previous_revision: Option<&str>,
) -> ApiResult<Value> {
    let raw =
        String::from_utf8(bytes).map_err(|_| ApiError::conflict("agent source is not UTF-8"))?;
    let revision = owner.revision(raw.as_bytes(), &stamp)?;
    let parsed = rupu_agent::AgentSpec::parse(&raw);
    let (parsed_name, parse_error) = match parsed {
        Ok(spec) => (Some(spec.name), None),
        Err(e) => (None, Some(capped_error(e.to_string()))),
    };
    let authoring_error = validate_new_name("agent", &owner.stem)
        .and_then(|_| {
            let spec = super::parse_agent_for_write(&raw)?;
            if spec.name != owner.stem {
                return Err(ApiError::bad_request(
                    "frontmatter name must equal the actual agent file stem",
                ));
            }
            Ok(())
        })
        .err()
        .map(|e| capped_error(e.1));
    let value = json!({"schema_version":1,"host_id":"local","owner_instance_id":digest(&serde_json::to_vec(&(&owner.global,&owner.global_identity)).map_err(storage)?),
        "scope_kind":owner.scope_kind,"scope_id":owner.scope_id,"requested_path":owner.requested_path,"path":owner.path,"file_stem":owner.stem,
        "revision":revision,"raw":raw,"parsed_name":parsed_name,"parse_error":parse_error,"authoring_error":authoring_error,
        "save_supported":validate_new_name("agent",&owner.stem).is_ok(),"consistency":"observed_with_revision_checks","saved":saved,"request_id":request_id,"previous_revision":previous_revision});
    if serde_json::to_vec(&value).map_err(storage)?.len() > MAX_ENVELOPE {
        return Err(large("agent source response exceeds byte budget"));
    }
    Ok(value)
}
fn load(s: &AppState, q: SourceQuery) -> ApiResult<Value> {
    let owner = Owner::resolve(s, &q)?;
    let (bytes, stamp) = disk::read(&owner.path, MAX_SOURCE)?;
    owner.check(s, &q)?;
    envelope(&owner, bytes, stamp, false, None, None)
}
async fn read_source(
    State(s): State<AppState>,
    Query(q): Query<SourceQuery>,
) -> ApiResult<Json<Value>> {
    super::super::runs::blocking(move || load(&s, q).map(Json)).await
}
fn save(s: &AppState, q: SourceQuery, body: RepairBody, fault: Fault) -> ApiResult<Value> {
    if body.raw.len() > MAX_SOURCE {
        return Err(large("agent source exceeds 256KiB"));
    }
    if body.expected_revision.len() != 64
        || !body
            .expected_revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || !safe_id(&body.request_id)
    {
        return Err(ApiError::bad_request(
            "valid expected_revision and request_id are required",
        ));
    }
    let owner = Owner::resolve(s, &q)?;
    validate_new_name("agent", &owner.stem)?;
    let spec = super::parse_agent_for_write(&body.raw)?;
    if spec.name != owner.stem {
        return Err(ApiError::bad_request(
            "frontmatter name must equal the actual agent file stem",
        ));
    }
    let _lock = disk::lock(&owner.dir)?;
    owner.check(s, &q)?;
    let (bytes, stamp) = disk::read(&owner.path, MAX_SOURCE)?;
    if owner.revision(&bytes, &stamp)? != body.expected_revision {
        return Err(changed());
    }
    disk::replace(
        &owner,
        &stamp,
        body.raw.as_bytes(),
        || {
            owner.check(s, &q)?;
            let (latest, latest_stamp) = disk::read(&owner.path, MAX_SOURCE)?;
            if owner.revision(&latest, &latest_stamp)? != body.expected_revision {
                return Err(changed());
            }
            Ok(())
        },
        fault,
    )?;
    // Once replaced, any failed proof/read/receipt construction is unknown;
    // never disguise a possible write as a precondition refusal.
    (|| {
        owner.check(s, &q)?;
        let (written, new_stamp) = disk::read(&owner.path, MAX_SOURCE)?;
        if written != body.raw.as_bytes() {
            return Err(changed());
        }
        envelope(
            &owner,
            written,
            new_stamp,
            true,
            Some(&body.request_id),
            Some(&body.expected_revision),
        )
    })()
    .map_err(|_| unknown())
}
async fn repair_source(
    State(s): State<AppState>,
    Query(q): Query<SourceQuery>,
    Json(body): Json<RepairBody>,
) -> ApiResult<Json<Value>> {
    super::super::agentiflows::require_local(q.host.as_deref(), "agent source repair")?;
    super::super::config::require_writable_to(&s, "agent source repair")?;
    super::super::runs::blocking(move || save(&s, q, body, Fault::None).map(Json)).await
}
/// Legacy HTTP entry points acquire once around their existing write helpers.
/// Nonreentrant; this helper never resolves another scope or locks another dir.
pub(super) fn legacy_lock(dir: &FsPath) -> ApiResult<std::fs::File> {
    fs::create_dir_all(dir).map_err(storage)?;
    disk::lock(&dir.canonicalize().map_err(storage)?)
}

#[cfg(test)]
mod tests;
