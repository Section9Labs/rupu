use super::{available, owner};
use crate::{
    forge_write_store::{self as journal, Outcome, Phase, Reservation},
    state::AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rupu_scm::forge_write::{
    DispatchOutcome, DispatchResult, ForgeAction, ForgeError, ForgeInput, ForgeRequest,
    ForgeTarget, PreparedWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, LazyLock},
    time::Duration,
};

static PREPARERS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
static READERS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));
const PUBLIC_BYTES: usize = 512 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/forge/write-capabilities", get(capabilities))
        .route("/api/forge/intents", post(prepare))
        .route("/api/forge/intents/:id", get(intent))
        .route("/api/forge/intents/:id/execute", post(execute))
        .route("/api/forge/operations/:id", get(status))
        .layer(DefaultBodyLimit::max(128 * 1024))
}
#[derive(Debug)]
struct Error(StatusCode, Value);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}
type Result<T> = std::result::Result<T, Error>;
fn error(status: StatusCode, code: &str, message: &str) -> Error {
    Error(status, json!({"code":code,"error":message}))
}
fn store_error(e: io::Error) -> Error {
    match e.kind(){
        io::ErrorKind::WouldBlock=>error(StatusCode::SERVICE_UNAVAILABLE,"busy","A forge operation is active; inspect its status before trying again"),
        io::ErrorKind::AlreadyExists=>error(StatusCode::CONFLICT,"intent_conflict","The operation, intent or identical unresolved action is already reserved"),
        io::ErrorKind::StorageFull=>error(StatusCode::SERVICE_UNAVAILABLE,"journal_capacity","The local operation journal reached its current storage limit; no write was attempted"),
        io::ErrorKind::NotFound=>error(StatusCode::NOT_FOUND,"not_found","The reviewed intent or operation was not found"),
        _=>error(StatusCode::SERVICE_UNAVAILABLE,"journal_unavailable","The private operation journal could not be verified; inspect status before any further action"),
    }
}
fn provider_error(e: ForgeError) -> Error {
    let status = match e.code.as_str() {
        "invalid_intent" | "unsupported_quick_action" => StatusCode::BAD_REQUEST,
        "choice_required" | "selection_changed" | "labels_unavailable" => StatusCode::CONFLICT,
        "account_unavailable" | "choices_incomplete" => StatusCode::SERVICE_UNAVAILABLE,
        "unsupported" => StatusCode::NOT_IMPLEMENTED,
        "not_found" => StatusCode::NOT_FOUND,
        "forbidden" => StatusCode::FORBIDDEN,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    };
    Error(
        status,
        json!({"code":e.code,"error":e.message,"choices":e.choices}),
    )
}
fn json_value(value: Value) -> Result<Json<Value>> {
    if serde_json::to_vec(&value)
        .map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "encoding_failed",
                "The operation observation could not be encoded",
            )
        })?
        .len()
        > PUBLIC_BYTES
    {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "response_too_large",
            "The operation observation exceeded its display budget; use its retained operation ID",
        ));
    }
    Ok(Json(value))
}
fn local(host: Option<&str>) -> Result<()> {
    if host.is_some_and(|h| h != "local") {
        return Err(error(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "Forge operations require a direct connection to the owning instance",
        ));
    }
    Ok(())
}
fn gate(s: &AppState, host: Option<&str>) -> Result<()> {
    local(host)?;
    if !available(s) {
        return Err(error(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "This owning instance has no guarded forge writer",
        ));
    }
    Ok(())
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalQuery {
    host: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareBody {
    host: Option<String>,
    action: ForgeAction,
    target: ForgeTarget,
    expected: BTreeMap<String, String>,
    input: ForgeInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteBody {
    operation_id: String,
    expected_digest: String,
    confirm: bool,
}

async fn current_state(s: &AppState) -> Result<AppState> {
    crate::repos::snapshot_state(s).await.map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "enrollment_unavailable",
            "Current SCM enrollment is unavailable",
        )
    })
}

