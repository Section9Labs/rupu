//! Spend sources outside the [`RunStore`] for the aggregate usage surfaces
//! (spec 2026-09-29 §5.2): standalone agent runs (`<global>/transcripts/
//! *.jsonl`, non-recursive — `archive/` excluded) and session turns
//! (`session.json` `runs[].transcript_path`).
//!
//! Every transcript is counted once across an aggregate, attributed
//! workflow run > session > standalone: the caller passes the transcripts
//! every workflow run already claims ([`claimed_transcripts`]); a session's
//! `runs[]` is walked before the standalone scan, so a session turn's
//! transcript is a session source even when it also sits in the global
//! transcripts dir.
//!
//! Each source covers its own transcript plus its recursively dispatched
//! sub-runs (`<runs>/<run_id>/sub/…`, [`with_dispatch_children`]). Usage is
//! never computed here — callers fold `paths` through
//! [`crate::usage::transcripts_usage`]. Discovery is best-effort: an
//! unreadable directory, meta or session file contributes nothing, never an
//! error.

use crate::api::run_streams::{try_load_session_for_runs, StandaloneMetaDto};
use crate::usage_index::UsageIndex;
use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::RunStore;
use rupu_orchestrator::RunRecord;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Which kind of run a spend source is (`UsageRunRow.kind` on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Workflow,
    Agent,
    Session,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Workflow => "workflow",
            SourceKind::Agent => "agent",
            SourceKind::Session => "session",
        }
    }
}

/// A non-RunStore spend source.
#[derive(Debug, Clone)]
pub struct ExtraSource {
    /// `Agent` | `Session`.
    pub kind: SourceKind,
    /// Run id (the transcript key).
    pub id: String,
    pub session_id: Option<String>,
    /// The transcript's `RunStart` time (a session turn falls back to its
    /// `session.json` record). `None` = unknown; windowed aggregates skip it.
    pub started_at: Option<DateTime<Utc>>,
    /// The transcript's `RunStart` agent (a session turn falls back to the
    /// session's agent); empty when neither is known.
    pub agent: String,
    /// The transcript's `RunStart` workspace; empty when unknown.
    pub workspace_id: String,
    /// Own transcript + recursive dispatch sub-runs, each labelled with `id`.
    pub paths: Vec<(String, PathBuf)>,
}

/// The run-start facts a source is dated and attributed by.
#[derive(Debug, Clone, Default)]
struct Head {
    agent: String,
    workspace_id: String,
    started_at: Option<DateTime<Utc>>,
}

