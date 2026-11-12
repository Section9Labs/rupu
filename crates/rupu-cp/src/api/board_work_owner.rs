//! Exact local owner selection. Reading never creates a message space or queue.
use super::error::{Error, Result};
use crate::{
    api::{fs_open::open_anchored_regular_file, transcript::page},
    host::messages::MessageRoute,
};
use rupu_runtime::services::operator::{MessageRoot, RootKind};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

// Session owner records contain past prompts as well as immutable ownership.
// Bound that input independently of the 1 MiB queue response/page budget.
const MAX_OWNER_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Serialize)]
pub(super) struct Owner {
    pub route: &'static str,
    pub kind: RootKind,
    pub id: String,
    pub host: &'static str,
    pub token: String,
    pub archived: bool,
    pub writable: bool,
    pub allowed_actions: Vec<&'static str>,
}
pub(super) struct Context {
    pub owner: Owner,
    pub root: MessageRoot,
    pub global: PathBuf,
    pub configured_global: PathBuf,
    pub directory: PathBuf,
    pub record: Value,
}

// Use the already bounded record; the legacy liveness helper rereads all
// session history without this route's byte fence. Standalone authority uses
// the shared bounded roster reader, never process presence by itself.
fn live(root: &MessageRoot, record: &Value) -> bool {
    let runner = || match &record["runner_pid"] {
        Value::Null => true,
        value => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .is_some_and(rupu_orchestrator::runs::pid_is_running),
    };
    match root.kind {
        RootKind::Session => matches!(
            record["status"].as_str(),
            Some("idle" | "running" | "failed")
        ),
        RootKind::Workflow => {
            matches!(record["status"].as_str(), Some("running" | "pending")) && runner()
        }
        RootKind::Agentiflow => record["status"] == "running" && runner(),
        RootKind::Standalone => {
            if matches!(
                record["status"].as_str(),
                Some("completed" | "failed" | "rejected" | "cancelled")
            ) {
                return false;
            }
            rupu_fleet::Roster::read_work_authority(&root.space.join("participants.jsonl"))
                .ok()
                .is_some_and(|roster| {
                    roster.participants().any(|p| {
                        p.live
                            && p.member.run_id == root.id
                            && p.member
                                .pid
                                .is_some_and(rupu_orchestrator::runs::pid_is_running)
                    })
                })
        }
    }
}

fn directory(path: &Path, required: bool) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && path.canonicalize().ok().as_deref() == Some(path) => Ok(true),
        Ok(_) => Err(Error::owner_changed()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::missing("Board owner is unavailable"))
        }
        Err(e) => Err(Error::internal(e.to_string())),
    }
}
fn record(path: &Path, optional: bool) -> Result<Value> {
    let mut file = match open_anchored_regular_file(path) {
        Ok(file) => file,
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::missing("Board owner record is unavailable"))
        }
        Err(_) => return Err(Error::owner_changed()),
    };
    let stamp = page::Stamp::of(
        &file
            .metadata()
            .map_err(|e| Error::internal(e.to_string()))?,
    )
    .map_err(|_| Error::owner_changed())?;
    if stamp.len > MAX_OWNER_BYTES as u64 {
        return Err(Error::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "work_capacity",
            "Board owner record exceeds 16 MiB",
            "none",
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take((MAX_OWNER_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::internal(e.to_string()))?;
    if bytes.len() > MAX_OWNER_BYTES {
        return Err(Error::owner_changed());
    }
    page::check_stamp(path, &file, &stamp).map_err(|_| Error::owner_changed())?;
    serde_json::from_slice(&bytes).map_err(|_| Error::invalid("Board owner record is malformed"))
}
pub(super) fn resolve(
    global: &Path,
    route: MessageRoute,
    id: &str,
    service: bool,
) -> Result<Context> {
    if MessageRoute::of_id(id) != Some(route) {
        return Err(Error::missing("Board owner does not match this route"));
    }
    let configured_global = global.to_path_buf();
    let global = global
        .canonicalize()
        .map_err(|_| Error::missing("Global owner root is unavailable"))?;
    let collections: &[&str] = match route {
        MessageRoute::Sessions => &["sessions", "sessions-archive"],
        MessageRoute::Runs => &["runs", "runs-archive"],
        MessageRoute::Agentiflows => &["agentiflows"],
    };
    let mut selected = None;
    for collection in collections {
        let base = global.join(collection);
        if directory(&base, false)? && directory(&base.join(id), false)? {
            selected = Some((base.join(id), collection.ends_with("-archive")));
            break;
        }
    }
    let (dir, archived) = selected.ok_or_else(|| Error::missing("Board owner is unavailable"))?;
    let (file, key, optional) = match route {
        MessageRoute::Sessions => ("session.json", "session_id", false),
        MessageRoute::Runs => ("run.json", "id", true),
        MessageRoute::Agentiflows => ("agentiflow.json", "id", false),
    };
    let record = record(&dir.join(file), optional)?;
    if record != Value::Null
        && (record[key].as_str() != Some(id) || record["status"].as_str().is_none())
    {
        return Err(Error::invalid(
            "Board owner record identity or lifecycle is invalid",
        ));
    }
    let kind = match route {
        MessageRoute::Sessions => RootKind::Session,
        MessageRoute::Agentiflows => RootKind::Agentiflow,
        MessageRoute::Runs if record == Value::Null => RootKind::Standalone,
        MessageRoute::Runs => match record["workflow_name"].as_str() {
            Some(name) if name.starts_with("agent:") => RootKind::Standalone,
            Some(_) => RootKind::Workflow,
            None => return Err(Error::invalid("Board run kind is unavailable")),
        },
    };
    let space = if kind == RootKind::Agentiflow {
        dir.clone()
    } else {
        dir.join("messages")
    };
    directory(&space, false)?;
    directory(&space.join("board"), false)?;
    let root = MessageRoot {
        kind,
        id: id.into(),
        space,
    };
    let writable = service && !archived && live(&root, &record);
    let metadata = fs::metadata(&dir).map_err(|_| Error::owner_changed())?;
    let mut digest = Sha256::new();
    digest.update(b"rupu.board-work.owner.v1\0");
    digest.update(dir.as_os_str().as_encoded_bytes());
    digest.update(
        json!([
            record[key],
            record["created_at"],
            record["started_at"],
            record["workflow_name"],
            record["workspace_id"],
            record["workspace_path"],
            record["project_root"]
        ])
        .to_string()
        .as_bytes(),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest.update(metadata.dev().to_le_bytes());
        digest.update(metadata.ino().to_le_bytes());
    }
    #[cfg(not(unix))]
    digest.update(format!("{:?}", metadata.created()).as_bytes());
    Ok(Context {
        owner: Owner {
            route: route.segment(),
            kind,
            id: id.into(),
            host: "local",
            token: format!("{:x}", digest.finalize()),
            archived,
            writable,
            allowed_actions: if writable {
                vec!["post", "cancel", "requeue"]
            } else {
                vec![]
            },
        },
        root,
        global,
        configured_global,
        directory: dir,
        record,
    })
}
pub(super) fn unchanged(ctx: &Context, route: MessageRoute, service: bool) -> Result<()> {
    let current = resolve(&ctx.global, route, &ctx.owner.id, service)?;
    if current.owner.token != ctx.owner.token {
        return Err(Error::owner_changed());
    }
    Ok(())
}
