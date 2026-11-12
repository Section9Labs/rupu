//! Recorded owner history, ordered within one canonical source only. This is
//! not a complete operator audit and never writes synthetic run events.
use crate::{
    api::recorded_cursor,
    api::{
        fs_open::open_anchored_regular_file, local_only::LocalOnlyQuery, transcript::page as paging,
    },
    error::{ApiError, ApiResult},
    host::messages::MessageRoute,
    state::AppState,
};
use axum::{
    extract::{Path as AxPath, Query, State},
    routing::get,
    Json, Router,
};
use rupu_fleet::LogLine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub(crate) const CAPABILITY: &str = "coordination.history.v1";
const MAX_SCAN: u64 = 8 * 1024 * 1024;
const MAX_RESPONSE: usize = 1024 * 1024;
const DOMAIN: &str = "coordination.history.v1\0";
const ANCHOR_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Source {
    Messages,
    Claims,
}
impl Source {
    fn relative(self) -> &'static str {
        match self {
            Self::Messages => "log.jsonl",
            Self::Claims => "board/claims.jsonl",
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    host: Option<String>,
    source: Source,
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Owner {
    route: &'static str,
    kind: &'static str,
    id: String,
    host: &'static str,
}
#[derive(Default, Serialize)]
struct Coverage {
    source_present: bool,
    invalid_records: usize,
    unsupported_records: usize,
    incomplete_tail_bytes: u64,
}
#[derive(Serialize)]
struct Record {
    record_id: String,
    position: u64,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<Value>,
}
#[derive(Serialize)]
struct Page {
    version: u8,
    owner: Owner,
    source: Source,
    ordering: &'static str,
    snapshot: String,
    consistency: &'static str,
    fence_bytes: u64,
    records: Vec<Record>,
    next_cursor: Option<String>,
    has_more: bool,
    coverage: Coverage,
}
struct Context {
    owner: Owner,
    path: PathBuf,
    scope: String,
    selection: String,
}

pub(crate) fn routes() -> Router<AppState> {
    let mut router = Router::new();
    for route in MessageRoute::ALL {
        router = router.route(
            &format!("/api/{}/:id/coordination/history", route.segment()),
            get(
                move |State(state): State<AppState>,
                      AxPath(id): AxPath<String>,
                      Query(query): Query<HistoryQuery>| async move {
                    LocalOnlyQuery {
                        host: query.host.clone(),
                    }
                    .require_local("recorded coordination history")?;
                    if query.limit.is_some_and(|n| !(1..=200).contains(&n)) {
                        return Err(ApiError::bad_request("limit must be 1..200"));
                    }
                    tokio::task::spawn_blocking(move || read(&state.global_dir, route, &id, &query))
                        .await
                        .map_err(|_| ApiError::internal("coordination history reader failed"))?
                        .map(Json)
                },
            ),
        );
    }
    router
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}
fn stale() -> ApiError {
    ApiError::conflict("coordination_snapshot_changed: reload without a cursor")
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Reject links and special files in every store component. A missing store is
/// allowed; the owner itself must exist. No reader follows a continuation alias
/// into another owner's space or touches unrelated workspace/transcript trees.
fn directory(path: &Path, required: bool) -> ApiResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => Err(ApiError::bad_request(
            "coordination owner/store is not a regular directory",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ApiError::not_found("coordination owner is unavailable"))
        }
        Err(e) => Err(internal(e)),
    }
}
fn owner_record(path: &Path, id: &str, key: &str, optional: bool) -> ApiResult<(Value, String)> {
    let mut file = match open_anchored_regular_file(path) {
        Ok(file) => file,
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Value::Null, "absent".into()))
        }
        Err(_) => return Err(ApiError::not_found("bounded owner record is unavailable")),
    };
    let stamp = paging::Stamp::of(&file.metadata().map_err(internal)?)?;
    if stamp.len > 1024 * 1024 {
        return Err(ApiError::bad_request("owner record exceeds 1 MiB"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(internal)?;
    if bytes.len() > 1024 * 1024 {
        return Err(stale());
    }
    paging::check_stamp(path, &file, &stamp).map_err(|_| stale())?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad_request("owner record is malformed"))?;
    if value[key].as_str() != Some(id) {
        return Err(ApiError::bad_request("owner record identity mismatch"));
    }
    let identity = serde_json::json!([
        value[key],
        value["created_at"],
        value["started_at"],
        value["workflow_name"],
        value["workspace_id"],
        value["workspace_path"],
        value["project_root"]
    ]);
    Ok((
        value,
        hash(serde_json::to_vec(&identity).map_err(internal)?),
    ))
}
fn context(global: &Path, route: MessageRoute, id: &str, source: Source) -> ApiResult<Context> {
    if MessageRoute::of_id(id) != Some(route) {
        return Err(ApiError::not_found(
            "coordination owner does not match route",
        ));
    }
    let global = global.canonicalize().map_err(internal)?;
    let collections: &[&str] = match route {
        MessageRoute::Sessions => &["sessions", "sessions-archive"],
        MessageRoute::Runs => &["runs", "runs-archive"],
        MessageRoute::Agentiflows => &["agentiflows"],
    };
    let mut selected = None;
    for collection in collections {
        let base = global.join(collection);
        if !directory(&base, false)? {
            continue;
        }
        let dir = base.join(id);
        if directory(&dir, false)? {
            selected = Some(dir);
            break;
        }
    }
    let dir = selected.ok_or_else(|| ApiError::not_found("coordination owner is unavailable"))?;
    let dir_identity = Identity::of(&fs::metadata(&dir).map_err(internal)?);
    let (record_file, key, optional) = match route {
        MessageRoute::Sessions => ("session.json", "session_id", false),
        MessageRoute::Runs => ("run.json", "id", true),
        MessageRoute::Agentiflows => ("agentiflow.json", "id", false),
    };
    let (record, record_hash) = owner_record(&dir.join(record_file), id, key, optional)?;
    let kind = match route {
        MessageRoute::Sessions => "session",
        MessageRoute::Agentiflows => "agentiflow",
        MessageRoute::Runs if record == Value::Null => "standalone",
        MessageRoute::Runs => match record["workflow_name"].as_str() {
            Some(name) if name.starts_with("agent:") => "standalone",
            Some(_) => "workflow",
            None => return Err(ApiError::bad_request("run owner kind is unavailable")),
        },
    };
    let space = if route == MessageRoute::Agentiflows {
        dir.clone()
    } else {
        dir.join("messages")
    };
    directory(&space, false)?;
    if source == Source::Claims {
        directory(&space.join("board"), false)?;
    }
    let path = space.join(source.relative());
    let owner = Owner {
        route: route.segment(),
        kind,
        id: id.into(),
        host: "local",
    };
    let scope = hash(
        serde_json::to_vec(&(
            CAPABILITY,
            &global,
            &dir,
            dir_identity,
            record_hash,
            &owner,
            source,
        ))
        .map_err(internal)?,
    );
    let selection = hash(
        serde_json::to_vec(&(CAPABILITY, &global, route.segment(), id, source))
            .map_err(internal)?,
    );
    Ok(Context {
        owner,
        path,
        scope,
        selection,
    })
}

/// Claim journal decoding is intentionally a whitelist: lease tokens and future
/// opaque extensions must never escape through an untyped raw payload.
#[derive(Deserialize, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
enum ClaimEvent {
    Claimed {
        key: String,
        owner: String,
        at: String,
        lease_expires_at: String,
    },
    Released {
        key: String,
        owner: String,
        at: String,
    },
    Expired {
        key: String,
        owner: String,
        at: String,
    },
}
fn project(source: Source, bytes: &[u8]) -> (&'static str, Option<Value>) {
    let Ok(raw) = serde_json::from_slice::<Value>(bytes) else {
        return ("malformed", None);
    };
    let tag = raw[if source == Source::Messages {
        "t"
    } else {
        "ev"
    }]
    .as_str();
    let supported = match source {
        Source::Messages => tag.is_some_and(|t| ["msg", "seen"].contains(&t)),
        Source::Claims => tag.is_some_and(|t| ["claimed", "released", "expired"].contains(&t)),
    };
    if !supported {
        return (
            if tag.is_some() {
                "unsupported"
            } else {
                "malformed"
            },
            None,
        );
    }
    let event = match source {
        Source::Messages => serde_json::from_value::<LogLine>(raw)
            .ok()
            .and_then(|v| serde_json::to_value(v).ok()),
        Source::Claims => serde_json::from_value::<ClaimEvent>(raw)
            .ok()
            .and_then(|v| serde_json::to_value(v).ok()),
    };
    if event.is_some() {
        ("event", event)
    } else {
        ("malformed", None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    created: Option<u128>,
}
impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            created: metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Anchor {
    offset: u64,
    length: u64,
    digest: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Cursor {
    version: u8,
    selection: String,
    scope: String,
    file: String,
    identity: Identity,
    fence: u64,
    head: Anchor,
    tail: Anchor,
    boundary: Anchor,
    observation: String,
    offset: u64,
    position: u64,
}
fn range_hash(file: &mut fs::File, offset: u64, length: u64) -> ApiResult<String> {
    file.seek(SeekFrom::Start(offset)).map_err(internal)?;
    let mut read = file.take(length);
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut bytes = [0u8; 16 * 1024];
    loop {
        let n = read.read(&mut bytes).map_err(internal)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        digest.update(&bytes[..n]);
    }
    if count != length {
        return Err(stale());
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn anchor(file: &mut fs::File, end: u64) -> ApiResult<Anchor> {
    let length = end.min(ANCHOR_BYTES);
    let offset = end - length;
    Ok(Anchor {
        offset,
        length,
        digest: range_hash(file, offset, length)?,
    })
}
fn verify(file: &mut fs::File, cursor: &Cursor) -> ApiResult<()> {
    let metadata = file.metadata().map_err(internal)?;
    if Identity::of(&metadata) != cursor.identity || metadata.len() < cursor.fence {
        return Err(stale());
    }
    for a in [&cursor.head, &cursor.tail, &cursor.boundary] {
        if a.length > ANCHOR_BYTES
            || a.offset
                .checked_add(a.length)
                .is_none_or(|end| end > cursor.fence)
            || range_hash(file, a.offset, a.length)? != a.digest
        {
            return Err(stale());
        }
    }
    Ok(())
}
/// Hash every byte actually read, including bounded buffer lookahead and bytes
/// discarded from oversized lines. Recheck this exact range before publishing.
struct ObservedRead<R> {
    inner: R,
    digest: Sha256,
    count: u64,
}
impl<R: Read> Read for ObservedRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(bytes)?;
        self.digest.update(&bytes[..n]);
        self.count += n as u64;
        Ok(n)
    }
}
fn read(global: &Path, route: MessageRoute, id: &str, query: &HistoryQuery) -> ApiResult<Page> {
    read_checked(global, route, id, query, || {})
}
fn read_checked(
    global: &Path,
    route: MessageRoute,
    id: &str,
    query: &HistoryQuery,
    after_read: impl FnOnce(),
) -> ApiResult<Page> {
    let ctx = context(global, route, id, query.source).map_err(|e| {
        if query.cursor.is_some() {
            stale()
        } else {
            e
        }
    })?;
    let prior: Option<Cursor> = query
        .cursor
        .as_deref()
        .map(|token| recorded_cursor::decode(DOMAIN, token))
        .transpose()?;
    if prior
        .as_ref()
        .is_some_and(|c| c.version != 1 || c.offset > c.fence || c.selection != ctx.selection)
    {
        return Err(ApiError::bad_request(
            "coordination cursor belongs to another owner or source",
        ));
    }
    if prior
        .as_ref()
        .is_some_and(|c| c.scope != ctx.scope || c.file != ctx.path.to_string_lossy())
    {
        return Err(stale());
    }
    let mut page = Page {
        version: 1,
        owner: ctx.owner.clone(),
        source: query.source,
        ordering: "source_append_order",
        snapshot: hash(format!("{}:absent", ctx.scope)),
        consistency: "fenced_source_observation",
        fence_bytes: 0,
        records: Vec::new(),
        next_cursor: None,
        has_more: false,
        coverage: Coverage::default(),
    };
    let mut file = match open_anchored_regular_file(&ctx.path) {
        Ok(file) => file,
        Err(_) if prior.is_some() => return Err(stale()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if context(global, route, id, query.source)?.scope != ctx.scope {
                return Err(stale());
            }
            return Ok(page);
        }
        Err(_) => {
            return Err(ApiError::bad_request(
                "coordination source is not a regular file",
            ))
        }
    };
    page.coverage.source_present = true;
    let mut cursor = match prior {
        Some(cursor) => {
            verify(&mut file, &cursor)?;
            cursor
        }
        None => {
            let meta = file.metadata().map_err(internal)?;
            let fence = meta.len();
            let identity = Identity::of(&meta);
            let head = anchor(&mut file, fence.min(ANCHOR_BYTES))?;
            let tail = anchor(&mut file, fence)?;
            let boundary = anchor(&mut file, 0)?;
            let observation = recorded_cursor::signature(
                DOMAIN,
                &serde_json::to_vec(&(&ctx.scope, &identity, fence, &head, &tail))
                    .map_err(internal)?,
            );
            Cursor {
                version: 1,
                selection: ctx.selection.clone(),
                scope: ctx.scope.clone(),
                file: ctx.path.to_string_lossy().into_owned(),
                identity,
                fence,
                head,
                tail,
                boundary,
                observation,
                offset: 0,
                position: 0,
            }
        }
    };
    page.snapshot = cursor.observation.clone();
    page.fence_bytes = cursor.fence;
    let start = cursor.offset;
    file.seek(SeekFrom::Start(start)).map_err(internal)?;
    let observed = ObservedRead {
        inner: file.take(MAX_SCAN.min(cursor.fence - start)),
        digest: Sha256::new(),
        count: 0,
    };
    let mut reader = BufReader::with_capacity(16 * 1024, observed);
    while cursor.offset < cursor.fence
        && page.records.len() < query.limit.unwrap_or(100).clamp(1, 200)
    {
        let before = cursor.clone();
        let line =
            paging::read_line(&mut reader, cursor.fence - cursor.offset).map_err(internal)?;
        if !line.complete {
            if cursor.offset == start {
                return Err(ApiError(
                    axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                    "coordination_history_scan_limit".into(),
                ));
            }
            break;
        }
        cursor.offset += line.consumed;
        if !line.terminated {
            page.coverage.incomplete_tail_bytes = line.consumed;
            break;
        }
        cursor.position += 1;
        if line.blank {
            continue;
        }
        let (status, event) = if line.oversized {
            ("oversized", None)
        } else {
            project(query.source, &line.bytes)
        };
        page.records.push(Record {
            record_id: format!("{}:{}", page.snapshot, cursor.position),
            position: cursor.position,
            status,
            event,
        });
        if response_size_bound(&page, &cursor)? > MAX_RESPONSE {
            page.records.pop();
            cursor = before;
            break;
        }
    }
    let observed = reader.into_inner();
    let scanned = observed.count;
    let digest = format!("{:x}", observed.digest.finalize());
    let mut original = observed.inner.into_inner();
    after_read();
    // Reopen through the anchored walker to prove both the descriptor and the
    // current name still identify the same source. Later appends are excluded.
    let mut verification = open_anchored_regular_file(&ctx.path).map_err(|_| stale())?;
    verify(&mut original, &cursor)?;
    verify(&mut verification, &cursor)?;
    if range_hash(&mut verification, start, scanned)? != digest {
        return Err(stale());
    }
    cursor.boundary = anchor(&mut verification, cursor.offset)?;
    verify(&mut verification, &cursor)?;
    continuation(&mut page, &cursor)?;
    page.coverage.invalid_records = page
        .records
        .iter()
        .filter(|r| ["malformed", "oversized"].contains(&r.status))
        .count();
    page.coverage.unsupported_records = page
        .records
        .iter()
        .filter(|r| r.status == "unsupported")
        .count();
    if context(global, route, id, query.source)?.scope != ctx.scope {
        return Err(stale());
    }
    if serde_json::to_vec(&page).map_err(internal)?.len() > MAX_RESPONSE {
        return Err(ApiError::bad_request(
            "coordination page envelope exceeds byte bound",
        ));
    }
    Ok(page)
}
// Admission includes the largest possible final continuation and coverage.
// Blanks read after the last admitted row can increase both cursor coordinates;
// the verified boundary is also populated only after the scan. Bounding every
// variable-width integer avoids rejecting an otherwise pageable near-limit row.
fn response_size_bound(page: &Page, cursor: &Cursor) -> ApiResult<usize> {
    let mut envelope = serde_json::to_value(page).map_err(internal)?;
    let mut largest = cursor.clone();
    largest.offset = u64::MAX;
    largest.position = u64::MAX;
    largest.boundary.offset = u64::MAX;
    largest.boundary.length = u64::MAX;
    largest.boundary.digest = "f".repeat(64);
    envelope["next_cursor"] = Value::String(recorded_cursor::encode(DOMAIN, &largest)?);
    envelope["has_more"] = Value::Bool(false);
    envelope["coverage"] = serde_json::json!({
        "source_present": false,
        "invalid_records": usize::MAX,
        "unsupported_records": usize::MAX,
        "incomplete_tail_bytes": u64::MAX,
    });
    Ok(serde_json::to_vec(&envelope).map_err(internal)?.len())
}
fn continuation(page: &mut Page, cursor: &Cursor) -> ApiResult<()> {
    page.has_more = cursor.offset < cursor.fence;
    page.next_cursor = page
        .has_more
        .then(|| recorded_cursor::encode(DOMAIN, cursor))
        .transpose()?;
    Ok(())
}

#[cfg(test)]
#[path = "coordination_history_tests.rs"]
mod tests;
