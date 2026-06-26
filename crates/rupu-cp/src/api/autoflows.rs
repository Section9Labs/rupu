use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_orchestrator::Workflow;
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/autoflows", get(list_autoflow_defs))
}

/// Slim DTO for a single autoflow-enabled workflow definition.
#[derive(Serialize)]
pub(crate) struct AutoflowDefRow {
    pub(crate) name: String,
    /// File stem (e.g. `my-workflow` for `my-workflow.yaml`). The workflow
    /// detail route is keyed by file stem, not parsed `name`, so the frontend
    /// links to `/workflows/{slug}`.
    pub(crate) slug: String,
    /// `TriggerKind` as a lowercase string: `"manual"`, `"cron"`, or `"event"`.
    pub(crate) trigger: String,
    /// `"global"` or `"project"` depending on the layer the file came from.
    pub(crate) scope: &'static str,
}

/// Scan `<dir>/*.{yaml,yml}`, parse each, and keep only those whose
/// `autoflow.enabled == true` (matching the CLI's `autoflow list` predicate).
/// Each kept row is tagged with `scope`. A missing/unreadable dir → empty vec
/// (tolerated). Unparseable files are skipped with a `tracing::warn!`.
pub(crate) fn scan_autoflow_defs(
    dir: &std::path::Path,
    scope: &'static str,
) -> Vec<AutoflowDefRow> {
    if !dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("autoflows: could not read workflows dir: {err}");
            return vec![];
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
            let slug = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())?;
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
                slug,
                trigger,
                scope,
            })
        })
        .collect();

    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// `GET /api/autoflows`
///
/// Scans `<global>/workflows/*.yaml`, parses each with [`Workflow::parse`],
/// keeps only those where `autoflow.enabled == true` (matching the CLI's
/// `autoflow list` predicate), and returns them sorted by name.
///
/// A missing workflows directory → `[]` (not an error).
/// An unparseable YAML file is skipped with a `tracing::warn!`.
async fn list_autoflow_defs(State(s): State<AppState>) -> ApiResult<Json<Vec<AutoflowDefRow>>> {
    let dir = s.global_dir.join("workflows");
    Ok(Json(scan_autoflow_defs(&dir, "global")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn slug_is_file_stem_distinct_from_parsed_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        // File stem (`my-file-stem`) deliberately differs from the workflow's
        // parsed `name` (`parsed-name`), because the workflow detail route is
        // keyed by file stem while the autoflow's display name is the parsed
        // name. The row must carry both.
        let path = dir.path().join("my-file-stem.yaml");
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(
            b"name: parsed-name\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n",
        )
        .expect("write");

        let rows = scan_autoflow_defs(dir.path(), "global");
        assert_eq!(rows.len(), 1, "the enabled autoflow should be returned");
        assert_eq!(rows[0].name, "parsed-name");
        assert_eq!(rows[0].slug, "my-file-stem");
    }
}
