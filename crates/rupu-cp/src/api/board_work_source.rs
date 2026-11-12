//! Resolution decorates immutable provenance; unavailable bytes never erase it.
use super::owner::Context;
use crate::api::children::pages::{
    capture::search::{self, Captured},
    Owner as ChildrenOwner,
};
use crate::api::{fs_open::open_anchored_regular_file, transcript::page};
use rupu_fleet::work_queue::{
    WorkActor, WorkItem, WorkItemSummary, WorkRecordingAnchor, WorkSource,
};
use rupu_runtime::services::operator::RootKind;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

pub(super) struct Resolver<'a> {
    context: &'a Context,
    cache: HashMap<String, Value>,
    membership: HashMap<String, Option<Captured>>,
}
impl<'a> Resolver<'a> {
    pub fn new(context: &'a Context) -> Self {
        Self {
            context,
            cache: HashMap::new(),
            membership: HashMap::new(),
        }
    }
    pub fn actor(&mut self, actor: &WorkActor, source: Option<&WorkSource>) -> Value {
        let key = serde_json::to_string(actor).unwrap_or_default();
        if !self.cache.contains_key(&key) {
            let value = resolve(self.context, actor, &mut self.membership);
            self.cache.insert(key.clone(), value);
        }
        let resolved = &self.cache[&key];
        let mut value = resolved.clone();
        value["recorded"] = json!({"actor": actor, "source": source});
        if value["availability"] == "available" {
            value["transcript"]["invocation"] = json!(source);
        }
        value
    }
    pub fn item(&mut self, item: &WorkItem) -> Value {
        let mut value = serde_json::to_value(item).expect("work item is serializable");
        let creator = self.actor(&item.creator, None);
        let claimant = item
            .latest_attempt
            .as_ref()
            .map(|a| self.actor(&a.claimant, None));
        let executor = item
            .latest_attempt
            .as_ref()
            .and_then(|a| a.executor.as_ref())
            .map(|a| self.actor(a, None));
        let latest_event = self.actor(&item.latest_event.actor, item.latest_event.source.as_ref());
        value["sources"] = json!({"creator":creator,"claimant":claimant,"executor":executor,"latest_event":latest_event});
        value
    }
    pub fn summary(&mut self, item: &WorkItemSummary) -> Value {
        let mut value = serde_json::to_value(item).expect("work summary is serializable");
        let creator = self.actor(&item.creator, None);
        let claimant = item.claimant.as_ref().map(|a| self.actor(a, None));
        let executor = item.executor.as_ref().map(|a| self.actor(a, None));
        value["sources"] = json!({"creator":creator,"claimant":claimant,"executor":executor});
        value
    }
}
fn unavailable(reason: &str) -> Value {
    json!({"availability":"unavailable","reason":reason})
}
fn unsupported(reason: &str) -> Value {
    json!({"availability":"unsupported","reason":reason})
}
fn relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && Path::new(path).extension().is_some_and(|x| x == "jsonl")
}
fn resolve(
    ctx: &Context,
    actor: &WorkActor,
    captures: &mut HashMap<String, Option<Captured>>,
) -> Value {
    let WorkActor::Agent {
        run_id,
        agent,
        recording,
        ..
    } = actor
    else {
        return unsupported("operator_has_no_transcript");
    };
    if !relative(&recording.relative_path) {
        return unavailable("invalid_recording_locator");
    }
    let anchor = match recording.anchor {
        WorkRecordingAnchor::Owner => ctx.directory.clone(),
        WorkRecordingAnchor::GlobalTranscripts => ctx.global.join("transcripts"),
        WorkRecordingAnchor::GlobalRuns => ctx.global.join("runs"),
        WorkRecordingAnchor::ProjectTranscripts => {
            let Some(id) = recording.workspace_id.as_deref() else {
                return unsupported("project_identity_missing");
            };
            let store = rupu_workspace::WorkspaceStore {
                root: ctx.global.join("workspaces"),
            };
            let Ok(Some(workspace)) = store.load(id) else {
                return unsupported("project_registration_unavailable");
            };
            let Ok(path) = PathBuf::from(workspace.path).canonicalize() else {
                return unavailable("project_directory_unavailable");
            };
            path.join(".rupu/transcripts")
        }
    };
    let mut path = anchor.join(&recording.relative_path);
    let project = recording
        .workspace_id
        .as_deref()
        .or_else(|| ctx.record["workspace_id"].as_str());
    if ctx.owner.kind == RootKind::Session && recording.anchor != WorkRecordingAnchor::GlobalRuns {
        // Archive changes the exact run's path. Neither its newest sibling nor
        // an arbitrary recorded path can substitute for this membership.
        let matches: Vec<_> = ctx.record["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["run_id"].as_str() == Some(run_id))
            .collect();
        if matches.len() != 1 {
            return unsupported("exact_session_run_membership_unavailable");
        }
        let Some(recorded_path) = matches[0]["transcript_path"].as_str() else {
            return unavailable("session_recording_path_missing");
        };
        let Some(transcripts_dir) = ctx.record["transcripts_dir"].as_str() else {
            return unsupported("session_transcript_root_unavailable");
        };
        let mut transcript_root = PathBuf::from(transcripts_dir);
        let mut member_path = PathBuf::from(recorded_path);
        // Owner-relative recordings move with their session directory. Preserve
        // the exact relative subdirectory when rebasing an archive/restore.
        if recording.anchor == WorkRecordingAnchor::Owner {
            for collection in ["sessions", "sessions-archive"] {
                let original = ctx.global.join(collection).join(&ctx.owner.id);
                if let Ok(relative) = transcript_root.strip_prefix(&original) {
                    transcript_root = ctx.directory.join(relative);
                }
                if let Ok(relative) = member_path.strip_prefix(&original) {
                    member_path = ctx.directory.join(relative);
                }
            }
        }
        let name = format!("{run_id}.jsonl");
        let valid_root = transcript_root
            .strip_prefix(&anchor)
            .ok()
            .is_some_and(|p| p.components().all(|c| matches!(c, Component::Normal(_))));
        let allowed = [
            transcript_root.join(&name),
            transcript_root.join("archive").join(&name),
        ];
        if member_path.file_name().and_then(|n| n.to_str()) != Some(name.as_str())
            || path.file_name().and_then(|n| n.to_str()) != Some(name.as_str())
            || recording.round.is_some()
            || !valid_root
            || !allowed.contains(&member_path)
            || !allowed.contains(&path)
        {
            return unavailable("session_recording_identity_mismatch");
        }
        path = member_path;
    }
    if let Some(round) = recording.round {
        if ctx.owner.kind != RootKind::Agentiflow
            || recording.anchor != WorkRecordingAnchor::Owner
            || recording.relative_path != format!("lead/transcript.r{round}.jsonl")
        {
            return unavailable("lead_round_identity_mismatch");
        }
    } else if ctx.owner.kind == RootKind::Agentiflow && recording.relative_path.starts_with("lead/")
    {
        return unavailable("lead_round_missing");
    }
    if recording.anchor == WorkRecordingAnchor::Owner && ctx.owner.kind != RootKind::Session {
        let exact_root = (ctx.owner.kind == RootKind::Standalone && *run_id == ctx.owner.id)
            || (ctx.owner.kind == RootKind::Agentiflow
                && *run_id == ctx.owner.id
                && recording.round.is_some());
        if !exact_root && !descendant(ctx, run_id, &path, captures) {
            return unsupported("recorded_execution_membership_unavailable");
        }
    }
    if recording.anchor == WorkRecordingAnchor::GlobalRuns {
        let parts: Vec<_> = Path::new(&recording.relative_path).components().collect();
        if parts.len() != 4
            || parts[1].as_os_str() != "sub"
            || parts[2].as_os_str() != run_id.as_str()
            || parts[3].as_os_str() != "transcript.jsonl"
            || recording.round.is_some()
        {
            return unavailable("descendant_recording_identity_mismatch");
        }
        if !descendant(ctx, run_id, &path, captures) {
            return unsupported("recorded_descendant_membership_unavailable");
        }
    } else if recording.anchor != WorkRecordingAnchor::Owner && ctx.owner.kind != RootKind::Session
    {
        // Non-owner anchors must still name the exact recorded run, never an
        // arbitrary file with a matching prologue appended later.
        if path.file_name().and_then(|n| n.to_str()) != Some(format!("{run_id}.jsonl").as_str()) {
            return unavailable("run_recording_identity_mismatch");
        }
        if !(ctx.owner.kind == RootKind::Standalone && *run_id == ctx.owner.id)
            && !descendant(ctx, run_id, &path, captures)
        {
            return unsupported("recorded_execution_membership_unavailable");
        }
    }
    let mut file = match open_anchored_regular_file(&path) {
        Ok(f) => f,
        Err(_) => return unavailable("recording_unavailable"),
    };
    let stamp = match file.metadata().ok().and_then(|m| page::Stamp::of(&m).ok()) {
        Some(s) => s,
        None => return unavailable("recording_unavailable"),
    };
    // First nonblank physical record only, with bounded allocation and read.
    let mut reader = BufReader::new((&mut file).take(256 * 1024 + 1));
    let mut consumed = 0usize;
    let raw = loop {
        let mut bytes = Vec::new();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => return unavailable("recording_prologue_unavailable"),
            Ok(n) => consumed += n,
        }
        if consumed > 256 * 1024 || !bytes.ends_with(b"\n") {
            return unavailable("recording_prologue_unavailable");
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        break bytes;
    };
    // Runtime writes this directly after this RunStart, before other notices.
    // Do not search for a later marker: a rewritten/continued recording with
    // the same path and run ID cannot inherit an earlier execution's result.
    let marker = loop {
        let mut bytes = Vec::new();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => return unavailable("execution_marker_unavailable"),
            Ok(n) => consumed += n,
        }
        if consumed > 256 * 1024 || !bytes.ends_with(b"\n") {
            return unavailable("execution_marker_unavailable");
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        break bytes;
    };
    drop(reader);
    let Ok(rupu_transcript::Event::Notice { kind, message }) = serde_json::from_slice(&marker)
    else {
        return unavailable("execution_marker_unavailable");
    };
    if kind != "work_queue_execution"
        || serde_json::from_str::<WorkActor>(&message).ok().as_ref() != Some(actor)
    {
        return unavailable("execution_marker_mismatch");
    }
    let Ok(rupu_transcript::Event::RunStart {
        run_id: actual_run,
        workspace_id,
        agent: actual_agent,
        ..
    }) = serde_json::from_slice(&raw)
    else {
        return unavailable("recording_prologue_invalid");
    };
    let Ok(raw): std::result::Result<Value, _> = serde_json::from_slice(&raw) else {
        return unavailable("recording_prologue_invalid");
    };
    if actual_run != *run_id
        || actual_agent != *agent
        || project.is_some_and(|p| p != workspace_id)
        || [&raw["run_id"], &raw["data"]["run_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some(run_id))
        || [&raw["workspace_id"], &raw["data"]["workspace_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some(workspace_id.as_str()))
        || [&raw["host_id"], &raw["data"]["host_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some("local"))
        || page::check_stamp(&path, &file, &stamp).is_err()
    {
        return unavailable("recording_identity_mismatch");
    }
    json!({"availability":"available","transcript":{
        "host":"local","path":path,"run_id":run_id,"workspace_id":workspace_id,"round":recording.round
    }})
}

// Reuse the bounded, anchored launch-tree capture. Legacy directory presence
// is deliberately excluded: every edge to this child must be a recorded
// Started or workflow-step membership. No scan of transcript bodies is used.
fn descendant(
    ctx: &Context,
    run: &str,
    path: &Path,
    captures: &mut HashMap<String, Option<Captured>>,
) -> bool {
    let roots: Vec<String> = if ctx.owner.kind == RootKind::Session {
        ctx.record["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["run_id"].as_str().map(str::to_owned))
            .collect()
    } else {
        vec![ctx.owner.id.clone()]
    };
    for root in roots {
        if !captures.contains_key(&root) {
            // Keep availability work bounded across a 200-row queue page.
            // A later exact-item read can resolve a source beyond this budget.
            if captures.len() >= 16 {
                return false;
            }
            let owner = ChildrenOwner {
                global: ctx.global.clone(),
                runs: ctx.global.join("runs"),
            };
            let captured = search::capture(owner, &root, &AtomicBool::new(false)).ok();
            captures.insert(root.clone(), captured);
        }
        let Some(Some(capture)) = captures.get(&root) else {
            continue;
        };
        let Some(index) = capture
            .members
            .iter()
            .position(|m| m.run_id == run && m.path.as_deref() == Some(path))
        else {
            continue;
        };
        let mut current = index;
        let mut valid = true;
        while let Some(parent) = capture.members[current].parent {
            if !matches!(
                capture.members[current].source,
                "children_ledger" | "workflow_step"
            ) || parent >= current
            {
                valid = false;
                break;
            }
            current = parent;
        }
        if current == 0 && index != 0 && valid && capture.proof.validate_search().is_ok() {
            return true;
        }
    }
    false
}
