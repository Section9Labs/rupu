//! Archived standalone transcripts: where `rupu transcript archive` puts
//! them, the stream-only run dirs that follow them, and the prune that
//! `rupu transcript prune`, `rupu cleanup` and the control plane's
//! `POST /api/maintenance/prune` share.

use crate::runs::RunStore;
use chrono::{DateTime, Utc};
use rupu_runtime::retention::parse_retention_duration;
pub use rupu_runtime::retention::PruneError;
pub use rupu_runtime::standalone_run_metadata::archived_transcripts_dir;
use rupu_runtime::standalone_run_metadata::{
    metadata_path_for_run, read_metadata, StandaloneRunMetadata,
};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// `rupu run` writes its coverage stream to `<global>/runs/<run_id>/` from the
/// moment it starts — and, when a coordinator launched it, its netflow
/// stream `netflow.jsonl` (spec 2026-10-14 remote netflow transport §N1),
/// each with its `.lock` — but only a run that finishes writes `run.json`
/// there — and with it joins `RunStore`, which archives and deletes it. A run
/// dir WITHOUT `run.json` (a launch that failed its pre-flight, a killed run)
/// belongs to nothing but the transcript, so it follows the transcript: moved
/// on archive, removed on prune and delete. The rule is the same whichever
/// streams the dir holds. `None` for an id that is not a plain run id — it is
/// joined onto a path that may be removed recursively.
pub fn stream_only_run_dir(runs_root: &Path, run_id: &str) -> Option<PathBuf> {
    let plain = !run_id.is_empty()
        && run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    let dir = runs_root.join(run_id);
    (plain && dir.is_dir() && !dir.join("run.json").exists()).then_some(dir)
}

/// Remove `run_id`'s stream-only run dir from `runs/` and `runs-archive/`
/// (a transcript archived before run dirs followed it left its dir active).
pub fn remove_stream_only_run_dirs(global: &Path, run_id: &str) -> std::io::Result<()> {
    let store = RunStore::new(global.join("runs"));
    for root in [store.root.clone(), store.archive_root()] {
        if let Some(dir) = stream_only_run_dir(&root, run_id) {
            fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

/// One archived transcript and its metadata path.
#[derive(Debug, Clone)]
pub struct ArchivedTranscript {
    pub run_id: String,
    pub transcript_path: PathBuf,
    pub metadata_path: PathBuf,
}

/// Every archived transcript of the project layer (first) and the global
/// one, each file once.
pub fn scan_archived_transcripts(
    global: &Path,
    project_root: Option<&Path>,
) -> std::io::Result<Vec<ArchivedTranscript>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_dir = |root: &Path| -> std::io::Result<()> {
        if !root.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let dedupe_key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(dedupe_key) {
                continue;
            }
            let Some(run_id) = run_id_from_transcript_path(&path) else {
                continue;
            };
            let metadata_path = metadata_path_for_run(root, &run_id);
            out.push(ArchivedTranscript {
                run_id,
                metadata_path,
                transcript_path: path,
            });
        }
        Ok(())
    };
    if let Some(project_root) = project_root {
        push_dir(&archived_transcripts_dir(
            &project_root.join(".rupu/transcripts"),
        ))?;
    }
    push_dir(&archived_transcripts_dir(&global.join("transcripts")))?;
    Ok(out)
}

fn run_id_from_transcript_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
}

fn load_metadata(path: &Path) -> Result<Option<StandaloneRunMetadata>, PruneError> {
    if !path.is_file() {
        return Ok(None);
    }
    read_metadata(path)
        .map(Some)
        .map_err(|e| PruneError::Failed(e.to_string()))
}

/// When the transcript was archived: its metadata's `archived_at`, else the
/// file's mtime.
fn archived_at(t: &ArchivedTranscript) -> Result<DateTime<Utc>, PruneError> {
    if let Some(metadata) = load_metadata(&t.metadata_path)? {
        if let Some(value) = metadata.archived_at.as_deref() {
            return DateTime::parse_from_rfc3339(value)
                .map(|at| at.with_timezone(&Utc))
                .map_err(|e| PruneError::Failed(e.to_string()));
        }
    }
    let modified = fs::metadata(&t.transcript_path)?.modified()?;
    Ok(DateTime::<Utc>::from(modified))
}

/// The cutoff of a transcript prune: now minus `older_than`, else minus the
/// global `[storage].archived_transcript_retention` (default `30d`).
pub fn transcript_prune_cutoff(
    global: &Path,
    older_than: Option<&str>,
) -> Result<DateTime<Utc>, PruneError> {
    let retention = match older_than {
        Some(value) => parse_retention_duration(value)?,
        None => {
            let path = global.join("config.toml");
            let cfg = rupu_config::layer_files_locked(rupu_config::LayerPaths::global_only(&path))
                .map_err(|e| PruneError::Config(e.to_string()))?;
            let value = cfg
                .storage
                .archived_transcript_retention
                .unwrap_or_else(|| "30d".to_string());
            parse_retention_duration(&value).map_err(|e| PruneError::Config(e.to_string()))?
        }
    };
    Ok(Utc::now() - retention)
}

