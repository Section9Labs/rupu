use crate::catalog::types::Severity;
use crate::ledger::events::{
    Attribution, FindingEvidence, FindingRecord, FindingScope, ScopeLocator,
};
use crate::ledger::paths::CoveragePaths;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFindingInput {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    /// Host / endpoint / resource this finding is about, for the non-code
    /// scopes.
    #[serde(default)]
    pub target_ref: Option<String>,
    pub scope: FindingScope,
    /// Required under the summary profile; derived from `report` under full.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub evidence: Option<FindingEvidence>,
    /// The full report. Required under the full profile; refused under summary.
    #[serde(default)]
    pub report: Option<crate::report::FindingReport>,
    /// The engagement asset this finding is about (engagement profiles). Its
    /// `kind` routes the finding to the owning profile for completeness
    /// validation, and the asset is stamped into the asset graph. Ignored on
    /// the native code path (no active engagement).
    #[serde(default)]
    pub asset: Option<AssetRef>,
}

/// The asset a finding is about: a profile-namespaced `kind` and the locator
/// coordinates that pin it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRef {
    pub kind: String,
    #[serde(default)]
    pub coordinates: Vec<crate::asset::Coordinate>,
    /// Optional human label; if omitted, the asset keeps its existing label
    /// (or, for a new asset, takes its kind).
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFindingOutput {
    pub id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportFindingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("scope `{scope}` requires {needs}, but {got}")]
    Locator {
        scope: &'static str,
        needs: &'static str,
        got: &'static str,
    },
    #[error("`{0}` is required under the summary findings profile")]
    MissingField(&'static str),
    #[error("this step records findings under the full profile: `report` is required (see the report_finding tool schema)")]
    ReportRequired,
    #[error("this step records findings under the summary profile: omit `report`, or have the workflow author set findings_profile: full")]
    ReportInSummaryMode,
    #[error("under the full findings profile `summary`, `severity` and `evidence` are derived from `report`; omit them")]
    DerivedFieldsSupplied,
    #[error("{0}")]
    Report(#[from] crate::report::ReportValidationError),
    #[error("{0}")]
    Artifact(#[from] crate::report::ArtifactError),
    #[error("asset kind `{kind}` is not owned by any active engagement profile ({active}); declare it in the profile or select the right engagement")]
    UnownedKind { kind: String, active: String },
    #[error("finding is incomplete for engagement profile `{profile}`: {}", unsatisfied.join("; "))]
    CompletenessFailed {
        profile: String,
        unsatisfied: Vec<String>,
    },
    #[error("completeness predicate error for profile `{profile}`: {source}")]
    Predicate {
        profile: String,
        source: crate::profile::PredicateError,
    },
    #[error("asset store: {0}")]
    AssetStore(#[from] crate::asset::AssetStoreError),
}

pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
    opts: &crate::report::FindingWriteOptions,
) -> Result<ReportFindingOutput, ReportFindingError> {
    use crate::report::FindingProfile;

    validate_locator(&input)?;
    let (summary, severity, evidence, report) = match opts.profile {
        FindingProfile::Summary => {
            if input.report.is_some() {
                return Err(ReportFindingError::ReportInSummaryMode);
            }
            (
                input
                    .summary
                    .ok_or(ReportFindingError::MissingField("summary"))?,
                input
                    .severity
                    .ok_or(ReportFindingError::MissingField("severity"))?,
                input
                    .evidence
                    .ok_or(ReportFindingError::MissingField("evidence"))?,
                None,
            )
        }
        FindingProfile::Full => {
            if input.summary.is_some() || input.severity.is_some() || input.evidence.is_some() {
                return Err(ReportFindingError::DerivedFieldsSupplied);
            }
            let report = input.report.ok_or(ReportFindingError::ReportRequired)?;
            let known: Vec<String> = crate::ledger::read_findings(paths)?
                .into_iter()
                .map(|f| f.id)
                .collect();
            let report = prepare_full_report(paths, report, &known, opts, ClaimHashes::Record)?;
            let (summary, severity, evidence) = derived_fields(&report);
            (summary, severity, evidence, Some(report))
        }
    };

    // Engagement routing + completeness gate + asset stamp. Only when a run
    // has an active engagement AND the finding names an asset; otherwise this
    // is the native code path, untouched.
    if let (Some(active), Some(asset_ref)) = (opts.engagement.as_ref(), input.asset.as_ref()) {
        let profile = active.profile_for_kind(&asset_ref.kind).ok_or_else(|| {
            ReportFindingError::UnownedKind {
                kind: asset_ref.kind.clone(),
                active: active.ids().join(", "),
            }
        })?;
        let locator = crate::asset::Locator(asset_ref.coordinates.clone());
        // Completeness gate: only under the full profile, where a report exists
        // to evaluate predicates against.
        if let Some(report) = report.as_ref() {
            let mut unsatisfied = Vec::new();
            for check in &profile.completeness {
                if !check.required {
                    continue;
                }
                let ok = crate::profile::evaluate(&check.satisfied_when, report, &locator)
                    .map_err(|source| ReportFindingError::Predicate {
                        profile: profile.id.clone(),
                        source,
                    })?;
                if !ok {
                    unsatisfied.push(check.label.clone());
                }
            }
            if !unsatisfied.is_empty() {
                return Err(ReportFindingError::CompletenessFailed {
                    profile: profile.id.clone(),
                    unsatisfied,
                });
            }
        }
        // Stamp the asset into the graph (and the run stream, so a
        // coordinator collecting this run sees it). The store folds
        // last-line-wins, so this line replaces whatever the asset holds:
        // `next_line` carries its label (unless one is given here), depth (an
        // `asset_mark`), parent and attributes forward instead of erasing them.
        // A finding is never lost to the asset graph: if the store cannot be
        // read the asset is stamped without that carried state, and the
        // finding records as normal.
        let id = crate::asset::Asset::derive_id(&asset_ref.kind, &locator);
        let current = match crate::asset::read_assets(&paths.assets) {
            Ok(assets) => assets.into_iter().find(|a| a.id == id),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %paths.assets.display(),
                    "asset store unreadable; stamping the finding's asset without its prior state"
                );
                None
            }
        };
        let asset = crate::asset::Asset::next_line(
            asset_ref.kind.clone(),
            locator,
            asset_ref.label.clone(),
            current,
        );
        crate::ledger::stream::append_record(paths, crate::ledger::stream::Ledger::Assets, &asset)
            .map_err(crate::asset::AssetStoreError::Io)?;
    }

    let id = format!("fnd_{}", Ulid::new());
    let record = FindingRecord {
        id: id.clone(),
        file_path: input.file_path,
        line_range: input.line_range,
        target_ref: input.target_ref,
        scope: input.scope,
        summary,
        severity,
        concern_id: input.concern_id,
        evidence,
        declared_by: attribution,
        declared_at: Utc::now(),
        profile: opts.profile,
        report,
    };
    paths.ensure_dir()?;
    // One write per line, under the ledger lock, then the run stream — see
    // `ledger::stream::append_record`.
    crate::ledger::stream::append_record(paths, crate::ledger::stream::Ledger::Findings, &record)?;
    Ok(ReportFindingOutput { id })
}

