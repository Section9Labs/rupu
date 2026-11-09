//! Independent ownership evidence for workflow, standalone and session roots.
//! A children directory alone is never proof that a requested root is owned.
use super::super::{stale, too_large, Owner};
use super::{valid_id, validate_root, Identity, Reader, MAX_ENTRIES};
use crate::error::{ApiError, ApiResult};
use rupu_orchestrator::runs::{RunRecord, RunStore};
use rupu_runtime::standalone_run_metadata::StandaloneRunMetadata;
use serde::Deserialize;
use std::{fs, path::PathBuf};

pub(crate) struct RootProof {
    pub(crate) kind: &'static str,
    anchors: Vec<Anchor>,
    directory: Option<(PathBuf, Identity)>,
}
struct Anchor {
    path: PathBuf,
    identity: Option<Identity>,
}
impl RootProof {
    pub(crate) fn validate(&self) -> ApiResult<()> {
        if let Some((path, identity)) = &self.directory {
            let metadata =
                fs::symlink_metadata(path).map_err(|_| stale("owning root disappeared"))?;
            if !metadata.is_dir() || Identity::of(&metadata) != *identity {
                return Err(stale("owning root was replaced or moved"));
            }
        }
        for anchor in &self.anchors {
            let metadata = fs::symlink_metadata(&anchor.path)
                .map_err(|_| stale("root ownership record disappeared"))?;
            if !metadata.is_file()
                || anchor
                    .identity
                    .as_ref()
                    .is_some_and(|identity| *identity != Identity::of(&metadata))
            {
                return Err(stale("root ownership record was replaced or moved"));
            }
        }
        Ok(())
    }
}
fn unsupported(reason: &str) -> ApiError {
    ApiError::not_available(format!("children_root_unsupported: {reason}"))
}

#[derive(Deserialize)]
struct SessionProof {
    session_id: String,
    #[serde(default)]
    runs: Vec<SessionTurn>,
}
#[derive(Deserialize)]
struct SessionTurn {
    run_id: String,
    transcript_path: PathBuf,
}

// The session record is authoritative for its turns. Validate its exact ID and
// recorded transcript locator without reading transcript bodies or prompts.
fn session(reader: &mut Reader, session_id: &str, run: &str) -> ApiResult<Option<RootProof>> {
    valid_id(session_id)?;
    for collection in ["sessions", "sessions-archive"] {
        let directory = reader.root.join(collection).join(session_id);
        let Some(metadata) = reader.checked_metadata(&directory)? else {
            continue;
        };
        if !metadata.is_dir() {
            return Err(ApiError::bad_request("session owner is not a directory"));
        }
        let identity = Identity::of(&metadata);
        let path = directory.join("session.json");
        let Some(bytes) = reader.bytes(&path)? else {
            continue;
        };
        let record: SessionProof = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::conflict("invalid session ownership record"))?;
        if record.session_id != session_id {
            return Err(ApiError::conflict("session ownership identity mismatch"));
        }
        if let Some(turn) = record.runs.iter().find(|turn| turn.run_id == run) {
            let expected = format!("{run}.jsonl");
            if turn.transcript_path.file_name().and_then(|s| s.to_str()) != Some(expected.as_str())
                || !turn.transcript_path.is_absolute()
            {
                return Err(ApiError::conflict(
                    "session turn transcript identity mismatch",
                ));
            }
            // A recorded foreign path never becomes permission to read it.
            // Project-local roots require their own resolver contract.
            crate::api::transcript::validate_transcript_path(
                &turn.transcript_path.to_string_lossy(),
                std::slice::from_ref(&reader.root),
            )
            .map_err(|_| {
                unsupported("session transcript is outside this owner's supported local roots")
            })?;
            let proof = RootProof {
                kind: "session_turn",
                anchors: vec![Anchor {
                    path,
                    identity: None,
                }],
                directory: Some((directory, identity)),
            };
            proof.validate()?;
            return Ok(Some(proof));
        }
    }
    Ok(None)
}

