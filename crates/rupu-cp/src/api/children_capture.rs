//! Bounded source capture for the paged children endpoint. The legacy endpoint
//! retains its old walk. This path never treats an arbitrary directory as a root.
use super::super::{from_entry, ChildDto, Parent};
use super::{row_weight, stale, too_large, Evidence, Owner, Snapshot, MAX_SNAPSHOT_BYTES};
use crate::error::{ApiError, ApiResult};
use rupu_launch::ledger::{self, LedgerLine};
use rupu_orchestrator::runs::StepResultRecord;
use rupu_transcript::{Event, RunStatus};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, Metadata},
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Instant, SystemTime},
};

#[path = "children_root.rs"]
mod root;
#[path = "search_membership.rs"]
pub(crate) mod search;
pub(crate) use root::RootProof;

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 256 * 1024;
const MAX_SOURCE_ROWS: usize = 500_000;
const MAX_DIRECTORIES: usize = 100_000;
const MAX_ENTRIES: usize = 100_000;
const MAX_PENDING: usize = 100_000;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Identity {
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl Identity {
    fn of(m: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            created: m.created().ok(),
            #[cfg(unix)]
            device: m.dev(),
            #[cfg(unix)]
            inode: m.ino(),
        }
    }
}

pub(super) fn valid_id(id: &str) -> ApiResult<()> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ApiError::bad_request("invalid recorded run identifier"));
    }
    Ok(())
}

pub(super) fn validate_root(root: &Path, identity: &Identity) -> ApiResult<()> {
    let m = fs::symlink_metadata(root).map_err(|_| stale("recorded root disappeared"))?;
    if !m.is_dir() || Identity::of(&m) != *identity {
        return Err(stale("recorded root was replaced or moved"));
    }
    let record = fs::symlink_metadata(root.join("run.json"))
        .map_err(|_| stale("root record disappeared"))?;
    if !record.is_file() {
        return Err(stale("root record is no longer a regular file"));
    }
    Ok(())
}

