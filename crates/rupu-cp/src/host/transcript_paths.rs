//! Which transcript files a mirrored run claims, and how a remote transcript
//! path maps to a coordinator-local cache file (spec §3.1, §3.4). Shared by
//! the SSH read path's allowlist and the tail pump's terminal pull so the two
//! can never disagree about what a run owns.

use std::collections::BTreeSet;
use std::io::{BufRead as _, BufReader};
use std::path::{Component, Path, PathBuf};

use rupu_orchestrator::{executor::Event, runs::RunStore};

/// Every transcript path a mirrored run's own artifacts claim (§3.4):
/// step-result rows and their items, unit checkpoints, `StepWorking` /
/// `UnitStarted` events, and the record's active-step path. Deduplicated,
/// sorted, empty paths dropped. Missing artifacts contribute nothing.
pub fn recorded_transcript_paths(store: &RunStore, run_id: &str) -> Vec<PathBuf> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    if let Ok(rec) = store.load(run_id) {
        if let Some(p) = rec.active_step_transcript_path {
            out.insert(p);
        }
    }
    for sr in store.read_step_results(run_id).unwrap_or_default() {
        out.insert(sr.transcript_path.clone());
        for item in &sr.items {
            out.insert(item.transcript_path.clone());
        }
    }
    for cp in store.read_unit_checkpoints(run_id).unwrap_or_default() {
        out.insert(cp.transcript_path.clone());
    }
    if let Ok(file) = std::fs::File::open(store.events_path(run_id)) {
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Event>(&line) {
                Ok(Event::StepWorking {
                    transcript_path: Some(p),
                    ..
                }) => {
                    out.insert(p);
                }
                Ok(Event::UnitStarted {
                    transcript_path, ..
                }) => {
                    out.insert(transcript_path);
                }
                _ => {}
            }
        }
    }
    out.retain(|p| !p.as_os_str().is_empty());
    out.into_iter().collect()
}

/// A file-name component we are willing to turn into a cache key: ULID-style
/// run ids and sub-run ids only. Anything else is not a transcript we serve.
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// §3.1: `…/transcripts/<name>.jsonl` → `<name>`;
/// `…/runs/<parent>/sub_runs/<sub>/transcript.jsonl` → `<parent>__sub_runs__<sub>`.
/// `None` for anything else — relative paths, `..`, non-`.jsonl`, or a
/// directory shape this design does not serve.
pub fn cache_key(recorded: &Path) -> Option<String> {
    if !recorded.is_absolute() {
        return None;
    }
    if recorded.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return None;
    }
    if recorded
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return None;
    }
    let comps: Vec<&str> = recorded
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let n = comps.len();
    if n >= 2 && comps[n - 2] == "transcripts" {
        let stem = recorded.file_stem()?.to_str()?;
        return safe_component(stem).then(|| stem.to_string());
    }
    if n >= 4 && comps[n - 1] == "transcript.jsonl" && comps[n - 3] == "sub_runs" {
        let (parent, sub) = (comps[n - 4], comps[n - 2]);
        if safe_component(parent) && safe_component(sub) {
            return Some(format!("{parent}__sub_runs__{sub}"));
        }
    }
    None
}

/// `<global>/mirror/<host_id>/transcripts/<key>.jsonl`.
pub fn cache_path(global: &Path, host_id: &str, recorded: &Path) -> Option<PathBuf> {
    let key = cache_key(recorded)?;
    Some(
        global
            .join("mirror")
            .join(host_id)
            .join("transcripts")
            .join(format!("{key}.jsonl")),
    )
}

/// The `.complete` sidecar (§6.1) that marks a cache file authoritative.
pub fn complete_marker(cache: &Path) -> PathBuf {
    PathBuf::from(format!("{}.complete", cache.display()))
}

