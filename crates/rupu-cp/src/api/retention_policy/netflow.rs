//! Exact reviewed ledgers for an intentionally narrow, positively inactive owner.
//! No fallback to orphan age or the legacy broad live-ledger scan.
use super::*;
use rupu_runtime::standalone_run_metadata::StandaloneRunMetadata;

pub(super) const QUALIFICATION: &str = "Only exact reviewed per-run network ledgers with archived local standalone ownership, no session attribution, positively verified current local CLI worker provenance when present, a positively dead recorded PID and no active counterpart are eligible. Shared, orphan, unproven/remote worker, session and workflow ledgers are retained. Transcripts and ownership metadata are retained. No execution-time candidate rescan or automatic replay. Advisory lifecycle and continuous per-ledger writer coordination and last-moment file checks are not an atomic filesystem transaction or protection against arbitrary/old writers.";

fn metadata(p: &Policy, root: &Root, id: &str, bytes: &[u8]) -> ApiResult<StandaloneRunMetadata> {
    let m: StandaloneRunMetadata = serde_json::from_slice(bytes)
        .map_err(|_| ApiError::conflict("archived ledger ownership unavailable"))?;
    let cutoff = DateTime::parse_from_rfc3339(&p.transcripts_before).map_err(internal)?;
    let archived = m
        .archived_at
        .as_deref()
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok());
    if m.version != StandaloneRunMetadata::VERSION
        || m.run_id != id
        || m.session_id.is_some()
        || m.backend_id != "local_checkout"
        || !m.workspace_path.is_absolute()
        || (root.id != "global" && m.project_root.as_deref() != root.path.parent())
        || archived.is_none_or(|at| at > cutoff)
        || m.pid.is_none_or(|pid| !plan::dead(pid))
    {
        return Err(ApiError::conflict(
            "ledger lacks exact inactive archived local standalone ownership",
        ));
    }
    // Missing/retired transcript evidence is not upgraded to orphan authority.
    disk::stamp(
        &root
            .path
            .join("transcripts/archive")
            .join(format!("{id}.jsonl")),
        false,
    )?;
    Ok(m)
}

pub(super) fn select(s: &AppState, p: &Policy, scan: &mut plan::Scan) -> ApiResult<()> {
    for root in &p.roots {
        let directory = root.path.join("netflow");
        for filename in scan.names(&directory)? {
            let Some(id) = filename.strip_suffix(".jsonl") else {
                continue;
            };
            if id == "flows" || valid_id(id).is_err() {
                scan.kept("netflow", id, "shared or unsupported ledger identity")?;
                continue;
            }
            let ledger = directory.join(&filename);
            let meta = root
                .path
                .join("transcripts/archive")
                .join(format!("{id}.meta.json"));
            let eligible = (|| -> ApiResult<Group> {
                let _lease = rupu_netflow::ledger::lease::LedgerLease::exclusive(&ledger)
                    .map_err(internal)?;
                // Charge metadata through the scan's shared byte/deadline budget.
                let bytes = scan.bytes(&meta)?;
                let owner = metadata(p, root, id, &bytes)?;
                let worker = worker_proof(s, p, owner.worker_id.as_deref(), |path| {
                    scan.bytes_limited(path, 65536)
                })?;
                let artifact = scan.artifact(p, &ledger, false)?;
                let cutoff =
                    DateTime::parse_from_rfc3339(&p.transcripts_before).map_err(internal)?;
                let modified = DateTime::from_timestamp(
                    artifact.stamp.modified.0,
                    artifact.stamp.modified.1 as u32,
                )
                .ok_or_else(|| ApiError::conflict("invalid ledger modification time"))?;
                if modified > cutoff {
                    return Err(ApiError::conflict(
                        "ledger is newer than the reviewed cutoff",
                    ));
                }
                let mut checks = vec![Check {
                    path: meta,
                    digest: Some(digest(&bytes)),
                }];
                if let Some(check) = worker {
                    checks.push(check);
                }
                for suffix in [format!("{id}.jsonl"), format!("{id}.meta.json")] {
                    let path = root.path.join("transcripts").join(suffix);
                    if disk::present(&path)? {
                        return Err(ApiError::conflict("active transcript counterpart exists"));
                    }
                    checks.push(Check { path, digest: None });
                }
                // A workflow/run-store owner is outside this standalone subset.
                for path in [
                    p.owner.path.join("runs").join(id),
                    p.owner.path.join("runs-archive").join(id),
                    root.path.join("runs").join(id),
                    root.path.join("runs-archive").join(id),
                ] {
                    if disk::present(&path)? {
                        return Err(ApiError::conflict(
                            "run-store owner is outside standalone ledger scope",
                        ));
                    }
                    if !checks.iter().any(|v| v.path == path) {
                        checks.push(Check { path, digest: None });
                    }
                }
                Ok(Group {
                    kind: "netflow".into(),
                    id: id.into(),
                    artifacts: vec![artifact],
                    checks,
                })
            })();
            match eligible {
                Ok(group) => scan.plan.groups.push(group),
                Err(e) if e.0 == axum::http::StatusCode::PAYLOAD_TOO_LARGE => return Err(e),
                Err(_) => scan.kept(
                    "netflow",
                    id,
                    "ownership, age, liveness or exact regular-file evidence unavailable",
                )?,
            }
            scan.check()?;
        }
    }
    Ok(())
}