/// Whether [`prepare_full_report`] records the SHA-256 of each evidence
/// claim's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimHashes {
    /// Hash each claim's file as it is now: a new finding, whose claims were
    /// just made against this code.
    Record,
    /// Leave every claim unhashed: an imported report, whose claims were made
    /// against the code as it was when the report was written. Hashing today's
    /// file would present old evidence as current.
    Skip,
}

/// Everything `validate_report` checks, plus the rule that a report may not
/// carry its own `verification`: all problems at once, no I/O. The dry-run
/// half of [`prepare_full_report`].
pub(crate) fn check_full_report(
    report: &crate::report::FindingReport,
    known: &[String],
    opts: &crate::report::FindingWriteOptions,
) -> Result<(), ReportFindingError> {
    // `verification` is part of a stored record, but it is the
    // verdict of a later verification run: the agent that wrote the
    // finding cannot confirm its own work. Reported alongside every
    // other problem so the agent still fixes them all in one retry.
    let mut problems = Vec::new();
    if report.verification.is_some() {
        problems.push(crate::report::FieldError {
            path: "report.verification".into(),
            message: "set by verification runs, not by the reporting agent; omit it".into(),
        });
    }
    if let Err(crate::report::ReportValidationError(errors)) = crate::report::validate_report(
        report,
        &crate::report::ValidateCtx {
            known_finding_ids: known,
            max_bytes: opts.report_max_bytes,
        },
    ) {
        problems.extend(errors);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ReportFindingError::Report(
            crate::report::ReportValidationError(problems),
        ))
    }
}

/// Validate a full-profile report and turn it into what the ledger stores:
/// claim hashes recomputed (or, with [`ClaimHashes::Skip`], left unset),
/// artifacts and evidence-block files verified and ingested into the store,
/// the size budget re-checked. Shared by
/// `report_finding` (a new finding, [`ClaimHashes::Record`]) and
/// `attach_reports` (a report imported onto an existing finding,
/// [`ClaimHashes::Skip`]).
pub(crate) fn prepare_full_report(
    paths: &CoveragePaths,
    mut report: crate::report::FindingReport,
    known: &[String],
    opts: &crate::report::FindingWriteOptions,
    hashes: ClaimHashes,
) -> Result<crate::report::FindingReport, ReportFindingError> {
    check_full_report(&report, known, opts)?;
    // A claim's hash is rupu's record of the file at write time, not
    // something the agent can assert: drop whatever it sent, then
    // hash what is actually there (or leave it unset).
    for claim in &mut report.evidence {
        claim.sha256 = None;
    }
    let has_block_files = report.blocks.iter().any(|b| b.artifact().is_some());
    if !report.artifacts.is_empty() || has_block_files {
        let store = opts
            .artifact_root
            .as_ref()
            .map(crate::report::ArtifactStore::new)
            .ok_or(crate::report::ArtifactError::NoStore)?;
        let limits = opts.ingest_limits();
        if !report.artifacts.is_empty() {
            report.artifacts = store.ingest(&paths.workspace, &report.artifacts, limits)?;
        }
        ingest_block_files(&store, &paths.workspace, &mut report, limits)?;
    }
    if hashes == ClaimHashes::Record {
        hash_claim_files(&paths.workspace, &mut report, opts.artifact_max_bytes);
    }
    check_stored_size(&report, opts)?;
    Ok(report)
}

/// The size budget, re-checked on the report as it will be stored. Directory
/// artifacts expand to one entry per file, and every artifact gains its hash,
/// size, kind and storage, so the report can grow far past the budget
/// `validate_report` checked.
pub(crate) fn check_stored_size(
    report: &crate::report::FindingReport,
    opts: &crate::report::FindingWriteOptions,
) -> Result<(), ReportFindingError> {
    let size = serde_json::to_vec(report)?.len();
    if size > opts.report_max_bytes {
        return Err(ReportFindingError::Report(
            crate::report::ReportValidationError(vec![crate::report::FieldError {
                path: "report.artifacts".into(),
                message: format!(
                    "after expanding directories the report is {size} bytes, over the {} byte budget ({} artifact files); list specific files instead of large directories",
                    opts.report_max_bytes,
                    report.artifacts.len()
                ),
            }]),
        ));
    }
    Ok(())
}

/// Verify and store every file an evidence block points at, against what is
/// left of the finding's ingest budget after `report.artifacts`, and replace
/// each block's ref (whatever the agent wrote) with the verified one. A block
/// names exactly one file. Block files stay in their blocks: they are not
/// PoC artifacts.
fn ingest_block_files(
    store: &crate::report::ArtifactStore,
    workspace: &std::path::Path,
    report: &mut crate::report::FindingReport,
    limits: crate::report::IngestLimits,
) -> Result<(), ReportFindingError> {
    resolve_block_files(workspace, report, limits, |requested, left| {
        store.ingest(workspace, std::slice::from_ref(requested), left)
    })
}

/// [`ingest_block_files`] without the store: every block file is checked the
/// way a real ingest checks it (it exists inside the workspace, is one file,
/// and fits what is left of the finding's budget), and each block's ref is
/// replaced with what the store would record, its sha256 a stand-in of the
/// same length. Nothing is hashed, copied or created. The dry-run half, for
/// `attach_reports`' dry run.
pub(crate) fn check_block_files(
    store: &crate::report::ArtifactStore,
    workspace: &std::path::Path,
    report: &mut crate::report::FindingReport,
    limits: crate::report::IngestLimits,
) -> Result<(), ReportFindingError> {
    resolve_block_files(workspace, report, limits, |requested, left| {
        Ok(store
            .check(workspace, std::slice::from_ref(requested), left)?
            .into_iter()
            .map(planned_ref)
            .collect())
    })
}

/// What [`crate::report::ArtifactStore::ingest`] would record for a file
/// [`crate::report::ArtifactStore::check`] planned, with a stand-in sha256
/// as long as a real digest, so a report's stored size can be checked
/// without hashing anything.
pub(crate) fn planned_ref(a: crate::report::PlannedArtifact) -> crate::report::ArtifactRef {
    crate::report::ArtifactRef {
        path: a.path,
        sha256: "0".repeat(64),
        size: a.size,
        kind: Some(a.kind),
        stored: Some(a.stored),
        host: None,
    }
}

