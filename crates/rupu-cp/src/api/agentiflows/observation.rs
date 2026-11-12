//! Finite, read-time observations. This is not an atomic coordinator snapshot.
//! Captured pages never read live files and never depend on a timestamp cursor.
mod capture;
mod input;
#[cfg(test)]
mod tests;

use super::*;
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

pub const FEATURE: &str = "agentiflow.observation.pages.v1";
const MAX_PAGE: usize = 1024 * 1024;
const MAX_CACHED: usize = 128 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 8;
const TTL: Duration = Duration::from_secs(600);
const REUSE: Duration = Duration::from_secs(5);

pub struct ObservationCache {
    snapshots: Mutex<Vec<Arc<Captured>>>,
    capture_gate: Arc<Semaphore>,
}
impl Default for ObservationCache {
    fn default() -> Self {
        Self {
            snapshots: Mutex::new(Vec::new()),
            capture_gate: Arc::new(Semaphore::new(1)),
        }
    }
}

struct Captured {
    id: String,
    token: String,
    key: [u8; 32],
    born: Instant,
    summary: Value,
    sections: BTreeMap<String, Vec<Value>>,
    bytes: usize,
}

#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotQuery {
    /// Local host only. Connect directly to a remote instance to observe it.
    host: Option<String>,
    /// Capture a new observation instead of the five-second cached view.
    #[serde(default)]
    refresh: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    /// Local host only.
    host: Option<String>,
    /// The exact opaque snapshot token returned by agentiflows.snapshot.
    snapshot: String,
    /// units, events, board, or leads.
    section: String,
    /// Omit for Start; otherwise use only the exact returned next_cursor.
    cursor: Option<String>,
    /// One through 100 records; the response also has a 1MiB byte limit.
    limit: Option<usize>,
}

pub(super) async fn snapshot(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SnapshotQuery>,
) -> ApiResult<Json<Value>> {
    require_local(q.host.as_deref(), "captured Agentiflow observations")?;
    {
        let mut cache = s
            .agentiflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        if !q.refresh {
            if let Some(c) = cache
                .iter()
                .rev()
                .find(|c| c.id == id && c.born.elapsed() < REUSE)
            {
                return Ok(Json(c.summary.clone()));
            }
        }
    }
    // No queued capture workers: one bounded capture is admitted, and existing
    // pages retain their immutable observation while capture IO is in progress.
    let permit = s
        .agentiflow_observations
        .capture_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "another observation is being captured; retry this read later".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let captured = Arc::new(capture::capture(&s, &id)?);
        let mut cache = s
            .agentiflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        while !cache.is_empty()
            && (cache.len() >= MAX_SNAPSHOTS
                || cache.iter().map(|c| c.bytes).sum::<usize>() + captured.bytes > MAX_CACHED)
        {
            cache.remove(0);
        }
        let summary = captured.summary.clone();
        cache.push(captured);
        Ok(Json(summary))
    })
    .await
    .map_err(|_| ApiError::internal("capturing Agentiflow observation failed"))?
}

pub(super) async fn page(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    require_local(q.host.as_deref(), "captured Agentiflow observations")?;
    if !valid_run_id(&id)
        || q.snapshot.len() > 64
        || q.cursor.as_ref().is_some_and(|c| c.len() > 256)
    {
        return Err(ApiError::bad_request(
            "invalid observation identity or cursor",
        ));
    }
    let limit = q.limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("limit must be 1 through 100"));
    }
    let captured = {
        let mut cache = s
            .agentiflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        cache
            .iter()
            .find(|c| c.token == q.snapshot && c.id == id)
            .cloned()
            .ok_or_else(|| {
                ApiError::conflict(
                    "observation expired or belongs to another flow; refresh explicitly",
                )
            })?
    };
    tokio::task::spawn_blocking(move || {
        captured
            .page(&q.section, q.cursor.as_deref(), limit)
            .map(Json)
    })
    .await
    .map_err(|_| ApiError::internal("reading captured Agentiflow page failed"))?
}

impl Captured {
    fn cursor(&self, section: &str, offset: usize) -> String {
        let body = format!("{}:{section}:{offset}", self.token);
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed HMAC key");
        mac.update(body.as_bytes());
        format!(
            "{offset}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }
    fn offset(&self, section: &str, cursor: Option<&str>) -> ApiResult<usize> {
        let Some(cursor) = cursor else { return Ok(0) };
        let (number, signature) = cursor
            .split_once('.')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let offset: usize = number
            .parse()
            .map_err(|_| ApiError::bad_request("invalid cursor"))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| ApiError::bad_request("invalid cursor"))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed HMAC key");
        mac.update(format!("{}:{section}:{offset}", self.token).as_bytes());
        mac.verify_slice(&bytes).map_err(|_| {
            ApiError::conflict("cursor belongs to another observation, section, or position")
        })?;
        Ok(offset)
    }
    fn page(&self, section: &str, cursor: Option<&str>, limit: usize) -> ApiResult<Value> {
        let rows = self.sections.get(section).ok_or_else(|| {
            ApiError::bad_request("section must be units, events, board, or leads")
        })?;
        let start = self.offset(section, cursor)?;
        if start > rows.len() {
            return Err(ApiError::conflict("cursor is outside this observation"));
        }
        let mut end = start;
        // Reserve a generous envelope budget; verify exact serialized size below.
        let mut bytes = 4096;
        while end < rows.len() && end - start < limit {
            let size = serde_json::to_vec(&rows[end])
                .map_err(|_| ApiError::internal("invalid captured row"))?
                .len();
            if bytes + size > MAX_PAGE {
                break;
            }
            bytes += size + 1;
            end += 1;
        }
        if end == start && start < rows.len() {
            return Err(too_large("one captured record exceeds the page budget"));
        }
        let value = serde_json::json!({
            "flow_id": self.id, "host_id": "local", "snapshot": self.token, "section": section,
            "source_generation": self.summary["source_generation"],
            "captured_at": self.summary["captured_at"], "expires_at": self.summary["expires_at"],
            "offset": start, "total": rows.len(), "records": &rows[start..end],
            "next_cursor": (end < rows.len()).then(|| self.cursor(section, end)),
            "evidence": self.summary["evidence"],
        });
        if serde_json::to_vec(&value)
            .map_err(|_| ApiError::internal("invalid captured page"))?
            .len()
            > MAX_PAGE
        {
            return Err(too_large("captured page exceeds the response budget"));
        }
        Ok(value)
    }
}

fn too_large(reason: &str) -> ApiError {
    ApiError(StatusCode::PAYLOAD_TOO_LARGE, reason.into())
}
