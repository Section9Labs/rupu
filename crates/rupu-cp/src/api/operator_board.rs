//! Local operator Board controls. No worker author, run launch, generated job,
//! remote fallback, or MCP feature advertisement is added by these routes.
use std::path::Path;
use std::time::Duration;

use axum::{
    extract::{Path as AxPath, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rupu_fleet::{
    Address, Board, Bus, Directive, FleetError, OperatorBoardWrite, OperatorVia, PostKind,
};
use rupu_runtime::services::operator::{Liveness, MessageRoot, RootKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    error::{ApiError, ApiResult},
    host::messages::{self, MessageRoute},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    let mut router = Router::new();
    for route in MessageRoute::ALL {
        router = router.route(&format!("/api/{}/:id/board/operator", route.segment()), get(
            move |State(s): State<AppState>, AxPath(id): AxPath<String>, Query(q): Query<HostQuery>| async move {
                local_route(route, &id, &q)?;
                let writable = crate::api::config::require_writable_to(&s, "operator Board changes").is_ok();
                let global = s.global_dir.clone();
                let mut result = tokio::task::spawn_blocking(move || inspect(&global, &id)).await
                    .map_err(|e| ApiError::internal(format!("operator Board inspection panicked: {e}")))??;
                result["writable"] = json!(writable);
                Ok::<_, ApiError>(Json(result))
            }
        ).post(
            move |State(s): State<AppState>, AxPath(id): AxPath<String>, Query(q): Query<HostQuery>, via: crate::mcp::ViaMcp, Json(body): Json<WriteRequest>| async move {
                local_route(route, &id, &q)?;
                crate::api::config::require_writable_to(&s, "operator Board changes")?;
                let caps = s.config.read().map(|c| rupu_runtime::services::messaging::caps(&c)).unwrap_or_default();
                let global = s.global_dir.clone();
                let client = via.is_some().then(|| "mcp".to_string());
                tokio::task::spawn_blocking(move || mutate(&global, &id, body, caps, client)).await
                    .map_err(|e| ApiError::internal(format!("operator Board write panicked: {e}; reconcile before another request")))?.map(Json)
            }
        ));
    }
    router
}
#[derive(Deserialize, Default)]
struct HostQuery {
    host: Option<String>,
}
fn local_route(route: MessageRoute, id: &str, q: &HostQuery) -> ApiResult<()> {
    if MessageRoute::of_id(id) != Some(route) {
        return Err(ApiError::not_found("Board owner does not match this route"));
    }
    if q.host.as_deref().is_some_and(|h| h != "local") {
        return Err(ApiError::not_available(
            "operator Board controls are local-only; connect directly to the owner",
        ));
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Owner {
    kind: RootKind,
    id: String,
    host: &'static str,
    token: String,
}
struct Context {
    owner: Owner,
    root: MessageRoot,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteRequest {
    owner_token: String,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Claim {
        work_unit: String,
        ttl_seconds: u64,
    },
    Release {
        work_unit: String,
        claim_token: String,
    },
    Post {
        kind: PostKind,
        body: String,
        addressed_to: Option<String>,
        expected_run_id: Option<String>,
    },
    Directive {
        body: String,
        addressed_to: Option<String>,
        expected_run_id: Option<String>,
    },
    Retract {
        expected: Directive,
    },
}
impl Operation {
    fn name(&self) -> &'static str {
        match self {
            Self::Claim { .. } => "claim",
            Self::Release { .. } => "release",
            Self::Post { .. } => "post",
            Self::Directive { .. } => "directive",
            Self::Retract { .. } => "retract",
        }
    }
}

/// A request token binds the route to this exact active owner directory, not
/// merely a reusable id. It is a precondition, not an authorization credential.
fn context(global: &Path, id: &str) -> ApiResult<Context> {
    let (_, root) = messages::root(global, id).map_err(|e| ApiError::not_found(e.message))?;
    let global = global
        .canonicalize()
        .map_err(|_| ApiError::not_found("global owner root is unavailable"))?;
    let (collection, file, key) = match root.kind {
        RootKind::Agentiflow => ("agentiflows", "agentiflow.json", "id"),
        RootKind::Session => ("sessions", "session.json", "session_id"),
        RootKind::Workflow | RootKind::Standalone => ("runs", "run.json", "id"),
    };
    let base = global.join(collection);
    let dir = base.join(id);
    exact_directory(&base)?;
    exact_directory(&dir)?;
    let expected_space = if root.kind == RootKind::Agentiflow {
        dir.clone()
    } else {
        dir.join("messages")
    };
    let record = dir.join(file);
    let mut record_identity = Value::Null;
    if std::fs::symlink_metadata(&record).is_ok() {
        let metadata = std::fs::symlink_metadata(&record)
            .map_err(|_| ApiError::not_found("owner record is unavailable"))?;
        if !metadata.is_file() || metadata.len() > 1_048_576 {
            return Err(ApiError::conflict(
                "owner record is not a bounded regular file",
            ));
        }
        let value: Value = serde_json::from_slice(
            &std::fs::read(&record).map_err(|e| ApiError::internal(e.to_string()))?,
        )
        .map_err(|_| ApiError::conflict("owner record is unreadable"))?;
        if value[key].as_str() != Some(id) {
            return Err(ApiError::conflict(
                "owner record identity does not match its directory",
            ));
        }
        if value["status"].as_str().is_none()
            || (matches!(root.kind, RootKind::Workflow | RootKind::Standalone)
                && value["workflow_name"].as_str().is_none())
        {
            return Err(ApiError::conflict(
                "owner record has no readable lifecycle identity",
            ));
        }
        record_identity = json!([
            value[key],
            value["started_at"],
            value["created_at"],
            value["workflow_name"],
            value["workspace_id"],
            value["workspace_path"],
            value["project_root"]
        ]);
    } else if root.kind != RootKind::Standalone {
        return Err(ApiError::not_found("owner record is missing"));
    }
    if std::fs::symlink_metadata(&expected_space).is_ok() {
        exact_directory(&expected_space)?;
    }
    inspect_store_paths(&expected_space)?;
    // The legacy resolver can find archives. They are never writable owners.
    let resolved_parent = if root.kind == RootKind::Agentiflow {
        root.space.clone()
    } else {
        root.space.parent().unwrap_or(&root.space).to_path_buf()
    };
    if resolved_parent.canonicalize().ok().as_deref() != Some(dir.as_path()) {
        return Err(ApiError::conflict(
            "the selected owner is archived or replaced",
        ));
    }
    let metadata = std::fs::metadata(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(b"rupu.operator-board.owner.v1\0");
    digest.update(dir.as_os_str().as_encoded_bytes());
    digest.update(record_identity.to_string().as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest.update(metadata.dev().to_le_bytes());
        digest.update(metadata.ino().to_le_bytes());
    }
    #[cfg(not(unix))]
    {
        digest.update(format!("{:?}", metadata.created()).as_bytes());
    }
    let token = format!("{:x}", digest.finalize());
    Ok(Context {
        owner: Owner {
            kind: root.kind,
            id: id.into(),
            host: "local",
            token,
        },
        root: MessageRoot {
            space: expected_space,
            ..root
        },
    })
}
fn exact_directory(path: &Path) -> ApiResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ApiError::not_found("canonical active owner directory is missing"))?;
    if !metadata.is_dir() || path.canonicalize().ok().as_deref() != Some(path) {
        return Err(ApiError::conflict(
            "operator Board ownership cannot traverse symlinked directories",
        ));
    }
    Ok(())
}
/// Bound traversal to the actual mutable Board/mailbox stores, never transcript
/// or artifact trees. Reject links/special files before opening a store writer.
/// The store is locally owned; this is not a hostile-filesystem transaction.
fn inspect_store_paths(space: &Path) -> ApiResult<()> {
    let mut pending = vec![
        (space.join("board"), 0usize),
        (space.join("mailboxes"), 0usize),
        (space.join("log.jsonl"), 0usize),
        (space.join("log.jsonl.lock"), 0usize),
        (space.join("participants.jsonl"), 0usize),
    ];
    let mut visited = 0usize;
    while let Some((path, depth)) = pending.pop() {
        visited += 1;
        if visited > 20_000 || depth > 3 {
            return Err(ApiError::conflict(
                "operator Board store path inspection exceeds its bound",
            ));
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(ApiError::internal(error.to_string())),
        };
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(ApiError::conflict(
                "operator Board stores cannot contain symlinks or special files",
            ));
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path).map_err(|e| ApiError::internal(e.to_string()))? {
                if visited + pending.len() >= 20_000 {
                    return Err(ApiError::conflict(
                        "operator Board store path inspection exceeds its bound",
                    ));
                }
                pending.push((
                    entry.map_err(|e| ApiError::internal(e.to_string()))?.path(),
                    depth + 1,
                ));
            }
        }
    }
    Ok(())
}
fn live(ctx: &Context, global: &Path) -> ApiResult<()> {
    let canonical_global = global
        .canonicalize()
        .map_err(|_| ApiError::not_found("global owner root is unavailable"))?;
    match ctx
        .root
        .liveness(&canonical_global, &rupu_orchestrator::runs::pid_is_running)
    {
        Liveness::Live => Ok(()),
        Liveness::NotLive(why) => Err(ApiError::conflict(why)),
    }
}
fn inspect(global: &Path, id: &str) -> ApiResult<Value> {
    let ctx = context(global, id)?;
    let state = live(&ctx, global);
    let board = Board::new(&ctx.root.space);
    // Standing directives are required for exact retraction even in sessions,
    // which have no legacy /board route. Never silently truncate this snapshot.
    let live_directives = board.operator_directives().map_err(fleet_error)?;
    Ok(
        json!({"owner": ctx.owner, "actor": "operator", "live": state.is_ok(),
        "notice": state.err().map(|e| e.1), "operator_leases": board.operator_leases().map_err(fleet_error)?, "live_directives": live_directives}),
    )
}
fn mutate(
    global: &Path,
    id: &str,
    request: WriteRequest,
    caps: rupu_fleet::BusCaps,
    client: Option<String>,
) -> Result<Value, WriteError> {
    if request.owner_token.len() != 64 {
        return Err(ApiError::bad_request(
            "owner_token must be the exact token from operator Board inspection",
        )
        .into());
    }
    let ctx = context(global, id)?;
    if request.owner_token != ctx.owner.token {
        return Err(
            ApiError::conflict("Board owner changed; inspect it again before writing").into(),
        );
    }
    live(&ctx, global)?;
    let action = request.operation.name();
    let bus = Bus::open(&ctx.root.space, rupu_fleet::address::OPERATOR).with_caps(caps);
    let check = || {
        let current = context(global, id).map_err(|e| FleetError::Refused(e.1))?;
        if current.owner != ctx.owner {
            return Err(FleetError::Refused("Board owner changed".into()));
        }
        live(&current, global).map_err(|e| FleetError::Refused(e.1))
    };
    let receipt = match request.operation {
        Operation::Claim {
            work_unit,
            ttl_seconds,
        } => serde_json::to_value(
            bus.operator_claim(&work_unit, Duration::from_secs(ttl_seconds), check)
                .map_err(write_error)?,
        )
        .map_err(|e| ApiError::internal(e.to_string()))?,
        Operation::Release {
            work_unit,
            claim_token,
        } => {
            if !bus
                .operator_release(&work_unit, &claim_token, check)
                .map_err(write_error)?
            {
                return Err(ApiError::conflict(
                    "operator claim is absent, expired or replaced; no claim was released",
                )
                .into());
            }
            json!({"released": true, "work_unit": work_unit, "claim_token": claim_token})
        }
        operation => {
            let write = match operation {
                Operation::Post {
                    kind,
                    body,
                    addressed_to,
                    expected_run_id,
                } => OperatorBoardWrite::Post {
                    kind,
                    body,
                    to: recipient(addressed_to)?,
                    expected_run_id,
                },
                Operation::Directive {
                    body,
                    addressed_to,
                    expected_run_id,
                } => OperatorBoardWrite::Directive {
                    body,
                    to: recipient(addressed_to)?,
                    expected_run_id,
                },
                Operation::Retract { expected } => OperatorBoardWrite::Retract { expected },
                _ => unreachable!(),
            };
            let sent = Bus::open(&ctx.root.space, rupu_fleet::address::OPERATOR)
                .with_caps(caps)
                .operator_board(write, OperatorVia::Cp, client, check)
                .map_err(write_error)?;
            serde_json::to_value(sent.message).map_err(|e| ApiError::internal(e.to_string()))?
        }
    };
    Ok(
        json!({"owner": ctx.owner, "actor": "operator", "via": "cp", "action": action, "receipt": receipt}),
    )
}
fn recipient(value: Option<String>) -> ApiResult<Option<Address>> {
    value
        .map(|value| Address::parse(&value).map_err(|e| ApiError::bad_request(e.to_string())))
        .transpose()
}
fn fleet_error(error: FleetError) -> ApiError {
    match error {
        FleetError::RecipientChanged | FleetError::Refused(_) | FleetError::LogFull { .. } => {
            ApiError::conflict(error.to_string())
        }
        FleetError::Address(_) => ApiError::bad_request(error.to_string()),
        _ => ApiError::internal(format!(
            "{error}; reconcile recorded state before another request"
        )),
    }
}

/// A failed response never tells the UI to replay a write. Logged partial
/// application supplies the canonical message id for read-only reconciliation.
struct WriteError {
    error: ApiError,
    logged_message_id: Option<String>,
}
impl From<ApiError> for WriteError {
    fn from(error: ApiError) -> Self {
        Self {
            error,
            logged_message_id: None,
        }
    }
}
impl IntoResponse for WriteError {
    fn into_response(self) -> Response {
        let uncertain = self.error.0.is_server_error() && self.error.0.as_u16() != 501;
        (self.error.0, Json(json!({
            "error": self.error.1,
            "code": if uncertain { "operator_board_write_uncertain" } else { "operator_board_refused" },
            "mutation_outcome": if uncertain { "reconcile" } else { "not_applied" },
            "logged_message_id": self.logged_message_id
        }))).into_response()
    }
}
fn write_error(error: FleetError) -> WriteError {
    let logged_message_id = match &error {
        FleetError::PartlyDelivered { id, .. } => Some(id.clone()),
        _ => None,
    };
    WriteError {
        error: fleet_error(error),
        logged_message_id,
    }
}