/// A path's canonical form, or the path itself when it can't be resolved.
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A run id we are willing to join onto `<runs>/<id>/sub/` (no separators,
/// no `..`).
fn safe_run_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A transcript's `RunStart` head. Found heads never change, so they are
/// cached process-wide; a file with no `RunStart` yet is re-read next time.
fn head_of(path: &Path) -> Head {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Head>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(h) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(path) {
        return h.clone();
    }
    let Ok(h) = rupu_transcript::JsonlReader::head(path) else {
        return Head::default();
    };
    let head = Head {
        agent: h.agent,
        workspace_id: h.workspace_id,
        started_at: Some(h.started_at),
    };
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(path.to_path_buf(), head.clone());
    head
}

/// `own` labelled `run_id`, plus every transcript of `run_id`'s recursive
/// dispatch sub-run tree (`<runs>/<run_id>/sub/…`) that exists, labelled the
/// same. The one definition of "an agent run's transcripts" for the agent
/// runs list, session usage and the aggregates.
pub fn with_dispatch_children(
    run_store: &RunStore,
    run_id: &str,
    own: &Path,
) -> Vec<(String, PathBuf)> {
    let mut out = vec![(run_id.to_string(), own.to_path_buf())];
    if safe_run_id(run_id) {
        for kt in run_store.dispatched_transcripts([run_id]) {
            if kt.path.is_file() {
                out.push((run_id.to_string(), kt.path));
            }
        }
    }
    out
}

/// Every transcript a workflow run claims — the resolved known set of each
/// of `runs` (active) and of every archived run next to `run_store` — as
/// both the resolved path and its canonical form, for [`extra_sources`].
/// Archived runs claim too: archiving moves the run directory but not its
/// transcripts, which must not turn into standalone spend.
pub fn claimed_transcripts(run_store: &RunStore, runs: &[RunRecord]) -> HashSet<PathBuf> {
    let index = UsageIndex::global();
    let mut claimed = HashSet::new();
    let mut claim = |store: &RunStore, id: &str| {
        for p in index.resolved_transcripts(store, id) {
            if p.exists() {
                claimed.insert(canon(&p));
            }
            claimed.insert(p);
        }
    };
    for r in runs {
        claim(run_store, &r.id);
    }
    let archive = RunStore::new(run_store.root.with_file_name("runs-archive"));
    for r in archive.list().unwrap_or_default() {
        claim(&archive, &r.id);
    }
    claimed
}

/// Standalone + session transcripts under `global`, EXCLUDING any path in
/// `claimed` (compared as given and canonicalized). Only transcripts that
/// exist are sources. Session turns (`session.json` `runs[]`, active and
/// archived sessions) come first; then each remaining
/// `<global>/transcripts/*.jsonl` is a `Session` when its `.meta.json`
/// names a session (or `trigger_source == "session_turn"`), else an `Agent`.
pub fn extra_sources(
    global: &Path,
    run_store: &RunStore,
    claimed: &HashSet<PathBuf>,
) -> Vec<ExtraSource> {
    let is_claimed = |p: &Path| claimed.contains(p) || claimed.contains(&canon(p));
    // Canonical own paths already attributed to a source.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();

    // 1. Session turns.
    for root in [global.join("sessions"), global.join("sessions-archive")] {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(session) = try_load_session_for_runs(&dir.join("session.json")) else {
                continue;
            };
            for run in session.runs {
                let Some(tp) = run.transcript_path.filter(|t| !t.is_empty()) else {
                    continue;
                };
                let own = PathBuf::from(tp);
                if !own.is_file() || is_claimed(&own) || !seen.insert(canon(&own)) {
                    continue;
                }
                let head = head_of(&own);
                let id = if run.run_id.is_empty() {
                    rupu_transcript::transcript_key(&own).unwrap_or_default()
                } else {
                    run.run_id
                };
                let started_at = head.started_at.or_else(|| {
                    run.started_at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                });
                let agent = if head.agent.is_empty() {
                    session.agent_name.clone().unwrap_or_default()
                } else {
                    head.agent
                };
                let paths = source_paths(run_store, &id, &own, &is_claimed);
                out.push(ExtraSource {
                    kind: SourceKind::Session,
                    id,
                    session_id: session.session_id.clone(),
                    started_at,
                    agent,
                    workspace_id: head.workspace_id,
                    paths,
                });
            }
        }
    }

    // 2. Standalone transcripts (and session turns not listed in any
    //    `session.json`), classified by their meta sidecar.
    let tdir = global.join("transcripts");
    let Ok(entries) = std::fs::read_dir(&tdir) else {
        return out;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".jsonl") && !n.ends_with(".meta.json"))
        })
        .collect();
    files.sort();
    for own in files {
        if is_claimed(&own) || !seen.insert(canon(&own)) {
            continue;
        }
        let stem = own
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let meta = std::fs::read_to_string(tdir.join(format!("{stem}.meta.json")))
            .ok()
            .and_then(|t| serde_json::from_str::<StandaloneMetaDto>(&t).ok());
        let (run_id, session_id, trigger) = match meta {
            Some(m) => (m.run_id, m.session_id, m.trigger_source),
            None => (String::new(), None, None),
        };
        let kind = if session_id.is_some() || trigger.as_deref() == Some("session_turn") {
            SourceKind::Session
        } else {
            SourceKind::Agent
        };
        let id = if run_id.is_empty() { stem } else { run_id };
        let head = head_of(&own);
        let paths = source_paths(run_store, &id, &own, &is_claimed);
        out.push(ExtraSource {
            kind,
            id,
            session_id,
            started_at: head.started_at,
            agent: head.agent,
            workspace_id: head.workspace_id,
            paths,
        });
    }
    out
}

