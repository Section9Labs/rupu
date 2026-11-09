//! Current operator issue reads. Recorded trigger payloads are not current state.
use crate::{
    issue_reader::{IssueReadError, IssueReadTarget, IssueReader},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, LazyLock, OnceLock},
    time::Duration,
};
use tokio::sync::Semaphore;

const BODY_BYTES: usize = 64 * 1024;
const COMMENT_BYTES: usize = 8 * 1024;
const RESPONSE_BYTES: usize = 512 * 1024;
static READERS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/issues/accounts", get(accounts))
        .route("/api/issues/:tracker/:number", get(context))
        .route("/api/issues/:tracker/:number/body", get(body_page))
        .route("/api/issues/:tracker/:number/comments", get(comments_page))
}

#[derive(Debug)]
pub(super) struct ReadError(
    pub(super) StatusCode,
    pub(super) &'static str,
    pub(super) &'static str,
);
impl IntoResponse for ReadError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"code":self.1,"error":self.2}))).into_response()
    }
}
impl From<IssueReadError> for ReadError {
    fn from(e: IssueReadError) -> Self {
        match e {
            IssueReadError::AccountUnavailable => Self(
                StatusCode::SERVICE_UNAVAILABLE,
                "account_unavailable",
                "The selected issue account is unavailable",
            ),
            IssueReadError::Unsupported => Self(
                StatusCode::NOT_IMPLEMENTED,
                "unsupported",
                "This connector does not support the requested operator read",
            ),
            IssueReadError::SelectionChanged => Self(
                StatusCode::CONFLICT,
                "selection_changed",
                "The account origin or issue identity changed; refresh the selection",
            ),
            IssueReadError::InvalidTarget => Self(
                StatusCode::BAD_REQUEST,
                "invalid_target",
                "An exact issue target under the configured account origin is required",
            ),
            IssueReadError::NotFound => Self(
                StatusCode::NOT_FOUND,
                "not_found",
                "The issue was not found by the selected account",
            ),
            IssueReadError::Forbidden => Self(
                StatusCode::FORBIDDEN,
                "forbidden",
                "The selected account is not authorized for this issue",
            ),
            IssueReadError::RateLimited => Self(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "The issue service is rate limited",
            ),
            IssueReadError::UpstreamUnavailable => Self(
                StatusCode::BAD_GATEWAY,
                "upstream_unavailable",
                "The issue service did not return a usable response",
            ),
        }
    }
}
type ReadResult<T> = Result<T, ReadError>;
fn invalid_cursor() -> ReadError {
    ReadError(
        StatusCode::BAD_REQUEST,
        "invalid_cursor",
        "The issue continuation is invalid for this selection",
    )
}
fn stale() -> ReadError {
    ReadError(
        StatusCode::CONFLICT,
        "stale_cursor",
        "The issue changed or the continuation expired; refresh current issue context",
    )
}
fn timeout() -> ReadError {
    ReadError(
        StatusCode::GATEWAY_TIMEOUT,
        "timeout",
        "The current issue read exceeded its deadline",
    )
}
pub(super) fn require_local(host: Option<&str>) -> ReadResult<()> {
    if host.is_none_or(|h| h.is_empty() || h == "local") {
        Ok(())
    } else {
        Err(ReadError(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "Issue reads require a direct connection to the owning instance",
        ))
    }
}
pub(super) fn reader(s: &AppState) -> ReadResult<&dyn IssueReader> {
    s.repos
        .as_ref()
        .and_then(|p| p.issue_reader())
        .ok_or(ReadError(
            StatusCode::SERVICE_UNAVAILABLE,
            "adapter_missing",
            "The owning instance has no operator issue reader",
        ))
}

#[derive(Default, Deserialize)]
struct HostQuery {
    host: Option<String>,
}
async fn accounts(
    State(s): State<AppState>,
    Query(q): Query<HostQuery>,
) -> ReadResult<Json<Value>> {
    require_local(q.host.as_deref())?;
    let all = reader(&s)?.accounts();
    let count = all.len();
    let rows: Vec<_> = all
        .into_iter()
        .filter(|a| valid_text(&a.account, 128))
        .take(64)
        .collect();
    bounded(json!({"host_id":"local","truncated":rows.len() < count,"accounts":rows}))
}

