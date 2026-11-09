//! Direct-child catalogs authorized by a sealed transcript window, never caller paths.
use super::{
    member::{Authority, Hop, VerifiedMember},
    transcript_follow,
};
use crate::api::transcript::page::{check_stamp, Stamp};
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const MAX_LEDGER: usize = 4 * 1024 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_ROWS: usize = 20_000;
const MAX_LEGACY: usize = 256;
const MAX_IO: usize = 8 * 1024 * 1024;
const MAX_SNAPSHOT: usize = 2 * 1024 * 1024;
const MAX_CACHE: usize = 16 * 1024 * 1024;
const MAX_REPLY: usize = 1024 * 1024;
const TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Edge {
    Ledger {
        directory: String,
        offset: u64,
        end: u64,
        sha256: String,
        device: u64,
        inode: u64,
        handle: String,
    },
    Legacy {
        directory: String,
    },
}
#[derive(Clone, Serialize)]
pub(super) struct Row {
    record_id: String,
    source: &'static str,
    child_run_id: String,
    handle: Option<String>,
    kind: String,
    name: String,
    recorded_status: Option<String>,
    transcript_available: bool,
    reason: Option<String>,
    #[serde(skip)]
    hop: Hop,
}
#[derive(Clone, Serialize, Default)]
struct Evidence {
    ledger_state: String,
    legacy_state: String,
    malformed_rows: usize,
    omitted_total: Option<usize>,
}
struct Snapshot {
    id: String,
    authority: Authority,
    parent_identity: String,
    rows: Vec<Row>,
    evidence: Evidence,
    created: Instant,
    observed: chrono::DateTime<chrono::Utc>,
    bytes: usize,
}
pub(super) struct Cache {
    key: [u8; 32],
    instance: String,
    snapshots: Mutex<Vec<Arc<Snapshot>>>,
}
impl Default for Cache {
    fn default() -> Self {
        let mut key = [0; 32];
        for chunk in key.chunks_mut(8) {
            chunk.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        Self {
            key,
            instance: ulid::Ulid::new().to_string(),
            snapshots: Mutex::new(Vec::new()),
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    snapshot: String,
    position: usize,
}
#[derive(Serialize)]
pub(super) struct Reply {
    schema: &'static str,
    workflow_run_id: String,
    workspace_id: String,
    host_id: &'static str,
    parent_run_id: String,
    parent_path: String,
    parent_file_identity: String,
    instance_id: String,
    snapshot: String,
    depth: usize,
    observed_at: chrono::DateTime<chrono::Utc>,
    records: Vec<Row>,
    next_cursor: Option<String>,
    captured_total: usize,
    source_complete: bool,
    evidence: Evidence,
    limits: Value,
}
pub(super) struct Query {
    pub parent_cursor: String,
    pub cursor: Option<String>,
    pub limit: usize,
}
pub(super) struct Selection {
    pub parent_cursor: String,
    pub snapshot: String,
    pub record_id: String,
}
fn fail(code: &str) -> ApiError {
    ApiError::conflict(code)
}
fn large() -> ApiError {
    ApiError(
        StatusCode::PAYLOAD_TOO_LARGE,
        "descendant_catalog_limit".into(),
    )
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn same(a: &Authority, b: &Authority) -> bool {
    serde_json::to_vec(a).ok() == serde_json::to_vec(b).ok()
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
struct Work<'a> {
    cancel: &'a CancellationToken,
    start: Instant,
    bytes: usize,
}
impl<'a> Work<'a> {
    fn new(cancel: &'a CancellationToken) -> Self {
        Self {
            cancel,
            start: Instant::now(),
            bytes: 0,
        }
    }
    fn check(&self) -> ApiResult<()> {
        if self.cancel.is_cancelled() {
            return Err(ApiError(
                StatusCode::REQUEST_TIMEOUT,
                "descendant_cancelled".into(),
            ));
        }
        if self.start.elapsed() > Duration::from_secs(5) {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "descendant_work_limit".into(),
            ));
        }
        Ok(())
    }
    fn read(&mut self, file: &mut File, start: u64, len: usize) -> ApiResult<Vec<u8>> {
        self.check()?;
        if len > MAX_LEDGER || self.bytes.checked_add(len).is_none_or(|n| n > MAX_IO) {
            return Err(large());
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|_| fail("descendant_source_changed"))?;
        let mut bytes = vec![0; len];
        for part in bytes.chunks_mut(16 * 1024) {
            self.check()?;
            file.read_exact(part)
                .map_err(|_| fail("descendant_source_changed"))?;
            self.bytes += part.len();
        }
        self.check()?;
        Ok(bytes)
    }
}
fn sign(cache: &Cache, c: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(c).map_err(|_| large())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&cache.key).expect("fixed key");
    mac.update(&bytes);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
fn decode(cache: &Cache, token: &str) -> ApiResult<Cursor> {
    let invalid = || ApiError::bad_request("descendant_cursor_invalid");
    if token.len() > 2048 {
        return Err(invalid());
    }
    let (a, b) = token.split_once('.').ok_or_else(invalid)?;
    let bytes = URL_SAFE_NO_PAD.decode(a).map_err(|_| invalid())?;
    let sig = URL_SAFE_NO_PAD.decode(b).map_err(|_| invalid())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&cache.key).expect("fixed key");
    mac.update(&bytes);
    mac.verify_slice(&sig).map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

/// One exact optional registration per chain, held open through final verification.
/// Candidate run directories are still reopened no-follow for every edge proof.
pub(super) struct DirectoryScope {
    roots: Vec<PathBuf>,
    inline: BTreeMap<String, PathBuf>,
    registration: Option<File>,
    project: Option<String>,
}
pub(super) fn scope(s: &AppState, a: &Authority) -> ApiResult<DirectoryScope> {
    let global = s
        .global_dir
        .canonicalize()
        .map_err(|_| fail("descendant_global_unavailable"))?;
    let mut roots = vec![s.run_store.root.clone(), s.run_store.archive_root()];
    let mut registration = None;
    let mut project_path = None;
    match super::input::Input::open(&global.join("workspaces")) {
        Ok(mut input) => {
            match super::owner::registration(&mut input, &a.workspace_id) {
                Ok(project) => {
                    roots.push(PathBuf::from(&project.path).join(".rupu/runs"));
                    roots.push(PathBuf::from(&project.path).join(".rupu/runs-archive"));
                    project_path = Some(project.path);
                }
                Err(e) if e.0 == StatusCode::NOT_FOUND => {}
                Err(e) => return Err(e),
            }
            registration = Some(input.dir);
        }
        Err(e) if e.0 == StatusCode::NOT_FOUND => {}
        Err(e) => return Err(e),
    }
    let mut inline = BTreeMap::new();
    for hop in &a.ancestry {
        if !valid_id(&hop.child_run_id) {
            return Err(fail("descendant_child_invalid"));
        }
        let directory = match &hop.edge {
            Edge::Ledger { directory, .. } | Edge::Legacy { directory } => directory,
        };
        let child_dir = Path::new(directory).join("sub").join(&hop.child_run_id);
        if Path::new(&hop.path) == child_dir.join("transcript.jsonl") {
            if inline.insert(hop.child_run_id.clone(), child_dir).is_some() {
                return Err(fail("descendant_inline_ancestry_cycle"));
            }
        }
    }
    Ok(DirectoryScope {
        roots,
        inline,
        registration,
        project: project_path,
    })
}
impl DirectoryScope {
    pub(super) fn registered_project_root(&self) -> Option<&str> {
        self.project.as_deref()
    }
    pub(super) fn resolve(&self, parent: &str) -> ApiResult<Option<PathBuf>> {
        if !valid_id(parent) {
            return Err(fail("descendant_parent_invalid"));
        }
        let mut candidates = BTreeSet::new();
        let candidates_to_check = self
            .roots
            .iter()
            .map(|root| root.join(parent))
            .chain(self.inline.get(parent).cloned());
        for candidate in candidates_to_check {
            match super::input::Input::open(&candidate) {
                Ok(_) => {
                    candidates.insert(candidate);
                }
                Err(e) if e.0 == StatusCode::NOT_FOUND => {}
                Err(e) => return Err(e),
            }
        }
        if candidates.len() > 1 {
            return Err(fail("descendant_parent_store_ambiguous"));
        }
        Ok(candidates.into_iter().next())
    }
    pub(super) fn verify(&self, s: &AppState, a: &Authority) -> ApiResult<()> {
        let after = scope(s, a)?;
        let identity = |file: &Option<File>| -> ApiResult<Option<(u64, u64)>> {
            file.as_ref()
                .map(|f| {
                    f.metadata()
                        .map(|m| (m.dev(), m.ino()))
                        .map_err(|_| fail("descendant_registry_unavailable"))
                })
                .transpose()
        };
        if self.roots != after.roots
            || self.inline != after.inline
            || self.project != after.project
            || identity(&self.registration)? != identity(&after.registration)?
        {
            return Err(fail("descendant_registry_changed"));
        }
        Ok(())
    }
}
fn open(path: &Path) -> ApiResult<File> {
    crate::api::fs_open::open_anchored_regular_file(path)
        .map_err(|_| fail("descendant_source_unavailable"))
}
fn checked_stamp(path: &Path, file: &File, stamp: &Stamp) -> ApiResult<()> {
    check_stamp(path, file, stamp).map_err(stamp_error)
}
fn stamp_error(error: ApiError) -> ApiError {
    if error.0 == StatusCode::CONFLICT {
        fail("transcript_window_changed_during_read")
    } else {
        error
    }
}
/// Display-only label: trim on a UTF-8 boundary, never truncate an identity.
fn display_name(name: &str) -> String {
    let mut end = name.len().min(4096);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}
fn ledger_match(bytes: &[u8], hop: &Hop, handle: &str) -> ApiResult<()> {
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| fail("descendant_edge_malformed"))?;
    let rupu_launch::ledger::LedgerLine::Started(start) =
        serde_json::from_slice(bytes).map_err(|_| fail("descendant_edge_malformed"))?
    else {
        return Err(fail("descendant_edge_not_started"));
    };
    if start.run_id != hop.child_run_id
        || start.handle != handle
        || start.transcript.as_deref() != Some(Path::new(&hop.path))
        || start
            .request
            .as_ref()
            .and_then(|r| r.link.as_ref())
            .is_some_and(|l| l.parent_run != hop.parent_run_id)
        || [&raw["host"], &raw["host_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some("local"))
    {
        return Err(fail("descendant_edge_identity_changed"));
    }
    if serde_json::to_value(start.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .as_deref()
        == Some("workflow")
    {
        return Err(fail("descendant_workflow_requires_navigation"));
    }
    Ok(())
}
/// Called only with a MAC-authenticated or freshly cache-selected sealed Hop.
pub(super) fn validate_edge(
    s: &AppState,
    a: &Authority,
    hop: &Hop,
    cancel: &CancellationToken,
) -> ApiResult<()> {
    let scope = scope(s, a)?;
    validate_edge_scoped(&scope, hop, cancel)?;
    scope.verify(s, a)
}
pub(super) fn validate_edge_scoped(
    scope: &DirectoryScope,
    hop: &Hop,
    cancel: &CancellationToken,
) -> ApiResult<()> {
    let mut work = Work::new(cancel);
    work.check()?;
    let selected = scope
        .resolve(&hop.parent_run_id)?
        .ok_or_else(|| fail("descendant_parent_store_missing"))?;
    match &hop.edge {
        Edge::Ledger {
            directory,
            offset,
            end,
            sha256,
            device,
            inode,
            handle,
        } => {
            if selected.to_str() != Some(directory)
                || end <= offset
                || end - offset > MAX_LINE as u64
            {
                return Err(fail("descendant_edge_locator_changed"));
            }
            let path = selected.join("children.jsonl");
            let mut file = open(&path)?;
            let metadata = file
                .metadata()
                .map_err(|_| fail("descendant_edge_unavailable"))?;
            if metadata.dev() != *device || metadata.ino() != *inode || metadata.len() < *end {
                return Err(fail("descendant_edge_replaced"));
            }
            let stamp = Stamp::of(&metadata)?;
            let bytes = work.read(&mut file, *offset, (*end - *offset) as usize)?;
            if !bytes.ends_with(b"\n") || hash(&bytes) != *sha256 {
                return Err(fail("descendant_edge_changed"));
            }
            ledger_match(&bytes, hop, handle)?;
            checked_stamp(&path, &file, &stamp)?;
        }
        Edge::Legacy { directory } => {
            if selected.to_str() != Some(directory)
                || !valid_id(&hop.child_run_id)
                || Path::new(&hop.path)
                    != selected
                        .join("sub")
                        .join(&hop.child_run_id)
                        .join("transcript.jsonl")
            {
                return Err(fail("descendant_legacy_locator_changed"));
            }
            let _ = open(Path::new(&hop.path))?;
        }
    }
    if scope.resolve(&hop.parent_run_id)?.as_ref() != Some(&selected) {
        return Err(fail("descendant_parent_store_changed"));
    }
    work.check()
}
fn make_row(
    parent: &str,
    start: &rupu_launch::ledger::Started,
    edge: Edge,
    status: Option<String>,
    source: &'static str,
) -> ApiResult<Option<Row>> {
    if !valid_id(&start.run_id) || start.handle.is_empty() || start.handle.len() > 256 {
        return Ok(None);
    }
    let kind = serde_json::to_value(start.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let path = start
        .transcript
        .as_ref()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_string();
    let reason = if kind == "workflow" {
        Some("workflow_navigation_required".into())
    } else if path.is_empty() || path.len() > 4096 {
        Some("identity_or_transcript_unavailable".into())
    } else {
        None
    };
    let hop = Hop {
        parent_run_id: parent.into(),
        child_run_id: start.run_id.clone(),
        path,
        prologue_sha256: String::new(),
        parent_file_epoch: String::new(),
        edge,
    };
    let record_id = format!(
        "child:{}",
        hash(&serde_json::to_vec(&hop).map_err(|_| large())?)
    );
    Ok(Some(Row {
        record_id,
        source,
        child_run_id: start.run_id.clone(),
        handle: Some(start.handle.clone()),
        kind,
        name: display_name(&start.name),
        recorded_status: status,
        transcript_available: reason.is_none(),
        reason,
        hop,
    }))
}
fn capture(
    s: &AppState,
    parent: &VerifiedMember,
    identity: String,
    cancel: &CancellationToken,
) -> ApiResult<Snapshot> {
    let mut work = Work::new(cancel);
    let scope = scope(s, &parent.authority)?;
    let mut evidence = Evidence {
        ledger_state: "absent".into(),
        legacy_state: "absent".into(),
        ..Evidence::default()
    };
    let mut rows = Vec::new();
    if let Some(dir) = scope.resolve(parent.authority.leaf_run_id())? {
        let ledger = dir.join("children.jsonl");
        match std::fs::symlink_metadata(&ledger) {
            Ok(m) => {
                if !m.is_file() {
                    return Err(fail("descendant_ledger_not_regular"));
                }
                let mut file = open(&ledger)?;
                let meta = file
                    .metadata()
                    .map_err(|_| fail("descendant_ledger_unavailable"))?;
                let stamp = Stamp::of(&meta)?;
                if meta.len() > MAX_LEDGER as u64 {
                    evidence.ledger_state = "oversized".into();
                } else {
                    let mut bytes = work.read(&mut file, 0, meta.len() as usize)?;
                    checked_stamp(&ledger, &file, &stamp)?;
                    let complete = bytes.last() == Some(&b'\n') || bytes.is_empty();
                    if !complete {
                        bytes
                            .truncate(bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1));
                    }
                    let count = bytes.iter().filter(|b| **b == b'\n').count();
                    if count > MAX_ROWS {
                        evidence.ledger_state = "row_limit".into();
                    } else {
                        let mut starts = BTreeMap::new();
                        let mut text = String::new();
                        let mut offset = 0u64;
                        for line in bytes.split_inclusive(|b| *b == b'\n') {
                            work.check()?;
                            let end = offset + line.len() as u64;
                            if line.len() > MAX_LINE {
                                evidence.malformed_rows += 1;
                            } else {
                                match serde_json::from_slice::<rupu_launch::ledger::LedgerLine>(
                                    line,
                                ) {
                                    Ok(value) => {
                                        if let rupu_launch::ledger::LedgerLine::Started(v) = &value
                                        {
                                            starts.insert(
                                                v.handle.clone(),
                                                (offset, end, hash(line)),
                                            );
                                        }
                                        text.push_str(
                                            std::str::from_utf8(line)
                                                .map_err(|_| fail("descendant_ledger_encoding"))?,
                                        );
                                    }
                                    Err(_) => {
                                        if line.iter().any(|b| !b.is_ascii_whitespace()) {
                                            evidence.malformed_rows += 1;
                                        }
                                    }
                                }
                            }
                            offset = end;
                        }
                        for entry in rupu_launch::ledger::read_captured(&text) {
                            work.check()?;
                            let Some((offset, end, sha256)) = starts.get(&entry.started.handle)
                            else {
                                continue;
                            };
                            let edge = Edge::Ledger {
                                directory: dir.to_string_lossy().into(),
                                offset: *offset,
                                end: *end,
                                sha256: sha256.clone(),
                                device: meta.dev(),
                                inode: meta.ino(),
                                handle: entry.started.handle.clone(),
                            };
                            // Only explicit completion state is projected; an absent Finished is unknown.
                            let status = entry
                                .finished
                                .as_ref()
                                .and_then(|f| serde_json::to_value(f.status).ok())
                                .and_then(|v| v.as_str().map(str::to_owned));
                            match make_row(
                                parent.authority.leaf_run_id(),
                                &entry.started,
                                edge,
                                status,
                                "ledger",
                            )? {
                                Some(row) => rows.push(row),
                                None => evidence.malformed_rows += 1,
                            }
                        }
                        evidence.ledger_state =
                            if complete { "captured" } else { "partial_tail" }.into();
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(fail("descendant_ledger_unavailable")),
        }
        let sub = dir.join("sub");
        match super::input::Input::open(&sub) {
            Ok(anchor) => {
                let before = anchor
                    .dir
                    .metadata()
                    .map_err(|_| fail("descendant_legacy_unavailable"))?;
                let mut names = Vec::new();
                for entry in std::fs::read_dir(&sub)
                    .map_err(|_| fail("descendant_legacy_unavailable"))?
                    .take(MAX_LEGACY + 1)
                {
                    work.check()?;
                    let entry = entry.map_err(|_| fail("descendant_legacy_unavailable"))?;
                    names.push(entry.file_name());
                }
                let limited = names.len() > MAX_LEGACY;
                names.truncate(MAX_LEGACY);
                names.sort();
                for name in names {
                    work.check()?;
                    let Some(child) = name.to_str().filter(|s| valid_id(s)) else {
                        evidence.malformed_rows += 1;
                        continue;
                    };
                    if rows.iter().any(|r| r.child_run_id == child) {
                        continue;
                    }
                    let path = sub.join(&name).join("transcript.jsonl");
                    let mut file = match open(&path) {
                        Ok(f) => f,
                        Err(_) => {
                            evidence.malformed_rows += 1;
                            continue;
                        }
                    };
                    let metadata = file
                        .metadata()
                        .map_err(|_| fail("descendant_legacy_unavailable"))?;
                    let stamp = Stamp::of(&metadata)?;
                    let bytes =
                        work.read(&mut file, 0, (metadata.len() as usize).min(64 * 1024))?;
                    let Some(end) = bytes.iter().position(|b| *b == b'\n') else {
                        evidence.malformed_rows += 1;
                        continue;
                    };
                    let prologue: Value = match serde_json::from_slice(&bytes[..=end]) {
                        Ok(v) => v,
                        Err(_) => {
                            evidence.malformed_rows += 1;
                            continue;
                        }
                    };
                    let event = serde_json::from_value::<rupu_transcript::Event>(prologue.clone());
                    let Ok(rupu_transcript::Event::RunStart {
                        run_id,
                        workspace_id,
                        agent,
                        ..
                    }) = event
                    else {
                        evidence.malformed_rows += 1;
                        continue;
                    };
                    if run_id != child
                        || workspace_id != parent.authority.workspace_id
                        || [
                            &prologue["host"],
                            &prologue["host_id"],
                            &prologue["data"]["host_id"],
                        ]
                        .iter()
                        .any(|v| !v.is_null() && v.as_str() != Some("local"))
                    {
                        evidence.malformed_rows += 1;
                        continue;
                    }
                    checked_stamp(&path, &file, &stamp)?;
                    let hop = Hop {
                        parent_run_id: parent.authority.leaf_run_id().into(),
                        child_run_id: child.into(),
                        path: path.to_string_lossy().into(),
                        prologue_sha256: String::new(),
                        parent_file_epoch: String::new(),
                        edge: Edge::Legacy {
                            directory: dir.to_string_lossy().into(),
                        },
                    };
                    rows.push(Row {
                        record_id: format!(
                            "child:{}",
                            hash(&serde_json::to_vec(&hop).map_err(|_| large())?)
                        ),
                        source: "legacy",
                        child_run_id: child.into(),
                        handle: None,
                        kind: "subagent".into(),
                        name: display_name(&agent),
                        recorded_status: None,
                        transcript_available: true,
                        reason: None,
                        hop,
                    });
                }
                let after = super::input::Input::open(&sub)?
                    .dir
                    .metadata()
                    .map_err(|_| fail("descendant_legacy_unavailable"))?;
                if before.dev() != after.dev() || before.ino() != after.ino() {
                    return Err(fail("descendant_legacy_replaced"));
                }
                evidence.legacy_state = if limited { "entry_limit" } else { "captured" }.into();
            }
            Err(e) if e.0 == StatusCode::NOT_FOUND => {}
            Err(e) => return Err(e),
        }
        if scope.resolve(parent.authority.leaf_run_id())?.as_ref() != Some(&dir) {
            return Err(fail("descendant_parent_store_changed"));
        }
    }
    if scope.resolve(parent.authority.leaf_run_id())?.is_none() {
        evidence.ledger_state = "parent_store_absent".into();
        evidence.legacy_state = "parent_store_absent".into();
    }
    work.check()?;
    let bytes = serde_json::to_vec(&rows).map_err(|_| large())?.len()
        + rows
            .iter()
            .map(|r| serde_json::to_vec(&r.hop).map_or(MAX_SNAPSHOT, |v| v.len()))
            .sum::<usize>();
    if bytes > MAX_SNAPSHOT {
        return Err(large());
    }
    let fresh = super::member::reauthorize_with_cancel(s, &parent.authority, cancel)?;
    if !same(&fresh.authority, &parent.authority) {
        return Err(fail("descendant_parent_changed"));
    }
    scope.verify(s, &parent.authority)?;
    work.check()?;
    Ok(Snapshot {
        id: ulid::Ulid::new().to_string(),
        authority: parent.authority.clone(),
        parent_identity: identity,
        rows,
        evidence,
        created: Instant::now(),
        observed: chrono::Utc::now(),
        bytes,
    })
}
fn publish(cache: &Cache, snapshot: Arc<Snapshot>, work: &Work<'_>) -> ApiResult<()> {
    let mut snapshots = cache
        .snapshots
        .lock()
        .map_err(|_| fail("descendant_cache_unavailable"))?;
    // Cancellation may arrive while waiting for the cache mutex. Check before
    // any eviction/publication, while the mutation transaction is exclusive.
    work.check()?;
    snapshots.retain(|s| s.created.elapsed() < TTL);
    while snapshots.len() >= 32
        || snapshots.iter().map(|s| s.bytes).sum::<usize>() + snapshot.bytes > MAX_CACHE
    {
        snapshots.remove(0);
    }
    snapshots.push(snapshot);
    Ok(())
}
pub(super) fn catalog(
    s: &AppState,
    workflow: &str,
    workspace: &str,
    q: Query,
    cancel: &CancellationToken,
) -> ApiResult<Reply> {
    let request_work = Work::new(cancel);
    request_work.check()?;
    if !(1..=50).contains(&q.limit) {
        return Err(ApiError::bad_request("descendant_page_limit_invalid"));
    }
    let (parent, identity) =
        transcript_follow::authorize_parent(s, workflow, workspace, &q.parent_cursor, cancel)?;
    let cache = &s.workflow_observations.descendants;
    let continuation = q.cursor.as_deref().map(|v| decode(cache, v)).transpose()?;
    let (snapshot, position) = if let Some(c) = continuation {
        let mut snapshots = cache
            .snapshots
            .lock()
            .map_err(|_| fail("descendant_cache_unavailable"))?;
        snapshots.retain(|s| s.created.elapsed() < TTL);
        let snapshot = snapshots
            .iter()
            .find(|s| s.id == c.snapshot)
            .cloned()
            .ok_or_else(|| fail("descendant_snapshot_expired"))?;
        (snapshot, c.position)
    } else {
        let snapshot = Arc::new(capture(s, &parent, identity.clone(), cancel)?);
        publish(cache, snapshot.clone(), &request_work)?;
        (snapshot, 0)
    };
    if !same(&snapshot.authority, &parent.authority) || snapshot.parent_identity != identity {
        return Err(fail("descendant_snapshot_owner_mismatch"));
    }
    if position > snapshot.rows.len() {
        return Err(ApiError::bad_request("descendant_cursor_position_invalid"));
    }
    let end = (position + q.limit).min(snapshot.rows.len());
    let complete = matches!(
        snapshot.evidence.ledger_state.as_str(),
        "absent" | "captured"
    ) && matches!(
        snapshot.evidence.legacy_state.as_str(),
        "absent" | "captured"
    ) && snapshot.evidence.malformed_rows == 0;
    let reply = Reply {
        schema: "workflow.transcript_children.v1",
        workflow_run_id: workflow.into(),
        workspace_id: workspace.into(),
        host_id: "local",
        parent_run_id: parent.authority.leaf_run_id().into(),
        parent_path: parent.authority.leaf_path().into(),
        parent_file_identity: identity,
        instance_id: cache.instance.clone(),
        snapshot: snapshot.id.clone(),
        depth: parent.authority.ancestry.len(),
        observed_at: snapshot.observed,
        records: snapshot.rows[position..end].to_vec(),
        next_cursor: if end < snapshot.rows.len() {
            Some(sign(
                cache,
                &Cursor {
                    snapshot: snapshot.id.clone(),
                    position: end,
                },
            )?)
        } else {
            None
        },
        captured_total: snapshot.rows.len(),
        source_complete: complete,
        evidence: snapshot.evidence.clone(),
        limits: json!({"ledger_bytes":MAX_LEDGER,"source_bytes":MAX_IO,"ledger_rows":MAX_ROWS,"legacy_entries":MAX_LEGACY,"page_records":50,"reply_bytes":MAX_REPLY,"depth":16,"snapshot_ttl_seconds":600}),
    };
    if serde_json::to_vec(&reply).map_err(|_| large())?.len() > MAX_REPLY {
        return Err(large());
    }
    request_work.check()?;
    Ok(reply)
}
pub(super) fn select(
    s: &AppState,
    workflow: &str,
    workspace: &str,
    q: Selection,
    cancel: &CancellationToken,
) -> ApiResult<VerifiedMember> {
    let request_work = Work::new(cancel);
    request_work.check()?;
    if q.snapshot.len() > 64 || q.record_id.len() > 80 {
        return Err(ApiError::bad_request("descendant_selector_invalid"));
    }
    let (parent, identity) =
        transcript_follow::authorize_parent(s, workflow, workspace, &q.parent_cursor, cancel)?;
    if parent.authority.ancestry.len() >= 16 {
        return Err(fail("descendant_depth_limit"));
    }
    let cache = &s.workflow_observations.descendants;
    let hop = {
        let mut snapshots = cache
            .snapshots
            .lock()
            .map_err(|_| fail("descendant_cache_unavailable"))?;
        snapshots.retain(|s| s.created.elapsed() < TTL);
        let snapshot = snapshots
            .iter()
            .find(|s| s.id == q.snapshot)
            .ok_or_else(|| fail("descendant_snapshot_expired"))?;
        if !same(&snapshot.authority, &parent.authority) || snapshot.parent_identity != identity {
            return Err(fail("descendant_snapshot_owner_mismatch"));
        }
        let row = snapshot
            .rows
            .iter()
            .find(|r| r.record_id == q.record_id)
            .ok_or_else(|| ApiError::not_found("descendant_record_missing"))?;
        if !row.transcript_available {
            return Err(fail(
                row.reason.as_deref().unwrap_or("descendant_unavailable"),
            ));
        }
        row.hop.clone()
    };
    let member = super::member::descendant(s, &parent.authority, hop, cancel)?;
    request_work.check()?;
    Ok(member)
}

#[cfg(test)]
mod tests;