/// [`with_dispatch_children`] minus any child a workflow run already claims.
fn source_paths(
    run_store: &RunStore,
    id: &str,
    own: &Path,
    is_claimed: &dyn Fn(&Path) -> bool,
) -> Vec<(String, PathBuf)> {
    let mut paths = with_dispatch_children(run_store, id, own);
    let children = paths.split_off(1);
    paths.extend(children.into_iter().filter(|(_, p)| !is_claimed(p)));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(path: &Path, agent: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ev = rupu_transcript::Event::RunStart {
            run_id: "r".into(),
            workspace_id: "ws".into(),
            agent: agent.into(),
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: Utc::now(),
            mode: rupu_transcript::RunMode::Ask,
            schema: None,
            system_prompt: None,
        };
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        std::fs::write(path, line).unwrap();
    }

    #[test]
    fn scan_skips_meta_archive_and_claimed_and_walks_children() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path();
        let store = RunStore::new(global.join("runs"));
        let tdir = global.join("transcripts");
        transcript(&tdir.join("run_A.jsonl"), "lead");
        std::fs::write(tdir.join("run_A.meta.json"), r#"{"run_id":"run_A"}"#).unwrap();
        transcript(&tdir.join("archive").join("run_OLD.jsonl"), "old");
        transcript(&tdir.join("run_C.jsonl"), "claimed");
        let child = global.join("runs/run_A/sub/sub_1/transcript.jsonl");
        transcript(&child, "helper");
        // A sub-run directory without a transcript contributes nothing.
        std::fs::create_dir_all(global.join("runs/run_A/sub/sub_2")).unwrap();

        let claimed: HashSet<PathBuf> = [tdir.join("run_C.jsonl")].into_iter().collect();
        let got = extra_sources(global, &store, &claimed);
        assert_eq!(got.len(), 1, "{got:?}");
        let a = &got[0];
        assert_eq!(a.kind, SourceKind::Agent);
        assert_eq!(a.id, "run_A");
        assert_eq!(a.agent, "lead");
        assert_eq!(a.workspace_id, "ws");
        assert!(a.started_at.is_some());
        assert_eq!(
            a.paths,
            vec![
                ("run_A".to_string(), tdir.join("run_A.jsonl")),
                ("run_A".to_string(), child),
            ]
        );
    }

    #[test]
    fn a_session_turn_is_a_session_source_once() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path();
        let store = RunStore::new(global.join("runs"));
        let tdir = global.join("transcripts");
        let in_global = tdir.join("run_T.jsonl");
        transcript(&in_global, "chatter");
        // No meta sidecar: the session's `runs[]` alone makes it a session turn.
        let in_project = global.join("proj/.rupu/transcripts/run_P.jsonl");
        transcript(&in_project, "chatter");
        let sdir = global.join("sessions/ses_1");
        std::fs::create_dir_all(&sdir).unwrap();
        let session = serde_json::json!({
            "session_id": "ses_1",
            "agent_name": "chatter",
            "runs": [
                { "run_id": "run_T", "transcript_path": in_global },
                { "run_id": "run_P", "transcript_path": in_project },
                { "run_id": "run_GONE", "transcript_path": global.join("nope.jsonl") },
            ],
        });
        std::fs::write(sdir.join("session.json"), session.to_string()).unwrap();

        let got = extra_sources(global, &store, &HashSet::new());
        let mut ids: Vec<(&str, SourceKind, Option<&str>)> = got
            .iter()
            .map(|s| (s.id.as_str(), s.kind, s.session_id.as_deref()))
            .collect();
        ids.sort_by_key(|(id, ..)| *id);
        assert_eq!(
            ids,
            vec![
                ("run_P", SourceKind::Session, Some("ses_1")),
                ("run_T", SourceKind::Session, Some("ses_1")),
            ]
        );
    }

    #[test]
    fn unsafe_run_ids_never_walk_outside_the_run_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let own = tmp.path().join("t.jsonl");
        assert_eq!(with_dispatch_children(&store, "../x", &own).len(), 1);
        assert_eq!(with_dispatch_children(&store, "", &own).len(), 1);
    }
}
