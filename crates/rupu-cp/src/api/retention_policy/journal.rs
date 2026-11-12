use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rustix::fs::{Mode, OFlags};
use std::{io::Write, path::Path};

pub(super) trait Fault {
    fn check(&mut self, point: &str) -> ApiResult<()>;
}
pub(super) struct NoFault;
impl Fault for NoFault {
    fn check(&mut self, _: &str) -> ApiResult<()> {
        Ok(())
    }
}
fn store(global: &Path) -> PathBuf {
    global.join("maintenance/retention")
}
fn directory(p: &Prepared) -> PathBuf {
    store(&p.policy.owner.path).join(&p.operation_id)
}
pub(super) fn create(p: &Prepared) -> ApiResult<()> {
    disk::create_dir(&p.policy.owner.path, "maintenance")?;
    disk::create_dir(&p.policy.owner.path.join("maintenance"), "retention")?;
    let root = store(&p.policy.owner.path);
    let mut entries = 0;
    for entry in fs::read_dir(&root).map_err(internal)? {
        entry.map_err(internal)?;
        entries += 1;
        if entries >= 256 {
            return Err(large(
                "retention journal quota reached; existing receipts are retained",
            ));
        }
    }
    let dir = directory(p);
    if disk::present(&dir)? {
        return Err(ApiError::conflict("retention operation ID collision"));
    }
    disk::create_dir(&root, &p.operation_id)?;
    disk::atomic(&dir.join("events.jsonl"), b"")?;
    disk::atomic(
        &dir.join("prepared.json"),
        &serde_json::to_vec(p).map_err(internal)?,
    )
}
pub(super) fn load(global: &Path, id: &str) -> ApiResult<Prepared> {
    valid_id(id)?;
    let global = global.canonicalize().map_err(internal)?;
    let path = store(&global).join(id).join("prepared.json");
    if !disk::present(&path)? {
        return Err(ApiError::not_found(
            "retention receipt not found; this does not establish a prior remote attempt's outcome",
        ));
    }
    let p: Prepared = serde_json::from_slice(&disk::read(&path, MAX_BYTES)?).map_err(internal)?;
    if p.schema_version != 1
        || p.operation_id != id
        || p.preview_id != id
        || p.policy.owner.path != global
        || p.policy.owner.identity != disk::identity(&global)?
        || p.policy_digest != digest(&serde_json::to_vec(&p.policy).map_err(internal)?)
        || (p.policy.kind == "netflow"
            && (p.policy.authorization_mode != "exact_preview_set"
                || p.preview_set_digest.as_deref()
                    != Some(digest(&serde_json::to_vec(&p.preview).map_err(internal)?).as_str())))
        || (p.policy.kind != "netflow" && p.preview_set_digest.is_some())
    {
        return Err(ApiError::conflict(
            "retention receipt owner or policy mismatch",
        ));
    }
    Ok(p)
}
pub(super) fn save_plan(p: &Prepared, plan: &Plan) -> ApiResult<()> {
    disk::atomic(
        &directory(p).join("execution.json"),
        &serde_json::to_vec(plan).map_err(internal)?,
    )
}
fn captured_events(p: &Prepared) -> ApiResult<(Vec<Value>, bool)> {
    let bytes = disk::read(&directory(p).join("events.jsonl"), MAX_BYTES)?;
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut rows = Vec::new();
    let mut damaged = complete != bytes.len();
    for line in bytes[..complete]
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
    {
        match serde_json::from_slice::<Value>(line) {
            Ok(row) if row["sequence"].as_u64() == Some(rows.len() as u64 + 1) => rows.push(row),
            _ => {
                damaged = true;
                break;
            }
        }
    }
    Ok((rows, damaged))
}
pub(super) fn events(p: &Prepared) -> ApiResult<Vec<Value>> {
    let (rows, damaged) = captured_events(p)?;
    if damaged {
        return Err(ApiError::conflict("retention journal has an uncommitted or invalid tail; inspect the operation, never replay it"));
    }
    Ok(rows)
}
pub(super) struct Writer<'a, F: Fault> {
    p: &'a Prepared,
    fault: &'a mut F,
    file: fs::File,
    sequence: usize,
}
impl<'a, F: Fault> Writer<'a, F> {
    pub fn new(p: &'a Prepared, fault: &'a mut F) -> ApiResult<Self> {
        let sequence = events(p)?.len();
        let parent = disk::directory(&directory(p))?;
        let file = fs::File::from(
            rustix::fs::openat(
                &parent,
                "events.jsonl",
                OFlags::WRONLY
                    | OFlags::APPEND
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(internal)?,
        );
        if !file.metadata().map_err(internal)?.is_file() {
            return Err(internal("retention journal is not regular"));
        }
        Ok(Self {
            p,
            fault,
            file,
            sequence,
        })
    }
    pub fn append(&mut self, mut row: Value) -> ApiResult<()> {
        self.fault.check("before_journal")?;
        row["sequence"] = json!(self.sequence + 1);
        row["recorded_at"] = json!(Utc::now());
        let mut bytes = serde_json::to_vec(&row).map_err(internal)?;
        bytes.push(b'\n');
        let metadata = self.file.metadata().map_err(internal)?;
        if metadata.len().saturating_add(bytes.len() as u64) > MAX_BYTES as u64 {
            return Err(large("retention event journal quota exceeded"));
        }
        use std::os::unix::fs::MetadataExt;
        let named = disk::stamp(&directory(self.p).join("events.jsonl"), false)?;
        if named.identity != disk::Identity::of(&metadata) || metadata.nlink() != 1 {
            return Err(ApiError::conflict("retention journal identity changed"));
        }
        self.file.write_all(&bytes).map_err(internal)?;
        self.file.sync_all().map_err(internal)?;
        self.sequence += 1;
        Ok(())
    }
}
pub(super) fn apply<F: Fault>(
    s: &AppState,
    p: &Prepared,
    selected: &Plan,
    writer: &mut Writer<'_, F>,
) -> ApiResult<()> {
    let started = Instant::now();
    let mut partial = false;
    for (group_index, g) in selected.groups.iter().enumerate() {
        if started.elapsed().as_secs() > 30 {
            writer.append(json!({"phase":"partial","reason":"execution deadline; remaining actions not attempted"}))?;
            return Ok(());
        }
        // Keep the exclusive descriptor-backed guard through durable outcome.
        let lease = if p.policy.kind == "netflow" {
            match super::netflow::lease(&p.policy, g) {
                Ok(held) => Some(held),
                Err(e) => {
                    partial = true;
                    writer.append(
                        json!({"group":group_index,"outcome":"kept_changed","reason":e.1}),
                    )?;
                    continue;
                }
            }
        } else {
            None
        };
        if let Err(e) = plan::verify_group(s, &p.policy, g) {
            partial = true;
            writer.append(json!({"group":group_index,"outcome":"kept_changed","reason":e.1}))?;
            continue;
        }
        for (artifact_index, a) in g.artifacts.iter().enumerate() {
            if started.elapsed().as_secs() > 30 {
                writer.append(json!({"phase":"partial","reason":"execution deadline; remaining actions not attempted"}))?;
                return Ok(());
            }
            plan::verify_policy(&p.policy)?;
            writer.append(
                json!({"group":group_index,"artifact":artifact_index,"outcome":"dispatching"}),
            )?;
            let result = if p.policy.kind == "netflow" {
                disk::remove_exact_file(
                    &p.policy.roots[a.root],
                    a,
                    lease
                        .as_ref()
                        .ok_or_else(|| internal("missing ledger lease"))?,
                )
            } else {
                disk::remove(&p.policy.roots[a.root], a)
            };
            writer.fault.check("after_unlink")?;
            match result {
                Ok(removed)=>writer.append(json!({"group":group_index,"artifact":artifact_index,"outcome":if removed{"unlinked_confirmed"}else{"already_absent_observed"},"logical_bytes_unlinked":if removed && !a.directory {a.stamp.bytes}else{0}}))?,
                Err(e)=>{
                    // An error can follow unlink (e.g. parent fsync failed).
                    // Never classify the whole action as no-effect from HTTP status.
                    writer.append(json!({"group":group_index,"artifact":artifact_index,"outcome":"effect_unknown","reason":e.1}))?;
                    writer.append(json!({"phase":"interrupted_unknown","reason":"artifact result could not be durably confirmed; no remaining action was attempted"}))?;
                    return Ok(());
                }
            }
        }
    }
    writer.append(json!({"phase":if partial{"partial"}else{"complete"}}))
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    operation: String,
    section: String,
    sequence: usize,
    position: usize,
    preview: String,
}
fn mac() -> Hmac<Sha256> {
    static KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut b = [0; 32];
        b[..16].copy_from_slice(&ulid::Ulid::new().to_bytes());
        b[16..].copy_from_slice(&ulid::Ulid::new().to_bytes());
        b
    });
    Hmac::<Sha256>::new_from_slice(key).expect("HMAC key")
}
fn encode(c: &Cursor) -> ApiResult<String> {
    let b = serde_json::to_vec(c).map_err(internal)?;
    let mut m = mac();
    m.update(&b);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&b),
        URL_SAFE_NO_PAD.encode(m.finalize().into_bytes())
    ))
}
fn decode(token: &str) -> ApiResult<Cursor> {
    if token.len() > 8192 {
        return Err(ApiError::bad_request("retention cursor too large"));
    }
    let (b, s) = token
        .split_once('.')
        .ok_or_else(|| ApiError::bad_request("invalid retention cursor"))?;
    let b = URL_SAFE_NO_PAD
        .decode(b)
        .map_err(|_| ApiError::bad_request("invalid cursor encoding"))?;
    let s = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| ApiError::bad_request("invalid cursor signature"))?;
    let mut m = mac();
    m.update(&b);
    m.verify_slice(&s).map_err(|_| {
        ApiError::bad_request("retention cursor scope mismatch or server restarted")
    })?;
    serde_json::from_slice(&b).map_err(|_| ApiError::bad_request("invalid retention cursor"))
}