/// The walk shared by [`ingest_block_files`] and [`check_block_files`]:
/// `resolve` turns one block's requested ref into what the store records (or
/// would), against the budget left.
fn resolve_block_files(
    workspace: &std::path::Path,
    report: &mut crate::report::FindingReport,
    limits: crate::report::IngestLimits,
    mut resolve: impl FnMut(
        &crate::report::ArtifactRef,
        crate::report::IngestLimits,
    )
        -> Result<Vec<crate::report::ArtifactRef>, crate::report::ArtifactError>,
) -> Result<(), ReportFindingError> {
    use crate::report::{ArtifactError, ArtifactStorage};
    let field_error = |i: usize, message: &str| {
        ReportFindingError::Report(crate::report::ReportValidationError(vec![
            crate::report::FieldError {
                path: format!("report.blocks[{i}].artifact.path"),
                message: message.into(),
            },
        ]))
    };
    // What the finding has already spent: the files `report.artifacts`
    // expanded to, and the bytes of those that were copied.
    let mut used_files = report.artifacts.len();
    let mut used_copied = 0usize;
    let mut used_bytes = 0u64;
    for a in &report.artifacts {
        if a.stored == Some(ArtifactStorage::Copied) {
            used_copied += 1;
            used_bytes = used_bytes.saturating_add(a.size);
        }
    }
    for i in 0..report.blocks.len() {
        let Some(requested) = report.blocks[i].artifact().cloned() else {
            continue;
        };
        if workspace.join(&requested.path).is_dir() {
            return Err(field_error(
                i,
                "names a directory; an evidence block shows exactly one file",
            ));
        }
        let left = crate::report::IngestLimits {
            max_files: limits.max_files.saturating_sub(used_files),
            max_total_bytes: limits.max_total_bytes.saturating_sub(used_bytes),
            ..limits
        };
        // The store judges this one block against what is left; report the
        // whole finding's numbers and the configured limits instead.
        let got = resolve(&requested, left).map_err(|e| match e {
            ArtifactError::TooManyFiles { .. } => ArtifactError::TooManyFiles {
                max: limits.max_files,
            },
            ArtifactError::TooLarge { total, files, .. } => ArtifactError::TooLarge {
                total: used_bytes.saturating_add(total),
                files: used_copied + files,
                max: limits.max_total_bytes,
            },
            other => other,
        });
        let got = match got {
            Ok(got) => got,
            // These name the block's own file: point at it.
            Err(
                e @ (ArtifactError::Missing { .. }
                | ArtifactError::Escapes { .. }
                | ArtifactError::Path { .. }),
            ) => return Err(field_error(i, &e.to_string())),
            Err(e) => return Err(e.into()),
        };
        let [one] = got.as_slice() else {
            return Err(field_error(i, "must name exactly one file"));
        };
        used_files += 1;
        if one.stored == Some(ArtifactStorage::Copied) {
            used_copied += 1;
            used_bytes = used_bytes.saturating_add(one.size);
        }
        *report.blocks[i]
            .artifact_mut()
            .expect("the block had an artifact above") = one.clone();
    }
    Ok(())
}

/// The record fields a full-profile finding derives from its report.
pub(crate) fn derived_fields(
    report: &crate::report::FindingReport,
) -> (String, Severity, FindingEvidence) {
    (
        report.title.clone(),
        Severity::from(report.rating.risk_rating),
        FindingEvidence {
            code_excerpt: report.evidence.iter().find_map(|c| c.excerpt.clone()),
            rationale: report.root_cause.clone(),
            references: Vec::new(),
        },
    )
}

/// Record the SHA-256 of each evidence claim's file as it is right now, so a
/// viewer can later flag a claim whose code has changed. Claims whose file
/// is not in the workspace (binary targets, other trees) are left unhashed;
/// the caller has already cleared anything the agent supplied.
///
/// Same containment rule as the artifact store: the claim's path is
/// canonicalized and a file that resolves outside the workspace (a symlink
/// out, say) is never read. Files over `max_bytes` (the artifact copy cap)
/// are left unhashed rather than streamed in full, and each distinct file is
/// hashed once however many claims cite it.
fn hash_claim_files(
    workspace: &std::path::Path,
    report: &mut crate::report::FindingReport,
    max_bytes: u64,
) {
    let Ok(ws_canon) = std::fs::canonicalize(workspace) else {
        return;
    };
    let mut hashed: std::collections::HashMap<std::path::PathBuf, Option<String>> =
        std::collections::HashMap::new();
    for claim in &mut report.evidence {
        let Some(file) = &claim.file else { continue };
        let Ok(canon) = std::fs::canonicalize(workspace.join(file)) else {
            continue;
        };
        if !canon.starts_with(&ws_canon) {
            continue;
        }
        claim.sha256 = hashed
            .entry(canon)
            .or_insert_with_key(|canon| {
                let meta = std::fs::metadata(canon).ok()?;
                if !meta.is_file() || meta.len() > max_bytes {
                    return None;
                }
                crate::report::artifacts::sha256_file(canon).ok()
            })
            .clone();
    }
}