pub fn is_complete(cache: &Path) -> bool {
    complete_marker(cache).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::runs::{ItemResultRecord, StepKind, StepResultRecord, UnitCheckpoint};
    use rupu_orchestrator::{RunRecord, RunStatus};
    use std::io::Write as _;

    fn store() -> (RunStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        (RunStore::new(tmp.path().join("runs")), tmp)
    }

    fn record(id: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "wf".into(),
            status: RunStatus::Running,
            inputs: Default::default(),
            event: None,
            workspace_id: String::new(),
            workspace_path: PathBuf::from("."),
            transcript_dir: PathBuf::from("/remote/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: Some("host_abc".into()),
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: Some("build".into()),
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: Some(PathBuf::from(
                "/remote/proj/.rupu/transcripts/run_01ACTIVE.jsonl",
            )),
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
        }
    }

    #[test]
    fn recorded_paths_union_every_artifact_and_dedup() {
        let (store, _tmp) = store();
        let id = "run_01RECORDED";
        store.create(record(id), "").unwrap();
        store
            .append_step_result(
                id,
                &StepResultRecord {
                    step_id: "plan".into(),
                    run_id: "run_01PLAN".into(),
                    transcript_path: PathBuf::from(
                        "/remote/proj/.rupu/transcripts/run_01PLAN.jsonl",
                    ),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: StepKind::ForEach,
                    items: vec![ItemResultRecord {
                        index: 0,
                        item: serde_json::Value::Null,
                        sub_id: String::new(),
                        rendered_prompt: String::new(),
                        run_id: "run_01ITEM".into(),
                        transcript_path: PathBuf::from(
                            "/remote/proj/.rupu/transcripts/run_01ITEM.jsonl",
                        ),
                        output: String::new(),
                        success: true,
                        is_fixer: false,
                    }],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                    host: None,
                },
            )
            .unwrap();
        store
            .append_unit_checkpoint(
                id,
                &UnitCheckpoint {
                    step_id: "plan".into(),
                    index: 1,
                    item: serde_json::Value::Null,
                    run_id: "run_01CKPT".into(),
                    transcript_path: PathBuf::from(
                        "/remote/proj/.rupu/transcripts/run_01CKPT.jsonl",
                    ),
                    output: String::new(),
                    success: true,
                    finished_at: chrono::Utc::now(),
                    host: Some("host_abc".into()),
                },
            )
            .unwrap();
        // events.jsonl: a StepWorking with a path, one without, a UnitStarted,
        // and a duplicate of the step-result path.
        let mut f = std::fs::File::create(store.events_path(id)).unwrap();
        for ev in [
            Event::StepWorking {
                run_id: id.into(),
                step_id: "build".into(),
                note: None,
                transcript_path: Some(PathBuf::from(
                    "/remote/proj/.rupu/transcripts/run_01WORK.jsonl",
                )),
            },
            Event::StepWorking {
                run_id: id.into(),
                step_id: "build".into(),
                note: Some("tool".into()),
                transcript_path: None,
            },
            Event::UnitStarted {
                run_id: id.into(),
                step_id: "panel".into(),
                index: 0,
                unit_key: "a".into(),
                agent: None,
                transcript_path: PathBuf::from(
                    "/remote/.rupu/runs/run_01RECORDED/sub_runs/sub_01P/transcript.jsonl",
                ),
                host: None,
            },
            Event::StepWorking {
                run_id: id.into(),
                step_id: "plan".into(),
                note: None,
                transcript_path: Some(PathBuf::from(
                    "/remote/proj/.rupu/transcripts/run_01PLAN.jsonl",
                )),
            },
        ] {
            writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
        }

        let got = recorded_transcript_paths(&store, id);
        let want: Vec<PathBuf> = [
            "/remote/.rupu/runs/run_01RECORDED/sub_runs/sub_01P/transcript.jsonl",
            "/remote/proj/.rupu/transcripts/run_01ACTIVE.jsonl",
            "/remote/proj/.rupu/transcripts/run_01CKPT.jsonl",
            "/remote/proj/.rupu/transcripts/run_01ITEM.jsonl",
            "/remote/proj/.rupu/transcripts/run_01PLAN.jsonl",
            "/remote/proj/.rupu/transcripts/run_01WORK.jsonl",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn recorded_paths_of_unknown_or_empty_run_is_empty() {
        let (store, _tmp) = store();
        assert!(recorded_transcript_paths(&store, "run_01NOPE").is_empty());
        let mut rec = record("run_01EMPTY");
        rec.active_step_transcript_path = Some(PathBuf::new());
        store.create(rec, "").unwrap();
        assert!(
            recorded_transcript_paths(&store, "run_01EMPTY").is_empty(),
            "empty paths are dropped"
        );
    }

    #[test]
    fn cache_key_maps_step_unit_and_sub_run_shapes() {
        assert_eq!(
            cache_key(Path::new(
                "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl"
            ))
            .as_deref(),
            Some("run_01STEP")
        );
        assert_eq!(
            cache_key(Path::new("/home/ci/.rupu/transcripts/run_01AGENT.jsonl")).as_deref(),
            Some("run_01AGENT")
        );
        assert_eq!(
            cache_key(Path::new(
                "/home/ci/.rupu/runs/run_01P/sub_runs/sub_01X/transcript.jsonl"
            ))
            .as_deref(),
            Some("run_01P__sub_runs__sub_01X")
        );
    }

    #[test]
    fn cache_key_rejects_non_transcript_shapes() {
        for bad in [
            "relative/.rupu/transcripts/run_01A.jsonl",
            "/etc/passwd",
            "/home/ci/.rupu/transcripts/run_01A.json",
            "/home/ci/.rupu/transcripts/../secrets/run_01A.jsonl",
            "/home/ci/notes/run_01A.jsonl",
            "/home/ci/.rupu/transcripts/we ird.jsonl",
            "/home/ci/.rupu/runs/run_01P/sub_runs/transcript.jsonl",
        ] {
            assert_eq!(cache_key(Path::new(bad)), None, "{bad} must not map");
        }
    }

    #[test]
    fn cache_path_and_complete_marker_layout() {
        let cache = cache_path(
            Path::new("/g"),
            "host_abc",
            Path::new("/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl"),
        )
        .unwrap();
        assert_eq!(
            cache,
            PathBuf::from("/g/mirror/host_abc/transcripts/run_01STEP.jsonl")
        );
        assert_eq!(
            complete_marker(&cache),
            PathBuf::from("/g/mirror/host_abc/transcripts/run_01STEP.jsonl.complete")
        );
        let tmp = tempfile::tempdir().unwrap();
        let c = tmp.path().join("x.jsonl");
        assert!(!is_complete(&c));
        std::fs::write(complete_marker(&c), b"").unwrap();
        assert!(is_complete(&c));
    }
}