pub(super) fn answer(p: &Prepared, q: ReadQuery, worker_stopped: bool) -> ApiResult<Value> {
    let section = q.section.as_deref().unwrap_or("summary");
    if !["summary", "preview", "effects"].contains(&section) {
        return Err(ApiError::bad_request("invalid retention receipt section"));
    }
    let limit = q.limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("retention page limit must be 1..100"));
    }
    let (all, damaged_journal) = captured_events(p)?;
    let cursor = q.cursor.as_deref().map(decode).transpose()?;
    let (sequence, position) = if let Some(c) = cursor {
        if c.operation != p.operation_id
            || c.section != section
            || c.preview != p.policy_digest
            || c.sequence > all.len()
        {
            return Err(ApiError::bad_request(
                "retention cursor belongs to another operation or section",
            ));
        }
        (c.sequence, c.position)
    } else {
        (all.len(), 0)
    };
    let rows = &all[..sequence];
    let phase = rows
        .iter()
        .rev()
        .find_map(|v| v["phase"].as_str())
        .unwrap_or("prepared");
    let terminal = ["complete", "partial", "refused", "interrupted_unknown"].contains(&phase);
    let state = if damaged_journal || (!rows.is_empty() && !terminal && worker_stopped) {
        "interrupted_unknown"
    } else {
        phase
    };
    let execution_path = directory(p).join("execution.json");
    let execution: Option<Plan> = if rows.iter().any(|v| v["phase"] == "executing")
        && disk::present(&execution_path)?
    {
        Some(serde_json::from_slice(&disk::read(&execution_path, MAX_BYTES)?).map_err(internal)?)
    } else {
        None
    };
    let mut group_events = std::collections::BTreeMap::new();
    let mut artifact_events = std::collections::BTreeMap::new();
    for row in rows {
        if let Some(group) = row["group"].as_u64() {
            if let Some(artifact) = row["artifact"].as_u64() {
                artifact_events.insert((group, artifact), row);
            } else {
                group_events.insert(group, row);
            }
        }
    }
    let mut effects = Vec::new();
    if section == "effects" {
        if let Some(plan) = &execution {
            for (gi, g) in plan.groups.iter().enumerate() {
                for (ai, a) in g.artifacts.iter().enumerate() {
                    let event = artifact_events
                        .get(&(gi as u64, ai as u64))
                        .or_else(|| group_events.get(&(gi as u64)))
                        .copied();
                    let outcome = event
                        .and_then(|v| v["outcome"].as_str())
                        .unwrap_or("not_attempted");
                    effects.push(json!({"kind":g.kind,"resource_id":g.id,"root_id":p.policy.roots[a.root].id,"relative_path":a.relative,"directory":a.directory,"outcome":if outcome=="dispatching"{"effect_unknown"}else{outcome},"reason":event.and_then(|v|v.get("reason")),"logical_bytes_unlinked":event.and_then(|v|v.get("logical_bytes_unlinked")).cloned().unwrap_or(json!(0))}));
                }
            }
        }
    }
    let list=match section {
        "preview"=>p.preview.groups.iter().flat_map(|g|g.artifacts.iter().map(move |a|json!({"kind":g.kind,"resource_id":g.id,"root_id":p.policy.roots[a.root].id,"relative_path":a.relative,"directory":a.directory,"logical_bytes":a.stamp.bytes}))).collect::<Vec<_>>(),
        "effects"=>effects,_=>Vec::new()
    };
    if position > list.len() {
        return Err(ApiError::bad_request("retention cursor position invalid"));
    }
    let mut end = position;
    let mut bytes = 0;
    while end < list.len() && end - position < limit {
        let size = serde_json::to_vec(&list[end]).map_err(internal)?.len();
        if bytes + size > 512 * 1024 {
            break;
        }
        bytes += size;
        end += 1;
    }
    if end == position && end < list.len() {
        return Err(large("one retention row exceeds page bounds"));
    }
    let next = if end < list.len() {
        Some(encode(&Cursor {
            operation: p.operation_id.clone(),
            section: section.into(),
            sequence,
            position: end,
            preview: p.policy_digest.clone(),
        })?)
    } else {
        None
    };
    let confirmed = rows
        .iter()
        .filter(|v| v["outcome"] == "unlinked_confirmed")
        .count();
    let response = json!({"schema_version":1,"owner_host_id":"local","owner_instance_id":digest(&serde_json::to_vec(&p.policy.owner).map_err(internal)?),"operation_id":p.operation_id,"preview_id":p.preview_id,"policy_digest":p.policy_digest,"policy":p.policy,"authorization_mode":p.policy.authorization_mode,"preview_set_digest":p.preview_set_digest,"preview_selection":if p.policy.kind == "netflow" {"exact_reviewed_candidate_set"} else {"observed_not_exact_candidate_authorization"},"state":state,"recorded_phase":phase,"journal_complete":!damaged_journal,"latest_recorded_event":rows.last(),"sequence":sequence,"current_sequence":all.len(),"section":section,"rows":list[position..end],"next_cursor":next,"preview_group_count":p.preview.groups.len(),"preview_kept":p.preview.kept,"execution_group_count":execution.as_ref().map(|v|v.groups.len()),"execution_kept":execution.as_ref().map(|v|&v.kept),"confirmed_unlinked_artifacts":confirmed,"executable":all.is_empty()&&!damaged_journal&&Utc::now()<=p.policy.expires_at,"remaining_actions":"not_attempted unless a recorded dispatch/outcome says otherwise","bounds":{"groups":MAX_GROUPS,"artifacts":MAX_ARTIFACTS,"complete_bounded_selection":true}});
    if serde_json::to_vec(&response).map_err(internal)?.len() > 1024 * 1024 {
        return Err(large("retention receipt exceeds response bound"));
    }
    Ok(response)
}