/// A scope must actually point at something.
///
/// Without this, `scope` is decoration: a caller can declare `host` and give
/// no host, and the ledger accepts a finding nobody can act on. A finding
/// that says "something is wrong somewhere" is worse than no finding, because
/// it still costs a reader their attention.
///
/// Deliberately NOT enforced: that `file_path` is absent on a target scope,
/// or `target_ref` on a code scope. A finding can legitimately carry both --
/// a misconfiguration observed on a live host AND present in the Terraform
/// that produced it. Requiring the locator is a floor, not an exclusion.
fn validate_locator(input: &ReportFindingInput) -> Result<(), ReportFindingError> {
    let scope = input.scope.as_str();
    let missing = |needs, got| Err(ReportFindingError::Locator { scope, needs, got });
    match input.scope.locator() {
        ScopeLocator::None => Ok(()),
        ScopeLocator::File => match input.file_path {
            Some(_) => Ok(()),
            None => missing("a file_path", "none was given"),
        },
        ScopeLocator::FileAndLine => {
            if input.file_path.is_none() {
                return missing("a file_path and a line_range", "no file_path was given");
            }
            if input.line_range.is_none() {
                return missing("a file_path and a line_range", "no line_range was given");
            }
            Ok(())
        }
        ScopeLocator::Target => {
            let named = input
                .target_ref
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty());
            if named {
                Ok(())
            } else {
                missing(
                    "a target_ref naming the host, endpoint or resource",
                    "none was given",
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::Surface;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        }
    }

    fn input(scope: FindingScope) -> ReportFindingInput {
        ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope,
            summary: Some("s".to_string()),
            severity: Some(Severity::Medium),
            concern_id: None,
            evidence: Some(FindingEvidence {
                code_excerpt: None,
                rationale: "r".to_string(),
                references: vec![],
            }),
            report: None,
            asset: None,
        }
    }

    #[test]
    fn target_scopes_require_a_target_ref() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        for scope in [
            FindingScope::Host,
            FindingScope::Endpoint,
            FindingScope::Resource,
        ] {
            let err = report_finding(&paths, attribution(), input(scope), &summary_opts())
                .expect_err("a target scope with no target_ref must be refused");
            assert!(
                matches!(err, ReportFindingError::Locator { .. }),
                "expected a locator error for {scope:?}, got {err:?}"
            );
        }
        // Nothing was written: a refused finding must not reach the ledger.
        assert!(!paths.findings.exists());
    }

    #[test]
    fn whitespace_only_target_ref_does_not_count_as_naming_a_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut i = input(FindingScope::Host);
        i.target_ref = Some("   ".to_string());
        assert!(matches!(
            report_finding(&paths, attribution(), i, &summary_opts()),
            Err(ReportFindingError::Locator { .. })
        ));
    }

    #[test]
    fn host_scope_with_a_target_ref_is_recorded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut i = input(FindingScope::Host);
        i.target_ref = Some("identity.us-westjordan-1.example".to_string());
        report_finding(&paths, attribution(), i, &summary_opts()).expect("should record");
        let text = std::fs::read_to_string(&paths.findings).unwrap();
        let rec: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(rec["scope"], "host");
        assert_eq!(rec["target_ref"], "identity.us-westjordan-1.example");
        assert!(rec["file_path"].is_null());
    }

    #[test]
    fn code_scopes_still_require_their_file_locators() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        // file: no file_path
        assert!(matches!(
            report_finding(
                &paths,
                attribution(),
                input(FindingScope::File),
                &summary_opts()
            ),
            Err(ReportFindingError::Locator { .. })
        ));
        // line: file_path but no line_range
        let mut i = input(FindingScope::Line);
        i.file_path = Some("src/a.rs".to_string());
        assert!(matches!(
            report_finding(&paths, attribution(), i, &summary_opts()),
            Err(ReportFindingError::Locator { .. })
        ));
        // repo: locates nothing, needs nothing
        report_finding(
            &paths,
            attribution(),
            input(FindingScope::Repo),
            &summary_opts(),
        )
        .expect("repo scope needs no locator");
    }

    #[test]
    fn a_legacy_record_without_target_ref_still_deserializes() {
        // The field is additive. Every finding written before it existed must
        // keep loading, or adding a scope variant silently orphans the
        // existing ledger.
        let legacy = r#"{"id":"fnd_1","file_path":"src/a.rs","line_range":[1,2],"scope":"line","summary":"s","severity":"high","concern_id":null,"evidence":{"code_excerpt":null,"rationale":"r","references":[]},"declared_by":{"run_id":"r","model":"m","surface":"workflow"},"declared_at":"2026-01-01T00:00:00Z"}"#;
        let rec: FindingRecord = serde_json::from_str(legacy).expect("legacy record must load");
        assert_eq!(rec.scope, FindingScope::Line);
        assert!(rec.target_ref.is_none());
    }

    #[test]
    fn appends_finding_and_returns_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let out = report_finding(
            &paths,
            attribution(),
            ReportFindingInput {
                file_path: Some("src/config.rs".to_string()),
                line_range: Some([20, 28]),
                target_ref: None,
                scope: FindingScope::Line,
                summary: Some("Hardcoded API key.".to_string()),
                severity: Some(Severity::High),
                concern_id: Some("secrets-in-source".to_string()),
                evidence: Some(FindingEvidence {
                    code_excerpt: Some("const X = \"...\";".to_string()),
                    rationale: "Key in source.".to_string(),
                    references: vec![],
                }),
                report: None,
                asset: None,
            },
            &summary_opts(),
        )
        .unwrap();
        assert!(out.id.starts_with("fnd_"));
        let body = std::fs::read_to_string(&paths.findings).unwrap();
        assert_eq!(body.lines().count(), 1);
    }

    #[test]
    fn accepts_null_concern_for_serendipitous_finding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let out = report_finding(
            &paths,
            attribution(),
            ReportFindingInput {
                file_path: None,
                line_range: None,
                target_ref: None,
                scope: FindingScope::Repo,
                summary: Some("Spotted while looking for something else.".to_string()),
                severity: Some(Severity::Low),
                concern_id: None,
                evidence: Some(FindingEvidence {
                    code_excerpt: None,
                    rationale: "ad-hoc".to_string(),
                    references: vec![],
                }),
                report: None,
                asset: None,
            },
            &summary_opts(),
        )
        .unwrap();
        assert!(out.id.starts_with("fnd_"));
    }

    /// Verify that all three JSON strings that the `report_finding` tool schema
    /// advertises ("line", "file", "repo") deserialize cleanly into `FindingScope`.
    /// If the schema enum and the Rust enum ever diverge, serde will reject the
    /// value with an obscure error rather than a compile-time failure — this test
    /// catches that mismatch at the unit level before it affects LLM calls.
    #[test]
    fn all_schema_scope_values_deserialize_to_finding_scope() {
        for (json_str, expected) in [
            ("\"line\"", FindingScope::Line),
            ("\"file\"", FindingScope::File),
            ("\"repo\"", FindingScope::Repo),
        ] {
            let decoded: FindingScope = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("failed to deserialize scope {json_str:?}: {e}"));
            assert_eq!(
                decoded, expected,
                "scope {json_str:?} should round-trip cleanly"
            );
        }
    }

    fn summary_opts() -> crate::report::FindingWriteOptions {
        crate::report::FindingWriteOptions::default()
            .with_profile(crate::report::FindingProfile::Summary)
    }

    fn full_opts(store: &std::path::Path) -> crate::report::FindingWriteOptions {
        crate::report::FindingWriteOptions {
            artifact_root: Some(store.to_path_buf()),
            ..Default::default()
        }
    }

    fn fixture_report() -> crate::report::FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn full_input(report: crate::report::FindingReport) -> ReportFindingInput {
        ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: None,
            severity: None,
            concern_id: None,
            evidence: None,
            report: Some(report),
            asset: None,
        }
    }

    #[test]
    fn engagement_routes_gates_and_stamps_the_asset() {
        use std::sync::Arc;
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().join("store");
        let paths = CoveragePaths::new(tmp.path(), "t");
        let active = crate::profile::builtin_registry()
            .unwrap()
            .active_set(&["code".into()])
            .unwrap();
        let mut opts = full_opts(&store);
        opts.engagement = Some(Arc::new(active));

        // a kind no active profile owns is refused.
        let mut inp = full_input(fixture_report());
        inp.asset = Some(AssetRef {
            kind: "network:service".into(),
            coordinates: vec![],
            label: None,
        });
        assert!(matches!(
            report_finding(&paths, attribution(), inp, &opts),
            Err(ReportFindingError::UnownedKind { .. })
        ));

        // code:file with no `path` coordinate fails the `located` check.
        let mut inp = full_input(fixture_report());
        inp.asset = Some(AssetRef {
            kind: "code:file".into(),
            coordinates: vec![],
            label: None,
        });
        assert!(matches!(
            report_finding(&paths, attribution(), inp, &opts),
            Err(ReportFindingError::CompletenessFailed { .. })
        ));

        // code:file with a path satisfies completeness, records, and stamps the asset.
        let mut inp = full_input(fixture_report());
        inp.asset = Some(AssetRef {
            kind: "code:file".into(),
            coordinates: vec![crate::asset::Coordinate::Path("src/a.rs".into())],
            label: Some("src/a.rs".into()),
        });
        let out = report_finding(&paths, attribution(), inp, &opts).unwrap();
        assert!(out.id.starts_with("fnd_"));
        let assets = crate::asset::read_assets(&paths.assets).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].kind, "code:file");
        assert_eq!(assets[0].label, "src/a.rs");
    }

    #[test]
    fn engagement_stamp_streams_the_asset_before_the_finding() {
        use crate::ledger::stream::{RunStream, StreamLine};
        use std::sync::Arc;
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().join("store");
        let stream = tmp.path().join("runs/run_1/coverage.jsonl");
        let paths = CoveragePaths::new(tmp.path(), "t").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        let mut opts = full_opts(&store);
        opts.engagement = Some(Arc::new(
            crate::profile::builtin_registry()
                .unwrap()
                .active_set(&["code".into()])
                .unwrap(),
        ));
        let mut inp = full_input(fixture_report());
        inp.asset = Some(AssetRef {
            kind: "code:file".into(),
            coordinates: vec![crate::asset::Coordinate::Path("src/a.rs".into())],
            label: Some("src/a.rs".into()),
        });

        let out = report_finding(&paths, attribution(), inp, &opts).unwrap();

        let streamed: Vec<StreamLine> = std::fs::read_to_string(&stream)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        match streamed.as_slice() {
            [StreamLine::Assets {
                scope_name: a_scope,
                record: asset,
            }, StreamLine::Findings {
                scope_name: f_scope,
                record: finding,
            }] => {
                assert_eq!((a_scope.as_str(), f_scope.as_str()), ("sec", "sec"));
                assert_eq!(asset.kind, "code:file");
                assert_eq!(asset.label, "src/a.rs");
                assert_eq!(finding.id, out.id);
                // The streamed asset is the one the ledger holds.
                assert_eq!(
                    crate::asset::read_assets(&paths.assets).unwrap(),
                    vec![asset.clone()]
                );
            }
            other => panic!("expected an assets line then a findings line, got {other:?}"),
        }
    }

    fn code_engagement_opts(store: &std::path::Path) -> crate::report::FindingWriteOptions {
        let mut opts = full_opts(store);
        opts.engagement = Some(std::sync::Arc::new(
            crate::profile::builtin_registry()
                .unwrap()
                .active_set(&["code".into()])
                .unwrap(),
        ));
        opts
    }

    fn code_file_input(path: &str) -> ReportFindingInput {
        let mut inp = full_input(fixture_report());
        inp.asset = Some(AssetRef {
            kind: "code:file".into(),
            coordinates: vec![crate::asset::Coordinate::Path(path.into())],
            label: Some(path.into()),
        });
        inp
    }

    #[test]
    fn stamping_a_marked_asset_keeps_its_depth() {
        use crate::ledger::stream::{RunStream, StreamLine};
        use crate::tools::asset_mark::{asset_mark, AssetMarkInput};
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().join("store");
        let stream = tmp.path().join("runs/run_1/coverage.jsonl");
        let paths = CoveragePaths::new(tmp.path(), "t").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        let opts = code_engagement_opts(&store);
        let engagement = opts.engagement.clone().unwrap();

        let marked = asset_mark(
            &paths,
            AssetMarkInput {
                kind: "code:file".into(),
                coordinates: vec![crate::asset::Coordinate::Path("src/a.rs".into())],
                depth: "reviewed".into(),
                label: None,
            },
            &engagement,
        )
        .unwrap();
        assert_eq!(marked.effective_depth, "reviewed");

        report_finding(&paths, attribution(), code_file_input("src/a.rs"), &opts).unwrap();

        let stored = crate::asset::read_assets(&paths.assets).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].depth.as_deref(),
            Some("reviewed"),
            "a finding about a marked asset must not erase its depth"
        );
        // The unit's stream carries the full current state, so a coordinator
        // folding it ends where the unit did.
        let stamped: Vec<crate::asset::Asset> = std::fs::read_to_string(&stream)
            .unwrap()
            .lines()
            .filter_map(|l| match serde_json::from_str(l).unwrap() {
                StreamLine::Assets { record, .. } => Some(record),
                _ => None,
            })
            .collect();
        assert_eq!(stamped.len(), 2, "the mark and the stamp");
        assert_eq!(stamped[1].depth.as_deref(), Some("reviewed"));
    }

    #[test]
    fn stamping_an_existing_asset_keeps_its_parent_and_attributes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().join("store");
        let paths = CoveragePaths::new(tmp.path(), "t");
        let opts = code_engagement_opts(&store);
        let locator =
            crate::asset::Locator(vec![crate::asset::Coordinate::Path("src/a.rs".into())]);
        let mut existing = crate::asset::Asset::new(
            "code:file",
            locator,
            "src/a.rs",
            Some("code:dir:0123456789abcdef".into()),
        );
        existing.depth = Some("reviewed".into());
        existing
            .attributes
            .insert("lang".into(), serde_json::json!("rust"));
        crate::asset::store::upsert_asset(&paths.assets, &existing).unwrap();

        report_finding(&paths, attribution(), code_file_input("src/a.rs"), &opts).unwrap();

        assert_eq!(
            crate::asset::read_assets(&paths.assets).unwrap(),
            vec![existing]
        );
    }

    #[test]
    fn stamping_without_a_label_keeps_the_assets_descriptive_label() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().join("store");
        let paths = CoveragePaths::new(tmp.path(), "t");
        let opts = code_engagement_opts(&store);
        let locator =
            crate::asset::Locator(vec![crate::asset::Coordinate::Path("src/a.rs".into())]);
        let existing = crate::asset::Asset::new("code:file", locator, "the auth handler", None);
        crate::asset::store::upsert_asset(&paths.assets, &existing).unwrap();

        // No label on the stamp: the last-line-wins fold must not demote the
        // descriptive label to the bare kind.
        let mut inp = code_file_input("src/a.rs");
        inp.asset.as_mut().unwrap().label = None;
        report_finding(&paths, attribution(), inp, &opts).unwrap();
        let stored = crate::asset::read_assets(&paths.assets).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].label, "the auth handler");

        // An explicit label still replaces it.
        let mut inp = code_file_input("src/a.rs");
        inp.asset.as_mut().unwrap().label = Some("renamed".into());
        report_finding(&paths, attribution(), inp, &opts).unwrap();
        assert_eq!(
            crate::asset::read_assets(&paths.assets).unwrap()[0].label,
            "renamed"
        );
    }

    #[test]
    fn a_new_asset_stamped_without_a_label_is_labelled_with_its_kind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut inp = code_file_input("src/a.rs");
        inp.asset.as_mut().unwrap().label = None;
        report_finding(
            &paths,
            attribution(),
            inp,
            &code_engagement_opts(&tmp.path().join("store")),
        )
        .unwrap();
        assert_eq!(
            crate::asset::read_assets(&paths.assets).unwrap()[0].label,
            "code:file"
        );
    }

    #[test]
    fn a_corrupt_asset_store_does_not_lose_the_finding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().join("store");
        let paths = CoveragePaths::new(tmp.path(), "t");
        paths.ensure_dir().unwrap();
        std::fs::write(&paths.assets, "this is not json\n{\"half\":\n").unwrap();

        let out = report_finding(
            &paths,
            attribution(),
            code_file_input("src/a.rs"),
            &code_engagement_opts(&store),
        )
        .expect("a finding must not be lost to the asset graph");

        assert_eq!(only_record(&paths).id, out.id);
    }

    #[test]
    fn no_engagement_never_touches_the_asset_store() {
        // The native code path: with no active engagement, an asset on the
        // input is ignored and no assets.jsonl is written.
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let mut inp = input(FindingScope::Repo);
        inp.asset = Some(AssetRef {
            kind: "network:service".into(),
            coordinates: vec![],
            label: None,
        });
        report_finding(&paths, attribution(), inp, &summary_opts()).unwrap();
        assert!(!paths.assets.exists(), "no engagement => no asset store");
    }

    fn only_record(paths: &CoveragePaths) -> FindingRecord {
        let text = std::fs::read_to_string(&paths.findings).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    #[test]
    fn full_profile_records_report_and_derives_top_level_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        report_finding(
            &paths,
            attribution(),
            full_input(fixture_report()),
            &full_opts(store.path()),
        )
        .expect("valid full report records");
        let rec = only_record(&paths);
        assert_eq!(rec.profile, crate::report::FindingProfile::Full);
        let report = rec.report.as_ref().unwrap();
        assert_eq!(rec.summary, report.title);
        assert_eq!(rec.severity, Severity::Critical);
        assert_eq!(rec.evidence.rationale, report.root_cause);
        assert_eq!(
            rec.evidence.code_excerpt.as_deref(),
            report.evidence[0].excerpt.as_deref()
        );
    }

    #[test]
    fn full_profile_without_report_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = full_input(fixture_report());
        i.report = None;
        let err = report_finding(&paths, attribution(), i, &full_opts(ws.path())).unwrap_err();
        assert!(matches!(err, ReportFindingError::ReportRequired), "{err}");
        assert!(!paths.findings.exists(), "nothing written on rejection");
    }

    #[test]
    fn full_profile_rejects_derived_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = full_input(fixture_report());
        i.summary = Some("x".into());
        let err = report_finding(&paths, attribution(), i, &full_opts(ws.path())).unwrap_err();
        assert!(
            matches!(err, ReportFindingError::DerivedFieldsSupplied),
            "{err}"
        );
    }

    #[test]
    fn full_profile_surfaces_every_validation_problem() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut r = fixture_report();
        r.root_cause = String::new();
        r.regression_test = crate::report::OrSentinel::Sentinel("Unknown".into());
        let err = report_finding(&paths, attribution(), full_input(r), &full_opts(ws.path()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("report.root_cause"), "{err}");
        assert!(err.contains("report.regression_test"), "{err}");
        assert!(!paths.findings.exists());
    }

    #[test]
    fn summary_profile_rejects_a_report() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let err = report_finding(
            &paths,
            attribution(),
            full_input(fixture_report()),
            &summary_opts(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ReportFindingError::ReportInSummaryMode),
            "{err}"
        );
    }

    #[test]
    fn full_profile_ingests_artifacts_and_hashes_claim_files() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("src/routes")).unwrap();
        std::fs::write(
            ws.path().join("src/routes/notes.rs"),
            "async fn get_note() {}\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.path().join("pocs")).unwrap();
        std::fs::write(
            ws.path().join("pocs/out.txt"),
            "GET /api/notes/42 as user B: 200\n",
        )
        .unwrap();
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "pocs/out.txt".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        report_finding(
            &paths,
            attribution(),
            full_input(r),
            &full_opts(store.path()),
        )
        .unwrap();
        let rec = only_record(&paths);
        let rep = rec.report.unwrap();
        assert_eq!(
            rep.artifacts[0].stored,
            Some(crate::report::ArtifactStorage::Copied)
        );
        assert_eq!(rep.evidence[0].sha256.as_ref().map(String::len), Some(64));
    }

    #[test]
    fn agent_supplied_verification_is_rejected_with_the_other_problems() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut r = fixture_report();
        r.verification = Some(crate::report::Verification {
            status: crate::report::VerificationStatus::Confirmed,
            by_run: None,
            notes: None,
        });
        r.root_cause = String::new();
        let err = report_finding(&paths, attribution(), full_input(r), &full_opts(ws.path()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("report.verification"), "{err}");
        assert!(err.contains("set by verification runs"), "{err}");
        assert!(err.contains("report.root_cause"), "{err}");
        assert!(!paths.findings.exists(), "nothing written on rejection");
    }

    #[test]
    fn agent_supplied_claim_hashes_are_never_stored() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("src/routes")).unwrap();
        std::fs::write(ws.path().join("src/routes/notes.rs"), "fn get_note() {}\n").unwrap();
        let mut r = fixture_report();
        let forged = "0".repeat(64);
        // Claim 0's file exists: rupu's hash replaces the forged one.
        r.evidence[0].file = Some("src/routes/notes.rs".into());
        r.evidence[0].sha256 = Some(forged.clone());
        // Claim 1's file is not in the workspace: the forged hash is dropped.
        let mut other = r.evidence[0].clone();
        other.file = Some("not/here.rs".into());
        r.evidence.push(other);
        let paths = CoveragePaths::new(ws.path(), "t");
        report_finding(
            &paths,
            attribution(),
            full_input(r),
            &full_opts(store.path()),
        )
        .unwrap();
        let rep = only_record(&paths).report.unwrap();
        let h0 = rep.evidence[0].sha256.clone().expect("hashed by rupu");
        assert_ne!(h0, forged);
        assert_eq!(h0.len(), 64);
        assert_eq!(rep.evidence[1].sha256, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_claim_file_symlinked_outside_the_workspace_is_not_hashed() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "not yours\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            ws.path().join("looks_local.txt"),
        )
        .unwrap();
        std::fs::write(ws.path().join("big.bin"), vec![0u8; 64]).unwrap();
        std::fs::write(ws.path().join("small.rs"), "fn a() {}\n").unwrap();
        let mut r = fixture_report();
        let mut claim = r.evidence[0].clone();
        claim.file = Some("looks_local.txt".into());
        let mut big = claim.clone();
        big.file = Some("big.bin".into());
        let mut small = claim.clone();
        small.file = Some("small.rs".into());
        let small_again = small.clone();
        r.evidence = vec![claim, big, small, small_again];
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = crate::report::FindingWriteOptions {
            artifact_max_bytes: 32,
            ..full_opts(store.path())
        };
        report_finding(&paths, attribution(), full_input(r), &opts).unwrap();
        let rep = only_record(&paths).report.unwrap();
        assert_eq!(rep.evidence[0].sha256, None, "outside the workspace");
        assert_eq!(rep.evidence[1].sha256, None, "over the size cap");
        let h = rep.evidence[2]
            .sha256
            .clone()
            .expect("small local file hashed");
        assert_eq!(rep.evidence[3].sha256.as_deref(), Some(h.as_str()));
    }

    #[test]
    fn artifact_caps_come_from_the_options() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("pocs")).unwrap();
        for n in 0..3 {
            std::fs::write(ws.path().join(format!("pocs/f{n}.txt")), "xx").unwrap();
        }
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "pocs".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        let count = crate::report::FindingWriteOptions {
            artifact_max_files: 2,
            ..full_opts(store.path())
        };
        let err = report_finding(&paths, attribution(), full_input(r.clone()), &count)
            .unwrap_err()
            .to_string();
        assert!(err.contains("more than 2 files"), "{err}");
        let total = crate::report::FindingWriteOptions {
            artifact_total_max_bytes: 5,
            ..full_opts(store.path())
        };
        let err = report_finding(&paths, attribution(), full_input(r), &total)
            .unwrap_err()
            .to_string();
        assert!(err.contains("6 bytes across 3 files"), "{err}");
        assert!(!paths.findings.exists(), "nothing written on rejection");
    }

    #[test]
    fn an_artifact_over_the_per_file_cap_is_recorded_by_reference_not_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("huge.bin"), vec![9u8; 100]).unwrap();
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "huge.bin".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        // Bigger than both the per-file and the total cap.
        let opts = crate::report::FindingWriteOptions {
            artifact_max_bytes: 10,
            artifact_total_max_bytes: 15,
            ..full_opts(store.path())
        };
        report_finding(&paths, attribution(), full_input(r), &opts)
            .expect("an over-cap file is recorded external, never a rejection");
        assert!(paths.findings.exists(), "the finding was written");
    }

    #[test]
    fn artifacts_without_a_store_fail_loudly() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("out.txt"), "x").unwrap();
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "out.txt".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = crate::report::FindingWriteOptions::default(); // no artifact_root
        let err = report_finding(&paths, attribution(), full_input(r), &opts).unwrap_err();
        assert!(
            matches!(
                err,
                ReportFindingError::Artifact(crate::report::ArtifactError::NoStore)
            ),
            "{err}"
        );
        assert!(!paths.findings.exists());
    }

    fn file_ref(path: &str) -> crate::report::ArtifactRef {
        crate::report::ArtifactRef {
            path: path.into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }
    }

    fn image_block(path: &str) -> crate::report::EvidenceBlock {
        crate::report::EvidenceBlock::Image {
            artifact: file_ref(path),
            caption: Some("the login page".into()),
        }
    }

    #[test]
    fn an_image_block_file_is_verified_and_stored_but_not_listed_as_a_poc() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("shots")).unwrap();
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(b"fake");
        std::fs::write(ws.path().join("shots/login.png"), &bytes).unwrap();
        let real_sha = crate::report::sha256_file(&ws.path().join("shots/login.png")).unwrap();
        let mut r = fixture_report();
        r.artifacts = vec![];
        r.blocks = vec![crate::report::EvidenceBlock::Image {
            artifact: crate::report::ArtifactRef {
                path: "shots/login.png".into(),
                sha256: "0".repeat(64),
                size: 1,
                kind: Some(crate::report::ArtifactKind::Text),
                stored: Some(crate::report::ArtifactStorage::External),
                host: Some("agent-claimed-host".into()),
            },
            caption: Some("the login page".into()),
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        report_finding(
            &paths,
            attribution(),
            full_input(r),
            &full_opts(store.path()),
        )
        .unwrap();
        let rep = only_record(&paths).report.unwrap();
        let a = rep.blocks[0].artifact().expect("still an image block");
        assert_eq!(a.path, "shots/login.png");
        assert_eq!(a.sha256, real_sha, "the agent's sha is replaced");
        assert_ne!(a.sha256, "0".repeat(64));
        assert_eq!(a.size, bytes.len() as u64, "the agent's size is replaced");
        assert_eq!(a.kind, Some(crate::report::ArtifactKind::Binary));
        assert_eq!(a.stored, Some(crate::report::ArtifactStorage::Copied));
        assert_eq!(a.host, None, "the agent cannot claim a host");
        assert!(matches!(
            &rep.blocks[0],
            crate::report::EvidenceBlock::Image { caption: Some(c), .. } if c == "the login page"
        ));
        let blob = crate::report::ArtifactStore::new(store.path()).blob_path(&real_sha);
        assert_eq!(std::fs::read(blob).unwrap(), bytes);
        assert!(
            rep.artifacts.is_empty(),
            "a block's file is not a PoC artifact"
        );
    }

    #[test]
    fn a_block_naming_a_directory_is_a_field_error() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("dumps")).unwrap();
        std::fs::write(ws.path().join("dumps/a.bin"), b"aa").unwrap();
        std::fs::write(ws.path().join("dumps/b.bin"), b"bb").unwrap();
        let mut r = fixture_report();
        r.blocks = vec![crate::report::EvidenceBlock::Hexdump {
            base: 0x1000,
            artifact: file_ref("dumps/"),
            rendered: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        let err = report_finding(
            &paths,
            attribution(),
            full_input(r),
            &full_opts(store.path()),
        )
        .unwrap_err();
        match &err {
            ReportFindingError::Report(e) => {
                assert_eq!(e.0.len(), 1, "{err}");
                assert_eq!(e.0[0].path, "report.blocks[0].artifact.path");
            }
            other => panic!("expected a report field error, got {other}"),
        }
        assert!(!paths.findings.exists(), "nothing written on rejection");
    }

    #[test]
    fn a_block_naming_a_missing_file_is_refused() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let mut r = fixture_report();
        r.blocks = vec![crate::report::EvidenceBlock::PcapRef {
            artifact: file_ref("caps/none.pcap"),
            summary: "three SYNs".into(),
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        let err = report_finding(
            &paths,
            attribution(),
            full_input(r),
            &full_opts(store.path()),
        )
        .unwrap_err();
        match &err {
            ReportFindingError::Report(e) => {
                assert_eq!(e.0.len(), 1, "{err}");
                assert_eq!(e.0[0].path, "report.blocks[0].artifact.path");
                assert!(e.0[0].message.contains("caps/none.pcap"), "{err}");
                assert!(e.0[0].message.contains("does not exist"), "{err}");
            }
            other => panic!("expected a report field error, got {other}"),
        }
        assert!(!paths.findings.exists());
    }

    #[test]
    fn a_block_path_leaving_the_workspace_is_a_field_error_at_its_index() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let mut r = fixture_report();
        r.blocks = vec![
            crate::report::EvidenceBlock::Text {
                text: "note".into(),
            },
            crate::report::EvidenceBlock::PcapRef {
                artifact: file_ref("../outside.pcap"),
                summary: "three SYNs".into(),
            },
        ];
        let paths = CoveragePaths::new(ws.path(), "t");
        let err = report_finding(
            &paths,
            attribution(),
            full_input(r),
            &full_opts(store.path()),
        )
        .unwrap_err();
        match &err {
            ReportFindingError::Report(e) => {
                assert_eq!(e.0.len(), 1, "{err}");
                assert_eq!(e.0[0].path, "report.blocks[1].artifact.path");
            }
            other => panic!("expected a report field error, got {other}"),
        }
        assert!(!paths.findings.exists());
    }

    #[test]
    fn block_files_share_the_findings_ingest_budget() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("a.txt"), "aaaaaa").unwrap();
        std::fs::write(ws.path().join("b.png"), "bbbbbb").unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = crate::report::FindingWriteOptions {
            artifact_total_max_bytes: 10,
            ..full_opts(store.path())
        };
        let mut only_artifact = fixture_report();
        only_artifact.artifacts = vec![file_ref("a.txt")];
        report_finding(&paths, attribution(), full_input(only_artifact), &opts)
            .expect("6 of 10 bytes fits");
        std::fs::remove_file(&paths.findings).unwrap();

        let mut both = fixture_report();
        both.artifacts = vec![file_ref("a.txt")];
        both.blocks = vec![image_block("b.png")];
        let err =
            report_finding(&paths, attribution(), full_input(both.clone()), &opts).unwrap_err();
        match &err {
            // The numbers are the whole finding's, not what was left over.
            ReportFindingError::Artifact(crate::report::ArtifactError::TooLarge {
                total,
                files,
                max,
            }) => assert_eq!((*total, *files, *max), (12, 2, 10), "{err}"),
            other => panic!("expected TooLarge, got {other}"),
        }
        assert!(!paths.findings.exists(), "nothing written on rejection");

        // The file count is shared too, and reported as the configured limit.
        let files = crate::report::FindingWriteOptions {
            artifact_max_files: 1,
            ..full_opts(store.path())
        };
        let err = report_finding(&paths, attribution(), full_input(both), &files).unwrap_err();
        assert!(
            matches!(
                err,
                ReportFindingError::Artifact(crate::report::ArtifactError::TooManyFiles { max: 1 })
            ),
            "{err}"
        );
        assert!(!paths.findings.exists());
    }

    #[test]
    fn block_files_without_a_store_fail_loudly() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("x.png"), "x").unwrap();
        let mut r = fixture_report();
        r.blocks = vec![image_block("x.png")];
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = crate::report::FindingWriteOptions::default(); // no artifact_root
        let err = report_finding(&paths, attribution(), full_input(r), &opts).unwrap_err();
        assert!(
            matches!(
                err,
                ReportFindingError::Artifact(crate::report::ArtifactError::NoStore)
            ),
            "{err}"
        );
        assert!(!paths.findings.exists());
    }

    #[test]
    fn claim_hashes_are_skipped_on_request() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("src/routes")).unwrap();
        std::fs::write(ws.path().join("src/routes/notes.rs"), "fn get_note() {}\n").unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut r = fixture_report();
        r.evidence[0].sha256 = Some("0".repeat(64));
        let opts = crate::report::FindingWriteOptions::default();
        let recorded =
            prepare_full_report(&paths, r.clone(), &[], &opts, ClaimHashes::Record).unwrap();
        assert_eq!(
            recorded.evidence[0].sha256.as_ref().map(String::len),
            Some(64)
        );
        assert_ne!(recorded.evidence[0].sha256, r.evidence[0].sha256);
        let skipped = prepare_full_report(&paths, r, &[], &opts, ClaimHashes::Skip).unwrap();
        assert_eq!(skipped.evidence[0].sha256, None, "not hashed, and not kept");
    }

    #[test]
    fn summary_profile_requires_its_three_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = input(FindingScope::Repo);
        i.severity = None;
        let err = report_finding(&paths, attribution(), i, &summary_opts()).unwrap_err();
        assert!(
            matches!(err, ReportFindingError::MissingField("severity")),
            "{err}"
        );
    }

    #[test]
    fn directory_artifact_expansion_is_held_to_the_report_budget() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("pocs/big")).unwrap();
        for n in 0..40 {
            std::fs::write(ws.path().join(format!("pocs/big/f{n}.txt")), format!("{n}")).unwrap();
        }
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "pocs/big".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        // Just above the pre-ingest size: validation passes, the expanded
        // report (40 entries, each with a 64-char hash) cannot.
        let pre = serde_json::to_vec(&r).unwrap().len();
        let opts = crate::report::FindingWriteOptions {
            report_max_bytes: pre + 64,
            ..full_opts(store.path())
        };
        let paths = CoveragePaths::new(ws.path(), "t");
        let err = report_finding(&paths, attribution(), full_input(r), &opts)
            .unwrap_err()
            .to_string();
        assert!(err.contains("report.artifacts"), "{err}");
        assert!(err.contains("40 artifact files"), "{err}");
        assert!(!paths.findings.exists(), "nothing written on rejection");
    }
}