pub(super) fn resolve(owner: &Owner, id: &str) -> ApiResult<(Reader, RootProof)> {
    let store = RunStore::new(owner.runs.clone());
    for root in [owner.runs.clone(), store.archive_root()] {
        if !root.exists() {
            continue;
        }
        let root = root
            .canonicalize()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let mut reader = Reader::new(root.clone());
        let directory = root.join(id);
        let Some(metadata) = reader.checked_metadata(&directory)? else {
            continue;
        };
        if !metadata.is_dir() {
            return Err(ApiError::bad_request("root is not a directory"));
        }
        let identity = Identity::of(&metadata);
        let path = directory.join("run.json");
        let Some(bytes) = reader.bytes(&path)? else {
            continue;
        };
        let record: RunRecord = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::conflict("invalid canonical root run record"))?;
        if record.id != id {
            return Err(ApiError::conflict("canonical root run identity mismatch"));
        }
        validate_root(&directory, &identity)?;
        let proof = RootProof {
            kind: "recorded_run",
            anchors: vec![Anchor {
                path,
                identity: None,
            }],
            directory: Some((directory, identity)),
        };
        return Ok((reader, proof));
    }
    let mut reader = Reader::new(owner.global.clone());
    for collection in ["transcripts", "transcripts/archive"] {
        let path = owner
            .global
            .join(collection)
            .join(format!("{id}.meta.json"));
        let Some(before) = reader.checked_metadata(&path)? else {
            continue;
        };
        let identity = Identity::of(&before);
        let Some(bytes) = reader.bytes(&path)? else {
            return Err(stale("standalone ownership disappeared"));
        };
        let metadata: StandaloneRunMetadata = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::conflict("invalid standalone ownership metadata"))?;
        if metadata.run_id != id {
            return Err(ApiError::conflict("standalone ownership identity mismatch"));
        }
        if metadata.version != StandaloneRunMetadata::VERSION {
            return Err(unsupported("unsupported standalone metadata version"));
        }
        let mut proof = if let Some(session_id) = metadata.session_id {
            session(&mut reader, &session_id, id)?.ok_or_else(|| {
                ApiError::conflict("session metadata has no matching canonical turn record")
            })?
        } else {
            if metadata.trigger_source == "session_turn" {
                return Err(unsupported(
                    "session metadata does not record its owning session",
                ));
            }
            RootProof {
                kind: "standalone_agent",
                anchors: Vec::new(),
                directory: None,
            }
        };
        proof.anchors.push(Anchor {
            path,
            identity: Some(identity),
        });
        reader.root = owner.runs.clone();
        proof.validate()?;
        return Ok((reader, proof));
    }
    // Queued turns and older sessions can precede standalone metadata. Search
    // only canonical session collections, under the same byte/entry budgets.
    for collection in ["sessions", "sessions-archive"] {
        let directory = owner.global.join(collection);
        let Some(metadata) = reader.checked_metadata(&directory)? else {
            continue;
        };
        if !metadata.is_dir() {
            return Err(ApiError::bad_request(
                "session collection is not a directory",
            ));
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(&directory).map_err(|e| ApiError::internal(e.to_string()))? {
            reader.evidence.directory_entries_scanned += 1;
            if reader.evidence.directory_entries_scanned > MAX_ENTRIES {
                return Err(too_large("ownership directory entry budget exceeded"));
            }
            let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
            let kind = entry
                .file_type()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            if kind.is_symlink() {
                return Err(ApiError::bad_request(
                    "symlinked session ownership is not supported",
                ));
            }
            if !kind.is_dir() {
                continue;
            }
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| ApiError::bad_request("invalid session identifier"))?;
            valid_id(&id)?;
            ids.push(id);
        }
        ids.sort();
        for session_id in ids {
            if let Some(proof) = session(&mut reader, &session_id, id)? {
                reader.root = owner.runs.clone();
                return Ok((reader, proof));
            }
        }
    }
    let existing = reader.checked_metadata(&owner.runs.join(id))?.is_some()
        || reader
            .checked_metadata(&owner.global.join("transcripts").join(format!("{id}.jsonl")))?
            .is_some();
    if existing {
        Err(unsupported(
            "existing root has no supported canonical ownership record",
        ))
    } else {
        Err(ApiError::not_found(
            "recorded root not found on local owner",
        ))
    }
}
