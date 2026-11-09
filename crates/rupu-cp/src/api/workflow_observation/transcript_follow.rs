//! Server-owned bounded transcript windows. No legacy SSE/line cursor conversion.
//! The parent module alone constructs member authority and exposes the HTTP route.
use super::member::{Authority, VerifiedMember};
use crate::{
    api::transcript::page::{read_line, Stamp},
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
    fs::File,
    io::{Read, Seek, SeekFrom},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

pub(super) const SCHEMA: &str = "transcript.window_follow.v1";
const MAX_WINDOW: u64 = 512 * 1024;
const MAX_REPLY: usize = 1024 * 1024;
const MAX_CURSOR: usize = 32 * 1024;
const MAX_RECORDS: usize = 200;

pub(super) struct Cache {
    key: [u8; 32],
    instance: String,
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
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Latest,
    Poll,
    Earlier,
}
pub(super) struct Query {
    pub action: Action,
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileIdentity {
    device: u64,
    inode: u64,
    created: Option<u128>,
}
impl FileIdentity {
    fn of(file: &File) -> ApiResult<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(|_| changed())?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            created: metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Anchor {
    start: u64,
    end: u64,
    hash: String,
    before: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Cursor {
    version: u8,
    instance: String,
    authority: Authority,
    identity: FileIdentity,
    stamp: Stamp,
    snapshot: String,
    anchor: Anchor,
}
#[derive(Serialize)]
pub(super) struct Reply {
    schema: &'static str,
    workflow_run_id: String,
    workspace_id: String,
    child_run_id: String,
    host_id: &'static str,
    path: String,
    instance_id: String,
    file_identity: String,
    snapshot: String,
    previous_snapshot: Option<String>,
    state: &'static str,
    relationship: &'static str,
    observed_at: chrono::DateTime<chrono::Utc>,
    prologue: Value,
    window: Option<Window>,
    records: Vec<Record>,
    poll_cursor: String,
    earlier_cursor: Option<String>,
    limits: Value,
}
#[derive(Serialize)]
struct Window {
    source_bytes: String,
    observed_start_byte: String,
    observed_end_byte: String,
    display_start_byte: String,
    display_end_byte: String,
    omitted_before_bytes: String,
    omitted_after_bytes: String,
    partial_tail_bytes: String,
    requested_before_byte: Option<String>,
    complete_source: bool,
    boundary_available: bool,
}
#[derive(Serialize)]
struct Record {
    identity: String,
    byte_offset: String,
    byte_end: String,
    status: &'static str,
    content_sha256: String,
    event: Option<Value>,
}
struct Work {
    cancel: CancellationToken,
    started: Instant,
    bytes: usize,
}
impl Work {
    fn check(&self) -> ApiResult<()> {
        if self.cancel.is_cancelled() {
            return Err(error(
                StatusCode::REQUEST_TIMEOUT,
                "transcript_window_cancelled",
            ));
        }
        if self.started.elapsed() > std::time::Duration::from_secs(5) {
            return Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "transcript_window_work_limit",
            ));
        }
        Ok(())
    }
    fn interval(&mut self, file: &mut File, start: u64, end: u64) -> ApiResult<Vec<u8>> {
        self.check()?;
        if end < start || end - start > MAX_WINDOW {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "transcript_window_range_invalid",
            ));
        }
        file.seek(SeekFrom::Start(start)).map_err(|_| changed())?;
        let mut bytes = Vec::with_capacity((end - start) as usize);
        let mut remaining = end - start;
        while remaining > 0 {
            self.check()?;
            let mut chunk = [0u8; 16 * 1024];
            let limit = chunk.len().min(remaining as usize);
            let n = file.read(&mut chunk[..limit]).map_err(|_| changed())?;
            if n == 0 {
                return Err(changed());
            }
            bytes.extend_from_slice(&chunk[..n]);
            self.bytes += n;
            remaining -= n as u64;
            if self.bytes > 2 * MAX_WINDOW as usize {
                return Err(error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "transcript_window_io_limit",
                ));
            }
        }
        self.check()?;
        Ok(bytes)
    }
}
fn error(status: StatusCode, code: &str) -> ApiError {
    ApiError(status, code.into())
}
fn changed() -> ApiError {
    error(
        StatusCode::CONFLICT,
        "transcript_window_changed_during_read",
    )
}
fn mac(cache: &Cache) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(&cache.key).expect("fixed key")
}
fn digest(cache: &Cache, value: &impl Serialize) -> ApiResult<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transcript_window_encoding",
        )
    })?;
    let mut h = mac(cache);
    h.update(&bytes);
    Ok(URL_SAFE_NO_PAD.encode(h.finalize().into_bytes()))
}
fn encode(cache: &Cache, cursor: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transcript_window_encoding",
        )
    })?;
    let mut h = mac(cache);
    h.update(&bytes);
    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(h.finalize().into_bytes())
    );
    if token.len() > MAX_CURSOR {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "transcript_window_cursor_limit",
        ));
    }
    Ok(token)
}
fn decode(cache: &Cache, token: &str) -> ApiResult<Cursor> {
    let invalid = || error(StatusCode::BAD_REQUEST, "transcript_window_cursor_invalid");
    if token.len() > MAX_CURSOR {
        return Err(invalid());
    }
    let (body, signature) = token.split_once('.').ok_or_else(invalid)?;
    let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
    let mut h = mac(cache);
    h.update(&bytes);
    h.verify_slice(&signature).map_err(|_| invalid())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if cursor.version != 1
        || cursor.instance != cache.instance
        || cursor.anchor.end < cursor.anchor.start
        || cursor.anchor.end - cursor.anchor.start > MAX_WINDOW
        || cursor.anchor.end > cursor.stamp.len
        || cursor
            .anchor
            .before
            .is_some_and(|b| b > cursor.anchor.end || (b > 0 && b <= cursor.anchor.start))
    {
        return Err(invalid());
    }
    Ok(cursor)
}
fn check_file(member: &VerifiedMember, stamp: &Stamp) -> ApiResult<()> {
    if Stamp::of(&member.file.metadata().map_err(|_| changed())?)? != *stamp {
        return Err(changed());
    }
    let named =
        crate::api::fs_open::open_anchored_regular_file(&member.path).map_err(|_| changed())?;
    if Stamp::of(&named.metadata().map_err(|_| changed())?)? != *stamp {
        return Err(changed());
    }
    Ok(())
}
fn compare_identity(member: &VerifiedMember, prior: &Cursor) -> ApiResult<()> {
    if FileIdentity::of(&member.file)? != prior.identity {
        return Err(error(
            StatusCode::CONFLICT,
            "transcript_window_file_replaced",
        ));
    }
    if member.stamp.len < prior.stamp.len {
        return Err(error(
            StatusCode::CONFLICT,
            "transcript_window_file_truncated",
        ));
    }
    Ok(())
}
fn validate_owner(authority: &Authority, workflow: &str, workspace: &str) -> ApiResult<()> {
    if authority.workflow_run_id != workflow || authority.workspace_id != workspace {
        return Err(error(
            StatusCode::CONFLICT,
            "transcript_window_owner_mismatch",
        ));
    }
    Ok(())
}

