//! Sealed captured membership -> fresh exact owner/prologue proof. No user path API.
use super::*;
use crate::api::transcript::page::{check_stamp, Stamp};
use sha2::Digest;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read as _},
    path::{Component, Path as FsPath, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selector {
    pub snapshot: String,
    pub section: String,
    pub parent: Option<String>,
    pub step_id: Option<String>,
    pub position: usize,
    pub record_id: String,
}
/// Only deserialize after the bridge's MAC/instance/expiry checks. Never a public
/// request body or a caller-supplied source of filesystem authority.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Authority {
    pub workflow_run_id: String,
    pub workspace_id: String,
    pub child_run_id: String,
    pub path: String,
    pub transcript_root: String,
    pub workflow_started_at: String,
    pub workflow_name: String,
    pub workspace_path: String,
    pub step_id: String,
    pub member_record_id: String,
    pub prologue_sha256: String,
    pub store_kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestry: Vec<Hop>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Hop {
    pub parent_run_id: String,
    pub parent_file_epoch: String,
    pub child_run_id: String,
    pub path: String,
    pub prologue_sha256: String,
    pub edge: super::descendants::Edge,
}
impl Authority {
    pub(super) fn leaf_run_id(&self) -> &str {
        self.ancestry
            .last()
            .map_or(&self.child_run_id, |h| &h.child_run_id)
    }
    pub(super) fn leaf_path(&self) -> &str {
        self.ancestry.last().map_or(&self.path, |h| &h.path)
    }

}
#[path = "member_descendants.rs"]
mod descendant_proof;
pub(super) fn descendant(
    s: &AppState,
    root: &Authority,
    hop: Hop,
    cancel: &tokio_util::sync::CancellationToken,
) -> ApiResult<VerifiedMember> {
    descendant_proof::descendant(s, root, hop, cancel)
}
pub(super) fn reauthorize_with_cancel(
    s: &AppState,
    authority: &Authority,
    cancel: &tokio_util::sync::CancellationToken,
) -> ApiResult<VerifiedMember> {
    descendant_proof::reauthorize(s, authority, cancel)
}
pub(super) struct VerifiedMember {
    pub authority: Authority,
    pub file: File,
    pub path: PathBuf,
    pub prologue: Value,
    pub stamp: Stamp,
}
fn string(value: &Value, key: &str, max: usize) -> ApiResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::conflict("captured member identity unavailable"))
}
#[cfg(test)]
pub(super) fn initial(s: &AppState, id: &str, ws: &str, q: &Selector) -> ApiResult<VerifiedMember> {
    initial_with_cancel(s, id, ws, q, &tokio_util::sync::CancellationToken::new())
}
pub(super) fn initial_with_cancel(
    s: &AppState,
    id: &str,
    ws: &str,
    q: &Selector,
    cancel: &tokio_util::sync::CancellationToken,
) -> ApiResult<VerifiedMember> {
    check_cancel(cancel)?;
    owner(id, ws, Some("local"))?;
    if q.section.len() > 24
        || q.step_id.as_ref().is_some_and(|v| v.len() > 1024)
        || q.position > 100_000
        || q.snapshot.len() > 64
        || q.record_id.len() > 128
        || q.parent.as_ref().is_some_and(|p| p.len() > 40)
    {
        return Err(ApiError::bad_request("invalid captured member selector"));
    }
    let c = {
        let mut cache = s
            .workflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        cache
            .iter()
            .find(|c| c.run_id == id && c.workspace == ws && c.token == q.snapshot)
            .cloned()
            .ok_or_else(|| {
                ApiError::conflict("member observation expired or belongs to another owner")
            })?
    };
    let mut key = q
        .parent
        .as_ref()
        .map_or(q.section.clone(), |p| format!("{}/{p}", q.section));
    if let Some(step) = &q.step_id {
        key = format!(
            "{key}/step:{}",
            serde_json::to_string(step).expect("step string")
        );
    }
    let cursor = c.cursor(&key, q.position);
    let page = c.filtered_page(
        &q.section,
        q.parent.as_deref(),
        q.step_id.as_deref(),
        Some(&cursor),
        1,
    )?;
    let row = page["records"]
        .as_array()
        .and_then(|r| r.first())
        .ok_or_else(|| ApiError::not_found("captured member position unavailable"))?;
    if row["record_id"] != q.record_id || row["state"] != "available" {
        return Err(ApiError::conflict(
            "captured member identity differs or is unavailable",
        ));
    }
    let value = &row["value"];
    let mut step = string(value, "step_id", 1024).ok();
    let mut unit = value
        .get("unit_index")
        .or_else(|| value.get("index"))
        .and_then(Value::as_u64);
    let sub_id = value
        .get("sub_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let run_key = match q.section.as_str() {
        "results" | "checkpoints" => "run_id",
        "attempts" => "agent_run_id",
        "items" => {
            let parent = q
                .parent
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("exact parent required"))?;
            let index = c
                .result_records
                .get(parent)
                .ok_or_else(|| ApiError::conflict("captured parent unavailable"))?;
            let parent: Value =
                serde_json::from_slice(&c.sections["results"][*index]).expect("captured JSON");
            if parent["state"] != "available" {
                return Err(ApiError::conflict("captured parent unavailable"));
            }
            if remote(&parent["value"]) {
                return Err(ApiError::not_available(
                    "captured parent records remote execution",
                ));
            }
            step = Some(string(&parent["value"], "step_id", 1024)?);
            "run_id"
        }
        "events" if value["type"] == "agent_started" && value["run_id"] == id => "agent_run_id",
        _ => {
            return Err(ApiError::not_available(
                "this captured record has no exact child transcript identity",
            ))
        }
    };
    if remote(value) {
        return Err(ApiError::not_available(
            "remote member proof requires its owning host",
        ));
    }
    let child = string(value, run_key, 128)?;
    if child == id {
        return Err(ApiError::conflict(
            "member child cannot equal workflow root",
        ));
    }
    let path = string(value, "transcript_path", 4096)?;
    let step = step.ok_or_else(|| ApiError::conflict("exact member step unavailable"))?;
    if q.section == "results" {
        unit = None;
    }
    attribution(
        &c,
        &child,
        &path,
        &step,
        unit,
        sub_id,
        matches!(q.section.as_str(), "items" | "events"),
        cancel,
    )?;
    let run = &c.header["run"];
    let authority = Authority {
        workflow_run_id: id.into(),
        workspace_id: ws.into(),
        child_run_id: child,
        path,
        transcript_root: string(run, "transcript_dir", 4096)?,
        workflow_started_at: string(run, "started_at", 128)?,
        workflow_name: string(run, "workflow_name", 4096)?,
        workspace_path: string(run, "workspace_path", 4096)?,
        step_id: step,
        member_record_id: q.record_id.clone(),
        prologue_sha256: String::new(),
        store_kind: string(&c.header, "store_kind", 32)?,
        ancestry: Vec::new(),
    };
    check_cancel(cancel)?;
    let verified = verify(s, authority, None)?;
    check_cancel(cancel)?;
    Ok(verified)
}
fn check_cancel(cancel: &tokio_util::sync::CancellationToken) -> ApiResult<()> {
    if cancel.is_cancelled() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "transcript_window_cancelled".into(),
        ));
    }
    Ok(())
}
fn remote(value: &Value) -> bool {
    ["host", "host_id"].iter().any(|key| {
        value
            .get(*key)
            .is_some_and(|h| !h.is_null() && h.as_str() != Some("local"))
    })
}
fn attribution(
    c: &Captured,
    child: &str,
    path: &str,
    step: &str,
    unit: Option<u64>,
    sub_id: Option<&str>,
    required: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> ApiResult<()> {
    let start = Instant::now();
    let mut local = false;
    for section in ["attempts", "checkpoints"] {
        for bytes in &c.sections[section] {
            check_cancel(cancel)?;
            if start.elapsed() > Duration::from_secs(5) {
                return Err(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "member attribution work deadline exceeded".into(),
                ));
            }
            let row: Value = serde_json::from_slice(bytes).expect("captured JSON");
            if row["state"] != "available" {
                continue;
            }
            let value = &row["value"];
            let key = if section == "attempts" {
                "agent_run_id"
            } else {
                "run_id"
            };
            if value[key].as_str() != Some(child)
                || value["transcript_path"].as_str() != Some(path)
                || value["step_id"].as_str() != Some(step)
            {
                continue;
            }
            if remote(value) {
                return Err(ApiError::conflict(
                    "captured attribution records conflicting remote execution",
                ));
            }
            let recorded_unit = value
                .get(if section == "attempts" {
                    "unit_index"
                } else {
                    "index"
                })
                .and_then(Value::as_u64);
            let recorded_sub = value
                .get("sub_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let matches = if let Some(sub) = sub_id {
                (recorded_sub == Some(sub) && recorded_unit.is_none_or(|n| Some(n) == unit))
                    || (recorded_sub.is_none() && unit.is_some() && recorded_unit == unit)
            } else {
                recorded_unit == unit
            };
            local |= matches;
        }
    }
    if required && !local {
        return Err(ApiError::not_available(
            "exact captured local attempt or checkpoint attribution unavailable",
        ));
    }
    Ok(())
}
#[cfg(test)]
pub(super) fn reauthorize(s: &AppState, authority: &Authority) -> ApiResult<VerifiedMember> {
    reauthorize_with_cancel(s, authority, &tokio_util::sync::CancellationToken::new())
}
fn verify(
    s: &AppState,
    mut authority: Authority,
    expected: Option<&str>,
) -> ApiResult<VerifiedMember> {
    owner(
        &authority.workflow_run_id,
        &authority.workspace_id,
        Some("local"),
    )?;
    super::super::runs::validate_id(&authority.child_run_id)?;
    let mut current = super::owner::open(s, &authority.workflow_run_id, &authority.workspace_id)?;
    let started = chrono::DateTime::parse_from_rfc3339(&authority.workflow_started_at)
        .map_err(|_| ApiError::conflict("invalid member owner start"))?;
    if current.run.started_at != started.with_timezone(&chrono::Utc)
        || current.run.workflow_name != authority.workflow_name
        || current.run.workspace_path.to_str() != Some(authority.workspace_path.as_str())
        || current.run.transcript_dir.to_str() != Some(authority.transcript_root.as_str())
        || current.store_kind != authority.store_kind
    {
        return Err(ApiError::conflict("member workflow owner changed"));
    }
    let path = PathBuf::from(&authority.path);
    let root = FsPath::new(&authority.transcript_root);
    if authority.path.len() > 4096
        || authority.transcript_root.len() > 4096
        || !path.is_absolute()
        || !root.is_absolute()
        || path.extension().and_then(|s| s.to_str()) != Some("jsonl")
        || !path.starts_with(root)
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(ApiError::conflict(
            "member transcript is outside its recorded root",
        ));
    }
    // Retain the existing transcript reader's allowed-root boundary without an
    // unbounded all-workspace scan. A recorded path/prologue alone cannot grant
    // access to an arbitrary location outside global or the exact registration.
    let global = s
        .global_dir
        .canonicalize()
        .map_err(|_| ApiError::conflict("global transcript root unavailable"))?;
    let mut registration = None;
    if !path.starts_with(&global) {
        if let Some((_, registered)) = &current.registry {
            if !path.starts_with(FsPath::new(registered)) {
                return Err(ApiError::conflict(
                    "member transcript is outside its registered project",
                ));
            }
        } else {
            let mut input = super::input::Input::open(&global.join("workspaces"))?;
            let workspace = super::owner::registration(&mut input, &authority.workspace_id)?;
            if !path.starts_with(FsPath::new(&workspace.path)) {
                return Err(ApiError::conflict(
                    "member transcript is outside its registered project",
                ));
            }
            registration = Some((input, workspace.path));
        }
    }
    let mut file = super::super::fs_open::open_anchored_regular_file(&path).map_err(|_| {
        ApiError::conflict("member transcript is not an available anchored regular file")
    })?;
    let stamp = Stamp::of(
        &file
            .metadata()
            .map_err(|_| ApiError::conflict("member transcript metadata unavailable"))?,
    )?;
    let mut reader = BufReader::with_capacity(4096, (&mut file).take(256 * 1024));
    let mut line = Vec::new();
    reader
        .read_until(b'\n', &mut line)
        .map_err(|_| ApiError::conflict("member prologue read failed"))?;
    drop(reader);
    if !line.ends_with(b"\n") {
        return Err(ApiError::conflict(
            "member prologue is incomplete or exceeds its limit",
        ));
    }
    let event: rupu_transcript::Event = serde_json::from_slice(&line)
        .map_err(|_| ApiError::conflict("member prologue malformed"))?;
    let raw: Value = serde_json::from_slice(&line).expect("validated prologue JSON");
    let rupu_transcript::Event::RunStart {
        run_id,
        workspace_id,
        agent,
        provider,
        model,
        started_at,
        ..
    } = event
    else {
        return Err(ApiError::conflict(
            "member transcript has no first-record owner proof",
        ));
    };
    if run_id != authority.child_run_id
        || workspace_id != authority.workspace_id
        || [&raw["host_id"], &raw["data"]["host_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some("local"))
        || (!raw["run_id"].is_null() && raw["run_id"] != run_id)
        || (!raw["workspace_id"].is_null() && raw["workspace_id"] != workspace_id)
    {
        return Err(ApiError::conflict("member transcript owner proof differs"));
    }
    if [&agent, &provider, &model].iter().any(|v| v.len() > 4096) {
        return Err(too_large("member prologue metadata exceeds field limit"));
    }
    let hash = format!("{:x}", Sha256::digest(&line));
    if expected.is_some_and(|h| h != hash) {
        return Err(ApiError::conflict("member transcript prologue changed"));
    }
    current.verify(&authority.workflow_run_id, &authority.workspace_id)?;
    if let Some((input, registered)) = &mut registration {
        if super::owner::registration(input, &authority.workspace_id)?.path != *registered {
            return Err(ApiError::conflict("member registered project changed"));
        }
    }
    check_stamp(&path, &file, &stamp).map_err(|error| {
        if error.0 == StatusCode::CONFLICT {
            ApiError::conflict("transcript_window_changed_during_read")
        } else {
            error
        }
    })?;
    authority.prologue_sha256 = hash;
    let prologue = json!({"run_id":run_id,"workspace_id":workspace_id,"agent":agent,"provider":provider,"model":model,"started_at":started_at});
    Ok(VerifiedMember {
        authority,
        file,
        path,
        prologue,
        stamp,
    })
}
