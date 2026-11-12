use super::{discovery, model::*};
use crate::error::{ApiError, ApiResult};
use rupu_coverage::{
    tools::{capture_import_ledger, prepare_import_report},
    FindingWriteOptions,
};
use rupu_findings_report::import::{parse_report, retain_known_cross_references, Parsed};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::Path,
};

pub(super) fn digest(value: &impl Serialize) -> ApiResult<String> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| ApiError::internal("cannot encode import review"))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
pub(super) fn io_error(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found("import review or operation not found"),
        std::io::ErrorKind::WouldBlock => {
            ApiError::conflict("another import review operation holds the store lease")
        }
        std::io::ErrorKind::AlreadyExists => {
            ApiError::conflict("import review operation already exists")
        }
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            ApiError::bad_request(error.to_string())
        }
        _ => ApiError::internal(
            "import review storage is unavailable; do not retry a submitted import",
        ),
    }
}
pub(super) fn owner(global: &Path) -> ApiResult<String> {
    let path = global.canonicalize().map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&path).map_err(io_error)?;
        digest(&(path, metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(ApiError::not_available(
            "reviewed report import requires Unix file identity",
        ))
    }
}
pub(super) fn options(global: &Path) -> ApiResult<(FindingWriteOptions, String)> {
    let (mut opts, warning) = rupu_runtime::findings_opts::global_options(global);
    if warning.is_some() {
        return Err(ApiError::conflict(
            "findings policy is unreadable; no import review prepared",
        ));
    }
    // These are additional operator-review bounds, never raised policy limits.
    opts.artifact_max_bytes = opts.artifact_max_bytes.min(MAX_DEPENDENCY_BYTES);
    opts.artifact_total_max_bytes = opts.artifact_total_max_bytes.min(MAX_DEPENDENCY_BYTES);
    opts.artifact_max_files = opts.artifact_max_files.min(100);
    opts.report_max_bytes = opts.report_max_bytes.min(4 * 1024 * 1024);
    let policy = digest(&serde_json::json!({
        "version": VERSION, "parser": "finding-report-v1", "profile": opts.profile,
        "artifact_root": opts.artifact_root, "artifact_max_bytes": opts.artifact_max_bytes,
        "artifact_total_max_bytes": opts.artifact_total_max_bytes,
        "artifact_max_files": opts.artifact_max_files, "report_max_bytes": opts.report_max_bytes,
        "ticket_patterns": opts.ticket_patterns,
    }))?;
    Ok((opts, policy))
}

