//! `POST /api/findings/import`: the HTTP side of
//! [`rupu_findings_report::import_files`], the run `rupu findings import`
//! makes.

use crate::api::local_only::LocalOnlyQuery;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use rupu_findings_report::import_files::{ImportPlan, ImportReport, ImportRequest};
use serde::Deserialize;
use std::path::PathBuf;

/// The most directory entries one import may search: a request names paths
/// on this machine, so a broad one (`/`) is refused rather than walked.
pub const API_MAX_ENTRIES: usize = 100_000;

/// `POST /api/findings/import` body.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportBody {
    /// Absolute paths on this machine: report files, or directories searched
    /// for `*.md` files.
    paths: Vec<String>,
    /// The finding a single report belongs to, for a report with no
    /// `Finding ID:` line. Only with one file, and refused when the report's
    /// own `Finding ID:` line names a different finding.
    #[serde(default)]
    id: Option<String>,
    /// Parse and check every report; write nothing.
    #[serde(default)]
    dry_run: bool,
}

/// `POST /api/findings/import` — [`ImportPlan`] over this control
/// plane's registered workspaces. Local-only. `200` with the report (a file
/// that failed is a `failed` line in it, not an error status); `400` for a
/// relative or missing path, a directory that cannot be listed, a search
/// past [`API_MAX_ENTRIES`], an `id` with more than one file, or nothing to
/// import; `501` for a remote `host`, or a real import on a control plane
/// that is not `rupu cp serve`.
pub(crate) async fn import_handler(
    State(s): State<AppState>,
    Query(q): Query<LocalOnlyQuery>,
    Json(body): Json<ImportBody>,
) -> ApiResult<Json<ImportReport>> {
    q.require_local("importing findings")?;
    if !body.dry_run {
        crate::api::config::require_writable_to(&s, "importing findings")?;
    }
    if body.paths.is_empty() {
        return Err(ApiError::bad_request("`paths` names no report"));
    }
    let mut paths = Vec::with_capacity(body.paths.len());
    for p in &body.paths {
        let path = PathBuf::from(p);
        if !path.is_absolute() {
            return Err(ApiError::bad_request(format!(
                "`{p}` is not an absolute path on this machine"
            )));
        }
        paths.push(path);
    }
    let id = match body.id.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::bad_request("`id` is blank")),
        other => other.map(str::to_string),
    };
    let req = ImportRequest {
        paths,
        id,
        dry_run: body.dry_run,
        id_flag: "`id`",
        max_entries: Some(API_MAX_ENTRIES),
    };
    let global = s.global_dir.clone();
    tokio::task::spawn_blocking(move || import(&global, req))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map(Json)
}

/// Find, then import, into the ledgers registered under `global`, with the
/// global config's limits. A refused search is a 400.
fn import(global: &std::path::Path, req: ImportRequest) -> ApiResult<ImportReport> {
    let plan = ImportPlan::find(req).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let (opts, warning) = rupu_runtime::findings_opts::global_options(global);
    let ledgers = crate::api::findings::finding_ledgers(global);
    let mut report = plan.run(&ledgers, &opts);
    report.warnings.extend(warning);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_past_the_cap_is_a_400_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        for f in ["a.md", "b.md", "c.md"] {
            std::fs::write(reports.join(f), "x").unwrap();
        }
        let req = |max_entries| ImportRequest {
            paths: vec![reports.clone()],
            id: None,
            dry_run: true,
            id_flag: "`id`",
            max_entries: Some(max_entries),
        };
        let err = import(tmp.path(), req(2)).unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            err.1,
            "the search passed 2 directory entries; name narrower paths"
        );
        // Within the cap the search goes on: three files, none a report.
        let report = import(tmp.path(), req(3)).unwrap();
        assert_eq!(report.skipped, 3);
    }
}
