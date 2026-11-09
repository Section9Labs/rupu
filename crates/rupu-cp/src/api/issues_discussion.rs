//! Bounded live issue discussion pages with immutable captured text replay.
use super::issues_read::{clip, identity, instance, reader, require_local, valid_text, ReadError};
use crate::{
    issue_reader::{IssueReadError, IssueReadTarget},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use rupu_scm::issue_discussion::{DiscussionIdentity, DiscussionPage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

pub const FEATURE: &str = "issues.discussion.pages.v1";
const RETAINED: usize = 4 * 1024 * 1024;
const OUTPUT: usize = 512 * 1024;
const TTL: Duration = Duration::from_secs(600);
const FRAMES: usize = 256;
static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);
static READERS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));
type Result<T> = std::result::Result<T, ReadError>;
#[derive(Default)]
struct Cache {
    entries: Mutex<HashMap<String, Arc<Observation>>>,
}
struct Observation {
    owner: String,
    target: IssueReadTarget,
    epoch: String,
    created: Instant,
    observed_at: String,
    expires_at: String,
    state: AsyncMutex<Vec<u8>>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Text {
    captured_at: String,
    page_id: String,
    comment_id: String,
    revision: String,
    text: String,
    metadata: Value,
}
#[derive(Clone, Serialize, Deserialize)]
enum Pending {
    Provider { after: Option<String> },
    Text { key: String, offset: usize },
}
#[derive(Clone, Serialize, Deserialize)]
struct Frame {
    kind: String,
    pending: Option<Pending>,
    response: Option<Value>,
}
#[derive(Default, Serialize, Deserialize)]
struct Pages {
    identity: Option<DiscussionIdentity>,
    frames: HashMap<String, Frame>,
    texts: HashMap<String, Text>,
    seen: BTreeMap<String, String>,
    issued_provider: std::collections::BTreeSet<String>,
    ordinal: usize,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscussionQuery {
    pub host: Option<String>,
    pub project: String,
    pub account: String,
    pub expected_origin: String,
    pub cursor: Option<String>,
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/issues/:tracker/:number/discussion", get(page))
        .route("/api/issues/:tracker/:number/discussion/text", get(text))
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> ReadError {
    ReadError(status, code, message)
}
fn deadline() -> ReadError {
    error(
        StatusCode::GATEWAY_TIMEOUT,
        "timeout",
        "The discussion read exceeded its deadline",
    )
}
fn invalid() -> ReadError {
    error(
        StatusCode::BAD_REQUEST,
        "invalid_cursor",
        "The discussion cursor does not belong to this exact selection",
    )
}
fn expired() -> ReadError {
    error(
        StatusCode::GONE,
        "observation_expired",
        "The captured discussion expired; refresh explicitly",
    )
}
fn capacity() -> ReadError {
    error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "observation_capacity",
        "The captured discussion reached its bounded capacity; refresh explicitly",
    )
}
fn busy() -> ReadError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "busy",
        "Discussion readers are busy; retry this read later",
    )
}
fn malformed() -> ReadError {
    error(
        StatusCode::BAD_GATEWAY,
        "upstream_unavailable",
        "The provider discussion did not satisfy its bounded identity or pagination contract",
    )
}
fn changed() -> ReadError {
    error(
        StatusCode::CONFLICT,
        "discussion_changed",
        "The captured discussion identity or repeated comment changed; refresh explicitly",
    )
}
fn token() -> String {
    // Two independently random ULID suffixes provide 160 random bits.
    format!(
        "{}{}",
        &ulid::Ulid::new().to_string()[10..],
        &ulid::Ulid::new().to_string()[10..]
    )
}
fn target(tracker: &str, number: &str, q: &DiscussionQuery) -> Result<IssueReadTarget> {
    require_local(q.host.as_deref())?;
    let bad = || ReadError::from(IssueReadError::InvalidTarget);
    let n: u64 = number.parse().map_err(|_| bad())?;
    if n == 0
        || n.to_string() != number
        || !valid_text(&q.project, 512)
        || !valid_text(&q.account, 128)
        || !valid_text(&q.expected_origin, 2048)
        || rupu_scm::operator_issues::safe_origin(&q.expected_origin).as_deref()
            != Some(&q.expected_origin)
    {
        return Err(bad());
    }
    Ok(IssueReadTarget {
        tracker: tracker.parse().map_err(|_| bad())?,
        number: n,
        project: q.project.clone(),
        account: q.account.clone(),
        origin: q.expected_origin.clone(),
    })
}
fn encode(pages: &Pages) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(pages).map_err(|_| malformed())?;
    if bytes.len() > RETAINED || pages.frames.len() > FRAMES {
        return Err(capacity());
    }
    Ok(bytes)
}
fn bounded(value: Value) -> Result<Json<Value>> {
    if serde_json::to_vec(&value).map_err(|_| malformed())?.len() > OUTPUT {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "response_too_large",
            "The discussion response exceeds its bounded display budget",
        ));
    }
    Ok(Json(value))
}
fn issue_frame(
    pages: &mut Pages,
    observation: &str,
    kind: &str,
    pending: Pending,
) -> Result<String> {
    if pages.frames.len() >= FRAMES {
        return Err(capacity());
    }
    let id = token();
    pages.frames.insert(
        id.clone(),
        Frame {
            kind: kind.into(),
            pending: Some(pending),
            response: None,
        },
    );
    Ok(format!("{observation}.{id}"))
}
fn base(obs: &Observation, id: &str, identity_value: &DiscussionIdentity) -> Value {
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(identity_value).expect("plain DTO"))
    );
    json!({"schema_version":1,"identity":identity(&obs.target),"source":"current_issue","observation_id":id,"observed_at":obs.observed_at,"expires_at":obs.expires_at,"consistency":"captured_pages_live_collection","provider_identity":digest})
}
fn text_chunk(
    pages: &mut Pages,
    observation: &str,
    key: &str,
    offset: usize,
    cap: usize,
) -> Result<Value> {
    let captured = pages.texts.get(key).cloned().ok_or_else(invalid)?;
    if offset > captured.text.len() || !captured.text.is_char_boundary(offset) {
        return Err(invalid());
    }
    let (part, truncated) = clip(&captured.text[offset..], cap);
    let next = if truncated {
        Some(issue_frame(
            pages,
            observation,
            "text",
            Pending::Text {
                key: key.into(),
                offset: offset + part.len(),
            },
        )?)
    } else {
        None
    };
    let mut body = captured.metadata;
    body["text"] = json!(part);
    body["offset"] = json!(offset);
    body["byte_count"] = json!(part.len());
    body["total_bytes"] = json!(captured.text.len());
    body["truncated"] = json!(truncated);
    body["next_cursor"] = json!(next);
    Ok(body)
}
fn publish_page(
    pages: &mut Pages,
    obs: &Observation,
    oid: &str,
    page: DiscussionPage,
) -> Result<Value> {
    if page.identity.connector_epoch != obs.epoch
        || page.items.len() > 20
        || page.input_bytes > 2 * 1024 * 1024 + 4 * 64 * 1024
        || !matches!(
            page.order.as_str(),
            "oldest_first" | "provider_created_order"
        )
        || page.next.as_ref().is_some_and(|s| !valid_text(s, 2048))
        || (page.provider_exhausted && page.next.is_some())
        || (!page.provider_exhausted && page.next.is_none())
    {
        return Err(malformed());
    }
    for field in [
        &page.identity.connector_epoch,
        &page.identity.project_id,
        &page.identity.issue_id,
        &page.identity.identifier,
    ] {
        if !valid_text(field, 2048) {
            return Err(malformed());
        }
    }
    if !valid_text(&page.identity.api_profile, 8192) {
        return Err(malformed());
    }
    if pages.identity.as_ref().is_some_and(|i| i != &page.identity) {
        return Err(changed());
    }
    if page
        .next
        .as_ref()
        .is_some_and(|cursor| pages.issued_provider.contains(cursor))
    {
        return Err(malformed());
    }
    if let Some(cursor) = &page.next {
        pages.issued_provider.insert(cursor.clone());
    }
    pages.identity = Some(page.identity.clone());
    let pid = token();
    let captured_at = chrono::Utc::now().to_rfc3339();
    let mut items = vec![];
    let mut duplicates = 0usize;
    let mut seen = std::collections::HashSet::new();
    for c in page.items {
        if !valid_text(&c.id, 256)
            || !seen.insert(c.id.clone())
            || !matches!(c.kind.as_str(), "comment" | "system")
            || c.author.as_ref().is_some_and(|a| a.len() > 512)
            || !valid_text(&c.created_at, 128)
            || c.updated_at.as_ref().is_some_and(|v| !valid_text(v, 128))
            || c.parent_comment_id
                .as_ref()
                .is_some_and(|v| !valid_text(v, 256))
            || !matches!(c.format.as_str(), "markdown" | "plain_text")
            || !matches!(c.source_format.as_str(), "markdown" | "adf" | "plain_text")
            || c.body.len() > 2 * 1024 * 1024
        {
            return Err(malformed());
        }
        let revision = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &page.identity,
                    &c.id,
                    &c.body,
                    &c.format,
                    &c.source_format,
                    c.projection_complete,
                    c.unsupported_nodes,
                    c.unsupported_marks
                ))
                .map_err(|_| malformed())?
            )
        );
        if let Some(previous) = pages.seen.get(&c.id) {
            if previous != &revision {
                return Err(changed());
            }
            duplicates += 1;
            continue;
        }
        pages.seen.insert(c.id.clone(), revision.clone());
        let key = token();
        pages.texts.insert(key.clone(),Text {captured_at:captured_at.clone(),page_id:pid.clone(),comment_id:c.id.clone(),revision:revision.clone(),text:c.body,metadata:json!({"format":c.format,"source_format":c.source_format,"projection_complete":c.projection_complete,"unsupported_nodes":c.unsupported_nodes,"unsupported_marks":c.unsupported_marks})});
        let body = text_chunk(pages, oid, &key, 0, 8 * 1024)?;
        items.push(json!({"id":c.id,"kind":c.kind,"author":c.author,"created_at":c.created_at,"updated_at":c.updated_at,"metadata_truncated":c.metadata_truncated,"parent_comment_id":c.parent_comment_id,"body":body,"revision":revision}));
    }
    let next = page
        .next
        .map(|after| {
            issue_frame(
                pages,
                oid,
                "discussion",
                Pending::Provider { after: Some(after) },
            )
        })
        .transpose()?;
    let mut out = base(obs, oid, &page.identity);
    out["observed_at"] = json!(captured_at);
    out["page"] = json!({"id":pid,"ordinal":pages.ordinal,"order":page.order,"completeness":if page.provider_exhausted{"end_observed"}else{"page_only"},"duplicate_observations":duplicates,"items":items,"next_cursor":next});
    pages.ordinal += 1;
    bounded(out.clone())?;
    Ok(out)
}
impl Cache {
    fn get(&self, id: &str) -> Result<Arc<Observation>> {
        let mut entries = self.entries.lock().map_err(|_| busy())?;
        entries.retain(|_, v| v.created.elapsed() < TTL || Arc::strong_count(v) > 1);
        entries
            .get(id)
            .filter(|v| v.created.elapsed() < TTL)
            .cloned()
            .ok_or_else(expired)
    }
    fn insert(&self, id: String, obs: Arc<Observation>) -> Result<()> {
        let mut entries = self.entries.lock().map_err(|_| busy())?;
        entries.retain(|_, v| v.created.elapsed() < TTL || Arc::strong_count(v) > 1);
        if entries.len() >= 8 {
            return Err(capacity());
        }
        entries.insert(id, obs);
        Ok(())
    }
}
async fn page(
    State(s): State<AppState>,
    Path((tracker, number)): Path<(String, String)>,
    Query(q): Query<DiscussionQuery>,
) -> Result<Json<Value>> {
    read(&CACHE, &READERS, &s, &tracker, &number, q, false).await
}
async fn text(
    State(s): State<AppState>,
    Path((tracker, number)): Path<(String, String)>,
    Query(q): Query<DiscussionQuery>,
) -> Result<Json<Value>> {
    read(&CACHE, &READERS, &s, &tracker, &number, q, true).await
}
async fn read(
    cache: &Cache,
    admission: &Semaphore,
    s: &AppState,
    tracker: &str,
    number: &str,
    q: DiscussionQuery,
    is_text: bool,
) -> Result<Json<Value>> {
    let target = target(tracker, number, &q)?;
    let current = super::issues_read::current_state(s).await?;
    let s = &current;
    let port = reader(s)?;
    let started = Instant::now();
    let _permit = admission.try_acquire().map_err(|_| busy())?;
    tokio::time::timeout(Duration::from_secs(20), async {
        let epoch = port.discussion_epoch(&target).map_err(ReadError::from)?;
        let owner = instance(s);
        if let Some(cursor) = q.cursor {
            if cursor.len() > 128 {
                return Err(invalid());
            }
            let (oid, fid) = cursor.split_once('.').ok_or_else(invalid)?;
            let obs = cache.get(oid)?;
            if obs.owner != owner || obs.target != target {
                return Err(invalid());
            }
            if obs.epoch != epoch {
                return Err(changed());
            }
            let mut encoded = obs.state.try_lock().map_err(|_| busy())?;
            let mut pages: Pages = serde_json::from_slice(&encoded).map_err(|_| malformed())?;
            let frame = pages.frames.get(fid).cloned().ok_or_else(invalid)?;
            if frame.kind != if is_text { "text" } else { "discussion" } {
                return Err(invalid());
            }
            if let Some(response) = frame.response {
                return bounded(response);
            }
            let out = match frame.pending.ok_or_else(invalid)? {
                Pending::Provider { after } => {
                    let next = port
                        .discussion_page(&target, after.as_deref())
                        .await
                        .map_err(ReadError::from)?;
                    if next.next.as_ref() == after.as_ref() && next.next.is_some() {
                        return Err(malformed());
                    }
                    publish_page(&mut pages, &obs, oid, next)?
                }
                Pending::Text { key, offset } => {
                    let captured = pages.texts.get(&key).cloned().ok_or_else(invalid)?;
                    let mut out = base(&obs, oid, pages.identity.as_ref().ok_or_else(invalid)?);
                    out["observed_at"] = json!(captured.captured_at);
                    out["page_id"] = json!(captured.page_id);
                    out["comment_id"] = json!(captured.comment_id);
                    out["revision"] = json!(captured.revision);
                    out["body"] = text_chunk(&mut pages, oid, &key, offset, 64 * 1024)?;
                    out
                }
            };
            if port.discussion_epoch(&target).map_err(ReadError::from)? != epoch {
                return Err(changed());
            }
            if obs.created.elapsed() >= TTL {
                return Err(expired());
            }
            bounded(out.clone())?;
            pages.frames.insert(
                fid.into(),
                Frame {
                    kind: frame.kind,
                    pending: None,
                    response: Some(out.clone()),
                },
            );
            let published = encode(&pages)?;
            if started.elapsed() >= Duration::from_secs(20) {
                return Err(deadline());
            }
            *encoded = published;
            bounded(out)
        } else {
            if is_text {
                return Err(invalid());
            }
            let oid = token();
            let now = chrono::Utc::now();
            let obs = Arc::new(Observation {
                owner,
                target: target.clone(),
                epoch: epoch.clone(),
                created: Instant::now(),
                observed_at: now.to_rfc3339(),
                expires_at: (now + chrono::Duration::seconds(TTL.as_secs() as i64)).to_rfc3339(),
                state: AsyncMutex::new(vec![]),
            });
            let page = port
                .discussion_page(&target, None)
                .await
                .map_err(ReadError::from)?;
            let mut pages = Pages::default();
            let out = publish_page(&mut pages, &obs, &oid, page)?;
            if port.discussion_epoch(&target).map_err(ReadError::from)? != epoch {
                return Err(changed());
            }
            *obs.state.lock().await = encode(&pages)?;
            if started.elapsed() >= Duration::from_secs(20) {
                return Err(deadline());
            }
            cache.insert(oid, obs)?;
            bounded(out)
        }
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "The discussion read exceeded its deadline",
        )
    })?
}

#[cfg(test)]
mod tests;