pub(super) fn prepare(
    global: &Path,
    request: PreviewRequest,
    review_id: String,
    now: i64,
    private_artifacts: &Path,
    files: &[discovery::CapturedFile],
) -> ApiResult<Manifest> {
    let _ = request; // Selection was resolved once by bounded discovery before staging.
    let ledgers =
        discovery::discover_ledgers(global).map_err(|e| ApiError::conflict(e.to_string()))?;
    let (opts, policy_digest) = options(global)?;
    let parsed: Vec<_> = files
        .iter()
        .map(|file| {
            std::str::from_utf8(&file.bytes)
                .map_err(|_| "report is not UTF-8".to_string())
                .and_then(|text| parse_report(text).map_err(|e| e.to_string()))
        })
        .collect();
    let mut ids = HashMap::<String, usize>::new();
    for item in &parsed {
        if let Ok(Parsed::Report { own_ids, .. }) = item {
            if own_ids.len() == 1 {
                *ids.entry(own_ids[0].clone()).or_default() += 1;
            }
        }
    }
    let mut rows = Vec::new();
    let mut dependencies = 0u64;
    let mut dependency_files = 0usize;
    let mut ledger_proofs = HashMap::new();
    for (ordinal, (file, parsed)) in files.iter().zip(parsed).enumerate() {
        let mut row = Row {
            ordinal,
            source_path: file.original_path.clone(),
            canonical_source_path: file.canonical_path.clone(),
            source_sha256: file.sha256.clone(),
            source_bytes: file.bytes.len() as u64,
            target: None,
            status: "failed".into(),
            reason: None,
            title: None,
            normalized_report_sha256: None,
            planned_finding: None,
            dependencies: Vec::new(),
            dependency_count: 0,
            dependency_bytes: 0,
        };
        let mut prepared = None;
        let mut prepared_ledger = None;
        let result = (|| -> Result<(), String> {
            let (mut report, own_ids) = match parsed {
                Ok(Parsed::Report { report, own_ids }) => (report, own_ids),
                Ok(Parsed::NotAReport) => {
                    row.status = "skipped".into();
                    return Err("not a finding report".into());
                }
                Err(e) => return Err(e),
            };
            if own_ids.len() != 1 {
                return Err("batch reports require exactly one Finding ID; attach an unlabelled report separately".into());
            }
            let id = &own_ids[0];
            if ids.get(id).copied().unwrap_or(0) != 1 {
                return Err("multiple selected reports cite the same finding".into());
            }
            let matching: Vec<_> = ledgers
                .iter()
                .filter(|ledger| ledger.records.iter().any(|r| &r.id == id))
                .collect();
            let [ledger] = matching.as_slice() else {
                return Err(
                    "finding must resolve to exactly one completely discovered target ledger"
                        .into(),
                );
            };
            let matches: Vec<_> = ledger
                .raw_records
                .iter()
                .filter(|raw| raw["id"].as_str() == Some(id))
                .collect();
            if matches.len() != 1 {
                return Err("finding occurs more than once in its target ledger".into());
            }
            row.target = Some(target(ledger, id));
            if matches[0].get("report").is_some_and(|v| !v.is_null())
                || matches[0]["profile"].as_str() == Some("full")
            {
                row.status = "skipped".into();
                return Err("finding already has a full report; it will not be replaced".into());
            }
            let known: HashSet<_> = ledger
                .records
                .iter()
                .filter(|r| &r.id != id)
                .map(|r| r.id.clone())
                .collect();
            retain_known_cross_references(&mut report, &known);
            let paths =
                rupu_coverage::CoveragePaths::new(&ledger.workspace_path, &ledger.target_id);
            if !ledger_proofs.contains_key(&ledger.ledger.canonical_path) {
                let proof = capture_import_ledger(&paths).map_err(|e| e.to_string())?;
                if proof.1 != ledger.ledger.sha256 {
                    return Err("ledger changed while preparing review".into());
                }
                ledger_proofs.insert(ledger.ledger.canonical_path.clone(), proof);
            }
            let known: Vec<_> = known.into_iter().collect();
            // Remaining aggregate capacity also bounds this preparation, not just its response.
            let mut bounded = opts.clone();
            bounded.artifact_total_max_bytes = bounded
                .artifact_total_max_bytes
                .min(MAX_DEPENDENCY_BYTES.saturating_sub(dependencies));
            bounded.artifact_max_files = bounded
                .artifact_max_files
                .min(MAX_DEPENDENCY_FILES.saturating_sub(dependency_files));
            let value = prepare_import_report(&paths, report, &known, &bounded, private_artifacts)
                .map_err(|e| e.to_string())?;
            let bytes: u64 = value.blobs.iter().map(|blob| blob.size).sum();
            dependencies = dependencies
                .checked_add(bytes)
                .ok_or("dependency budget exceeded")?;
            if dependencies > MAX_DEPENDENCY_BYTES {
                return Err("dependency budget exceeded".into());
            }
            row.title = Some(value.report.title.clone());
            row.planned_finding = Some(PlannedFinding {
                summary: value.report.title.clone(),
                severity: rupu_coverage::Severity::from(value.report.rating.risk_rating),
                evidence: rupu_coverage::FindingEvidence {
                    code_excerpt: value
                        .report
                        .evidence
                        .iter()
                        .find_map(|claim| claim.excerpt.clone()),
                    rationale: value.report.root_cause.clone(),
                    references: Vec::new(),
                },
            });
            row.dependencies = value.report.artifact_refs().cloned().collect();
            row.normalized_report_sha256 = Some(digest(&value.report).map_err(|e| e.1)?);
            dependency_files += value.blobs.len();
            row.dependency_count = value.blobs.len();
            row.dependency_bytes = bytes;
            row.status = "ready".into();
            prepared_ledger = ledger_proofs
                .get(&ledger.ledger.canonical_path)
                .map(|p| p.0.clone());
            prepared = Some(value);
            Ok(())
        })();
        if let Err(reason) = result {
            row.reason = Some(reason_text(&reason));
        }
        rows.push(StoredRow {
            row,
            prepared,
            prepared_ledger,
        });
    }
    Ok(Manifest {
        version: VERSION,
        review_id,
        owner: owner(global)?,
        created_at: now,
        expires_at: now + REVIEW_SECONDS,
        policy_digest,
        rows,
    })
}