async fn capabilities(
    State(s): State<AppState>,
    Query(q): Query<LocalQuery>,
) -> Result<Json<Value>> {
    gate(&s, q.host.as_deref())?;
    let s = current_state(&s).await?;
    let port = s
        .repos
        .as_ref()
        .and_then(|p| p.forge_writer())
        .expect("gated immutable adapter");
    let rows = port.capabilities();
    if rows.len() > 64 {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_capacity",
            "The configured forge account catalog exceeds this reader's current limit",
        ));
    }
    json_value(
        json!({"host_id":"local","capabilities":rows,"preconditions_atomic":false,"automatic_retry":false}),
    )
}
fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| error(
            StatusCode::BAD_REQUEST,
            "invalid_intent",
            "Intent could not be represented"
        ))?)
    ))
}
fn request_owner_id(request: &ForgeRequest) -> Result<Option<&str>> {
    if request.action.is_repository() {
        Ok(Some(
            request
                .target
                .ws_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "invalid_intent",
                        "Repository writes require an exact registered project",
                    )
                })?,
        ))
    } else if request.target.ws_id.is_some() {
        Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_intent",
            "Issue writes require explicit tracker authority independent of a selected workspace",
        ))
    } else {
        Ok(None)
    }
}
fn instance_id(s: &AppState) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let path = s.global_dir.canonicalize().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "instance_unavailable",
            "Owning instance identity is unavailable",
        )
    })?;
    let metadata = path.metadata().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "instance_unavailable",
            "Owning instance identity is unavailable",
        )
    })?;
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(metadata.dev().to_be_bytes());
    hash.update(metadata.ino().to_be_bytes());
    Ok(format!("{:x}", hash.finalize()))
}
async fn prepared(s: &AppState, request: &ForgeRequest) -> Result<PreparedWrite> {
    let current = current_state(s).await?;
    let s = &current;
    let owner = match request_owner_id(request)? {
        Some(id) => Some(owner::resolve(s, id).await.map_err(|_| {
            error(
                StatusCode::CONFLICT,
                "selection_changed",
                "The owning checkout could not be verified; refresh its context",
            )
        })?),
        None => None,
    };
    let port = s
        .repos
        .as_ref()
        .and_then(|p| p.forge_writer())
        .ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "adapter_missing",
                "The owning forge writer is unavailable",
            )
        })?;
    let mut value = port
        .prepare(request, owner.as_ref())
        .await
        .map_err(provider_error)?;
    if value.request != *request {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "invalid_preparation",
            "The adapter changed the submitted action or content",
        ));
    }
    if !value.authority.is_object() {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "invalid_preparation",
            "Prepared authority is invalid",
        ));
    }
    value.authority["instance_id"] = json!(instance_id(s)?);
    Ok(value)
}
fn dedupe_key(prepared: &PreparedWrite) -> Result<String> {
    let authority = &prepared.authority;
    for key in ["provider", "account", "api_origin", "provider_project_id"] {
        if authority[key].as_str().is_none_or(str::is_empty) {
            return Err(error(
                StatusCode::BAD_GATEWAY,
                "invalid_preparation",
                "Prepared authority lacks a stable provider identity",
            ));
        }
    }
    if (prepared.request.action.needs_existing_number()
        || prepared.request.action == rupu_scm::forge_write::ForgeAction::WorkflowDispatch)
        && authority["provider_object_id"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "invalid_preparation",
            "Prepared action lacks an immutable target identity",
        ));
    }
    let mut input = prepared.request.input.clone();
    input.labels.sort();
    // An uncertain creation owns this destination branch even if a caller
    // changes the requested base before preparing again.
    if prepared.request.action == rupu_scm::forge_write::ForgeAction::BranchCreate {
        input.base = None;
    }
    // Filename and numeric aliases of one workflow must share uncertainty.
    if prepared.request.action == rupu_scm::forge_write::ForgeAction::WorkflowDispatch {
        input.workflow = None;
    }
    hash(
        &json!({"action":prepared.request.action,"provider":authority["provider"],"account":authority["account"],"api_origin":authority["api_origin"],"provider_project_id":authority["provider_project_id"],"provider_object_id":authority["provider_object_id"],"input":input}),
    )
}

