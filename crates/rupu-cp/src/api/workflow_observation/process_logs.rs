//! Finite local retained process diagnostics. Never proxies the legacy event SSE.
use super::*;
use rustix::fs::{Mode, OFlags};
use sha2::Digest;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) const FEATURE: &str = "workflow.process_logs.pages.v1";
const CAPTURE_BYTES: usize = 256 * 1024;
const PAGE_BYTES: usize = 16 * 1024;
const ENCODED_CAPTURE_BYTES: usize = CAPTURE_BYTES * 3;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Source {
    Retry,
    SessionLaunch,
}
impl Source {
    fn filename(self) -> &'static str {
        match self {
            Self::Retry => "retry.log",
            Self::SessionLaunch => "session-launch.log",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueryParams {
    ws_id: String,
    host: Option<String>,
    source: Source,
    snapshot: Option<String>,
    cursor: Option<String>,
}
#[derive(Default)]
pub(super) struct Cache {
    entries: Mutex<Vec<Arc<Capture>>>,
}
struct Capture {
    id: String,
    ws: String,
    source: Source,
    snapshot: String,
    key: [u8; 32],
    born: Instant,
    header: Value,
    text: String,
}
struct Cancellation(Arc<AtomicBool>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
fn check(cancel: &AtomicBool) -> ApiResult<()> {
    if cancel.load(Ordering::Acquire) {
        Err(ApiError::conflict("process log capture cancelled"))
    } else {
        Ok(())
    }
}
fn file(dir: &File, name: &str) -> ApiResult<Option<File>> {
    let fd = match rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => {
            return Err(ApiError::conflict(
                "process log is not an anchored regular file",
            ))
        }
    };
    let file = File::from(fd);
    if !file
        .metadata()
        .map_err(|_| ApiError::conflict("process log metadata unavailable"))?
        .is_file()
    {
        return Err(ApiError::conflict("process log is not a regular file"));
    }
    Ok(Some(file))
}
fn stamp(file: &File) -> ApiResult<(u64, u64, u64, i64, i64, i64, i64)> {
    let m = file
        .metadata()
        .map_err(|_| ApiError::conflict("process log metadata unavailable"))?;
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn same_dir(a: &File, b: &File) -> ApiResult<bool> {
    let a = stamp(a)?;
    let b = stamp(b)?;
    Ok(a.0 == b.0 && a.1 == b.1)
}
fn capture(s: &AppState, id: &str, q: &QueryParams, cancel: &AtomicBool) -> ApiResult<Capture> {
    capture_checked(s, id, q, cancel, || {})
}
fn local_owner(owner: &super::owner::Owner) -> ApiResult<()> {
    let raw: Value = serde_json::from_slice(&owner.owner_bytes)
        .map_err(|_| ApiError::conflict("invalid owner"))?;
    for key in ["host_id", "host"] {
        if !raw[key].is_null() && raw[key].as_str() != Some("local") {
            return Err(ApiError::conflict("nonlocal workflow artifact owner"));
        }
    }
    Ok(())
}
fn capture_checked(
    s: &AppState,
    id: &str,
    q: &QueryParams,
    cancel: &AtomicBool,
    after_read: impl FnOnce(),
) -> ApiResult<Capture> {
    check(cancel)?;
    let mut owner = super::owner::open(s, id, &q.ws_id)?;
    local_owner(&owner)?;
    let opened = file(&owner.input.dir, q.source.filename())?;
    let (text, source_bytes, start, invalid, source_stamp) = if let Some(mut f) = opened {
        let before = stamp(&f)?;
        let start = before.2.saturating_sub(CAPTURE_BYTES as u64);
        f.seek(SeekFrom::Start(start))
            .map_err(|_| ApiError::conflict("process log seek failed"))?;
        let mut bytes = Vec::with_capacity(CAPTURE_BYTES);
        let mut buffer = [0u8; 8192];
        while bytes.len() < CAPTURE_BYTES {
            check(cancel)?;
            owner.input.deadline()?;
            let cap = buffer.len().min(CAPTURE_BYTES - bytes.len());
            let n = f
                .read(&mut buffer[..cap])
                .map_err(|_| ApiError::conflict("process log read failed"))?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..n]);
        }
        if stamp(&f)? != before || bytes.len() as u64 != before.2 - start {
            return Err(ApiError::conflict("process log changed during capture"));
        }
        let invalid = std::str::from_utf8(&bytes).is_err();
        (
            String::from_utf8_lossy(&bytes).into_owned(),
            before.2,
            start,
            invalid,
            Some(before),
        )
    } else {
        (String::new(), 0, 0, false, None)
    };
    check(cancel)?;
    after_read();
    owner.verify(id, &q.ws_id)?;
    let fresh = super::owner::open(s, id, &q.ws_id)?;
    local_owner(&fresh)?;
    if fresh.run.workflow_name != owner.run.workflow_name
        || fresh.run.workspace_path != owner.run.workspace_path
        || fresh.run.transcript_dir != owner.run.transcript_dir
        || fresh.store_kind != owner.store_kind
        || fresh.run.started_at != owner.run.started_at
        || !same_dir(&fresh.input.dir, &owner.input.dir)?
    {
        return Err(ApiError::conflict(
            "process log owner changed during capture",
        ));
    }
    let after = file(&fresh.input.dir, q.source.filename())?
        .as_ref()
        .map(stamp)
        .transpose()?;
    if after != source_stamp {
        return Err(ApiError::conflict(
            "process log pathname changed during capture",
        ));
    }
    owner.input.deadline()?;
    check(cancel)?;
    let snapshot = ulid::Ulid::new().to_string();
    let mut key = [0; 32];
    key[..16].copy_from_slice(&ulid::Ulid::new().to_bytes());
    key[16..].copy_from_slice(&ulid::Ulid::new().to_bytes());
    let header = json!({"schema":FEATURE,"run_id":id,"workspace_id":q.ws_id,"host_id":"local","source":q.source,
        "snapshot":snapshot,"observed_at":chrono::Utc::now().to_rfc3339(),"store_kind":owner.store_kind,"archived":owner.archived,
        "provenance":"locally_retained_workflow_artifact","process_host":"unknown","history_complete":false,
        "state":if source_stamp.is_some(){"available"}else{"missing"},"source_bytes":source_bytes.to_string(),
        "captured_start":start.to_string(),"captured_end":source_bytes.to_string(),"head_truncated":start>0,
        "text_encoding":"utf8_lossy","invalid_utf8":invalid,"text_bytes":text.len(),"text_sha256":format!("sha256:{:x}",Sha256::digest(text.as_bytes()))});
    if text.len() > ENCODED_CAPTURE_BYTES {
        return Err(too_large("process log projection limit"));
    }
    Ok(Capture {
        id: id.into(),
        ws: q.ws_id.clone(),
        source: q.source,
        snapshot,
        key,
        born: Instant::now(),
        header,
        text,
    })
}
impl Capture {
    fn mac(&self, offset: usize) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed key");
        mac.update(
            &serde_json::to_vec(&(&self.id, &self.ws, self.source, &self.snapshot, offset))
                .expect("scalar tuple"),
        );
        mac
    }
    fn cursor(&self, offset: usize) -> String {
        format!(
            "{offset}.{}",
            URL_SAFE_NO_PAD.encode(self.mac(offset).finalize().into_bytes())
        )
    }
    fn page(&self, cursor: Option<&str>) -> ApiResult<Value> {
        let offset = if let Some(cursor) = cursor {
            if cursor.len() > 128 {
                return Err(ApiError::bad_request("invalid process log cursor"));
            }
            let (offset, sig) = cursor
                .split_once('.')
                .ok_or_else(|| ApiError::bad_request("invalid process log cursor"))?;
            let offset: usize = offset
                .parse()
                .map_err(|_| ApiError::bad_request("invalid process log cursor"))?;
            let sig = URL_SAFE_NO_PAD
                .decode(sig)
                .map_err(|_| ApiError::bad_request("invalid process log cursor"))?;
            self.mac(offset)
                .verify_slice(&sig)
                .map_err(|_| ApiError::conflict("foreign process log cursor"))?;
            offset
        } else {
            0
        };
        if offset > self.text.len() || !self.text.is_char_boundary(offset) {
            return Err(ApiError::bad_request("invalid process log offset"));
        }
        let mut end = offset.saturating_add(PAGE_BYTES).min(self.text.len());
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(
            json!({"header":self.header,"offset":offset,"end":end,"text":&self.text[offset..end],
            "next_cursor":if end<self.text.len(){Some(self.cursor(end))}else{None}}),
        )
    }
}
pub(super) async fn get(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<QueryParams>,
) -> ApiResult<Json<Value>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    if q.snapshot.as_ref().is_some_and(|v| v.len() > 64)
        || (q.cursor.is_some() && q.snapshot.is_none())
    {
        return Err(ApiError::bad_request("invalid process log selection"));
    }
    let _page = s
        .workflow_observations
        .pages
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "process log readers busy".into(),
            )
        })?;
    if let Some(snapshot) = &q.snapshot {
        let mut cache = s
            .workflow_observations
            .process_logs
            .entries
            .lock()
            .map_err(|_| ApiError::internal("process log cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        let c = cache
            .iter()
            .find(|c| {
                c.snapshot == *snapshot && c.id == id && c.ws == q.ws_id && c.source == q.source
            })
            .ok_or_else(|| {
                ApiError::conflict("process log snapshot expired or belongs to another owner")
            })?;
        return c.page(q.cursor.as_deref()).map(Json);
    }
    let permit = s
        .workflow_observations
        .gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "workflow capture busy".into(),
            )
        })?;
    let cancel = Cancellation(Arc::new(AtomicBool::new(false)));
    let flag = cancel.0.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let c = Arc::new(capture(&s, &id, &q, &flag)?);
        let page = c.page(None)?;
        publish(&s.workflow_observations.process_logs, c, &flag)?;
        Ok(Json(page))
    })
    .await
    .map_err(|_| ApiError::internal("process log capture failed"))?
}
fn publish(store: &Cache, capture: Arc<Capture>, cancelled: &AtomicBool) -> ApiResult<()> {
    let mut cache = store
        .entries
        .lock()
        .map_err(|_| ApiError::internal("process log cache unavailable"))?;
    check(cancelled)?;
    cache.retain(|c| c.born.elapsed() < TTL);
    while cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push(capture);
    Ok(())
}
#[cfg(test)]
mod tests;
