use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_orchestrator::Workflow;
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/autoflows", get(list_autoflow_defs))
}

/// Slim DTO for a single autoflow-enabled workflow definition.
#[derive(Serialize)]
struct AutoflowDefRow {
    name: String,
    /// `TriggerKind` as a lowercase string: `"manual"`, `"cron"`, or `"event"`.
    trigger: String,
    /// Phase-1: always `"global"` (project-local workflows are out of scope).
    scope: &'static str,
}

/// `GET /api/autoflows`
///
/// Scans `<global>/workflows/*.yaml`, parses each with [`Workflow::parse`],
/// keeps only those where `autoflow.enabled == true` (matching the CLI's
/// `autoflow list` predicate), and returns them sorted by name.
///
/// A missing workflows directory → `[]` (not an error).
/// An unparseable YAML file is skipped with a `tracing::warn!`.
async fn list_autoflow_defs(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<AutoflowDefRow>>> {
    let dir = s.global_dir.join("workflows");
    if !dir.is_dir() {
        return Ok(Json(vec![]));
    }

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("autoflows: could not read workflows dir: {err}");
            return Ok(Json(vec![]));
        }
    };

    let mut rows: Vec<AutoflowDefRow> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext == "yaml" || ext == "yml")
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let path = e.path();
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!("autoflows: could not read {}: {err}", path.display());
                    return None;
                }
            };
            let workflow = match Workflow::parse(&body) {
                Ok(w) => w,
                Err(err) => {
                    tracing::warn!("autoflows: skipping {}: {err}", path.display());
                    return None;
                }
            };
            // Mirror the CLI's `autoflow list` predicate exactly:
            // `workflow.autoflow.as_ref().map(|a| a.enabled).unwrap_or(false)`
            if !workflow
                .autoflow
                .as_ref()
                .map(|a| a.enabled)
                .unwrap_or(false)
            {
                return None;
            }
            let trigger = match workflow.trigger.on {
                rupu_orchestrator::TriggerKind::Manual => "manual",
                rupu_orchestrator::TriggerKind::Cron => "cron",
                rupu_orchestrator::TriggerKind::Event => "event",
            }
            .to_string();
            Some(AutoflowDefRow {
                name: workflow.name,
                trigger,
                scope: "global",
            })
        })
        .collect();

    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(rows))
}