async fn prepare(State(s): State<AppState>, Json(body): Json<PrepareBody>) -> Result<Json<Value>> {
    gate(&s, body.host.as_deref())?;
    let _permit = PREPARERS.clone().try_acquire_owned().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Forge intent preparation is busy",
        )
    })?;
    let request = ForgeRequest {
        action: body.action,
        target: body.target,
        expected: body.expected,
        input: body.input,
    };
    let value = tokio::time::timeout(Duration::from_secs(30), prepared(&s, &request))
        .await
        .map_err(|_| {
            error(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                "Preparation exceeded its deadline; no forge write was attempted",
            )
        })??;
    let key = dedupe_key(&value)?;
    let payload = serde_json::to_value(value).map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "encoding_failed",
            "Prepared intent could not be encoded",
        )
    })?;
    let digest = hash(&payload)?;
    let id = ulid::Ulid::new().to_string();
    let expires = chrono::Utc::now().timestamp().max(0) as u64 + 600;
    let global = s.global_dir.clone();
    let stored = tokio::task::spawn_blocking(move || {
        let store = journal::Store::open(&global)?;
        store.compact_intents(chrono::Utc::now().timestamp().max(0) as u64)?;
        store.publish_intent(&id, &digest, &key, expires, payload)
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "journal_unavailable",
            "Intent publication was interrupted; no forge write was attempted",
        )
    })?
    .map_err(store_error)?;
    json_value(intent_value(&stored))
}
fn intent_value(intent: &journal::Intent) -> Value {
    json!({"host_id":"local","intent_id":intent.intent_id,"digest":intent.digest,"created_at":intent.created_at,"expires_at":intent.expires_at,"review":intent.payload,"execute_supported":true,"preconditions_atomic":false,"automatic_retry":false})
}
async fn read_record<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T> {
    let permit = READERS.clone().try_acquire_owned().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Operation journal readers are busy",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "journal_unavailable",
            "Journal observation was interrupted",
        )
    })?
    .map_err(store_error)
}
async fn intent(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
) -> Result<Json<Value>> {
    local(q.host.as_deref())?;
    let global = s.global_dir.clone();
    let row = read_record(move || journal::Reader::open(&global)?.intent(&id))
        .await?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Reviewed intent was not found",
            )
        })?;
    json_value(intent_value(&row))
}
fn receipt_value(
    operation: &journal::Operation,
    outcome: &str,
    active: bool,
    durability: &str,
) -> Value {
    json!({"host_id":"local","operation_id":operation.operation_id,"intent_id":operation.intent.intent_id,"digest":operation.intent.digest,
        "review":operation.intent.payload,"phase":operation.phase,"outcome":outcome,"reason":operation.reason,"created_at":operation.created_at,"updated_at":operation.updated_at,
        "receipt_version":operation.receipt_version.to_string(),"durability":durability,"writer_active":active,"result":operation.result,"preconditions_atomic":false,"automatic_retry":false})
}
fn terminal_outcome(operation: &journal::Operation) -> &'static str {
    match operation.outcome {
        Some(Outcome::NotSubmitted) => "not_submitted",
        Some(Outcome::Accepted) => "accepted",
        Some(Outcome::Rejected) => "rejected",
        Some(Outcome::Unknown) => "unknown",
        None => match operation.phase {
            Phase::Reserved => "not_submitted",
            _ => "unknown",
        },
    }
}
async fn status(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
) -> Result<Json<Value>> {
    local(q.host.as_deref())?;
    let global = s.global_dir.clone();
    let row = read_record(move || journal::Reader::open(&global)?.operation(&id))
        .await?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Operation was not found; this is not proof of provider outcome",
            )
        })?;
    let outcome = match row.observed_outcome {
        journal::ObservedOutcome::InProgress => "in_progress",
        journal::ObservedOutcome::NotSubmitted => "not_submitted",
        journal::ObservedOutcome::Accepted => "accepted",
        journal::ObservedOutcome::Rejected => "rejected",
        journal::ObservedOutcome::Unknown => "unknown",
    };
    json_value(receipt_value(
        &row.operation,
        outcome,
        row.writer_active,
        if row.writer_active {
            "unconfirmed"
        } else {
            "observed"
        },
    ))
}
async fn execute(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
    Json(body): Json<ExecuteBody>,
) -> Result<Json<Value>> {
    gate(&s, q.host.as_deref())?;
    if !body.confirm {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "confirmation_required",
            "Review the exact target and content before confirming this operation",
        ));
    }
    // Owned task survives HTTP cancellation. Its durable marker remains the
    // recovery authority if the CP itself exits; no restart resumes dispatch.
    let operation_id = body.operation_id.clone();
    let job = tokio::spawn(execute_job(s, id, body));
    match job.await {
        Ok(value) => json_value(value?),
        Err(_) => Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"code":"operation_uncertain","error":"Operation processing was interrupted; inspect this operation ID without resubmitting","operation_id":operation_id}),
        )),
    }
}
fn failed_receipt(
    mut before: journal::Operation,
    outcome: &str,
    reason: &str,
    evidence: Value,
) -> Value {
    before.reason = Some(reason.into());
    before.result = evidence;
    receipt_value(&before, outcome, false, "unconfirmed")
}
async fn journal_work<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| io::Error::other("journal worker interrupted"))?
}
async fn execute_job(s: AppState, id: String, body: ExecuteBody) -> Result<Value> {
    let global = s.global_dir.clone();
    let operation_id = body.operation_id.clone();
    let digest = body.expected_digest.clone();
    let (store, reservation) = journal_work(move || {
        let store = journal::Store::open(&global)?;
        let reservation = store.reserve(&operation_id, &id, &digest)?;
        Ok((store, reservation))
    })
    .await
    .map_err(store_error)?;
    let (intent, ticket) = match reservation {
        Reservation::Replay(operation) => {
            return Ok(receipt_value(
                &operation,
                terminal_outcome(&operation),
                false,
                "durable",
            ))
        }
        Reservation::New { intent, ticket } => (intent, ticket),
    };
    let before = ticket.operation().clone();
    let ready = tokio::time::timeout(Duration::from_secs(30), async {
        let s = current_state(&s).await?;
        let prepared: PreparedWrite = serde_json::from_value(intent.payload).map_err(|_| {
            error(
                StatusCode::CONFLICT,
                "invalid_preparation",
                "Stored preparation is incompatible",
            )
        })?;
        if prepared.authority["instance_id"].as_str() != Some(instance_id(&s)?.as_str()) {
            return Err(error(
                StatusCode::CONFLICT,
                "selection_changed",
                "The reviewed intent belongs to another owning instance",
            ));
        }
        let owner = match request_owner_id(&prepared.request)? {
            Some(id) => Some(owner::resolve(&s, id).await.map_err(|_| {
                error(
                    StatusCode::CONFLICT,
                    "selection_changed",
                    "Owning checkout changed or is unavailable",
                )
            })?),
            None => None,
        };
        let port = s
            .repos
            .as_ref()
            .and_then(|p| p.forge_writer())
            .ok_or_else(|| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "adapter_missing",
                    "Owning writer is unavailable",
                )
            })?;
        port.preflight(&prepared, owner.as_ref())
            .await
            .map_err(provider_error)
    })
    .await;
    let ready = match ready {
        Ok(Ok(ready)) => ready,
        result => {
            let reason = match result {
                Ok(Err(e)) => e.1["code"]
                    .as_str()
                    .unwrap_or("preflight_refused")
                    .to_string(),
                _ => "preflight_timeout".into(),
            };
            return Ok(
                match journal_work(move || store.finish_not_submitted(ticket, &reason, Value::Null))
                    .await
                {
                    Ok(op) => receipt_value(&op, "not_submitted", false, "durable"),
                    Err(_) => failed_receipt(
                        before,
                        "not_submitted",
                        "No mutation was dispatched, but the final receipt could not be confirmed",
                        Value::Null,
                    ),
                },
            );
        }
    };
    let (store, ticket) = match journal_work(move || {
        let ticket = store.mark_dispatching(ticket)?;
        Ok((store, ticket))
    })
    .await
    {
        Ok(value) => value,
        Err(_) => {
            return Ok(failed_receipt(
                before,
                "not_submitted",
                "The dispatch marker could not be confirmed; no mutation was sent",
                Value::Null,
            ))
        }
    };
    let before = ticket.operation().clone();
    let mut result = tokio::time::timeout(Duration::from_secs(30), ready.dispatch())
        .await
        .unwrap_or_else(|_| DispatchResult::unknown());
    if serde_json::to_vec(&result).map_or(true, |v| v.len() > 64 * 1024) {
        result = DispatchResult::unknown();
    }
    let outcome = match result.outcome {
        DispatchOutcome::Accepted => Outcome::Accepted,
        DispatchOutcome::Rejected => Outcome::Rejected,
        DispatchOutcome::Unknown => Outcome::Unknown,
    };
    let evidence = json!({"provider_outcome":result.outcome,"provider_reason":result.reason,"provider_result":result.result});
    Ok(
        match journal_work(move || {
            store.finish_dispatch(ticket, outcome, &result.reason, result.result)
        })
        .await
        {
            Ok(op) => receipt_value(&op, terminal_outcome(&op), false, "durable"),
            Err(_) => failed_receipt(
                before,
                "unknown",
                "Provider response was observed but the terminal receipt could not be confirmed",
                evidence,
            ),
        },
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
