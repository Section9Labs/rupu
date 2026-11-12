use super::*;
use rupu_runtime::sessions::{SessionStatus, StoredSession};
use rupu_runtime::standalone_run_metadata::StandaloneRunMetadata;
use std::{collections::BTreeSet, path::Path};

pub(super) fn policy(s: &AppState, body: PreviewBody) -> ApiResult<Policy> {
    let global = s.global_dir.canonicalize().map_err(internal)?;
    let owner = Root {
        id: "global".into(),
        identity: disk::identity(&global)?,
        path: global.clone(),
    };
    let mut roots = Vec::new();
    if body.scopes.global {
        roots.push(owner.clone());
    }
    let project_registry = if let Some(id) = &body.scopes.project_id {
        valid_id(id)?;
        let file = global.join("workspaces").join(format!("{id}.toml"));
        let bytes = disk::read(&file, 65536)?;
        let record: rupu_workspace::Workspace =
            toml::from_str(std::str::from_utf8(&bytes).map_err(internal)?).map_err(internal)?;
        if &record.id != id {
            return Err(ApiError::conflict("registered project identity mismatch"));
        }
        if body.kind == "netflow" && !Path::new(&record.path).is_absolute() {
            return Err(ApiError::conflict(
                "exact ledger project path must be absolute",
            ));
        }
        let path = PathBuf::from(record.path)
            .canonicalize()
            .map_err(internal)?
            .join(".rupu");
        let identity = disk::identity(&path)?;
        if identity == owner.identity || roots.iter().any(|r| r.identity == identity) {
            return Err(ApiError::conflict(
                "project storage aliases global storage; select global scope explicitly",
            ));
        }
        roots.push(Root {
            id: id.clone(),
            path,
            identity,
        });
        Some((file, digest(&bytes)))
    } else {
        None
    };
    let config = global.join("config.toml");
    let cfg: toml::Value = if disk::present(&config)? {
        toml::from_str(std::str::from_utf8(&disk::read(&config, 262144)?).map_err(internal)?)
            .map_err(internal)?
    } else {
        toml::Value::Table(Default::default())
    };
    let now = Utc::now();
    let cutoff = |key: &str| -> ApiResult<String> {
        let configured = if body.kind == "netflow" {
            None
        } else {
            cfg.get("storage").and_then(|v| v.get(key))
        };
        let age = body
            .older_than
            .as_deref()
            .or_else(|| configured.and_then(toml::Value::as_str))
            .unwrap_or("30d");
        if body.older_than.is_none() && configured.is_some_and(|v| !v.is_str()) {
            return Err(internal(
                "retention configuration must be a duration string",
            ));
        }
        let duration = rupu_runtime::retention::parse_retention_duration(age)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        if duration < chrono::Duration::zero()
            || (body.kind == "netflow" && duration == chrono::Duration::zero())
        {
            return Err(ApiError::bad_request("retention age must not be negative"));
        }
        Ok(now
            .checked_sub_signed(duration)
            .ok_or_else(|| ApiError::bad_request("retention cutoff out of range"))?
            .to_rfc3339())
    };
    let p=Policy{owner,roots,kind:body.kind.clone(),scopes:body.scopes.clone(),project_registry,sessions_before:cutoff("archived_session_retention")?,transcripts_before:cutoff("archived_transcript_retention")?,observed_at:now,expires_at:now+chrono::Duration::minutes(10),authorization_mode:if body.kind == "netflow" { "exact_preview_set" } else { "policy_at_execution" }.into(),auxiliary_artifacts:"stream-only run directories and active-location counterparts are retained; only declared archived artifacts are eligible".into()};
    let mut p = p;
    if p.kind == "netflow" {
        p.auxiliary_artifacts = super::netflow::QUALIFICATION.into();
    }
    verify_policy(&p)?;
    Ok(p)
}
pub(super) fn verify_policy(p: &Policy) -> ApiResult<()> {
    for r in std::iter::once(&p.owner).chain(p.roots.iter()) {
        if disk::identity(&r.path)? != r.identity {
            return Err(ApiError::conflict("retention root identity changed"));
        }
    }
    if let Some((path, expected)) = &p.project_registry {
        let bytes = disk::read(path, 65536)?;
        if digest(&bytes) != *expected {
            return Err(ApiError::conflict("retention project registration changed"));
        }
        if p.kind == "netflow" {
            // Registry text can stay unchanged while its recorded alias moves.
            // Re-resolve that original path, never only the old canonical target.
            let record: rupu_workspace::Workspace =
                toml::from_str(std::str::from_utf8(&bytes).map_err(internal)?).map_err(internal)?;
            let captured = p
                .roots
                .iter()
                .find(|root| root.id == record.id)
                .ok_or_else(|| ApiError::conflict("reviewed ledger project scope changed"))?;
            if p.scopes.project_id.as_deref() != Some(record.id.as_str())
                || !Path::new(&record.path).is_absolute()
                || Path::new(&record.path)
                    .canonicalize()
                    .map_err(internal)?
                    .join(".rupu")
                    != captured.path
            {
                return Err(ApiError::conflict(
                    "reviewed ledger registered project path changed",
                ));
            }
        }
    }
    Ok(())
}
pub(super) struct Scan {
    pub(super) plan: Plan,
    started: Instant,
    artifacts_seen: usize,
}
impl Scan {
    pub(super) fn check(&self) -> ApiResult<()> {
        if self.plan.entries_scanned > 50_000
            || self.artifacts_seen > MAX_ARTIFACTS
            || self.plan.metadata_bytes > 32 * 1024 * 1024
            || self.plan.groups.len() + self.plan.kept.len() > MAX_GROUPS
            || self
                .plan
                .groups
                .iter()
                .map(|g| g.artifacts.len())
                .sum::<usize>()
                > MAX_ARTIFACTS
            || self.started.elapsed().as_secs() > 5
        {
            return Err(large("retention selection incomplete: bounded scan limit exceeded; no execution selection accepted"));
        }
        Ok(())
    }
    pub(super) fn bytes(&mut self, p: &Path) -> ApiResult<Vec<u8>> {
        self.bytes_limited(p, 262144)
    }
    pub(super) fn bytes_limited(&mut self, p: &Path, cap: usize) -> ApiResult<Vec<u8>> {
        self.check()?;
        let b = disk::read(
            p,
            cap.min((32 * 1024 * 1024usize).saturating_sub(self.plan.metadata_bytes)),
        )?;
        self.plan.metadata_bytes += b.len();
        self.check()?;
        Ok(b)
    }
    pub(super) fn names(&mut self, p: &Path) -> ApiResult<Vec<String>> {
        if !disk::present(p)? {
            return Ok(Vec::new());
        }
        let before = disk::identity(p)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(p).map_err(internal)? {
            self.plan.entries_scanned += 1;
            self.check()?;
            let name = entry
                .map_err(internal)?
                .file_name()
                .into_string()
                .map_err(|_| ApiError::conflict("non-UTF8 retention entry"))?;
            if name.len() > 255 {
                return Err(large("retention filename exceeds limit"));
            }
            names.push(name);
        }
        if disk::identity(p)? != before {
            return Err(ApiError::conflict("retention directory changed"));
        }
        names.sort();
        Ok(names)
    }
    pub(super) fn kept(&mut self, kind: &str, id: &str, reason: &str) -> ApiResult<()> {
        self.plan.kept.push(Kept {
            kind: kind.into(),
            id: id.into(),
            reason: reason.into(),
        });
        self.check()
    }
    pub(super) fn artifact(&mut self, p: &Policy, path: &Path, dir: bool) -> ApiResult<Artifact> {
        self.artifacts_seen += 1;
        self.check()?;
        let (root, relative) = p
            .roots
            .iter()
            .enumerate()
            .find_map(|(i, r)| {
                path.strip_prefix(&r.path)
                    .ok()
                    .map(|x| (i, x.to_path_buf()))
            })
            .ok_or_else(|| ApiError::conflict("artifact is outside approved storage roots"))?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(ApiError::conflict("invalid retention artifact path"));
        }
        Ok(Artifact {
            root,
            relative,
            stamp: disk::stamp(path, dir)?,
            directory: dir,
        })
    }
    fn tree(
        &mut self,
        p: &Policy,
        path: &Path,
        depth: usize,
        out: &mut Vec<Artifact>,
    ) -> ApiResult<()> {
        if depth > 16 || out.len() > MAX_ARTIFACTS {
            return Err(large("archived session tree exceeds bounds"));
        }
        let stamp = disk::stamp(path, true)?;
        for name in self.names(path)? {
            let entry = path.join(name);
            let m = fs::symlink_metadata(&entry).map_err(internal)?;
            if m.is_dir() {
                self.tree(p, &entry, depth + 1, out)?;
            } else if m.is_file() {
                out.push(self.artifact(p, &entry, false)?);
            } else {
                return Err(ApiError::conflict(
                    "session tree contains unsupported file types",
                ));
            }
        }
        if disk::stamp(path, true)? != stamp {
            return Err(ApiError::conflict("session tree changed during capture"));
        }
        out.push(self.artifact(p, path, true)?);
        Ok(())
    }
}
fn absent(path: PathBuf, checks: &mut Vec<Check>) -> ApiResult<bool> {
    if disk::present(&path)? {
        return Ok(false);
    }
    checks.push(Check { path, digest: None });
    Ok(true)
}
pub(super) fn dead(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

pub(super) fn select(s: &AppState, p: &Policy) -> ApiResult<Plan> {
    verify_policy(p)?;
    let mut scan = Scan {
        plan: Plan::default(),
        started: Instant::now(),
        artifacts_seen: 0,
    };
    if p.kind == "netflow" {
        super::netflow::select(s, p, &mut scan)?;
    } else if p.kind != "transcripts" {
        sessions(p, &mut scan)?;
    }
    if p.kind != "sessions" && p.kind != "netflow" {
        transcripts(p, &mut scan)?;
    }
    scan.check()?;
    verify_policy(p)?;
    let mut paths = BTreeSet::new();
    for g in &scan.plan.groups {
        for a in &g.artifacts {
            if !paths.insert((a.root, a.relative.clone())) {
                return Err(ApiError::conflict(
                    "retention artifacts have ambiguous owners",
                ));
            }
        }
    }
    if serde_json::to_vec(&scan.plan).map_err(internal)?.len() > MAX_BYTES / 2 {
        return Err(large("retention execution manifest exceeds 4 MiB"));
    }
    Ok(scan.plan)
}
fn sessions(p: &Policy, s: &mut Scan) -> ApiResult<()> {
    let archive = p.owner.path.join("sessions-archive");
    let cutoff = DateTime::parse_from_rfc3339(&p.sessions_before)
        .map_err(internal)?
        .with_timezone(&Utc);
    for id in s.names(&archive)? {
        if valid_id(&id).is_err() {
            s.kept("session", &id, "invalid identity")?;
            continue;
        }
        let dir = archive.join(&id);
        let meta = dir.join("session.json");
        if !disk::present(&meta)? {
            s.kept("session", &id, "canonical metadata unavailable")?;
            continue;
        }
        let bytes = s.bytes(&meta)?;
        let stored = rupu_runtime::sessions::parse_stored_session(&bytes);
        let record = match stored {
            Ok(StoredSession::Current(r)) => r,
            _ => {
                s.kept("session", &id, "unknown, newer or malformed session schema")?;
                continue;
            }
        };
        if record.session_id != id {
            s.kept("session", &id, "record identity mismatch")?;
            continue;
        }
        if record.updated_at > cutoff {
            continue;
        }
        if record.status == SessionStatus::Running
            || record.active_pid.is_some_and(|p| !dead(p))
            || record.worker_pid.is_some_and(|p| !dead(p))
            || record.runs.iter().any(|r| {
                r.completed_at.is_none() || r.status.is_none() || r.pid.is_some_and(|p| !dead(p))
            })
        {
            s.kept("session", &id, "live or unconfirmed run ownership")?;
            continue;
        }
        let mut checks = vec![Check {
            path: meta.clone(),
            digest: Some(digest(&bytes)),
        }];
        if !absent(p.owner.path.join("sessions").join(&id), &mut checks)? {
            s.kept("session", &id, "active session counterpart exists")?;
            continue;
        }
        let transcript_root = record.transcripts_dir.clone();
        if !p
            .roots
            .iter()
            .any(|r| transcript_root == r.path.join("transcripts"))
        {
            s.kept(
                "session",
                &id,
                "owned transcript root outside approved policy",
            )?;
            continue;
        }
        let mut artifacts = Vec::new();
        let mut eligible = true;
        let mut ids = BTreeSet::new();
        for run in &record.runs {
            if valid_id(&run.run_id).is_err() || !ids.insert(run.run_id.clone()) {
                eligible = false;
                break;
            }
            let expected = transcript_root
                .join("archive")
                .join(format!("{}.jsonl", run.run_id));
            if run.transcript_path != expected {
                eligible = false;
                break;
            }
            let sidecar = transcript_root
                .join("archive")
                .join(format!("{}.meta.json", run.run_id));
            let sidecar_present = disk::present(&sidecar)?;
            if disk::present(&expected)? && !sidecar_present {
                eligible = false;
                break;
            }
            if sidecar_present {
                let sidecar_bytes = s.bytes(&sidecar)?;
                let owned = serde_json::from_slice::<StandaloneRunMetadata>(&sidecar_bytes).ok();
                if owned.as_ref().is_none_or(|m| {
                    m.version != StandaloneRunMetadata::VERSION
                        || m.run_id != run.run_id
                        || m.session_id.as_deref() != Some(id.as_str())
                        || m.worker_id.is_some()
                        || m.pid.is_none_or(|pid| !dead(pid))
                }) {
                    eligible = false;
                    break;
                }
                checks.push(Check {
                    path: sidecar.clone(),
                    digest: Some(digest(&sidecar_bytes)),
                });
            }
            for suffix in ["jsonl", "meta.json"] {
                if !absent(
                    transcript_root.join(format!("{}.{}", run.run_id, suffix)),
                    &mut checks,
                )? {
                    eligible = false;
                    break;
                }
                let path = transcript_root
                    .join("archive")
                    .join(format!("{}.{}", run.run_id, suffix));
                if disk::present(&path)? {
                    artifacts.push(s.artifact(p, &path, false)?);
                }
            }
            if !eligible {
                break;
            }
        }
        if !eligible {
            s.kept(
                "session",
                &id,
                "active, duplicate or foreign owned transcript locator",
            )?;
            continue;
        }
        s.tree(p, &dir, 0, &mut artifacts)?;
        // Ownership metadata is removed last among files; directories only after their children.
        artifacts.sort_by_key(|a| {
            (
                a.directory,
                a.relative == format!("sessions-archive/{id}/session.json"),
                std::cmp::Reverse(a.relative.components().count()),
            )
        });
        s.plan.groups.push(Group {
            kind: "session".into(),
            id,
            artifacts,
            checks,
        });
        s.check()?;
    }
    Ok(())
}
fn transcripts(p: &Policy, s: &mut Scan) -> ApiResult<()> {
    let cutoff = DateTime::parse_from_rfc3339(&p.transcripts_before)
        .map_err(internal)?
        .with_timezone(&Utc);
    for root in &p.roots {
        let archive = root.path.join("transcripts/archive");
        for filename in s.names(&archive)? {
            let Some(id) = filename.strip_suffix(".jsonl") else {
                continue;
            };
            if valid_id(id).is_err() {
                s.kept("transcript", id, "invalid identity")?;
                continue;
            }
            let path = archive.join(&filename);
            let meta = archive.join(format!("{id}.meta.json"));
            if !disk::present(&meta)? {
                s.kept(
                    "transcript",
                    id,
                    "standalone ownership metadata unavailable",
                )?;
                continue;
            }
            let bytes = s.bytes(&meta)?;
            let record: StandaloneRunMetadata = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(_) => {
                    s.kept("transcript", id, "malformed standalone ownership")?;
                    continue;
                }
            };
            if record.session_id.is_some() {
                continue;
            }
            if record.version != StandaloneRunMetadata::VERSION
                || record.run_id != id
                || record.worker_id.is_some()
            {
                s.kept(
                    "transcript",
                    id,
                    "unsupported or foreign standalone ownership",
                )?;
                continue;
            }
            if root.id != "global" && record.project_root.as_deref() != root.path.parent() {
                s.kept(
                    "transcript",
                    id,
                    "recorded project does not own selected storage",
                )?;
                continue;
            }
            let Some(at) = record
                .archived_at
                .as_deref()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            else {
                s.kept("transcript", id, "archival time is not positively recorded")?;
                continue;
            };
            if at.with_timezone(&Utc) > cutoff {
                continue;
            }
            if record.pid.is_none_or(|pid| !dead(pid)) {
                s.kept("transcript", id, "live or unknown process ownership")?;
                continue;
            }
            let mut checks = vec![Check {
                path: meta.clone(),
                digest: Some(digest(&bytes)),
            }];
            if !absent(root.path.join("transcripts").join(&filename), &mut checks)?
                || !absent(
                    root.path
                        .join("transcripts")
                        .join(format!("{id}.meta.json")),
                    &mut checks,
                )?
            {
                s.kept("transcript", id, "active transcript counterpart exists")?;
                continue;
            }
            let artifacts = vec![s.artifact(p, &path, false)?, s.artifact(p, &meta, false)?];
            s.plan.groups.push(Group {
                kind: "transcript".into(),
                id: id.into(),
                artifacts,
                checks,
            });
            s.check()?;
        }
    }
    Ok(())
}
pub(super) fn verify_group(s: &AppState, p: &Policy, g: &Group) -> ApiResult<()> {
    verify_policy(p)?;
    if p.kind == "netflow" {
        super::netflow::verify(s, p, g)?;
    }
    for c in &g.checks {
        match &c.digest {
            Some(expected) => {
                if digest(&disk::read(&c.path, 262144)?) != *expected {
                    return Err(ApiError::conflict("retention ownership changed"));
                }
            }
            None => {
                if disk::present(&c.path)? {
                    return Err(ApiError::conflict("retention active counterpart appeared"));
                }
            }
        }
    }
    for a in &g.artifacts {
        if disk::stamp(&p.roots[a.root].path.join(&a.relative), a.directory)? != a.stamp {
            return Err(ApiError::conflict("retention artifact changed"));
        }
    }
    Ok(())
}
