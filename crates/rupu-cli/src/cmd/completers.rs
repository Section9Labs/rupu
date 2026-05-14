//! Dynamic value completers wired onto positional arguments via
//! `#[arg(add = ArgValueCompleter::new(...))]`. Reached at completion
//! time through `clap_complete::CompleteEnv` (see `lib.rs::run`).
//!
//! These helpers walk the relevant directories cheaply (no frontmatter
//! parse, just basename collection) so tab-completion stays snappy
//! even on large agent / workflow sets.

use clap_complete::CompletionCandidate;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Agent name completer for `rupu agent show / edit` and the like.
/// Lists `.md` basenames from `<global>/agents/` and `.rupu/agents/`
/// (project shadow), de-duplicated.
pub fn agent_names(current: &OsStr) -> Vec<CompletionCandidate> {
    list_basenames("agents", "md", current)
}

/// Workflow name completer for `rupu workflow show / run / edit`
/// and `rupu issues run`. Lists `.yaml` and `.yml` basenames from
/// the same global + project layers.
pub fn workflow_names(current: &OsStr) -> Vec<CompletionCandidate> {
    let mut out = list_basenames("workflows", "yaml", current);
    out.extend(list_basenames("workflows", "yml", current));
    dedupe_in_place(&mut out);
    out
}

/// Active session id completer.
pub fn active_session_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    list_session_ids(current, SessionCompletionScope::Active)
}

/// Archived session id completer.
pub fn archived_session_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    list_session_ids(current, SessionCompletionScope::Archived)
}

/// All known session ids.
pub fn session_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    list_session_ids(current, SessionCompletionScope::All)
}

/// Any transcript run id, including session-backed transcripts.
pub fn transcript_run_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    list_transcript_run_ids(current, TranscriptCompletionKind::All)
}

/// Standalone transcript run ids only.
pub fn standalone_transcript_run_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    list_transcript_run_ids(current, TranscriptCompletionKind::Standalone)
}

#[derive(Clone, Copy)]
enum SessionCompletionScope {
    Active,
    Archived,
    All,
}

#[derive(Clone, Copy)]
enum TranscriptCompletionKind {
    All,
    Standalone,
}

/// Walk `<global>/<subdir>` and `<cwd>/.rupu/<subdir>` collecting
/// basenames of files with `ext`. Any IO error along the way returns
/// an empty list (completion must never panic / abort).
fn list_basenames(subdir: &str, ext: &str, current: &OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let mut names: Vec<String> = Vec::new();

    if let Some(global) = global_dir() {
        push_basenames(&global.join(subdir), ext, &mut names);
    }
    if let Some(project) = project_root() {
        push_basenames(&project.join(".rupu").join(subdir), ext, &mut names);
    }

    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|n| n.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

fn push_basenames(dir: &Path, ext: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(ext) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.push(stem.to_string());
    }
}

/// Resolve the user's global rupu directory (`~/.rupu` by default,
/// overridable by `$RUPU_HOME`). Mirrors `paths::global_dir` but
/// returns `None` instead of erroring — completion must degrade
/// gracefully when the dir is missing.
fn global_dir() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("RUPU_HOME") {
        return Some(PathBuf::from(custom));
    }
    dirs::home_dir().map(|h| h.join(".rupu"))
}

/// Walk up from cwd looking for a directory containing `.rupu/`.
/// Cheap stat-walk; bails at filesystem root.
fn project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut cur = cwd.as_path();
    loop {
        if cur.join(".rupu").is_dir() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return None,
        }
    }
}

fn dedupe_in_place(items: &mut Vec<CompletionCandidate>) {
    let mut seen = std::collections::BTreeSet::new();
    items.retain(|c| seen.insert(c.get_value().to_os_string()));
}

fn list_session_ids(current: &OsStr, scope: SessionCompletionScope) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let Some(global) = global_dir() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    match scope {
        SessionCompletionScope::Active => push_session_ids(&global.join("sessions"), &mut names),
        SessionCompletionScope::Archived => {
            push_session_ids(&global.join("sessions-archive"), &mut names)
        }
        SessionCompletionScope::All => {
            push_session_ids(&global.join("sessions"), &mut names);
            push_session_ids(&global.join("sessions-archive"), &mut names);
        }
    }
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|name| name.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

