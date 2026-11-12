//! Bounded immutable observations of a recorded workflow, separate from legacy graph.
mod capture;
mod descendant_routes;
mod descendants;
mod event_emitters;
mod frontier;
mod input;
mod member;
mod owner;
mod process_logs;
#[cfg(test)]
mod tests;
mod transcript_follow;
mod transcript_route;

use crate::{
    error::{ApiError, ApiResult},
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub(crate) use process_logs::FEATURE as PROCESS_LOG_FEATURE;
pub(crate) const EVENT_EMITTER_FEATURE: &str = "workflow.observation.event_emitters.v1";
pub(crate) const LIVE_FEATURE: &str = "workflow.observation.live.v1";
pub(crate) const DESCENDANT_FEATURE: &str = "workflow.observation.descendants.v1";
pub(crate) const LATEST_PAGE_FEATURE: &str = "workflow.observation.latest_page.v1";
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Window {
    #[default]
    Prefix,
    Latest,
}
pub(crate) const FEATURE: &str = "workflow.observation.pages.v1";
const MAX_PAGE: usize = 1024 * 1024;
const MAX_ROW: usize = 64 * 1024;
const MAX_CAPTURE: usize = 32 * 1024 * 1024;
const MAX_CACHE: usize = 64 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(600);

pub struct Cache {
    snapshots: Mutex<Vec<Arc<Captured>>>,
    gate: Arc<Semaphore>,
    pages: Arc<Semaphore>,
    metadata: Arc<Semaphore>,
    epoch: String,
    follow: transcript_follow::Cache,
    follow_jobs: Arc<Semaphore>,
    process_logs: process_logs::Cache,
    descendants: descendants::Cache,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            snapshots: Mutex::new(Vec::new()),
            gate: Arc::new(Semaphore::new(1)),
            pages: Arc::new(Semaphore::new(4)),
            metadata: Arc::new(Semaphore::new(2)),
            epoch: ulid::Ulid::new().to_string(),
            follow: transcript_follow::Cache::default(),
            follow_jobs: Arc::new(Semaphore::new(4)),
            process_logs: process_logs::Cache::default(),
            descendants: descendants::Cache::default(),
        }
    }
}
struct Captured {
    run_id: String,
    workspace: String,
    token: String,
    key: [u8; 32],
    born: Instant,
    window: Window,
    event_emitters: bool,
    header: Value,
    sections: BTreeMap<String, Vec<Vec<u8>>>,
    result_steps: BTreeMap<String, Vec<usize>>,
    result_records: BTreeMap<String, usize>,
    bytes: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotQuery {
    host: Option<String>,
    ws_id: String,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    window: Window,
    #[serde(default)]
    include_event_emitters: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    host: Option<String>,
    ws_id: String,
    snapshot: String,
    section: String,
    step_id: Option<String>,
    parent: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    latest: bool,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs/:id/process-log", get(process_logs::get))
        .route("/api/runs/:id/observation", get(snapshot))
        .route("/api/runs/:id/observation/page", get(page))
        .route("/api/runs/:id/observation/frontier", get(frontier::get))
        .route(
            "/api/runs/:id/observation/transcript",
            get(transcript_route::get),
        )
        .route(
            "/api/runs/:id/observation/transcript/children",
            get(descendant_routes::children),
        )
        .route(
            "/api/runs/:id/observation/transcript/descendant",
            get(descendant_routes::open),
        )
}
fn owner(id: &str, workspace: &str, host: Option<&str>) -> ApiResult<()> {
    super::fleet::require_local(host, "workflow observations")?;
    super::runs::validate_id(id)?;
    super::runs::validate_id(workspace)?;
    if id.len() > 128 || workspace.len() > 128 {
        return Err(ApiError::bad_request("owner identifier too long"));
    }
    Ok(())
}
async fn snapshot(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SnapshotQuery>,
) -> ApiResult<Json<Value>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    {
        let mut cache = s
            .workflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        if !q.refresh {
            if let Some(c) = cache.iter().rev().find(|c| {
                c.run_id == id
                    && c.workspace == q.ws_id
                    && c.window == q.window
                    && c.event_emitters == q.include_event_emitters
                    && c.born.elapsed() < Duration::from_secs(5)
            }) {
                return Ok(Json(c.header.clone()));
            }
        }
    }
    let permit = s
        .workflow_observations
        .gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "another workflow observation is being captured".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let captured = Arc::new(capture::capture_mode(
            &s,
            &id,
            &q.ws_id,
            q.window,
            q.include_event_emitters,
        )?);
        let mut cache = s
            .workflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        while !cache.is_empty()
            && (cache.len() >= 4
                || cache.iter().map(|c| c.bytes).sum::<usize>() + captured.bytes > MAX_CACHE)
        {
            cache.remove(0);
        }
        let header = captured.header.clone();
        cache.push(captured);
        Ok(Json(header))
    })
    .await
    .map_err(|_| ApiError::internal("workflow capture task failed"))?
}
async fn page(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    if q.snapshot.len() > 64
        || q.section.len() > 24
        || q.parent.as_ref().is_some_and(|s| s.len() > 40)
        || q.cursor.as_ref().is_some_and(|s| s.len() > 128)
    {
        return Err(ApiError::bad_request("invalid observation selector"));
    }
    let limit = q.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("limit must be 1 through 100"));
    }
    let _permit = s
        .workflow_observations
        .pages
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "workflow page preparation busy".into(),
            )
        })?;
    let c = {
        let mut cache = s
            .workflow_observations
            .snapshots
            .lock()
            .map_err(|_| ApiError::internal("observation cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        cache
            .iter()
            .find(|c| c.run_id == id && c.workspace == q.ws_id && c.token == q.snapshot)
            .cloned()
            .ok_or_else(|| {
                ApiError::conflict(
                    "observation expired, evicted, or belongs to another owner; capture again",
                )
            })?
    };
    if q.latest {
        if q.cursor.is_some() || q.parent.is_some() {
            return Err(ApiError::bad_request(
                "latest page excludes cursor and parent",
            ));
        }
        return c
            .latest_filtered_page(&q.section, q.step_id.as_deref(), limit)
            .map(Json);
    }
    c.filtered_page(
        &q.section,
        q.parent.as_deref(),
        q.step_id.as_deref(),
        q.cursor.as_deref(),
        limit,
    )
    .map(Json)
}
impl Captured {
    fn mac(&self, section: &str, offset: usize) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed key");
        // Length-delimited JSON binds every identity, including exact parent result.
        mac.update(
            &serde_json::to_vec(&(&self.run_id, &self.workspace, &self.token, section, offset))
                .expect("scalar tuple"),
        );
        mac
    }
    fn cursor(&self, section: &str, offset: usize) -> String {
        format!(
            "{offset}.{}",
            URL_SAFE_NO_PAD.encode(self.mac(section, offset).finalize().into_bytes())
        )
    }
    fn offset(&self, section: &str, cursor: Option<&str>) -> ApiResult<usize> {
        let Some(cursor) = cursor else {
            return Ok(0);
        };
        let (offset, signature) = cursor
            .split_once('.')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let offset = offset
            .parse()
            .map_err(|_| ApiError::bad_request("invalid cursor"))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| ApiError::bad_request("invalid cursor"))?;
        self.mac(section, offset)
            .verify_slice(&signature)
            .map_err(|_| {
                ApiError::conflict(
                    "cursor does not belong to this owner, snapshot, section and parent",
                )
            })?;
        Ok(offset)
    }
    #[cfg(test)]
    fn page(
        &self,
        section: &str,
        parent: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> ApiResult<Value> {
        self.filtered_page(section, parent, None, cursor, limit)
    }
    /// Choose a newest suffix on the server; keep chronology within the slice.
    fn latest_filtered_page(
        &self,
        section: &str,
        step_id: Option<&str>,
        limit: usize,
    ) -> ApiResult<Value> {
        if self.window != Window::Latest
            || !matches!(section, "results" | "events" | "checkpoints" | "attempts")
            || !(1..=100).contains(&limit)
            || step_id.is_some_and(|v| section != "results" || v.is_empty() || v.len() > 1024)
        {
            return Err(ApiError::bad_request(
                "latest page requires a latest log section and valid step filter",
            ));
        }
        let rows = &self.sections[section];
        let selection = step_id.map(|step| {
            self.result_steps
                .get(step)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        });
        let count = selection.map_or(rows.len(), |v| v.len());
        let mut start = count;
        let mut bytes = 16384;
        while start > 0 && count - start < limit {
            let index = selection.map_or(start - 1, |v| v[start - 1]);
            let weight = rows[index].len() + 1;
            if bytes + weight >= MAX_PAGE {
                break;
            }
            bytes += weight;
            start -= 1;
        }
        let key = step_id.map_or_else(
            || section.to_owned(),
            |step| {
                format!(
                    "{section}/step:{}",
                    serde_json::to_string(step).expect("step string")
                )
            },
        );
        let mut page = self.filtered_page(
            section,
            None,
            step_id,
            Some(&self.cursor(&key, start)),
            limit,
        )?;
        page["selection"] = json!("latest");
        if serde_json::to_vec(&page).expect("page JSON").len() > MAX_PAGE {
            return Err(too_large("latest page envelope limit"));
        }
        Ok(page)
    }
    fn filtered_page(
        &self,
        section: &str,
        parent: Option<&str>,
        step_id: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> ApiResult<Value> {
        if step_id.is_some_and(|v| section != "results" || v.is_empty() || v.len() > 1024) {
            return Err(ApiError::bad_request(
                "step_id requires results and 1 through 1024 UTF-8 bytes",
            ));
        }
        let key = match (section, parent) {
            ("items" | "findings", Some(parent))
                if parent.starts_with("result:") && parent[7..].parse::<u64>().is_ok() =>
            {
                format!("{section}/{parent}")
            }
            ("steps" | "edges" | "results" | "checkpoints" | "attempts" | "events", None) => {
                section.to_owned()
            }
            _ => {
                return Err(ApiError::bad_request(
                    "invalid section or exact parent selector",
                ))
            }
        };
        let rows = self.sections.get(&key).ok_or_else(|| {
            ApiError::not_found("parent result is not available in this observation")
        })?;
        let selection = step_id.map(|step| {
            self.result_steps
                .get(step)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        });
        let count = selection.map_or(rows.len(), |v| v.len());
        let row_at = |index: usize| &rows[selection.map_or(index, |v| v[index])];
        let key = step_id.map_or(key.clone(), |step| {
            format!(
                "{key}/step:{}",
                serde_json::to_string(step).expect("step string")
            )
        });
        let start = self.offset(&key, cursor)?;
        if start > count {
            return Err(ApiError::conflict("cursor outside observation"));
        }
        let mut end = start;
        let mut bytes = 16384;
        while end < count && end - start < limit && bytes + row_at(end).len() + 1 < MAX_PAGE {
            bytes += row_at(end).len() + 1;
            end += 1;
        }
        let records: Vec<Value> = (start..end)
            .map(|index| serde_json::from_slice(row_at(index)).expect("captured JSON"))
            .collect();
        let unassigned = if section == "results" {
            self.header["sections"]["results"]["unassigned_rows"]
                .as_u64()
                .unwrap_or(0)
        } else {
            0
        };
        let source_complete = if parent.is_some() {
            json!(true)
        } else {
            self.header["sections"][section]["complete"].clone()
        };
        let source_total = if parent.is_some() {
            json!(count)
        } else if step_id.is_some() {
            if source_complete == true && unassigned == 0 {
                json!(count)
            } else {
                Value::Null
            }
        } else {
            self.header["sections"][section]["source_total"].clone()
        };
        let mut value = json!({"schema":FEATURE,"run_id":self.run_id,"workspace_id":self.workspace,"host_id":"local","snapshot":self.token,
            "window":self.window,"frontier":self.header["frontier"],"source_generation":self.header["source_generation"],"observed_at":self.header["observed_at"],"expires_at":self.header["expires_at"],
            "section":section,"parent":parent,"step_id":step_id,"offset":start,"captured_total":count,"records":records,
            "source_complete":source_complete,"source_total":source_total,"unassigned_rows":unassigned,
            "next_cursor":(end < count).then(|| self.cursor(&key,end))});
        if self.event_emitters {
            value["event_emitters"] = json!(true);
        }
        if serde_json::to_vec(&value).expect("page JSON").len() > MAX_PAGE {
            return Err(too_large("page envelope limit"));
        }
        Ok(value)
    }
}
fn too_large(message: &str) -> ApiError {
    ApiError(StatusCode::PAYLOAD_TOO_LARGE, message.into())
}
