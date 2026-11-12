use super::input::{Input, Read};
use super::*;
use rupu_orchestrator::runs::RunRecord;
use std::fs::File;

pub(super) struct Owner {
    pub input: Input,
    pub run: RunRecord,
    pub owner_bytes: Vec<u8>,
    pub archived: bool,
    pub registry: Option<(File, String)>,
    pub store_kind: &'static str,
}
fn record(read: Option<Read>, id: &str, ws: &str) -> ApiResult<(RunRecord, Vec<u8>)> {
    let read = read.ok_or_else(|| ApiError::not_found("recorded workflow owner unavailable"))?;
    let run: RunRecord = serde_json::from_slice(&read.bytes)
        .map_err(|_| ApiError::conflict("workflow owner record malformed"))?;
    if run.id != id || run.workspace_id != ws {
        return Err(ApiError::not_found(
            "workflow run not recorded under this project",
        ));
    }
    if run.workflow_name.is_empty() || run.workflow_name.starts_with("agent:") {
        return Err(ApiError::bad_request(
            "observation requires a recorded workflow run",
        ));
    }
    Ok((run, read.bytes))
}
pub(super) fn registration(input: &mut Input, ws: &str) -> ApiResult<rupu_workspace::Workspace> {
    let read = input
        .read(&format!("{ws}.toml"), 256 * 1024, false)?
        .ok_or_else(|| ApiError::not_found("exact project registration unavailable"))?;
    let text = std::str::from_utf8(&read.bytes)
        .map_err(|_| ApiError::conflict("project registration malformed"))?;
    let record: rupu_workspace::Workspace =
        toml::from_str(text).map_err(|_| ApiError::conflict("project registration malformed"))?;
    if record.id != ws
        || record.path.len() > 4096
        || !std::path::Path::new(&record.path).is_absolute()
    {
        return Err(ApiError::conflict("project registration identity invalid"));
    }
    Ok(record)
}

pub(super) fn open(s: &AppState, id: &str, ws: &str) -> ApiResult<Owner> {
    let mut selected = None;
    for (root, archived) in [
        (s.run_store.root.clone(), false),
        (s.run_store.archive_root(), true),
    ] {
        let root = match root.canonicalize() {
            Ok(root) => root,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ApiError::conflict("run store unavailable")),
        };
        match Input::open(&root.join(id)) {
            Ok(mut input) => {
                let read = input.read("run.json", 256 * 1024, false)?;
                if read.is_none() {
                    continue;
                }
                let (run, bytes) = record(read, id, ws)?;
                selected = Some((input, archived, run, bytes));
                break;
            }
            Err(e) if e.0 == StatusCode::NOT_FOUND => continue,
            Err(e) => return Err(e),
        }
    }
    let (input, archived, registry, store_kind, run, owner_bytes) =
        if let Some((input, archived, run, bytes)) = selected {
            (
                input,
                archived,
                None,
                if archived { "global_archive" } else { "global" },
                run,
                bytes,
            )
        } else {
            let global = s
                .global_dir
                .canonicalize()
                .map_err(|_| ApiError::not_found("local owner store unavailable"))?;
            let mut input = Input::open(&global.join("workspaces"))?;
            let workspace = registration(&mut input, ws)?;
            let project = Input::open(
                &std::path::Path::new(&workspace.path)
                    .join(".rupu/runs")
                    .join(id),
            )?;
            let registry_dir = std::mem::replace(&mut input.dir, project.dir);
            let (run, bytes) = record(input.read("run.json", 256 * 1024, false)?, id, ws)?;
            (
                input,
                false,
                Some((registry_dir, workspace.path)),
                "project_local",
                run,
                bytes,
            )
        };
    Ok(Owner {
        input,
        run,
        owner_bytes,
        archived,
        registry,
        store_kind,
    })
}
impl Owner {
    pub fn verify(&mut self, id: &str, ws: &str) -> ApiResult<()> {
        let (after, _) = record(self.input.read("run.json", 256 * 1024, false)?, id, ws)?;
        if after.started_at != self.run.started_at
            || after.workflow_name != self.run.workflow_name
            || after.workspace_path != self.run.workspace_path
            || after.transcript_dir != self.run.transcript_dir
        {
            return Err(ApiError::conflict(
                "recorded workflow owner changed during capture",
            ));
        }
        if let Some((registry_dir, path)) = &self.registry {
            let registry_dir = registry_dir
                .try_clone()
                .map_err(|_| ApiError::conflict("registry handle unavailable"))?;
            let original = std::mem::replace(&mut self.input.dir, registry_dir);
            let checked = registration(&mut self.input, ws);
            self.input.dir = original;
            if checked?.path != *path {
                return Err(ApiError::conflict("project owner changed during capture"));
            }
        }
        self.input.deadline()?;
        Ok(())
    }
}