/// One transcript a prune removed, or would remove.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptPruneRow {
    pub run_id: String,
    /// `global` or `project`.
    pub scope: String,
    pub location: String,
    pub archived_at: String,
    /// `would_delete` or `deleted`.
    pub action: String,
}

/// `rupu transcript prune`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptPruneReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<TranscriptPruneRow>,
}

/// Delete (or, with `dry_run`, only list) every archived standalone
/// transcript archived before the cutoff ([`transcript_prune_cutoff`]),
/// with its metadata and stream-only run dirs. A transcript whose metadata
/// names a session belongs to that session and is left alone. Rows are
/// sorted by run id.
pub fn prune_archived_transcripts(
    global: &Path,
    project_root: Option<&Path>,
    older_than: Option<&str>,
    dry_run: bool,
) -> Result<TranscriptPruneReport, PruneError> {
    let cutoff = transcript_prune_cutoff(global, older_than)?;
    let mut rows = Vec::new();
    for t in scan_archived_transcripts(global, project_root)? {
        let archived_at = archived_at(&t)?;
        if archived_at > cutoff {
            continue;
        }
        let metadata = load_metadata(&t.metadata_path)?;
        if metadata
            .as_ref()
            .and_then(|m| m.session_id.as_ref())
            .is_some()
        {
            continue;
        }
        let scope = if t.transcript_path.starts_with(global.join("transcripts")) {
            "global"
        } else {
            "project"
        };
        rows.push(TranscriptPruneRow {
            run_id: t.run_id.clone(),
            scope: scope.to_string(),
            location: t.transcript_path.display().to_string(),
            archived_at: archived_at.to_rfc3339(),
            action: if dry_run { "would_delete" } else { "deleted" }.into(),
        });
        if !dry_run {
            remove_stream_only_run_dirs(global, &t.run_id)?;
            remove_file_if_exists(&t.transcript_path)?;
            remove_file_if_exists(&t.metadata_path)?;
        }
    }
    rows.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(TranscriptPruneReport {
        kind: "transcript_prune",
        version: 1,
        rows,
    })
}

fn remove_file_if_exists(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(global: &Path, run_id: &str, archived_at: Option<&str>, session: Option<&str>) {
        let dir = archived_transcripts_dir(&global.join("transcripts"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{run_id}.jsonl")), "{}\n").unwrap();
        let meta = StandaloneRunMetadata {
            version: StandaloneRunMetadata::VERSION,
            run_id: run_id.into(),
            session_id: session.map(Into::into),
            archived_at: archived_at.map(Into::into),
            workspace_path: global.to_path_buf(),
            project_root: None,
            repo_ref: None,
            issue_ref: None,
            backend_id: "local_checkout".into(),
            worker_id: None,
            trigger_source: "run_cli".into(),
            target: None,
            workspace_strategy: None,
            pid: None,
        };
        rupu_runtime::standalone_run_metadata::write_metadata(
            &metadata_path_for_run(&dir, run_id),
            &meta,
        )
        .unwrap();
    }

    #[test]
    fn a_dry_run_lists_old_standalone_transcripts_and_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        archive(g, "run_old", Some("2020-01-01T00:00:00Z"), None);
        archive(g, "run_new", Some(&Utc::now().to_rfc3339()), None);
        archive(g, "run_sess", Some("2020-01-01T00:00:00Z"), Some("ses_1"));

        let dry = prune_archived_transcripts(g, None, Some("30d"), true).unwrap();
        let ids: Vec<_> = dry
            .rows
            .iter()
            .map(|r| (r.run_id.as_str(), r.action.as_str()))
            .collect();
        assert_eq!(ids, [("run_old", "would_delete")]);
        let old = archived_transcripts_dir(&g.join("transcripts")).join("run_old.jsonl");
        assert!(old.exists());

        let done = prune_archived_transcripts(g, None, Some("30d"), false).unwrap();
        assert_eq!(done.rows[0].action, "deleted");
        assert!(!old.exists());
    }

    #[test]
    fn a_bad_argument_and_a_bad_config_are_different_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        assert!(matches!(
            prune_archived_transcripts(g, None, Some("soon"), true),
            Err(PruneError::InvalidRetention(_))
        ));
        fs::write(
            g.join("config.toml"),
            "[storage]\narchived_transcript_retention = \"soon\"\n",
        )
        .unwrap();
        assert!(matches!(
            prune_archived_transcripts(g, None, None, true),
            Err(PruneError::Config(_))
        ));
    }
}
