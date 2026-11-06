//! Session CLI parity. Identity is always the URL session on the explicitly
//! selected local host; slash commands never fall through to a model prompt.
use super::sessions::{
    active_session, compact_session, stop_session, CompactBody, SessionHostQuery,
};
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rupu_runtime::session_commands::{parse_attach_command, usage, AttachCommand, SLASH_COMMANDS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/:id/controls", get(controls))
        .route("/api/sessions/:id/commands", post(execute))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandBody {
    pub command: String,
    pub expected_run_id: Option<String>,
}
#[derive(Serialize)]
struct Capability {
    name: &'static str,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    mutating: bool,
    #[serde(skip)]
    conflict: bool,
    usage: String,
    description: &'static str,
}
fn record(s: &AppState, id: &str, host: Option<&str>) -> ApiResult<Value> {
    crate::api::runs::validate_id(id)?;
    crate::api::fleet::require_local(host, "session commands")?;
    active_session(s, id, "commands")?;
    let record: Value = serde_json::from_slice(
        &std::fs::read(s.global_dir.join("sessions").join(id).join("session.json"))
            .map_err(|e| ApiError::internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::internal(e.to_string()))?;
    if record["session_id"].as_str() != Some(id) {
        return Err(ApiError::conflict("session record identity mismatch"));
    }
    Ok(record)
}
fn capabilities(s: &AppState, id: &str, record: &Value) -> ApiResult<Vec<Capability>> {
    let dir = s.global_dir.join("sessions").join(id);
    let compacting =
        rupu_runtime::services::session_compaction::live(&dir, rupu_launch::proc::process_alive)
            .map_err(|e| ApiError::internal(e.to_string()))?
            .is_some();
    let run = record["active_run_id"].as_str().filter(|s| !s.is_empty());
    let adapter = s.session_mutator.as_ref();
    Ok(SLASH_COMMANDS.iter().map(|c| {
        let mutating = matches!(c.name, "cancel" | "compact" | "stop" | "quit");
        let reason = if mutating && s.launcher.is_none() {
            Some("session mutation requires a writable `rupu cp serve` deployment".into())
        } else if mutating && compacting {
            Some("session is being compacted; wait for completion".into())
        } else if c.name == "cancel" {
            match run {
                None => Some("no active run".into()),
                Some(run) if !rupu_runtime::session_cancel::available(&dir, run, record["active_pid"].as_u64().and_then(|p| u32::try_from(p).ok()), rupu_launch::proc::process_alive) => Some("active worker does not support cooperative cancellation; restart it after this turn".into()),
                Some(_) => None,
            }
        } else if matches!(c.name, "stop" | "quit" | "compact") && adapter.is_none() {
            Some("session mutation adapter is unavailable".into())
        } else if c.name == "compact" && (run.is_some() || record["status"] == "running") {
            Some("cancel the active run or wait for it to finish".into())
        } else if matches!(c.name, "help" | "detach" | "stop" | "quit" | "compact") {
            None
        } else if !adapter.is_some_and(|a| a.supports_commands()) {
            Some("session command adapter is unavailable; use an updated `rupu cp serve`".into())
        } else if c.name == "coverage" && record["concerns"].is_null() {
            Some("this session's agent doesn't declare concerns".into())
        } else if c.name == "coverage" {
            record["workspace_path"].as_str().and_then(|workspace| {
                let target = rupu_coverage::target_id(std::path::Path::new(workspace), id);
                let paths = rupu_coverage::CoveragePaths::new(std::path::Path::new(workspace), &target);
                (!paths.catalog.exists()).then(|| "no ledger yet for this session (run a turn first)".into())
            })
        } else { None };
        Capability {name:c.name, available:reason.is_none(), reason, mutating,
            conflict: s.launcher.is_some() && (mutating && compacting || c.name == "compact" && adapter.is_some() && (run.is_some() || record["status"] == "running")),
            usage:usage(c.name), description:c.description}
    }).collect())
}
async fn controls(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<Value>> {
    crate::api::runs::blocking(move || {
        let record = record(&s, &id, q.host.as_deref())?;
        Ok(Json(json!({"session_id":id,"host_id":"local","active_run_id":record["active_run_id"],"commands":capabilities(&s,&id,&record)?})))
    }).await
}
async fn execute(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
    Json(body): Json<CommandBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        execute_inner(State(s), Path(id), Query(q), Json(body)),
    ).await.map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT,
        "session command deadline exceeded; reload session state before retrying a mutation because it may already have been accepted".into()))?
}
async fn execute_inner(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
    Json(body): Json<CommandBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let command = parse_attach_command(&body.command).ok_or_else(|| {
        ApiError::bad_request("unsupported command or malformed arguments; use /help")
    })?;
    let (state, sid, host) = (s.clone(), id.clone(), q.host.clone());
    let (record, caps) = crate::api::runs::blocking(move || {
        let r = record(&state, &sid, host.as_deref())?;
        let c = capabilities(&state, &sid, &r)?;
        Ok((r, c))
    })
    .await?;
    if let Some(expected) = &body.expected_run_id {
        if record["active_run_id"].as_str() != Some(expected) {
            return Err(ApiError::conflict(
                "active run changed; reload before executing command",
            ));
        }
    } else if matches!(command, AttachCommand::Cancel) {
        return Err(ApiError::bad_request("/cancel requires expected_run_id"));
    }
    if command.mutating() {
        crate::api::config::require_writable_to(&s, "session commands")?;
    }
    if let Some(cap) = caps.iter().find(|c| c.name == command.name()) {
        if let Some(reason) = &cap.reason {
            return Err(if cap.conflict {
                ApiError::conflict(reason.clone())
            } else {
                ApiError::not_available(reason.clone())
            });
        }
    }
    let mut status = StatusCode::OK;
    let mut action = None;
    let mut output = match command {
        AttachCommand::Help => {
            let text = caps
                .iter()
                .map(|c| {
                    format!(
                        "{} — {}{}",
                        c.usage,
                        c.description,
                        c.reason
                            .as_ref()
                            .map(|r| format!(" (unavailable: {r})"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut o = crate::session_commands::CommandOutput::text(text);
            o.data = Some(json!({"commands":caps}));
            o
        }
        AttachCommand::Detach => {
            action = Some("detach");
            crate::session_commands::CommandOutput::text("detached; session continues running")
        }
        AttachCommand::Cancel => {
            let run = body.expected_run_id.clone().unwrap();
            let dir = s.global_dir.join("sessions").join(&id);
            let expected = run.clone();
            crate::api::runs::blocking(move || {
                rupu_runtime::session_cancel::request(
                    &dir,
                    &expected,
                    rupu_launch::proc::process_alive,
                )
                .map_err(|e| ApiError::conflict(e.to_string()))
            })
            .await?;
            action = Some("cancel_requested");
            let mut o = crate::session_commands::CommandOutput::text(
                "cancel requested; waiting for the active turn's safe boundary",
            );
            o.data = Some(json!({"run_id":run}));
            o
        }
        AttachCommand::Stop | AttachCommand::Quit => {
            if matches!(command, AttachCommand::Quit) {
                action = Some("detach");
            }
            let Json(data) = stop_session(State(s), Path(id.clone()), Query(q)).await?;
            let mut o = crate::session_commands::CommandOutput::text("session stopped");
            o.data = Some(data);
            o
        }
        AttachCommand::Compact => {
            let (code, Json(data)) = compact_session(
                State(s),
                Path(id.clone()),
                Query(q),
                crate::api::runs::OptionalJson(Some(CompactBody::default())),
            )
            .await?;
            status = code;
            action = Some("compact_started");
            let mut o = crate::session_commands::CommandOutput::text("session compaction started");
            o.data = Some(data);
            o
        }
        command => s
            .session_mutator
            .as_ref()
            .unwrap()
            .command(&s.global_dir, &id, command)
            .await
            .map_err(|e| match e {
                crate::session_commands::CommandError::Invalid(m) => ApiError::bad_request(m),
                crate::session_commands::CommandError::Failed(m) => ApiError::internal(m),
            })?,
    };
    output.bound();
    let mut response =
        serde_json::to_value(output).map_err(|e| ApiError::internal(e.to_string()))?;
    let fields = response.as_object_mut().unwrap();
    fields.insert("session_id".into(), json!(id));
    fields.insert("host_id".into(), json!("local"));
    fields.insert("command".into(), json!(body.command));
    if let Some(action) = action {
        fields.insert("action".into(), json!(action));
    }
    Ok((status, Json(response)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tower::ServiceExt;
    struct Launcher;
    #[async_trait::async_trait]
    impl crate::launcher::RunLauncher for Launcher {
        async fn launch(
            &self,
            _: crate::launcher::LaunchRequest,
        ) -> Result<String, crate::launcher::LaunchError> {
            panic!("commands never launch a run")
        }
    }
    struct Adapter(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl crate::session_mutator::SessionMutator for Adapter {
        fn supports_commands(&self) -> bool {
            true
        }
        async fn command(
            &self,
            _: &std::path::Path,
            id: &str,
            command: AttachCommand,
        ) -> Result<crate::session_commands::CommandOutput, crate::session_commands::CommandError>
        {
            assert_eq!(id, "ses_test");
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::session_commands::CommandOutput::text(format!(
                "result from {}",
                command.name()
            )))
        }
        async fn mutate(
            &self,
            _: &str,
            _: crate::session_mutator::SessionAction,
        ) -> Result<(), crate::session_mutator::SessionMutateError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn compact(
            &self,
            _: &str,
            _: &str,
            _: Option<u32>,
            _: &std::path::Path,
        ) -> Result<u32, crate::session_mutator::SessionMutateError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(std::process::id())
        }
    }
    fn fixture() -> (tempfile::TempDir, AppState, Arc<AtomicUsize>) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sessions/ses_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            json!({"session_id":"ses_test","status":"idle","active_run_id":null}).to_string(),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let state = AppState::new(temp.path().to_owned(), Default::default())
            .with_session_mutator(Some(Arc::new(Adapter(calls.clone()))));
        (temp, state, calls)
    }
    async fn request(s: AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let app = super::super::sessions::routes().with_state(s);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(if method == "GET" {
                        Body::empty()
                    } else {
                        Body::from(body.to_string())
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"error":String::from_utf8_lossy(&bytes)})),
        )
    }
    #[tokio::test]
    async fn routes_enforce_identity_policy_and_never_send_prompts() {
        let (_tmp, s, calls) = fixture();
        let (code, body) = request(
            s.clone(),
            "GET",
            "/api/sessions/ses_test/controls",
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["session_id"], "ses_test");
        assert_eq!(body["host_id"], "local");
        assert!(body.get("active_run_id").unwrap().is_null());
        assert_eq!(
            body["commands"].as_array().unwrap().len(),
            SLASH_COMMANDS.len()
        );
        assert!(body["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "cancel")
            .unwrap()["reason"]
            .as_str()
            .unwrap()
            .contains("writable"));
        for (path, body, expected) in [
            (
                "/api/sessions/ses_test/commands?host=remote",
                json!({"command":"/session list"}),
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                "/api/sessions/ses_missing/commands",
                json!({"command":"/session list"}),
                StatusCode::NOT_FOUND,
            ),
            (
                "/api/sessions/ses_test/commands",
                json!({"command":"/session delete ses_test"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "/api/sessions/ses_test/commands",
                json!({"command":"/cancel"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "/api/sessions/ses_test/commands",
                json!({"command":"/cancel","expected_run_id":"stale"}),
                StatusCode::CONFLICT,
            ),
            (
                "/api/sessions/ses_test/commands",
                json!({"command":"/stop"}),
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                "/api/sessions/ses_test/commands",
                json!({"command":"/session list","unexpected":true}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            assert_eq!(
                request(s.clone(), "POST", path, body).await.0,
                expected,
                "{path}"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let (code, body) = request(
            s,
            "POST",
            "/api/sessions/ses_test/commands",
            json!({"command":"/session list"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["output"], "result from session");
        assert_eq!(body["session_id"], "ses_test");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn cancellation_targets_only_the_expected_live_turn() {
        let (tmp, s, calls) = fixture();
        let s = s.with_launcher(Some(Arc::new(Launcher)));
        let dir = tmp.path().join("sessions/ses_test");
        let original=json!({"session_id":"ses_test","status":"running","active_run_id":"run_current","active_pid":std::process::id(),"worker_pid":std::process::id()}).to_string();
        std::fs::write(dir.join("session.json"), &original).unwrap();
        let guard = rupu_runtime::session_cancel::TurnCancellation::start(
            &dir,
            "run_current",
            std::process::id(),
            rupu_launch::proc::own_start_stamp(),
        )
        .unwrap();
        let (_, caps) = request(
            s.clone(),
            "GET",
            "/api/sessions/ses_test/controls?host=local",
            Value::Null,
        )
        .await;
        assert_eq!(caps["active_run_id"], "run_current");
        assert_eq!(
            caps["commands"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == "cancel")
                .unwrap()["available"],
            true
        );
        assert_eq!(
            request(
                s.clone(),
                "POST",
                "/api/sessions/ses_test/commands",
                json!({"command":"/cancel","expected_run_id":"run_old"})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert!(!guard.token.is_cancelled());
        let (code, body) = request(
            s.clone(),
            "POST",
            "/api/sessions/ses_test/commands",
            json!({"command":"/cancel","expected_run_id":"run_current"}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["action"], "cancel_requested");
        assert_eq!(body["data"]["run_id"], "run_current");
        tokio::time::timeout(std::time::Duration::from_secs(1), guard.token.cancelled())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("session.json")).unwrap(),
            original
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(guard);
        let (_, caps) = request(s, "GET", "/api/sessions/ses_test/controls", Value::Null).await;
        assert_eq!(
            caps["commands"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == "cancel")
                .unwrap()["available"],
            false
        );
    }
    #[tokio::test]
    async fn lifecycle_and_client_commands_keep_their_existing_behavior() {
        for (command, expected_action, expected_status, expected_calls) in [
            ("/detach", Some("detach"), StatusCode::OK, 0),
            ("/help", None, StatusCode::OK, 0),
            ("/stop", None, StatusCode::OK, 1),
            ("/quit", Some("detach"), StatusCode::OK, 1),
            ("/compact", Some("compact_started"), StatusCode::ACCEPTED, 1),
        ] {
            let (_tmp, s, calls) = fixture();
            let s = s.with_launcher(Some(Arc::new(Launcher)));
            let (code, body) = request(
                s,
                "POST",
                "/api/sessions/ses_test/commands",
                json!({"command":command}),
            )
            .await;
            assert_eq!(code, expected_status, "{command}: {body}");
            assert_eq!(body["action"].as_str(), expected_action);
            assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        }
    }
}