struct Reader {
    root: PathBuf,
    evidence: Evidence,
}
impl Reader {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            evidence: Evidence {
                consistency: "captured_observation",
                history_complete: "unknown",
                source_scan: "complete_within_recorded_local_sources",
                ..Evidence::default()
            },
        }
    }
    // No symlink in a source path may redirect membership to another run.
    fn checked_metadata(&self, path: &Path) -> ApiResult<Option<Metadata>> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| ApiError::bad_request("source outside run store"))?;
        let mut current = self.root.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(ApiError::bad_request("invalid source path"));
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(ApiError::bad_request(
                        "symlinked child source is not supported",
                    ))
                }
                Ok(m) if current == path => return Ok(Some(m)),
                Ok(m) if !m.is_dir() => {
                    return Err(ApiError::bad_request(
                        "child source parent is not a directory",
                    ))
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => {
                    return Err(ApiError::internal(format!(
                        "cannot inspect children source: {e}"
                    )))
                }
            }
        }
        Ok(None)
    }
    fn bytes(&mut self, path: &Path) -> ApiResult<Option<Vec<u8>>> {
        let Some(before) = self.checked_metadata(path)? else {
            return Ok(None);
        };
        if !before.is_file() {
            return Err(ApiError::bad_request(
                "children source must be a regular file",
            ));
        }
        let len = usize::try_from(before.len()).map_err(|_| too_large("source size overflow"))?;
        if len > MAX_FILE_BYTES || self.evidence.bytes_read.saturating_add(len) > MAX_SOURCE_BYTES {
            return Err(too_large(
                "source exceeds the 16 MiB file or 64 MiB capture byte budget",
            ));
        }
        let mut file = crate::api::fs_open::open_anchored_regular_file(path)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let opened = file
            .metadata()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !opened.is_file() || Identity::of(&opened) != Identity::of(&before) {
            return Err(stale("source replaced during capture"));
        }
        let mut bytes = vec![0; len];
        file.read_exact(&mut bytes)
            .map_err(|_| stale("source truncated during capture"))?;
        let after = self
            .checked_metadata(path)?
            .ok_or_else(|| stale("source disappeared during capture"))?;
        if Identity::of(&after) != Identity::of(&before)
            || after.len() < before.len()
            || (after.len() == before.len() && after.modified().ok() != before.modified().ok())
        {
            return Err(stale("source changed during capture"));
        }
        self.evidence.bytes_read += len;
        Ok(Some(bytes))
    }
    fn lines(&mut self, path: &Path) -> ApiResult<Option<String>> {
        let Some(mut bytes) = self.bytes(path)? else {
            return Ok(None);
        };
        let boundary = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
        self.evidence.incomplete_tail_bytes += bytes.len() - boundary;
        bytes.truncate(boundary);
        let text = String::from_utf8(bytes)
            .map_err(|_| ApiError::conflict("children source contains invalid UTF-8"))?;
        for line in text.lines() {
            if line.len() > MAX_LINE_BYTES {
                return Err(too_large("source row exceeds 256 KiB"));
            }
            if !line.trim().is_empty() {
                self.evidence.source_rows += 1;
            }
            if self.evidence.source_rows > MAX_SOURCE_ROWS {
                return Err(too_large("source row budget exceeded"));
            }
        }
        Ok(Some(text))
    }
    fn legacy_ids(&mut self, parent: &str) -> ApiResult<Vec<String>> {
        let path = self.root.join(parent).join("sub");
        let Some(m) = self.checked_metadata(&path)? else {
            return Ok(Vec::new());
        };
        if !m.is_dir() {
            return Err(ApiError::bad_request(
                "legacy children source is not a directory",
            ));
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(path).map_err(|e| ApiError::internal(e.to_string()))? {
            self.evidence.directory_entries_scanned += 1;
            if self.evidence.directory_entries_scanned > MAX_ENTRIES {
                return Err(too_large("directory entry budget exceeded"));
            }
            let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
            let kind = entry
                .file_type()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            if kind.is_symlink() {
                return Err(ApiError::bad_request(
                    "symlinked legacy child is not supported",
                ));
            }
            if !kind.is_dir() {
                continue;
            }
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| ApiError::bad_request("invalid legacy child ID"))?;
            valid_id(&id)?;
            ids.push(id);
        }
        ids.sort();
        Ok(ids)
    }
    fn legacy(&mut self, parent: &str, id: &str, depth: u32) -> ApiResult<ChildDto> {
        let path = self
            .root
            .join(parent)
            .join("sub")
            .join(id)
            .join("transcript.jsonl");
        let text = self.lines(&path)?;
        if text.is_none() {
            self.evidence.missing_legacy_transcripts += 1;
        }
        let mut start = None;
        let mut complete = None;
        for line in text
            .as_deref()
            .unwrap_or_default()
            .lines()
            .filter(|s| !s.trim().is_empty())
        {
            match serde_json::from_str::<Event>(line) {
                Ok(e @ Event::RunStart { .. }) if start.is_none() => start = Some(e),
                Ok(e @ Event::RunComplete { .. }) => complete = Some(e),
                Ok(_) => {}
                Err(_) => self.evidence.malformed_rows += 1,
            }
        }
        let mut row = ChildDto {
            handle: None,
            kind: "subagent".into(),
            name: String::new(),
            run_id: id.into(),
            parent_run_id: parent.into(),
            depth,
            codename: None,
            address: None,
            status: "running".into(),
            success: None,
            error: None,
            tokens: None,
            duration_ms: None,
            started_at: None,
            finished_at: None,
            transcript_path: Some(path.display().to_string()),
            legacy: true,
            round: None,
            generated: false,
            output_excerpt: None,
        };
        if let Some(Event::RunStart {
            agent,
            codename,
            started_at,
            ..
        }) = start
        {
            row.name = agent;
            row.codename = codename;
            row.started_at = Some(started_at.to_rfc3339());
            row.duration_ms = Some(0);
            if let Some(Event::RunComplete {
                status,
                duration_ms,
                error,
                ..
            }) = complete
            {
                row.duration_ms = Some(duration_ms);
                row.error = error;
                match status {
                    RunStatus::Ok => {
                        row.status = "done".into();
                        row.success = Some(true);
                    }
                    RunStatus::Error => {
                        row.status = "done".into();
                        row.success = Some(false);
                    }
                    _ => {}
                }
            }
        }
        Ok(row)
    }
}

fn enqueue(queue: &mut VecDeque<Parent>, parent: Parent) -> ApiResult<()> {
    valid_id(&parent.run_id)?;
    if queue.len() >= MAX_PENDING {
        return Err(too_large("pending run directory budget exceeded"));
    }
    queue.push_back(parent);
    Ok(())
}