#[derive(Deserialize)]
struct ReadQuery {
    host: Option<String>,
    project: String,
    account: String,
    expected_origin: String,
    cursor: Option<String>,
}
pub(super) fn valid_text(s: &str, limit: usize) -> bool {
    !s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control)
}
fn target(tracker: &str, number: &str, q: &ReadQuery) -> ReadResult<IssueReadTarget> {
    require_local(q.host.as_deref())?;
    let invalid = || ReadError::from(IssueReadError::InvalidTarget);
    let number: u64 = number.parse().map_err(|_| invalid())?;
    if number == 0
        || !valid_text(&q.project, 512)
        || !valid_text(&q.account, 128)
        || !valid_text(&q.expected_origin, 2048)
        || rupu_scm::operator_issues::safe_origin(&q.expected_origin).as_deref()
            != Some(q.expected_origin.as_str())
    {
        return Err(invalid());
    }
    Ok(IssueReadTarget {
        tracker: tracker.parse().map_err(|_| invalid())?,
        number,
        project: q.project.clone(),
        account: q.account.clone(),
        origin: q.expected_origin.clone(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Kind {
    Body,
    Comments,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    schema: u8,
    instance: String,
    target: IssueReadTarget,
    version: String,
    kind: Kind,
    position: u64,
    issued_at: i64,
}
fn mac() -> Hmac<Sha256> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut bytes = [0; 32];
        for part in bytes.chunks_mut(8) {
            part.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        bytes
    });
    Hmac::<Sha256>::new_from_slice(key).expect("fixed HMAC key")
}
// Each live adapter allocation receives a random epoch. Weak ownership avoids
// retaining old adapters; a recycled address receives a new epoch. This also
// separates two CP AppStates using the same global_dir within one process.
fn adapter_epoch(adapter: &Arc<dyn crate::repos::RepoLister>) -> String {
    type Entry = (std::sync::Weak<dyn crate::repos::RepoLister>, String);
    static EPOCHS: LazyLock<std::sync::Mutex<Vec<Entry>>> =
        LazyLock::new(|| std::sync::Mutex::new(Vec::new()));
    let mut entries = EPOCHS.lock().unwrap_or_else(|poison| poison.into_inner());
    entries.retain(|(weak, _)| weak.strong_count() > 0);
    if let Some((_, epoch)) = entries.iter().find(|(weak, _)| {
        weak.upgrade()
            .is_some_and(|existing| Arc::ptr_eq(&existing, adapter))
    }) {
        return epoch.clone();
    }
    let epoch = ulid::Ulid::new().to_string();
    entries.push((Arc::downgrade(adapter), epoch.clone()));
    epoch
}
pub(super) fn instance(s: &AppState) -> String {
    let epoch = adapter_epoch(s.repos.as_ref().expect("reader adapter checked"));
    let mut hash = Sha256::new();
    hash.update(s.global_dir.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(epoch.as_bytes());
    format!("{:x}", hash.finalize())
}
fn encode(cursor: Cursor) -> ReadResult<String> {
    let bytes = serde_json::to_vec(&cursor).map_err(|_| invalid_cursor())?;
    let mut signer = mac();
    signer.update(&bytes);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes())
    ))
}
fn decode(value: &str, owner: &str, target: &IssueReadTarget, kind: Kind) -> ReadResult<Cursor> {
    if value.len() > 8192 {
        return Err(invalid_cursor());
    }
    let (body, signature) = value.split_once('.').ok_or_else(invalid_cursor)?;
    let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid_cursor())?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid_cursor())?;
    let mut signer = mac();
    signer.update(&bytes);
    signer
        .verify_slice(&signature)
        .map_err(|_| invalid_cursor())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
    if cursor.schema != 1
        || cursor.instance != owner
        || cursor.target != *target
        || cursor.kind != kind
    {
        return Err(invalid_cursor());
    }
    let age = chrono::Utc::now()
        .timestamp()
        .saturating_sub(cursor.issued_at);
    if !(0..=900).contains(&age) {
        return Err(stale());
    }
    Ok(cursor)
}
fn next(
    owner: &str,
    target: &IssueReadTarget,
    version: &str,
    kind: Kind,
    position: u64,
) -> ReadResult<String> {
    encode(Cursor {
        schema: 1,
        instance: owner.into(),
        target: target.clone(),
        version: version.into(),
        kind,
        position,
        issued_at: chrono::Utc::now().timestamp(),
    })
}
pub(super) fn identity(t: &IssueReadTarget) -> Value {
    json!({"host_id":"local","tracker":t.tracker,"project":t.project,"account":t.account,"number":t.number.to_string(),"origin":t.origin})
}
fn envelope(t: &IssueReadTarget, version: &str) -> Value {
    json!({"identity":identity(t),"source":"current_issue","observed_at":chrono::Utc::now().to_rfc3339(),"freshness":"connector_cache_policy","version":version})
}
pub(super) fn clip(value: &str, limit: usize) -> (&str, bool) {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], end < value.len())
}
fn body_chunk(
    value: &str,
    offset: usize,
    owner: &str,
    target: &IssueReadTarget,
    version: &str,
) -> ReadResult<Value> {
    if offset > value.len() || !value.is_char_boundary(offset) {
        return Err(invalid_cursor());
    }
    let (text, truncated) = clip(&value[offset..], BODY_BYTES);
    let cursor = if truncated {
        Some(next(
            owner,
            target,
            version,
            Kind::Body,
            (offset + text.len()) as u64,
        )?)
    } else {
        None
    };
    Ok(
        json!({"text":text,"offset":offset,"byte_count":text.len(),"total_bytes":value.len(),"truncated":truncated,"next_cursor":cursor}),
    )
}
fn unavailable_comments(state: &str, reason: &str) -> Value {
    json!({"state":state,"reason":reason,"items":[],"next_cursor":null,"pagination":"live","order":"oldest_first","completeness":"unavailable"})
}
async fn comment_chunk(
    port: &dyn IssueReader,
    target: &IssueReadTarget,
    page: u32,
    owner: &str,
    version: &str,
) -> ReadResult<Value> {
    if !port.comments_supported(target).map_err(ReadError::from)? {
        return Ok(unavailable_comments(
            "unsupported",
            "This connector has no bounded operator comment reader",
        ));
    }
    if page == 0 || page > 10000 {
        return Err(invalid_cursor());
    }
    let page = tokio::time::timeout(Duration::from_secs(6), port.comments(target, page))
        .await
        .map_err(|_| timeout())?
        .map_err(ReadError::from)?;
    if page.items.len() > 50 {
        return Err(ReadError::from(IssueReadError::UpstreamUnavailable));
    }
    if page.items.iter().any(|c| !valid_text(&c.id, 256)) {
        return Err(ReadError::from(IssueReadError::UpstreamUnavailable));
    }
    let items:Vec<_> = page.items.iter().map(|c| {
        let (body,truncated) = clip(&c.body,COMMENT_BYTES);
        json!({"id":c.id,"author":clip(&c.author,512).0,"body":body,"body_truncated":truncated,"metadata_truncated":c.author.len() > 512 || c.author_association.as_ref().is_some_and(|s|s.len() > 128),
            "created_at":c.created_at,"author_association":c.author_association.as_deref().map(|s|clip(s,128).0)})
    }).collect();
    let cursor = page
        .next_page
        .map(|p| next(owner, target, version, Kind::Comments, p as u64))
        .transpose()?;
    Ok(
        json!({"state":"available","reason":if !page.end_observed && page.next_page.is_none() {Some("comment_page_limit")} else {None},"items":items,"next_cursor":cursor,"pagination":"live","order":"oldest_first",
        "completeness":if page.end_observed {"end_observed"} else {"page_only"}}),
    )
}
fn bounded(value: Value) -> ReadResult<Json<Value>> {
    if serde_json::to_vec(&value)
        .map_err(|_| ReadError::from(IssueReadError::UpstreamUnavailable))?
        .len()
        > RESPONSE_BYTES
    {
        Err(ReadError(
            StatusCode::BAD_GATEWAY,
            "response_too_large",
            "The issue response exceeded its bounded display budget",
        ))
    } else {
        Ok(Json(value))
    }
}

