//! `rupu cleanup`: archived sessions ([`rupu_runtime::sessions`]) and
//! archived standalone transcripts ([`crate::archived_transcripts`]) pruned
//! together, one report. The control plane's `POST /api/maintenance/prune
//! {kind: cleanup}` answers with the same rows. It lives here, not beside
//! the session prune in `rupu-runtime`, because the transcript prune is
//! this crate's.

use crate::archived_transcripts::prune_archived_transcripts;
use rupu_runtime::retention::PruneError;
use rupu_runtime::sessions::prune_archived_sessions;
use serde::Serialize;
use std::path::Path;

/// One archived session or transcript a cleanup removed, or would remove.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupRow {
    /// `session` or `transcript`.
    pub kind: String,
    pub id: String,
    pub scope: String,
    pub retained_at: String,
    /// `would_delete` or `deleted`.
    pub action: String,
    pub detail: String,
}

/// `rupu cleanup`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<CleanupRow>,
}

/// A cleanup stage that failed: which one (`cleanup archived sessions` /
/// `cleanup archived transcripts`), over the prune's own error.
#[derive(Debug, thiserror::Error)]
#[error("{stage}")]
pub struct CleanupError {
    pub stage: &'static str,
    #[source]
    pub source: PruneError,
}

/// What a cleanup prunes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupSelection {
    pub sessions: bool,
    pub transcripts: bool,
}

impl CleanupSelection {
    /// Archived sessions and transcripts both.
    pub const ALL: Self = Self {
        sessions: true,
        transcripts: true,
    };
}

/// Prune (or, with `dry_run`, only list) the selected archived resources
/// older than `older_than` (else each one's `[storage]` retention): the
/// sessions first, then the transcripts of `global` and `project_root`.
/// Rows sorted by kind, id, scope. A session record a newer rupu wrote is
/// kept, its warning given to `on_warning`.
pub fn cleanup_rows(
    global: &Path,
    project_root: Option<&Path>,
    older_than: Option<&str>,
    dry_run: bool,
    selection: CleanupSelection,
    on_warning: &mut dyn FnMut(&str),
) -> Result<Vec<CleanupRow>, CleanupError> {
    let mut rows = Vec::new();
    if selection.sessions {
        let sessions =
            prune_archived_sessions(global, older_than, dry_run, on_warning).map_err(|source| {
                CleanupError {
                    stage: "cleanup archived sessions",
                    source,
                }
            })?;
        for session in sessions {
            rows.push(CleanupRow {
                kind: "session".into(),
                id: session.session_id,
                scope: session.scope,
                retained_at: session.updated_at,
                action: session.action,
                detail: format!("status {}", session.status),
            });
        }
    }
    if selection.transcripts {
        let transcripts = prune_archived_transcripts(global, project_root, older_than, dry_run)
            .map_err(|source| CleanupError {
                stage: "cleanup archived transcripts",
                source,
            })?;
        for transcript in transcripts.rows {
            rows.push(CleanupRow {
                kind: "transcript".into(),
                id: transcript.run_id,
                scope: transcript.scope,
                retained_at: transcript.archived_at,
                action: transcript.action,
                detail: transcript.location,
            });
        }
    }
    rows.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.scope.cmp(&b.scope))
    });
    Ok(rows)
}

/// `rupu cleanup`'s report: [`cleanup_rows`] as `{kind: "cleanup",
/// version: 1, rows}`.
pub fn cleanup_report(
    global: &Path,
    project_root: Option<&Path>,
    older_than: Option<&str>,
    dry_run: bool,
    selection: CleanupSelection,
    on_warning: &mut dyn FnMut(&str),
) -> Result<CleanupReport, CleanupError> {
    Ok(CleanupReport {
        kind: "cleanup",
        version: 1,
        rows: cleanup_rows(
            global,
            project_root,
            older_than,
            dry_run,
            selection,
            on_warning,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed stage reads as the CLI printed it (`cleanup archived …:
    /// <error>`) and keeps the prune's error for a status.
    #[test]
    fn a_failed_stage_names_itself_over_the_prune_error() {
        let tmp = tempfile::tempdir().unwrap();
        let chain = |e: &CleanupError| format!("{e}: {}", e.source);
        let err = cleanup_rows(
            tmp.path(),
            None,
            Some("soon"),
            true,
            CleanupSelection::ALL,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err.source, PruneError::InvalidRetention(_)));
        assert_eq!(
            chain(&err),
            "cleanup archived sessions: invalid duration `soon`: invalid digit found in string"
        );
        let err = cleanup_rows(
            tmp.path(),
            None,
            Some("soon"),
            true,
            CleanupSelection {
                sessions: false,
                transcripts: true,
            },
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(err.stage, "cleanup archived transcripts");
    }
}