fn push_session_ids(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("session.json").is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        out.push(name.to_string());
    }
}

fn list_transcript_run_ids(
    current: &OsStr,
    kind: TranscriptCompletionKind,
) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let mut names = Vec::new();

    if let Some(project) = project_root() {
        let active = project.join(".rupu/transcripts");
        push_transcript_run_ids(&active, &mut names, kind);
        push_transcript_run_ids(&active.join("archive"), &mut names, kind);
    }
    if let Some(global) = global_dir() {
        let active = global.join("transcripts");
        push_transcript_run_ids(&active, &mut names, kind);
        push_transcript_run_ids(&active.join("archive"), &mut names, kind);
    }

    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|name| name.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

fn push_transcript_run_ids(dir: &Path, out: &mut Vec<String>, kind: TranscriptCompletionKind) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(run_id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if matches!(kind, TranscriptCompletionKind::Standalone)
            && transcript_owned_by_session(dir, run_id)
        {
            continue;
        }
        out.push(run_id.to_string());
    }
}

fn transcript_owned_by_session(dir: &Path, run_id: &str) -> bool {
    let metadata = crate::standalone_run_metadata::metadata_path_for_run(dir, run_id);
    if !metadata.is_file() {
        return false;
    }
    crate::standalone_run_metadata::read_metadata(&metadata)
        .ok()
        .and_then(|value| value.session_id)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn session_completers_list_active_and_archived_ids() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join("sessions/ses_active")).unwrap();
        std::fs::write(home.join("sessions/ses_active/session.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join("sessions-archive/ses_archived")).unwrap();
        std::fs::write(
            home.join("sessions-archive/ses_archived/session.json"),
            "{}",
        )
        .unwrap();
        std::env::set_var("RUPU_HOME", &home);

        let active = active_session_ids(OsStr::new("ses_"));
        let archived = archived_session_ids(OsStr::new("ses_"));
        let all = session_ids(OsStr::new("ses_"));

        assert_eq!(active.len(), 1);
        assert_eq!(active[0].get_value(), OsStr::new("ses_active"));
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].get_value(), OsStr::new("ses_archived"));
        assert_eq!(all.len(), 2);
        std::env::remove_var("RUPU_HOME");
    }

    #[test]
    fn transcript_completer_filters_session_owned_for_standalone() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let old_cwd = std::env::current_dir().unwrap();
        std::fs::create_dir_all(project.join(".rupu/transcripts")).unwrap();
        std::fs::write(project.join(".rupu/transcripts/run_free.jsonl"), "{}\n").unwrap();
        std::fs::write(project.join(".rupu/transcripts/run_owned.jsonl"), "{}\n").unwrap();
        let owned_meta = crate::standalone_run_metadata::StandaloneRunMetadata {
            version: crate::standalone_run_metadata::StandaloneRunMetadata::VERSION,
            run_id: "run_owned".into(),
            session_id: Some("ses_123".into()),
            archived_at: None,
            workspace_path: project.clone(),
            project_root: Some(project.clone()),
            repo_ref: None,
            issue_ref: None,
            backend_id: "local".into(),
            worker_id: None,
            trigger_source: "test".into(),
            target: None,
            workspace_strategy: None,
        };
        crate::standalone_run_metadata::write_metadata(
            &crate::standalone_run_metadata::metadata_path_for_run(
                &project.join(".rupu/transcripts"),
                "run_owned",
            ),
            &owned_meta,
        )
        .unwrap();
        std::env::set_var("RUPU_HOME", &home);
        std::env::set_current_dir(&project).unwrap();

        let all = transcript_run_ids(OsStr::new("run_"));
        let standalone = standalone_transcript_run_ids(OsStr::new("run_"));

        assert_eq!(all.len(), 2);
        assert_eq!(standalone.len(), 1);
        assert_eq!(standalone[0].get_value(), OsStr::new("run_free"));
        std::env::set_current_dir(old_cwd).unwrap();
        std::env::remove_var("RUPU_HOME");
    }
}