fn target(ledger: &discovery::CapturedLedger, id: &str) -> Target {
    Target {
        ws_id: ledger.ws_id.clone(),
        target_id: ledger.target_id.clone(),
        finding_id: id.into(),
        project: ledger
            .workspace_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        workspace_path: ledger.workspace_path.clone(),
        workspace_identity: ledger.workspace_identity.clone(),
        registry_path: ledger.registry_record.canonical_path.clone(),
        registry_sha256: ledger.registry_record.sha256.clone(),
        registry_identity: ledger.registry_record.identity.clone(),
        ledger_path: ledger.ledger.canonical_path.clone(),
        ledger_identity: ledger.ledger.identity.clone(),
        ledger_sha256: ledger.ledger.sha256.clone(),
    }
}
pub(super) fn target_current(target: &Target, ledgers: &[discovery::CapturedLedger]) -> bool {
    let matching: Vec<_> = ledgers
        .iter()
        .filter(|l| l.records.iter().any(|r| r.id == target.finding_id))
        .collect();
    let [ledger] = matching.as_slice() else {
        return false;
    };
    self::target(ledger, &target.finding_id) == *target
}
pub(super) fn preview(manifest: &Manifest, review_digest: String) -> ApiResult<Preview> {
    let rows: Vec<_> = manifest.rows.iter().map(|r| r.row.clone()).collect();
    let ready: Vec<_> = rows
        .iter()
        .filter(|r| r.status == "ready")
        .filter_map(|r| r.target.as_ref())
        .collect();
    let value = Preview { version: VERSION, review_id: manifest.review_id.clone(), review_digest,
        host: "local".into(), scope: "registered_projects".into(), created_at: manifest.created_at, expires_at: manifest.expires_at,
        discovery_complete: true, ready: ready.len(), skipped: rows.iter().filter(|r| r.status == "skipped").count(),
        failed: rows.iter().filter(|r| r.status == "failed").count(), project_count: ready.iter().map(|t| &t.ws_id).collect::<BTreeSet<_>>().len(),
        ledger_count: ready.iter().map(|t| &t.ledger_path).collect::<BTreeSet<_>>().len(), rows,
        warnings: vec!["Targets may span projects. Each finding ledger is updated separately.".into(),
            "Imported claim hashes remain unset; this does not verify historical claims against current source.".into(),
            "Preview stores private bounded snapshots, but changes no finding ledger or backup.".into()] };
    bounded(&value)?;
    Ok(value)
}
pub(super) fn bounded(value: &impl Serialize) -> ApiResult<()> {
    if serde_json::to_vec(value)
        .map_err(|_| ApiError::internal("cannot encode import response"))?
        .len()
        > MAX_RESPONSE_BYTES
    {
        return Err(ApiError::bad_request(
            "import response exceeds the bounded review; select fewer reports",
        ));
    }
    Ok(())
}

/// Bound diagnostic bytes while retaining valid UTF-8.
pub(super) fn reason_text(text: &str) -> String {
    let mut end = text.len().min(2048);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