/// Authenticate a parent window before catalog/open. Its metadata may have
/// advanced by append; identity and prologue must still match. No line cursor.
pub(super) fn authorize_parent(
    s: &AppState,
    workflow: &str,
    workspace: &str,
    token: &str,
    cancel: &CancellationToken,
) -> ApiResult<(VerifiedMember, String)> {
    let work = Work {
        cancel: cancel.clone(),
        started: Instant::now(),
        bytes: 0,
    };
    work.check()?;
    let cache = &s.workflow_observations.follow;
    let prior = decode(cache, token)?;
    validate_owner(&prior.authority, workflow, workspace)?;
    let member = super::member::reauthorize_with_cancel(s, &prior.authority, cancel)?;
    compare_identity(&member, &prior)?;
    work.check()?;
    let identity = digest(cache, &(&member.authority, FileIdentity::of(&member.file)?))?;
    Ok((member, identity))
}

/// Initial authority is supplied only by the parent module's exact captured
/// member proof. Continuations authenticate before reauthorizing that authority.
/// The caller owns admission and cancellation-on-drop for its blocking worker.
pub(super) fn read(
    s: &AppState,
    workflow: &str,
    workspace: &str,
    query: Query,
    initial: Option<VerifiedMember>,
    cancel: CancellationToken,
) -> ApiResult<Reply> {
    if !(1..=MAX_RECORDS).contains(&query.limit) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "transcript_window_limit_invalid",
        ));
    }
    let work = Work {
        cancel,
        started: Instant::now(),
        bytes: 0,
    };
    work.check()?;
    let cache = &s.workflow_observations.follow;
    let prior = query
        .cursor
        .as_deref()
        .map(|v| decode(cache, v))
        .transpose()?;
    let member = if let Some(prior) = &prior {
        if initial.is_some() {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "transcript_window_mixed_authority",
            ));
        }
        validate_owner(&prior.authority, workflow, workspace)?;
        super::member::reauthorize_with_cancel(s, &prior.authority, &work.cancel)?
    } else {
        if !matches!(query.action, Action::Latest) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "transcript_window_cursor_required",
            ));
        }
        initial
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "transcript_window_member_required"))?
    };
    read_verified(cache, workflow, workspace, query, prior, member, work)
}
fn read_verified(
    cache: &Cache,
    workflow: &str,
    workspace: &str,
    query: Query,
    prior: Option<Cursor>,
    mut member: VerifiedMember,
    mut work: Work,
) -> ApiResult<Reply> {
    validate_owner(&member.authority, workflow, workspace)?;
    if member.path.to_str() != Some(member.authority.leaf_path()) {
        return Err(error(
            StatusCode::CONFLICT,
            "transcript_window_owner_mismatch",
        ));
    }
    work.check()?;
    let stamp = member.stamp.clone();
    check_file(&member, &stamp)?;
    if let Some(prior) = &prior {
        compare_identity(&member, prior)?;
    }
    let identity = FileIdentity::of(&member.file)?;
    let file_identity = digest(cache, &(&member.authority, &identity))?;
    let snapshot = digest(cache, &(&member.authority, &identity, &stamp))?;
    let previous_snapshot = prior.as_ref().map(|p| p.snapshot.clone());
    let mut reply = Reply {
        schema: SCHEMA,
        workflow_run_id: workflow.into(),
        workspace_id: workspace.into(),
        child_run_id: member.authority.leaf_run_id().into(),
        host_id: "local",
        path: member.authority.leaf_path().into(),
        instance_id: cache.instance.clone(),
        file_identity: file_identity.clone(),
        snapshot: snapshot.clone(),
        previous_snapshot,
        state: "window",
        relationship: if prior.is_some() {
            "fresh_latest"
        } else {
            "initial"
        },
        observed_at: chrono::Utc::now(),
        prologue: member.prologue.clone(),
        window: None,
        records: Vec::new(),
        poll_cursor: String::new(),
        earlier_cursor: None,
        limits: json!({"window_bytes":MAX_WINDOW,"anchor_bytes":MAX_WINDOW,"prologue_bytes":256*1024+1,"records":MAX_RECORDS,"reply_bytes":MAX_REPLY}),
    };
    if matches!(query.action, Action::Poll) {
        let prior = prior.as_ref().expect("validated poll cursor");
        reply.state = if stamp == prior.stamp {
            "unchanged"
        } else {
            "changed"
        };
        reply.relationship = if stamp == prior.stamp {
            "same_snapshot"
        } else {
            "retained_snapshot"
        };
        // The unchanged observed window is retained by the client. A modified
        // file is normal refresh control flow, never asserted to be append-only.
        reply.snapshot = prior.snapshot.clone();
        reply.poll_cursor = encode(cache, prior)?;
        reply.earlier_cursor = prior
            .anchor
            .before
            .filter(|b| *b > 0)
            .map(|_| encode(cache, prior))
            .transpose()?;
        check_file(&member, &stamp)?;
        work.check()?;
        return bounded(reply, &work);
    }
    let requested_before = if matches!(query.action, Action::Earlier) {
        let prior = prior.as_ref().expect("validated earlier cursor");
        let before = prior.anchor.before.filter(|b| *b > 0).ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "transcript_window_no_earlier_boundary",
            )
        })?;
        let bytes = work.interval(&mut member.file, prior.anchor.start, prior.anchor.end)?;
        if format!("{:x}", Sha256::digest(&bytes)) != prior.anchor.hash {
            return Err(error(
                StatusCode::CONFLICT,
                "transcript_window_anchor_changed",
            ));
        }
        let boundary = usize::try_from(before - prior.anchor.start - 1)
            .map_err(|_| error(StatusCode::BAD_REQUEST, "transcript_window_cursor_invalid"))?;
        if bytes.get(boundary) != Some(&b'\n') {
            return Err(error(
                StatusCode::CONFLICT,
                "transcript_window_anchor_changed",
            ));
        }
        reply.relationship = if stamp == prior.stamp {
            "same_snapshot"
        } else {
            "verified_anchor_fresh_window"
        };
        Some(before)
    } else {
        None
    };
    let end = requested_before.unwrap_or(stamp.len);
    let start = end.saturating_sub(MAX_WINDOW);
    let raw = work.interval(&mut member.file, start, end)?;
    let parsed = window(
        &raw,
        start,
        end,
        query.limit,
        &file_identity,
        &member.authority,
        &work,
    )?;
    let anchor = Anchor {
        start,
        end,
        hash: format!("{:x}", Sha256::digest(&raw)),
        before: parsed.before,
    };
    let cursor = Cursor {
        version: 1,
        instance: cache.instance.clone(),
        authority: member.authority.clone(),
        identity,
        stamp: stamp.clone(),
        snapshot,
        anchor,
    };
    reply.poll_cursor = encode(cache, &cursor)?;
    reply.earlier_cursor = parsed
        .before
        .filter(|b| *b > 0)
        .map(|_| encode(cache, &cursor))
        .transpose()?;
    let display_start = parsed
        .records
        .first()
        .map(|r| r.byte_offset.parse::<u64>().expect("record offset"))
        .unwrap_or(parsed.aligned_start);
    let display_end = parsed
        .records
        .last()
        .map(|r| r.byte_end.parse::<u64>().expect("record end"))
        .unwrap_or(parsed.aligned_start);
    reply.window = Some(Window {
        source_bytes: stamp.len.to_string(),
        observed_start_byte: start.to_string(),
        observed_end_byte: end.to_string(),
        display_start_byte: display_start.to_string(),
        display_end_byte: display_end.to_string(),
        omitted_before_bytes: display_start.to_string(),
        omitted_after_bytes: stamp.len.saturating_sub(display_end).to_string(),
        partial_tail_bytes: parsed.partial_tail.to_string(),
        requested_before_byte: requested_before.map(|v| v.to_string()),
        complete_source: display_start == 0 && display_end == stamp.len,
        boundary_available: parsed.before.is_some(),
    });
    reply.records = parsed.records;
    check_file(&member, &stamp)?;
    work.check()?;
    // A whole typed event can expand during serialization; explicitly replace
    // that row's payload instead of clipping a field or exceeding the response.
    while serialized_size_checked(&reply, &work)? > MAX_REPLY {
        let Some(row) = reply.records.iter_mut().rev().find(|r| r.event.is_some()) else {
            return Err(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "transcript_window_reply_limit",
            ));
        };
        row.event = None;
        row.status = "oversized";
    }
    bounded(reply, &work)
}
fn serialized_size_checked(value: &impl Serialize, work: &Work) -> ApiResult<usize> {
    work.check()?;
    let bytes = serde_json::to_vec(value).map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transcript_window_encoding",
        )
    })?;
    work.check()?;
    Ok(bytes.len())
}
fn bounded(reply: Reply, work: &Work) -> ApiResult<Reply> {
    if serialized_size_checked(&reply, work)? > MAX_REPLY {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "transcript_window_reply_limit",
        ));
    }
    Ok(reply)
}
struct Parsed {
    records: Vec<Record>,
    before: Option<u64>,
    aligned_start: u64,
    partial_tail: u64,
}
fn window(
    raw: &[u8],
    start: u64,
    end: u64,
    limit: usize,
    file_identity: &str,
    authority: &Authority,
    work: &Work,
) -> ApiResult<Parsed> {
    // At a nonzero start the first byte is a boundary probe. If it is not a
    // newline, conservatively skip the cut leading record through its newline.
    let begin = if start == 0 {
        0
    } else if raw.first() == Some(&b'\n') {
        1
    } else {
        raw.iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(raw.len())
    };
    let last_newline = raw
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let aligned_start = start + begin.min(last_newline) as u64;
    let tail = last_newline.max(begin);
    let partial_tail = end.saturating_sub(start + last_newline as u64);
    let mut right = tail;
    let mut records = Vec::new();
    while right > begin && records.len() < limit {
        work.check()?;
        let left = raw[begin..right - 1]
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| begin + i + 1)
            .unwrap_or(begin);
        let bytes = &raw[left..right];
        let mut reader = std::io::Cursor::new(bytes);
        let line = read_line(&mut reader, bytes.len() as u64).map_err(|_| changed())?;
        if !line.blank {
            let (status, event) = if line.oversized {
                ("oversized", None)
            } else {
                match serde_json::from_slice::<rupu_transcript::Event>(&line.bytes) {
                    Ok(rupu_transcript::Event::RunStart {
                        ref run_id,
                        ref workspace_id,
                        ..
                    }) if run_id != authority.leaf_run_id()
                        || workspace_id != &authority.workspace_id =>
                    {
                        ("scope_mismatch", None)
                    }
                    Ok(event) => (
                        "event",
                        Some(serde_json::to_value(event).map_err(|_| {
                            error(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "transcript_window_encoding",
                            )
                        })?),
                    ),
                    Err(_) => ("malformed", None),
                }
            };
            let hash = format!("{:x}", Sha256::digest(bytes));
            let offset = start + left as u64;
            let finish = start + right as u64;
            let identity = format!(
                "sha256:{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(file_identity, offset, finish, &hash))
                        .expect("row identity")
                )
            );
            records.push(Record {
                identity,
                byte_offset: offset.to_string(),
                byte_end: finish.to_string(),
                status,
                content_sha256: hash,
                event,
            });
        }
        right = left;
    }
    records.reverse();
    let before = records
        .first()
        .map(|r| r.byte_offset.parse().expect("record offset"))
        .or_else(|| (begin < raw.len() || start == 0).then_some(aligned_start));
    Ok(Parsed {
        records,
        before,
        aligned_start,
        partial_tail,
    })
}

#[cfg(test)]
mod tests;