/// Only stores belonging to this connected owner are candidates. Archiving a
/// workflow moves its root directory, not its separately stored step agents.
fn child_stores(owner: &Owner) -> ApiResult<[PathBuf; 2]> {
    let stores = [
        owner.runs.clone(),
        owner.runs.with_file_name("runs-archive"),
    ];
    for store in &stores {
        match fs::symlink_metadata(store) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(ApiError::bad_request(
                    "child run store must be a directory without symlinks",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ApiError::internal(error.to_string())),
        }
    }
    Ok(stores)
}

fn select_child_store(
    reader: &mut Reader,
    stores: &[PathBuf; 2],
    run: &str,
) -> ApiResult<Option<(PathBuf, Identity)>> {
    let mut selected = None;
    for store in stores {
        reader.root = store.clone();
        let directory = store.join(run);
        let Some(metadata) = reader.checked_metadata(&directory)? else {
            continue;
        };
        if !metadata.is_dir() {
            return Err(ApiError::bad_request(
                "recorded child source is not a directory",
            ));
        }
        if selected.is_some() {
            return Err(ApiError::conflict("recorded child exists in both active and archive stores; refresh after the ownership conflict is resolved"));
        }
        selected = Some((directory, Identity::of(&metadata)));
    }
    reader.root = selected
        .as_ref()
        .and_then(|(directory, _)| directory.parent())
        .map_or_else(|| stores[0].clone(), Path::to_path_buf);
    Ok(selected)
}

fn validate_child_directory(selected: Option<&(PathBuf, Identity)>) -> ApiResult<()> {
    if let Some((directory, identity)) = selected {
        let metadata = fs::symlink_metadata(directory)
            .map_err(|_| stale("child directory moved during capture"))?;
        if !metadata.is_dir() || Identity::of(&metadata) != *identity {
            return Err(stale("child directory was replaced during capture"));
        }
    }
    Ok(())
}

