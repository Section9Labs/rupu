//! Current issue discovery over exact owning connector selection. A cursor is a
//! live provider continuation, not a stable snapshot, SSE ID or read grant.
use super::issues_read::{instance, reader, require_local, valid_text, ReadError};
use crate::{
    issue_reader::{IssueDiscoveryTarget, IssueReadError, IssueReader},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rupu_scm::issue_discovery::{DiscoveryPage, DiscoveryRequest, DiscoveryState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    sync::{LazyLock, OnceLock},
    time::Duration,
};
use tokio::sync::Semaphore;

pub const FEATURE: &str = "issues.discovery.pages.v1";
const OUTPUT: usize = 256 * 1024;
const CURSOR_BYTES: usize = 16 * 1024;
const TTL: i64 = 600;
static READERS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));
type Result<T> = std::result::Result<T, ReadError>;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/issues/:tracker/discovery", get(page))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryQuery {
    host: Option<String>,
    project: String,
    account: String,
    expected_origin: String,
    state: DiscoveryState,
    title_query: Option<String>,
    cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Filters {
    state: DiscoveryState,
    title_query: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    schema: u8,
    instance: String,
    target: IssueDiscoveryTarget,
    filters: Filters,
    epoch: String,
    after: String,
    provider_project_id: String,
    issued_at: i64,
    ordinal: u32,
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> ReadError {
    ReadError(status, code, message)
}
fn invalid() -> ReadError {
    error(
        StatusCode::BAD_REQUEST,
        "invalid_cursor",
        "The discovery cursor is invalid for this selection",
    )
}
fn stale() -> ReadError {
    error(
        StatusCode::CONFLICT,
        "selection_changed",
        "The owning account or discovery selection changed; refresh explicitly",
    )
}
fn expired() -> ReadError {
    error(
        StatusCode::GONE,
        "cursor_expired",
        "The discovery continuation expired; refresh explicitly",
    )
}
fn upstream() -> ReadError {
    IssueReadError::UpstreamUnavailable.into()
}
fn selection(tracker: &str, q: &DiscoveryQuery) -> Result<(IssueDiscoveryTarget, Filters)> {
    require_local(q.host.as_deref())?;
    if !valid_text(&q.project, 512)
        || !valid_text(&q.account, 128)
        || !valid_text(&q.expected_origin, 2048)
        || rupu_scm::operator_issues::safe_origin(&q.expected_origin).as_deref()
            != Some(q.expected_origin.as_str())
        || q.title_query
            .as_ref()
            .is_some_and(|x| !valid_text(x, 128) || x.trim().is_empty())
    {
        return Err(IssueReadError::InvalidTarget.into());
    }
    Ok((
        IssueDiscoveryTarget {
            tracker: tracker
                .parse()
                .map_err(|_| ReadError::from(IssueReadError::InvalidTarget))?,
            project: q.project.clone(),
            account: q.account.clone(),
            origin: q.expected_origin.clone(),
        },
        Filters {
            state: q.state,
            title_query: q.title_query.clone(),
        },
    ))
}
fn mac() -> Hmac<Sha256> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut key = [0; 32];
        for part in key.chunks_mut(8) {
            part.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        key
    });
    Hmac::<Sha256>::new_from_slice(key).expect("fixed HMAC key")
}
fn encode(c: &Cursor) -> Result<String> {
    let bytes = serde_json::to_vec(c).map_err(|_| invalid())?;
    let mut mac = mac();
    mac.update(&bytes);
    let value = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(bytes),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    if value.len() > CURSOR_BYTES {
        return Err(upstream());
    }
    Ok(value)
}
fn decode(
    value: &str,
    owner: &str,
    target: &IssueDiscoveryTarget,
    filters: &Filters,
    epoch: &str,
    now: i64,
) -> Result<Cursor> {
    if value.len() > CURSOR_BYTES {
        return Err(invalid());
    }
    let (payload, signature) = value.split_once('.').ok_or_else(invalid)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
    let mut mac = mac();
    mac.update(&bytes);
    mac.verify_slice(&signature).map_err(|_| invalid())?;
    let c: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if c.schema != 1
        || c.instance != owner
        || &c.target != target
        || &c.filters != filters
        || c.epoch != epoch
    {
        return Err(stale());
    }
    if c.issued_at > now || now.saturating_sub(c.issued_at) >= TTL {
        return Err(expired());
    }
    if !valid_text(&c.provider_project_id, 256)
        || !valid_text(&c.after, 4096)
        || c.ordinal == 0
        || c.ordinal > 10000
    {
        return Err(invalid());
    }
    Ok(c)
}
fn validate_page(p: &DiscoveryPage, query: &Filters) -> Result<()> {
    if p.rows.len() > 25
        || !valid_text(&p.provider_project_id, 256)
        || !valid_text(&p.project, 512)
        || !valid_text(&p.connector_epoch, 512)
        || p.next.as_ref().is_some_and(|x| !valid_text(x, 4096))
        || (p.end_observed && (p.next.is_some() || p.truncated))
        || (!p.end_observed && p.next.is_none() && !p.truncated)
        || p.search_mode
            != if query.title_query.is_some() {
                "provider_title_query"
            } else {
                "none"
            }
    {
        return Err(upstream());
    }
    let mut seen = std::collections::BTreeSet::new();
    for row in &p.rows {
        let number = row.number.parse::<u64>().ok().filter(|n| *n > 0);
        if number.is_none()
            || number.unwrap().to_string() != row.number
            || !seen.insert(&row.number)
            || row.title.len() > 4096
            || row.title.chars().any(char::is_control)
            || !matches!(
                (query.state, row.state.as_str()),
                (DiscoveryState::All, "open" | "closed")
                    | (DiscoveryState::Open, "open")
                    | (DiscoveryState::Closed, "closed")
            )
            || row
                .updated_at
                .as_ref()
                .is_some_and(|v| v.len() > 128 || chrono::DateTime::parse_from_rfc3339(v).is_err())
            || row.url.as_ref().is_some_and(|v| !safe_link(v))
        {
            return Err(upstream());
        }
    }
    Ok(())
}
fn safe_link(value: &str) -> bool {
    value.len() <= 2048
        && reqwest::Url::parse(value).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https")
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none()
        })
}
async fn read(
    port: &dyn IssueReader,
    owner: &str,
    target: IssueDiscoveryTarget,
    filters: Filters,
    cursor: Option<&str>,
) -> Result<Json<Value>> {
    if !port.supports_discovery() {
        return Err(IssueReadError::Unsupported.into());
    }
    let epoch = port.discovery_epoch(&target)?;
    if !valid_text(&epoch, 512) {
        return Err(upstream());
    }
    let now = chrono::Utc::now().timestamp();
    let previous = cursor
        .map(|c| decode(c, owner, &target, &filters, &epoch, now))
        .transpose()?;
    let issued_at = previous.as_ref().map_or(now, |c| c.issued_at);
    let ordinal = previous.as_ref().map_or(0, |c| c.ordinal);
    let request = DiscoveryRequest {
        project: target.project.clone(),
        state: filters.state,
        title_query: filters.title_query.clone(),
        after: previous.as_ref().map(|c| c.after.clone()),
    };
    let result = port.discovery_page(&target, &request).await?;
    if port.discovery_epoch(&target)? != epoch || result.connector_epoch != epoch {
        return Err(stale());
    }
    if chrono::Utc::now().timestamp().saturating_sub(issued_at) >= TTL {
        return Err(expired());
    }
    validate_page(&result, &filters)?;
    if previous
        .as_ref()
        .is_some_and(|p| p.provider_project_id != result.provider_project_id)
    {
        return Err(stale());
    }
    if result
        .next
        .as_deref()
        .is_some_and(|n| request.after.as_deref() == Some(n))
    {
        return Err(upstream());
    }
    let capped = ordinal >= 9999 && result.next.is_some();
    let next = if capped {
        None
    } else {
        result
            .next
            .as_ref()
            .map(|after| {
                encode(&Cursor {
                    schema: 1,
                    instance: owner.into(),
                    target: target.clone(),
                    filters: filters.clone(),
                    epoch: epoch.clone(),
                    after: after.clone(),
                    provider_project_id: result.provider_project_id.clone(),
                    issued_at,
                    ordinal: ordinal + 1,
                })
            })
            .transpose()?
    };
    let value = json!({"schema":FEATURE,"source":"current_provider","observed_at":chrono::Utc::now().to_rfc3339(),
        "expires_at":chrono::DateTime::from_timestamp(issued_at+TTL,0).ok_or_else(upstream)?.to_rfc3339(),
        "identity":{"host_id":"local","instance_id":owner,"tracker":target.tracker,"project":target.project,"account":target.account,"origin":target.origin,"connector_epoch":epoch},
        "provider_project":result.project,"provider_project_id":result.provider_project_id,"query":filters,"search_mode":result.search_mode,"rows":result.rows,
        "pagination":{"mode":"live","order":"provider_order","page_size":25,"next_cursor":next,"end_observed":result.end_observed,"truncated":result.truncated||capped,"total":Value::Null}});
    if serde_json::to_vec(&value).map_err(|_| upstream())?.len() > OUTPUT {
        return Err(upstream());
    }
    Ok(Json(value))
}
async fn page(
    State(s): State<AppState>,
    Path(tracker): Path<String>,
    Query(q): Query<DiscoveryQuery>,
) -> Result<Json<Value>> {
    let (target, filters) = selection(&tracker, &q)?;
    let s = super::issues_read::current_state(&s).await?;
    let port = reader(&s)?;
    let owner = instance(&s);
    let _permit = READERS.try_acquire().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Issue discovery readers are busy; retry later",
        )
    })?;
    tokio::time::timeout(
        Duration::from_secs(20),
        read(port, &owner, target, filters, q.cursor.as_deref()),
    )
    .await
    .map_err(|_| {
        error(
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "The issue discovery read exceeded its deadline",
        )
    })?
}

#[cfg(test)]
mod tests;