pub(super) fn verify(s: &AppState, p: &Policy, group: &Group) -> ApiResult<()> {
    if group.kind != "netflow"
        || group.artifacts.len() != 1
        || valid_id(&group.id).is_err()
        || group.id == "flows"
    {
        return Err(ApiError::conflict("invalid reviewed ledger group"));
    }
    let a = &group.artifacts[0];
    let root = p
        .roots
        .get(a.root)
        .ok_or_else(|| ApiError::conflict("invalid ledger root"))?;
    if a.directory || a.relative != PathBuf::from("netflow").join(format!("{}.jsonl", group.id)) {
        return Err(ApiError::conflict(
            "reviewed artifact is not the exact ledger",
        ));
    }
    let bytes = disk::read(
        &root
            .path
            .join("transcripts/archive")
            .join(format!("{}.meta.json", group.id)),
        262144,
    )?;
    let owner = metadata(p, root, &group.id, &bytes)?;
    worker_proof(s, p, owner.worker_id.as_deref(), |path| {
        disk::read(path, 65536)
    })?;
    Ok(())
}

/// Returns the exact worker record check; no workflow envelope is invented for
/// standalone CLI runs. This is local observational provenance, not a unique
/// machine signature. Matching copied host identities cannot be distinguished.
fn worker_proof(
    s: &AppState,
    p: &Policy,
    id: Option<&str>,
    mut read: impl FnMut(&std::path::Path) -> ApiResult<Vec<u8>>,
) -> ApiResult<Option<Check>> {
    let Some(id) = id else {
        return Ok(None);
    };
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ApiError::conflict("worker ownership identity unavailable"));
    }
    if s.node_registry.get(id).is_some()
        || s.hosts.has_persisted_identity(id).map_err(internal)?
        || disk::present(&p.owner.path.join("hosts").join(format!("{id}.toml")))?
    {
        return Err(ApiError::conflict("worker has conflicting host ownership"));
    }
    let hostname = whoami::fallible::hostname().map_err(internal)?;
    if hostname.is_empty() || hostname == "unknown-host" {
        return Err(ApiError::conflict("local worker hostname unavailable"));
    }
    let path = p
        .owner
        .path
        .join("autoflows/workers")
        .join(format!("{id}.toml"));
    let bytes = read(&path)?;
    let record: rupu_runtime::WorkerRecord =
        toml::from_str(std::str::from_utf8(&bytes).map_err(internal)?)
            .map_err(|_| ApiError::conflict("worker ownership record unavailable"))?;
    if record.version != rupu_runtime::WorkerRecord::VERSION
        || record.worker_id != id
        || record.kind != rupu_runtime::WorkerKind::Cli
        || record.host.as_bytes() != hostname.as_bytes()
        || !record
            .capabilities
            .backends
            .iter()
            .any(|v| v == "local_checkout")
    {
        return Err(ApiError::conflict(
            "worker is not a verified current local CLI owner",
        ));
    }
    Ok(Some(Check {
        path,
        digest: Some(digest(&bytes)),
    }))
}

pub(super) fn lease(
    p: &Policy,
    group: &Group,
) -> ApiResult<rupu_netflow::ledger::lease::LedgerLease> {
    let a = group
        .artifacts
        .first()
        .filter(|_| group.artifacts.len() == 1)
        .ok_or_else(|| ApiError::conflict("invalid ledger group"))?;
    let root = p
        .roots
        .get(a.root)
        .ok_or_else(|| ApiError::conflict("invalid ledger root"))?;
    if a.directory
        || a.relative != PathBuf::from("netflow").join(format!("{}.jsonl", group.id))
        || valid_id(&group.id).is_err()
        || group.id == "flows"
    {
        return Err(ApiError::conflict("invalid ledger identity"));
    }
    rupu_netflow::ledger::lease::LedgerLease::exclusive(&root.path.join(&a.relative)).map_err(
        |_| ApiError::conflict("ledger is busy or coordination evidence unavailable; kept"),
    )
}