pub(super) fn capture(
    owner: Owner,
    id: &str,
    parent_filter: Option<String>,
) -> ApiResult<Snapshot> {
    valid_id(id)?;
    let stores = child_stores(&owner)?;
    let (mut reader, proof) = root::resolve(&owner, id)?;
    let root_store = reader.root.clone();
    let root = root_store.join(id);
    let mut queue = VecDeque::from([Parent::root(id)]);
    if let Some(text) = if proof.kind == "agentiflow" { None } else { reader.lines(&root.join("step_results.jsonl"))? } {
        for line in text.lines().filter(|s| !s.trim().is_empty()) {
            match serde_json::from_str::<StepResultRecord>(line) {
                Ok(record) => {
                    if record.host.as_deref().is_none_or(|h| h == "local") {
                        if !record.run_id.is_empty() {
                            enqueue(&mut queue, Parent::root(&record.run_id))?;
                        }
                    } else {
                        reader.evidence.remote_step_roots_not_walked += 1;
                    }
                    for item in record.items {
                        if record.host.as_deref().is_none_or(|h| h == "local") {
                            if !item.run_id.is_empty() {
                                enqueue(&mut queue, Parent::root(&item.run_id))?;
                            }
                        } else {
                            reader.evidence.remote_step_roots_not_walked += 1;
                        }
                    }
                }
                Err(_) => reader.evidence.malformed_rows += 1,
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut latest = BTreeMap::new();
    let mut weight = 64 * 1024usize;
    while let Some(Parent {
        run_id,
        depth,
        round,
    }) = queue.pop_front()
    {
        if !seen.insert(run_id.clone()) {
            continue;
        }
        reader.evidence.run_directories_scanned += 1;
        if seen.len() > MAX_DIRECTORIES {
            return Err(too_large("run directory budget exceeded"));
        }
        let selected = if run_id == id {
            // The requested root remains pinned to the store whose canonical
            // ownership proof was validated; it is never replaced by a sibling.
            reader.root = root_store.clone();
            reader
                .checked_metadata(&root)?
                .map(|metadata| (root.clone(), Identity::of(&metadata)))
        } else {
            select_child_store(&mut reader, &stores, &run_id)?
        };
        let text = reader
            .lines(&reader.root.join(&run_id).join("children.jsonl"))?
            .unwrap_or_default();
        reader.evidence.malformed_rows += text
            .lines()
            .filter(|l| !l.trim().is_empty() && serde_json::from_str::<LedgerLine>(l).is_err())
            .count();
        let entries = ledger::read_captured(&text);
        let known: BTreeSet<_> = entries.iter().map(|e| e.started.run_id.clone()).collect();
        for entry in entries {
            if proof.kind == "agentiflow" && run_id == id {
                root::validate_agentiflow_edge(&entry.started, &root)?;
            }
            let at = entry.started.at;
            let mut child = from_entry(entry, &run_id);
            if let Some(round) = round {
                child.round = round;
            }
            enqueue(&mut queue, Parent::below(&child))?;
            if let Some(handle) = child.handle.clone() {
                if latest
                    .get(&handle)
                    .is_some_and(|&(_, previous)| previous >= at)
                {
                    continue;
                }
                latest.insert(handle, (out.len(), at));
            }
            weight = weight.saturating_add(row_weight(&child)?);
            if weight > MAX_SNAPSHOT_BYTES {
                return Err(too_large("materialized rows exceed 16 MiB"));
            }
            out.push(child);
        }
        let legacy = if proof.kind == "agentiflow" && run_id == id { Vec::new() } else { reader.legacy_ids(&run_id)? };
        for sub in legacy {
            if known.contains(&sub) {
                continue;
            }
            let mut child = reader.legacy(&run_id, &sub, depth.saturating_add(1))?;
            child.round = round.flatten();
            enqueue(&mut queue, Parent::below(&child))?;
            weight = weight.saturating_add(row_weight(&child)?);
            if weight > MAX_SNAPSHOT_BYTES {
                return Err(too_large("materialized rows exceed 16 MiB"));
            }
            out.push(child);
        }
        validate_child_directory(selected.as_ref())?;
    }
    if parent_filter
        .as_ref()
        .is_some_and(|parent| !seen.contains(parent))
    {
        return Err(ApiError::not_found(
            "parent is not recorded in this root's local launch tree",
        ));
    }
    let keep: BTreeSet<usize> = latest.values().map(|&(i, _)| i).collect();
    let mut rows: Vec<_> = out
        .into_iter()
        .enumerate()
        .filter(|(i, child)| {
            (child.handle.is_none() || keep.contains(i))
                && parent_filter
                    .as_ref()
                    .is_none_or(|p| p == &child.parent_run_id)
        })
        .map(|(_, child)| child)
        .collect();
    rows.shrink_to_fit();
    let weights: Vec<_> = rows.iter().map(row_weight).collect::<ApiResult<_>>()?;
    let weight = 64 * 1024 + weights.iter().sum::<usize>();
    proof.validate()?;
    Ok(Snapshot {
        id: String::new(),
        owner,
        run_id: id.into(),
        parent: parent_filter,
        proof,
        created: Instant::now(),
        rows,
        weights,
        weight,
        evidence: reader.evidence,
    })
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn selected_child_directory_move_or_replacement_invalidates_capture() {
        let tmp = tempfile::tempdir().unwrap();
        let active = tmp.path().join("runs");
        let archive = tmp.path().join("runs-archive");
        fs::create_dir_all(active.join("run_child")).unwrap();
        let mut reader = Reader::new(active.clone());
        let selected =
            select_child_store(&mut reader, &[active.clone(), archive], "run_child").unwrap();
        fs::rename(active.join("run_child"), active.join("moved")).unwrap();
        fs::create_dir(active.join("run_child")).unwrap();
        assert_eq!(
            validate_child_directory(selected.as_ref()).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
    }

    #[test]
    fn cumulative_bytes_rows_and_directory_entries_are_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(root.join("small"), b"{}\n{}\n").unwrap();
        let mut reader = Reader::new(root.clone());
        reader.evidence.bytes_read = MAX_SOURCE_BYTES - 1;
        assert_eq!(
            reader.bytes(&root.join("small")).unwrap_err().0,
            axum::http::StatusCode::PAYLOAD_TOO_LARGE
        );
        reader.evidence.bytes_read = 0;
        reader.evidence.source_rows = MAX_SOURCE_ROWS - 1;
        assert_eq!(
            reader.lines(&root.join("small")).unwrap_err().0,
            axum::http::StatusCode::PAYLOAD_TOO_LARGE
        );
        fs::create_dir_all(root.join("run_parent/sub/sub_one")).unwrap();
        fs::create_dir_all(root.join("run_parent/sub/sub_two")).unwrap();
        reader.evidence.directory_entries_scanned = MAX_ENTRIES - 1;
        assert_eq!(
            reader.legacy_ids("run_parent").unwrap_err().0,
            axum::http::StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
