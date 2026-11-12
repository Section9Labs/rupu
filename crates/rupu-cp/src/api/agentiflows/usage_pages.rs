//! Immutable bounded per-call accounting. Accounting locators never authorize navigation.
use super::*;
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rupu_runtime::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read},
    path::Path as FsPath,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub const FEATURE: &str = "agentiflow.usage.pages.v1";
const MAX_LEDGER: usize = 16 * 1024 * 1024;
const MAX_CHILDREN: usize = 8 * 1024 * 1024;
const MAX_PROOF: usize = 256 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_ROWS: usize = 100_000;
const MAX_CAPTURE: usize = 32 * 1024 * 1024;
const MAX_CACHE: usize = 64 * 1024 * 1024;
const MAX_PAGE: usize = 1024 * 1024;
const TTL: Duration = Duration::from_secs(600);

pub struct UsageCache {
    entries: Mutex<Vec<Arc<Capture>>>,
    gate: Arc<Semaphore>,
}
impl Default for UsageCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            gate: Arc::new(Semaphore::new(1)),
        }
    }
}
#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    pub host: Option<String>,
    pub project: String,
    pub snapshot: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
struct Capture {
    flow: String,
    project: String,
    token: String,
    key: [u8; 32],
    born: Instant,
    envelope: Value,
    rows: Vec<Value>,
    bytes: usize,
}
fn large(msg: &str) -> ApiError {
    ApiError(StatusCode::PAYLOAD_TOO_LARGE, msg.into())
}
fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
fn check_time(start: Instant) -> ApiResult<()> {
    if start.elapsed() > Duration::from_secs(5) {
        Err(large("usage capture exceeds its cooperative time budget"))
    } else {
        Ok(())
    }
}
pub(super) async fn page(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    require_local(q.host.as_deref(), "Agentiflow usage pages")?;
    if !safe_id(&id)
        || !id.starts_with("af_")
        || !safe_id(&q.project)
        || q.snapshot.as_ref().is_some_and(|s| s.len() > 64)
        || q.cursor.as_ref().is_some_and(|s| s.len() > 256)
        || (q.cursor.is_some() && q.snapshot.is_none())
    {
        return Err(ApiError::bad_request("invalid usage scope or cursor"));
    }
    let limit = q.limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("limit must be 1 through 100"));
    }
    if let Some(token) = &q.snapshot {
        let c = {
            let mut cache = s
                .agentiflow_usage_pages
                .entries
                .lock()
                .map_err(|_| ApiError::internal("usage cache unavailable"))?;
            cache.retain(|c| c.born.elapsed() < TTL);
            cache
                .iter()
                .find(|c| &c.token == token && c.flow == id && c.project == q.project)
                .cloned()
                .ok_or_else(|| {
                    ApiError::conflict(
                        "usage observation expired or belongs to another owner; refresh explicitly",
                    )
                })?
        };
        return c.page(q.cursor.as_deref(), limit).map(Json);
    }
    let permit = s
        .agentiflow_usage_pages
        .gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "another usage observation is being captured".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let c = Arc::new(capture(&s, &id, &q.project)?);
        let response = c.page(None, limit)?;
        let mut cache = s
            .agentiflow_usage_pages
            .entries
            .lock()
            .map_err(|_| ApiError::internal("usage cache unavailable"))?;
        cache.retain(|c| c.born.elapsed() < TTL);
        while !cache.is_empty()
            && (cache.len() >= 4
                || cache.iter().map(|c| c.rows.len()).sum::<usize>() + c.rows.len() > MAX_ROWS
                || cache.iter().map(|c| c.bytes).sum::<usize>() + c.bytes > MAX_CACHE)
        {
            cache.remove(0);
        }
        cache.push(c);
        Ok(Json(response))
    })
    .await
    .map_err(|_| ApiError::internal("usage capture worker failed"))?
}
impl Capture {
    fn cursor(&self, offset: usize) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed key");
        mac.update(
            format!(
                "{}:{}:{}:local:{offset}",
                self.token, self.flow, self.project
            )
            .as_bytes(),
        );
        format!(
            "{offset}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }
    fn page(&self, cursor: Option<&str>, limit: usize) -> ApiResult<Value> {
        let offset = if let Some(token) = cursor {
            let (n, signature) = token
                .split_once('.')
                .ok_or_else(|| ApiError::bad_request("invalid usage cursor"))?;
            let offset: usize = n
                .parse()
                .map_err(|_| ApiError::bad_request("invalid usage position"))?;
            let bytes = URL_SAFE_NO_PAD
                .decode(signature)
                .map_err(|_| ApiError::bad_request("invalid usage cursor encoding"))?;
            let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed key");
            mac.update(
                format!(
                    "{}:{}:{}:local:{offset}",
                    self.token, self.flow, self.project
                )
                .as_bytes(),
            );
            mac.verify_slice(&bytes)
                .map_err(|_| ApiError::conflict("usage cursor scope mismatch"))?;
            offset
        } else {
            0
        };
        if offset > self.rows.len() {
            return Err(ApiError::conflict("usage cursor outside observation"));
        }
        let mut end = offset;
        let mut bytes = 8192;
        while end < self.rows.len() && end - offset < limit {
            let size = serde_json::to_vec(&self.rows[end])
                .map_err(|_| ApiError::internal("invalid usage row"))?
                .len();
            if size + bytes > MAX_PAGE {
                break;
            }
            bytes += size + 1;
            end += 1;
        }
        if end == offset && end < self.rows.len() {
            return Err(large("usage row exceeds response budget"));
        }
        let mut out = self.envelope.clone();
        out["offset"] = json!(offset);
        out["total"] = json!(self.rows.len());
        out["records"] = json!(&self.rows[offset..end]);
        out["next_cursor"] = json!((end < self.rows.len()).then(|| self.cursor(end)));
        if serde_json::to_vec(&out)
            .map_err(|_| ApiError::internal("invalid usage page"))?
            .len()
            > MAX_PAGE
        {
            return Err(large("usage page exceeds response budget"));
        }
        Ok(out)
    }
}

/// Reads one finite EOF observation through anchored regular-file descriptors.
/// Append after the observed EOF is excluded; truncation or replacement refuses.
fn read(path: &FsPath, cap: usize, first_line: bool) -> ApiResult<Option<Vec<u8>>> {
    read_checked(path, cap, first_line, || {})
}
fn read_checked(
    path: &FsPath,
    cap: usize,
    first_line: bool,
    after_read: impl FnOnce(),
) -> ApiResult<Option<Vec<u8>>> {
    let file = match crate::api::fs_open::open_anchored_regular_file(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(ApiError::conflict(
                "usage source is not an anchored regular file",
            ))
        }
    };
    let before = file
        .metadata()
        .map_err(|_| ApiError::internal("usage source metadata unavailable"))?;
    if !first_line && before.len() > cap as u64 {
        return Err(large("usage source exceeds capture byte budget"));
    }
    let take = if first_line {
        cap as u64 + 1
    } else {
        before.len()
    };
    let mut reader = BufReader::new(file.take(take));
    let mut bytes = Vec::new();
    if first_line {
        reader.read_until(b'\n', &mut bytes)
    } else {
        reader.read_to_end(&mut bytes)
    }
    .map_err(|_| ApiError::internal("usage source read failed"))?;
    let after = reader
        .into_inner()
        .into_inner()
        .metadata()
        .map_err(|_| ApiError::internal("usage source metadata unavailable"))?;
    use std::os::unix::fs::MetadataExt;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || after.len() < before.len()
        || (!first_line && bytes.len() != before.len() as usize)
    {
        return Err(ApiError::conflict("usage source changed during capture"));
    }
    if bytes.len() > cap {
        return Err(large("usage proof exceeds capture budget"));
    }
    if first_line && !bytes.ends_with(b"\n") {
        return Err(ApiError::not_available("usage project proof is incomplete"));
    }
    after_read();
    // Verify the same anchored pathname still names the captured file, then
    // compare only the bounded captured prefix. Appends beyond that EOF remain
    // outside this observation. This is a race check, not a filesystem lock.
    let verification = crate::api::fs_open::open_anchored_regular_file(path)
        .map_err(|_| ApiError::conflict("usage source pathname changed during capture"))?;
    let verify_meta = verification
        .metadata()
        .map_err(|_| ApiError::internal("usage verification metadata unavailable"))?;
    if (before.dev(), before.ino()) != (verify_meta.dev(), verify_meta.ino())
        || verify_meta.len() < before.len()
    {
        return Err(ApiError::conflict(
            "usage source replaced or truncated during capture",
        ));
    }
    let mut verification = verification.take(bytes.len() as u64);
    let mut offset = 0;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let n = verification
            .read(&mut buffer)
            .map_err(|_| ApiError::internal("usage prefix verification failed"))?;
        if n == 0 {
            break;
        }
        if bytes.get(offset..offset + n) != Some(&buffer[..n]) {
            return Err(ApiError::conflict(
                "usage captured prefix changed during capture",
            ));
        }
        offset += n;
    }
    if offset != bytes.len() {
        return Err(ApiError::conflict("usage captured prefix was truncated"));
    }
    let verified_meta = verification
        .into_inner()
        .metadata()
        .map_err(|_| ApiError::internal("usage verification metadata unavailable"))?;
    if verified_meta.len() < verify_meta.len()
        || (verified_meta.len() == verify_meta.len()
            && (
                verified_meta.mtime(),
                verified_meta.mtime_nsec(),
                verified_meta.ctime(),
                verified_meta.ctime_nsec(),
            ) != (
                verify_meta.mtime(),
                verify_meta.mtime_nsec(),
                verify_meta.ctime(),
                verify_meta.ctime_nsec(),
            ))
    {
        return Err(ApiError::conflict(
            "usage source rewritten while verifying captured prefix",
        ));
    }
    Ok(Some(bytes))
}
#[derive(Default, Serialize)]
struct Evidence {
    usage_ledger_present: bool,
    children_ledger_present: bool,
    usage_bytes: usize,
    children_bytes: usize,
    source_lines: usize,
    malformed_rows: usize,
    unsupported_rows: usize,
    duplicate_rows: usize,
    incomplete_tail_count: usize,
    ambiguous_round_runs: usize,
    source_complete: bool,
    consistency: &'static str,
    navigation: &'static str,
}
fn lines<'a>(bytes: &'a [u8], evidence: &mut Evidence) -> ApiResult<Vec<(usize, &'a [u8])>> {
    let mut rows = Vec::new();
    for (i, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        evidence.source_lines += 1;
        if evidence.source_lines > MAX_ROWS {
            return Err(large("usage capture exceeds source row budget"));
        }
        if line.len() > MAX_LINE {
            return Err(large("usage capture line exceeds 64KiB"));
        }
        if !line.ends_with(b"\n") {
            evidence.incomplete_tail_count += 1;
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        rows.push((i + 1, line));
    }
    Ok(rows)
}
fn lead_round(row: &LedgerRow, flow: &str, root: &FsPath) -> Option<u32> {
    if row.agent_run_id != flow
        || row.parent_agent_run_id.is_some()
        || row.transcript.parent()? != root.join("lead")
    {
        return None;
    }
    row.transcript
        .file_name()?
        .to_str()?
        .strip_prefix("transcript.r")?
        .strip_suffix(".jsonl")?
        .parse()
        .ok()
}
fn capture(s: &AppState, flow: &str, project: &str) -> ApiResult<Capture> {
    let start = Instant::now();
    let global = fs::canonicalize(&s.global_dir)
        .map_err(|_| ApiError::internal("usage owning root unavailable"))?;
    let meta =
        fs::metadata(&global).map_err(|_| ApiError::internal("usage owning root unavailable"))?;
    use std::os::unix::fs::MetadataExt;
    let owner = format!(
        "{:x}",
        Sha256::digest(format!("{}:{}:{}", global.display(), meta.dev(), meta.ino()).as_bytes())
    );
    let root = global.join("agentiflows").join(flow);
    let root_meta =
        fs::metadata(&root).map_err(|_| ApiError::not_found("Agentiflow directory unavailable"))?;
    let record_path = root.join("agentiflow.json");
    let record_bytes = read(&record_path, MAX_PROOF, false)?
        .ok_or_else(|| ApiError::not_found("Agentiflow record unavailable"))?;
    let record: AgentiflowRecord = serde_json::from_slice(&record_bytes)
        .map_err(|_| ApiError::conflict("invalid Agentiflow ownership record"))?;
    let raw: Value = serde_json::from_slice(&record_bytes).expect("typed record parsed");
    if record.id != flow
        || (!raw["host_id"].is_null() && raw["host_id"] != "local")
        || (!raw["workspace_id"].is_null() && raw["workspace_id"] != project)
    {
        return Err(ApiError::conflict("Agentiflow usage owner mismatch"));
    }
    let prologue_path = root.join("lead/transcript.r0.jsonl");
    let prologue = read(&prologue_path, MAX_PROOF, true)?.ok_or_else(|| {
        ApiError::not_available("Agentiflow usage has no round-zero project proof")
    })?;
    let event: rupu_transcript::Event = serde_json::from_slice(&prologue)
        .map_err(|_| ApiError::conflict("invalid usage project prologue"))?;
    let rupu_transcript::Event::RunStart {
        run_id,
        workspace_id,
        ..
    } = event
    else {
        return Err(ApiError::conflict(
            "usage project proof must begin with run_start",
        ));
    };
    let raw: Value = serde_json::from_slice(&prologue).expect("typed prologue parsed");
    if run_id != flow
        || workspace_id != project
        || [&raw["host_id"], &raw["data"]["host_id"]]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some("local"))
        || (!raw["run_id"].is_null() && raw["run_id"] != flow)
        || (!raw["workspace_id"].is_null() && raw["workspace_id"] != project)
    {
        return Err(ApiError::conflict("usage flow/project/host proof mismatch"));
    }
    let usage = read(&root.join("usage.jsonl"), MAX_LEDGER, false)?;
    let children = read(&root.join("children.jsonl"), MAX_CHILDREN, false)?;
    let mut hash = Sha256::new();
    for bytes in [
        &record_bytes[..],
        &prologue[..],
        usage.as_deref().unwrap_or_default(),
        children.as_deref().unwrap_or_default(),
    ] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let mut evidence = Evidence {
        usage_ledger_present: usage.is_some(),
        children_ledger_present: children.is_some(),
        usage_bytes: usage.as_ref().map_or(0, Vec::len),
        children_bytes: children.as_ref().map_or(0, Vec::len),
        consistency: "captured_read_time_observation",
        navigation: "requires_independent_catalog_and_prologue",
        ..Default::default()
    };
    let rows = project_rows(
        flow,
        &root,
        usage.as_deref().unwrap_or_default(),
        children.as_deref().unwrap_or_default(),
        &mut evidence,
        start,
    )?;
    check_time(start)?;
    if read(&record_path, MAX_PROOF, false)?.as_ref() != Some(&record_bytes)
        || read(&prologue_path, MAX_PROOF, true)?.as_ref() != Some(&prologue)
    {
        return Err(ApiError::conflict(
            "usage ownership proof changed during capture",
        ));
    }
    let after_global =
        fs::metadata(&global).map_err(|_| ApiError::conflict("usage owner disappeared"))?;
    let after_root =
        fs::metadata(&root).map_err(|_| ApiError::conflict("usage flow disappeared"))?;
    if (meta.dev(), meta.ino()) != (after_global.dev(), after_global.ino())
        || (root_meta.dev(), root_meta.ino()) != (after_root.dev(), after_root.ino())
    {
        return Err(ApiError::conflict(
            "usage owning directory changed during capture",
        ));
    }
    evidence.source_complete = evidence.usage_ledger_present
        && evidence.malformed_rows == 0
        && evidence.unsupported_rows == 0
        && evidence.incomplete_tail_count == 0;
    let now = Utc::now();
    let token = ulid::Ulid::new().to_string();
    let envelope = json!({"schema_version":1,"flow_id":flow,"project_id":project,"host_id":"local","owner_instance_id":owner,
        "snapshot":token,"source_generation":format!("{:x}",hash.finalize()),"captured_at":now,"expires_at":now+chrono::Duration::minutes(10),"evidence":evidence});
    let bytes = serde_json::to_vec(&rows)
        .map_err(|_| ApiError::internal("invalid usage capture"))?
        .len();
    if bytes > MAX_CAPTURE {
        return Err(large("projected usage exceeds capture byte budget"));
    }
    check_time(start)?;
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(&ulid::Ulid::new().to_bytes());
    key[16..].copy_from_slice(&ulid::Ulid::new().to_bytes());
    Ok(Capture {
        flow: flow.into(),
        project: project.into(),
        token,
        key,
        born: Instant::now(),
        envelope,
        rows,
        bytes,
    })
}
fn project_rows(
    flow: &str,
    root: &FsPath,
    usage: &[u8],
    children: &[u8],
    evidence: &mut Evidence,
    start: Instant,
) -> ApiResult<Vec<Value>> {
    let mut rounds = BTreeMap::<String, Option<u32>>::new();
    for (_, line) in lines(children, evidence)? {
        check_time(start)?;
        match serde_json::from_slice::<rupu_launch::ledger::LedgerLine>(line) {
            Ok(rupu_launch::ledger::LedgerLine::Started(child)) => {
                if child.tree.as_ref().is_some_and(|t| t != root) || !safe_id(&child.run_id) {
                    evidence.malformed_rows += 1;
                    continue;
                }
                if let Some(round) = child.round {
                    rounds
                        .entry(child.run_id)
                        .and_modify(|old| {
                            if *old != Some(round) {
                                *old = None;
                            }
                        })
                        .or_insert(Some(round));
                }
            }
            Ok(_) => (),
            Err(_) => evidence.malformed_rows += 1,
        }
    }
    evidence.ambiguous_round_runs = rounds.values().filter(|r| r.is_none()).count();
    let mut seen = BTreeMap::<String, LedgerRow>::new();
    let mut ordinals = BTreeMap::<PathBuf, u64>::new();
    let mut tagged = Vec::new();
    for (line_number, line) in lines(usage, evidence)? {
        check_time(start)?;
        let row = match serde_json::from_slice::<LedgerRow>(line) {
            Ok(r) => r,
            Err(_) => {
                evidence.malformed_rows += 1;
                continue;
            }
        };
        if row.v != LEDGER_VERSION {
            evidence.unsupported_rows += 1;
            continue;
        }
        if !safe_id(&row.id)
            || !safe_id(&row.agent_run_id)
            || row
                .parent_agent_run_id
                .as_ref()
                .is_some_and(|s| !safe_id(s))
        {
            evidence.malformed_rows += 1;
            continue;
        }
        if let Some(old) = seen.get(&row.id) {
            if old != &row {
                return Err(ApiError::conflict(
                    "usage ledger ID has conflicting records",
                ));
            }
            evidence.duplicate_rows += 1;
            continue;
        }
        seen.insert(row.id.clone(), row.clone());
        let turn = (row.kind == LedgerKind::Turn).then(|| {
            let n = ordinals.entry(row.transcript.clone()).or_default();
            *n += 1;
            *n
        });
        let own_lead = lead_round(&row, flow, root);
        let attributed = own_lead
            .map(|r| (r, "lead_path"))
            .or_else(|| {
                rounds
                    .get(&row.agent_run_id)
                    .copied()
                    .flatten()
                    .map(|r| (r, "unit_catalog"))
            })
            .or_else(|| {
                row.parent_agent_run_id
                    .as_ref()
                    .and_then(|p| rounds.get(p).copied().flatten())
                    .map(|r| (r, "parent_attribution"))
            });
        tagged.push((row, line_number, turn, own_lead, attributed));
    }
    tagged.sort_by_key(|(r, line, _, _, _)| (r.at, *line));
    let mut current = None;
    let mut result = Vec::new();
    let mut bytes = 0;
    for (row, line, turn, lead, attributed) in tagged {
        check_time(start)?;
        if let Some(round) = lead {
            current = Some(round);
        }
        let (round, source) = match attributed.or_else(|| current.map(|r| (r, "temporal_fallback")))
        {
            Some((r, s)) => (Some(r), s),
            None => (None, "unknown"),
        };
        let value = json!({"ledger_id":row.id,"source_line":line,"ordinal":result.len()+1,"kind":row.kind,"at":row.at,
            "agent_run_id":row.agent_run_id,"parent_agent_run_id":row.parent_agent_run_id,"agent":row.agent,"provider":row.provider,"model":row.model,
            "step_id":row.step_id,"unit_index":row.unit_index,"unit_key":row.unit_key,"rollup":row.rollup,
            "input_tokens":row.input_tokens,"output_tokens":row.output_tokens,"cached_tokens":row.cached_tokens,"cache_write_tokens":row.cache_write_tokens,
            "transcript_path":row.transcript,"transcript_turn":turn,"round":round,"round_source":source,"cost_usd":Value::Null});
        bytes += serde_json::to_vec(&value)
            .map_err(|_| ApiError::internal("invalid projected usage"))?
            .len();
        if bytes > MAX_CAPTURE {
            return Err(large("projected usage exceeds capture byte budget"));
        }
        result.push(value);
    }
    Ok(result)
}
#[cfg(test)]
mod tests;
