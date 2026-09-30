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
}

pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
    opts: &crate::report::FindingWriteOptions,
) -> Result<ReportFindingOutput, ReportFindingError> {
    use crate::report::{FindingProfile, ValidateCtx};

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
            let mut report = input.report.ok_or(ReportFindingError::ReportRequired)?;
            let known: Vec<String> = crate::ledger::read_findings(paths)?
                .into_iter()
                .map(|f| f.id)
                .collect();
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
            if let Err(crate::report::ReportValidationError(errors)) =
                crate::report::validate_report(
                    &report,
                    &ValidateCtx {
                        known_finding_ids: &known,
                        max_bytes: opts.report_max_bytes,
                    },
                )
            {
                problems.extend(errors);
            }
            if !problems.is_empty() {
                return Err(ReportFindingError::Report(
                    crate::report::ReportValidationError(problems),
                ));
            }
            // A claim's hash is rupu's record of the file at write time, not
            // something the agent can assert: drop whatever it sent, then
            // hash what is actually there (or leave it unset).
            for claim in &mut report.evidence {
                claim.sha256 = None;
            }
            if !report.artifacts.is_empty() {
                let store = opts
                    .artifact_root
                    .as_ref()
                    .map(crate::report::ArtifactStore::new)
                    .ok_or(crate::report::ArtifactError::NoStore)?;
                report.artifacts =
                    store.ingest(&paths.workspace, &report.artifacts, opts.ingest_limits())?;
            }
            hash_claim_files(&paths.workspace, &mut report, opts.artifact_max_bytes);
            // Directory artifacts expand to one entry per file, so the report
            // can grow far past the budget `validate_report` checked. Re-check
            // before anything reaches the ledger.
            let size = serde_json::to_vec(&report)?.len();
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
            let evidence = FindingEvidence {
                code_excerpt: report.evidence.iter().find_map(|c| c.excerpt.clone()),
                rationale: report.root_cause.clone(),
                references: Vec::new(),
            };
            (
                report.title.clone(),
                Severity::from(report.rating.risk_rating),
                evidence,
                Some(report),
            )
        }
    };

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
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.findings)?;
    // One `write_all` of the line and its newline together: with two
    // writes, a concurrent writer appending to the same ledger could land
    // its line between ours and our newline, fusing two records into one
    // unparseable line.
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    f.flush()?;
    Ok(ReportFindingOutput { id })
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
        }
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