async fn context(
    State(s): State<AppState>,
    Path((tracker, number)): Path<(String, String)>,
    Query(q): Query<ReadQuery>,
) -> ReadResult<Json<Value>> {
    read(s, tracker, number, q, None).await
}
async fn body_page(
    State(s): State<AppState>,
    Path((tracker, number)): Path<(String, String)>,
    Query(q): Query<ReadQuery>,
) -> ReadResult<Json<Value>> {
    read(s, tracker, number, q, Some(Kind::Body)).await
}
async fn comments_page(
    State(s): State<AppState>,
    Path((tracker, number)): Path<(String, String)>,
    Query(q): Query<ReadQuery>,
) -> ReadResult<Json<Value>> {
    read(s, tracker, number, q, Some(Kind::Comments)).await
}

async fn read(
    s: AppState,
    tracker: String,
    number: String,
    q: ReadQuery,
    kind: Option<Kind>,
) -> ReadResult<Json<Value>> {
    let target = target(&tracker, &number, &q)?;
    let _ = reader(&s)?;
    let owner = instance(&s);
    let cursor = kind
        .map(|k| {
            decode(
                q.cursor.as_deref().ok_or_else(invalid_cursor)?,
                &owner,
                &target,
                k,
            )
        })
        .transpose()?;
    if kind.is_none() && q.cursor.is_some() {
        return Err(invalid_cursor());
    }
    let _permit = READERS.clone().try_acquire_owned().map_err(|_| {
        ReadError(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Issue readers are busy; retry after the current observation",
        )
    })?;
    tokio::time::timeout(Duration::from_secs(20),async {
        let port = reader(&s)?;
        let issue = tokio::time::timeout(Duration::from_secs(12),port.issue(&target)).await.map_err(|_|timeout())?.map_err(ReadError::from)?;
        let bytes = serde_json::to_vec(&issue).map_err(|_|ReadError::from(IssueReadError::UpstreamUnavailable))?;
        let version = format!("{:x}",Sha256::digest(bytes));
        if cursor.as_ref().is_some_and(|c|c.version != version) { return Err(stale()); }
        let mut out = envelope(&target,&version);
        match kind {
            Some(Kind::Body) => {
                let offset = usize::try_from(cursor.as_ref().ok_or_else(invalid_cursor)?.position).map_err(|_|invalid_cursor())?;
                out["body"] = body_chunk(&issue.body,offset,&owner,&target,&version)?;
            }
            Some(Kind::Comments) => {
                let page = u32::try_from(cursor.as_ref().ok_or_else(invalid_cursor)?.position).map_err(|_|invalid_cursor())?;
                out["comments"] = comment_chunk(port,&target,page,&owner,&version).await?;
            }
            None => {
                let (title,title_cut) = clip(&issue.title,4096); let (author,author_cut) = clip(&issue.author,512);
                let labels:Vec<_> = issue.labels.iter().take(64).map(|s|clip(s,256).0).collect();
                let cut = title_cut || author_cut || issue.labels.len() > 64 || issue.labels.iter().take(64).any(|s|s.len() > 256);
                out["issue"] = json!({"title":title,"state":issue.state,"labels":labels,"author":author,"created_at":issue.created_at,
                    "updated_at":issue.updated_at,"metadata_truncated":cut,"body":body_chunk(&issue.body,0,&owner,&target,&version)?});
                out["comments"] = match comment_chunk(port,&target,1,&owner,&version).await {
                    Ok(value) => value, Err(error) => unavailable_comments("unavailable",error.1)
                };
            }
        }
        bounded(out)
    }).await.map_err(|_|timeout())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected() -> IssueReadTarget {
        IssueReadTarget {
            tracker: rupu_scm::IssueTracker::Github,
            project: "org/repo".into(),
            account: "work".into(),
            number: u64::MAX,
            origin: "https://api.github.com".into(),
        }
    }
    fn query() -> ReadQuery {
        ReadQuery {
            host: Some("local".into()),
            project: "org/repo".into(),
            account: "work".into(),
            expected_origin: "https://api.github.com".into(),
            cursor: None,
        }
    }

    struct Catalog;
    #[async_trait::async_trait]
    impl crate::repos::RepoLister for Catalog {
        async fn list(&self) -> Result<Vec<crate::repos::RepoEntry>, crate::repos::RepoListError> {
            Ok(vec![])
        }
    }
    #[test]
    fn adapter_epoch_is_stable_only_for_the_same_live_allocation() {
        let first: Arc<dyn crate::repos::RepoLister> = Arc::new(Catalog);
        let second: Arc<dyn crate::repos::RepoLister> = Arc::new(Catalog);
        let epoch = adapter_epoch(&first);
        assert_eq!(epoch, adapter_epoch(&first.clone()));
        assert_ne!(epoch, adapter_epoch(&second));
        drop(first);
        let replacement: Arc<dyn crate::repos::RepoLister> = Arc::new(Catalog);
        assert_ne!(epoch, adapter_epoch(&replacement));
    }

    #[test]
    fn cursor_binds_every_authority_dimension_and_stream() {
        let target = selected();
        let value = next("instance", &target, "version", Kind::Body, 12).unwrap();
        assert_eq!(
            decode(&value, "instance", &target, Kind::Body)
                .unwrap()
                .position,
            12
        );
        assert!(decode(&value, "other-instance", &target, Kind::Body).is_err());
        assert!(decode(&value, "instance", &target, Kind::Comments).is_err());
        let mut variants = vec![target.clone(); 5];
        variants[0].account = "personal".into();
        variants[1].origin = "https://other.example".into();
        variants[2].project = "other/repo".into();
        variants[3].number = 1;
        variants[4].tracker = rupu_scm::IssueTracker::Gitlab;
        for other in variants {
            assert!(decode(&value, "instance", &other, Kind::Body).is_err());
        }
        let (payload, signature) = value.split_once('.').unwrap();
        let mut bytes = URL_SAFE_NO_PAD.decode(payload).unwrap();
        bytes[0] ^= 1;
        assert!(decode(
            &format!("{}.{}", URL_SAFE_NO_PAD.encode(bytes), signature),
            "instance",
            &target,
            Kind::Body
        )
        .is_err());
        let old = encode(Cursor {
            schema: 1,
            instance: "instance".into(),
            target: target.clone(),
            version: "version".into(),
            kind: Kind::Body,
            position: 12,
            issued_at: chrono::Utc::now().timestamp() - 901,
        })
        .unwrap();
        assert_eq!(
            decode(&old, "instance", &target, Kind::Body)
                .err()
                .unwrap()
                .1,
            "stale_cursor"
        );
    }

    #[test]
    fn body_pages_are_utf8_safe_and_reassemble_without_loss() {
        let value = "a".repeat(BODY_BYTES - 1) + &"🦀".repeat(BODY_BYTES);
        let target = selected();
        let mut offset = 0;
        let mut assembled = String::new();
        loop {
            let chunk = body_chunk(&value, offset, "instance", &target, "v").unwrap();
            let text = chunk["text"].as_str().unwrap();
            assert!(text.len() <= BODY_BYTES);
            assert_eq!(chunk["byte_count"], text.len());
            assembled.push_str(text);
            if let Some(cursor) = chunk["next_cursor"].as_str() {
                offset = decode(cursor, "instance", &target, Kind::Body)
                    .unwrap()
                    .position as usize;
            } else {
                break;
            }
        }
        assert_eq!(assembled, value);
        assert!(body_chunk("🦀", 1, "instance", &target, "v").is_err());
        assert!(body_chunk("x", 2, "instance", &target, "v").is_err());
    }

    #[test]
    fn target_requires_explicit_safe_authority_and_exact_integer_identity() {
        let q = query();
        assert_eq!(
            identity(&target("github", &u64::MAX.to_string(), &q).unwrap())["number"],
            u64::MAX.to_string()
        );
        assert!(target("github", "0", &q).is_err());
        assert!(target("github", "18446744073709551616", &q).is_err());
        let mut q = query();
        q.host = Some("remote-host".into());
        assert_eq!(target("github", "1", &q).err().unwrap().1, "unsupported");
        let mut q = query();
        q.expected_origin = "https://user:secret@api.github.com".into();
        assert_eq!(target("github", "1", &q).err().unwrap().1, "invalid_target");
    }

    #[test]
    fn serialized_response_budget_includes_json_escaping() {
        assert!(bounded(json!({"body":"x".repeat(RESPONSE_BYTES)})).is_err());
        assert!(bounded(json!({"body":"\u{0000}".repeat(RESPONSE_BYTES/5)})).is_err());
        assert!(bounded(json!({"body":"small"})).is_ok());
    }
}
