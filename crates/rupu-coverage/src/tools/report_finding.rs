use crate::asset::{append_asset, read_asset_graph, Asset, AssetId, Coordinate, Locator};
use crate::catalog::types::Severity;
use crate::ledger::events::{
    AssetRef, Attribution, FindingEvidence, FindingRecord, FindingScope, ScopeLocator,
};
use crate::ledger::paths::CoveragePaths;
use crate::profile::{unsatisfied, ActiveSet, EngagementProfile};
use crate::report::FieldError;
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
    /// The asset this finding is about. Deliberately NOT part of
    /// [`crate::report::FindingReport`]: the asset is ledger metadata about
    /// what the finding concerns, not report content, so the report schema
    /// (and its lockstep validator) is untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetInput>,
}

/// An asset as the reporting agent names it: the profile kind, the typed
/// locator that identifies it, and optionally its parent asset and a label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInput {
    pub kind: String,
    pub locator: crate::asset::Locator,
    #[serde(default)]
    pub parent: Option<String>,
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
}

pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
    opts: &crate::report::FindingWriteOptions,
) -> Result<ReportFindingOutput, ReportFindingError> {
    use crate::report::{FindingProfile, ValidateCtx};

    validate_locator(&input)?;
    // With engagement profiles active, route the finding to the profile that
    // owns its asset kind. `None` is the native `code` path, byte-for-byte.
    let engaged = opts
        .engagement
        .as_deref()
        .map(|engagement| engage(engagement, &input));
    let (summary, severity, evidence, report) = match opts.profile {
        FindingProfile::Summary => {
            if input.report.is_some() {
                return Err(ReportFindingError::ReportInSummaryMode);
            }
            if let Some(engaged) = engaged.as_ref().filter(|e| !e.problems.is_empty()) {
                return Err(ReportFindingError::Report(
                    crate::report::ReportValidationError(engaged.problems.clone()),
                ));
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
            if let Some(engaged) = &engaged {
                problems.extend(engaged.problems.iter().cloned());
                if let Some(profile) = engaged.profile {
                    problems.extend(profile_problems(profile, &report, &engaged.locator));
                }
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

    // Register the asset before the finding that references it, so a record
    // never points at an asset the store lacks (an orphaned asset, should the
    // finding write then fail, is harmless).
    let asset_ref = match engaged.and_then(|e| e.asset) {
        Some(EngagedAsset { asset, label_given }) => {
            let asset_ref = AssetRef {
                id: asset.id.0.clone(),
                kind: asset.kind.clone(),
            };
            upsert_asset(paths, asset, label_given, &attribution)?;
            Some(asset_ref)
        }
        None => None,
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
        asset: asset_ref,
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

/// The kind a finding that names no `asset` is filed under: the legacy
/// `scope`/`file_path` locators describe a file in the native `code` profile.
const LEGACY_KIND: &str = "code:file";

/// What `report_finding` resolved for a run with engagement profiles active.
struct Engaged<'a> {
    /// The ORIGIN profile that owns the finding's kind -- `None` when the
    /// kind could not be routed (then `problems` says why, and no profile's
    /// checks are run against a kind the agent will have to change anyway).
    profile: Option<&'a EngagementProfile>,
    /// What the completeness predicates see as the finding's locator.
    locator: Locator,
    /// The asset to register; `None` when the finding names none (a legacy
    /// finding with no `file_path`, such as a `repo`-scope one).
    asset: Option<EngagedAsset>,
    /// Routing failures, as report problems so they list alongside the rest.
    problems: Vec<FieldError>,
}

struct EngagedAsset {
    asset: Asset,
    /// Whether the agent named the label; a derived one never overwrites an
    /// existing asset's.
    label_given: bool,
}

/// Resolve the finding's asset kind and route it to the profile that owns it.
///
/// The kind is the explicit `asset.kind`, else `code:file` (the legacy
/// `scope`/`file_path` shape). It must be owned by an active profile AND be
/// one that profile declares: `ActiveSet::profile_for_kind` routes by
/// namespace alone, so `binary:nope` would otherwise reach `binary`.
fn engage<'a>(engagement: &'a ActiveSet, input: &ReportFindingInput) -> Engaged<'a> {
    let (kind, locator, parent, label) = match &input.asset {
        Some(a) => (
            a.kind.clone(),
            a.locator.clone(),
            a.parent.clone(),
            a.label.clone(),
        ),
        None => (
            LEGACY_KIND.to_string(),
            Locator(
                input
                    .file_path
                    .iter()
                    .map(|p| Coordinate::Path(p.clone()))
                    .collect(),
            ),
            None,
            None,
        ),
    };
    let active = || engagement.ids().join(", ");
    let mut problems = Vec::new();
    let mut profile = None;
    let mut kind_def = None;
    match engagement.profile_for_kind(&kind) {
        None => {
            let how = if input.asset.is_some() {
                format!("asset kind `{kind}` belongs to no active engagement profile")
            } else {
                format!(
                    "this finding names no `asset`, so its scope maps to `{LEGACY_KIND}`, which belongs to no active engagement profile"
                )
            };
            problems.push(FieldError {
                path: "asset.kind".into(),
                message: format!(
                    "{how} (active: {}); name an `asset` whose kind one of them declares",
                    active()
                ),
            });
        }
        Some(p) => match p.asset_kinds.iter().find(|k| k.id == kind) {
            None => problems.push(FieldError {
                path: "asset.kind".into(),
                message: format!(
                    "engagement profile `{}` does not declare asset kind `{kind}` (it declares: {})",
                    p.id,
                    p.asset_kinds
                        .iter()
                        .map(|k| k.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
            Some(def) => {
                profile = Some(p);
                kind_def = Some(def);
            }
        },
    }

    // Only a routed kind becomes an asset; a legacy finding with no file has
    // nothing to register.
    let named = input.asset.is_some() || input.file_path.is_some();
    let asset = (named && profile.is_some()).then(|| {
        let label_given = label.is_some();
        let label = label.unwrap_or_else(|| {
            kind_def
                .and_then(|d| locator.render_label(&d.label))
                .unwrap_or_else(|| {
                    let described = locator.describe();
                    if described.is_empty() {
                        kind.clone()
                    } else {
                        described
                    }
                })
        });
        let mut asset = Asset::new(kind.clone(), locator.clone(), label);
        asset.parent = parent.map(AssetId);
        EngagedAsset { asset, label_given }
    });
    Engaged {
        profile,
        locator,
        asset,
        problems,
    }
}

/// What the owning profile requires of a full report beyond the universal
/// validation: its REQUIRED completeness checks, and nothing else. A
/// profile's `evidence_blocks` and `classification_systems` are declarations
/// (they feed the agent guidance and the checklist predicates), not
/// allow-lists: an extra block or system is harmless, and rejecting one would
/// misfire (a `code` finding's CVSS score folds into its classifications, and
/// `code` lists only CWE). A block or system a profile truly needs is
/// enforced by naming it in a `completeness` check. Every unmet check is
/// returned (not just the first) so the agent fixes them all in one retry.
fn profile_problems(
    profile: &EngagementProfile,
    report: &crate::report::FindingReport,
    locator: &Locator,
) -> Vec<FieldError> {
    let mut problems = Vec::new();
    match unsatisfied(&profile.completeness, report, locator) {
        Ok(missing) => {
            for id in missing {
                let label = profile
                    .completeness
                    .iter()
                    .find(|c| c.id == id)
                    .map_or("", |c| c.label.as_str());
                problems.push(FieldError {
                    path: "report".into(),
                    message: format!(
                        "engagement profile `{}` requires `{id}` ({label}), and this report does not satisfy it",
                        profile.id
                    ),
                });
            }
        }
        // A profile whose own predicate is malformed cannot be enforced:
        // fail closed rather than let everything through.
        Err(e) => problems.push(FieldError {
            path: "report".into(),
            message: format!(
                "engagement profile `{}` has an invalid completeness check: {e}",
                profile.id
            ),
        }),
    }
    problems
}

/// Register `asset` in the append-only asset store.
///
/// The store folds last-write-wins on the WHOLE record, so a bare
/// `Asset::new` would erase what an earlier pass recorded (a coverage `depth`,
/// attributes, an agent-given label). The new record is therefore merged over
/// the existing one first, and nothing is appended when that changes nothing,
/// so a hundred findings on one function leave one line, not a hundred.
fn upsert_asset(
    paths: &CoveragePaths,
    mut asset: Asset,
    label_given: bool,
    attribution: &Attribution,
) -> std::io::Result<()> {
    let graph = read_asset_graph(paths);
    let existing = graph.get(&asset.id);
    if let Some(old) = existing {
        if !label_given {
            asset.label = old.label.clone();
        }
        if asset.parent.is_none() {
            asset.parent = old.parent.clone();
        }
        asset.depth = old.depth.clone();
        asset.attributes = old.attributes.clone();
    }
    if existing == Some(&asset) {
        return Ok(());
    }
    append_asset(paths, &asset, attribution)
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
    fn report_finding_input_asset_is_optional_and_round_trips() {
        // Absent -> None, and None is not serialized.
        let bare = serde_json::json!({
            "scope": "repo",
            "summary": "s",
            "severity": "low",
            "evidence": { "rationale": "r" },
        });
        let parsed: ReportFindingInput = serde_json::from_value(bare).unwrap();
        assert!(parsed.asset.is_none());
        assert!(serde_json::to_value(&parsed)
            .unwrap()
            .get("asset")
            .is_none());

        // Present: `parent` and `label` default; the locator is a coordinate list.
        let with_asset = serde_json::json!({
            "scope": "repo",
            "summary": "s",
            "severity": "low",
            "evidence": { "rationale": "r" },
            "asset": {
                "kind": "network_service",
                "locator": [{"host": "10.0.0.1"}, {"port": {"number": 443, "proto": "tcp"}}],
            },
        });
        let parsed: ReportFindingInput = serde_json::from_value(with_asset).unwrap();
        let asset = parsed.asset.clone().expect("asset");
        assert_eq!(asset.kind, "network_service");
        assert!(asset.locator.has("host") && asset.locator.has("port"));
        assert_eq!(asset.parent, None);
        assert_eq!(asset.label, None);
        let again: ReportFindingInput =
            serde_json::from_value(serde_json::to_value(&parsed).unwrap()).unwrap();
        assert_eq!(again.asset.unwrap().locator, asset.locator);
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

    // --- engagement profiles: routing + completeness gate + asset upsert ---

    fn engaged_opts(store: &std::path::Path, ids: &[&str]) -> crate::report::FindingWriteOptions {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let set = crate::profile::builtin_registry()
            .unwrap()
            .active_set(&ids)
            .unwrap();
        full_opts(store).with_engagement(Some(std::sync::Arc::new(set)))
    }

    fn disasm() -> crate::report::EvidenceBlock {
        crate::report::EvidenceBlock::Disasm {
            arch: "x86_64".into(),
            listing: vec![crate::report::DisasmLine {
                addr: "0x401000".into(),
                text: "push rbp".into(),
            }],
        }
    }

    fn function_asset(kind: &str) -> AssetInput {
        use crate::asset::Coordinate;
        AssetInput {
            kind: kind.into(),
            locator: crate::asset::Locator(vec![
                Coordinate::Sha256("ab".repeat(32)),
                Coordinate::Address(0x401000),
                Coordinate::Symbol("main".into()),
            ]),
            parent: None,
            label: None,
        }
    }

    fn binary_input(with_listing: bool, kind: &str) -> ReportFindingInput {
        let mut r = fixture_report();
        r.evidence[0].blocks = if with_listing { vec![disasm()] } else { vec![] };
        let mut i = full_input(r);
        i.asset = Some(function_asset(kind));
        i
    }

    #[test]
    fn a_binary_finding_with_a_listing_passes_and_registers_its_asset() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);
        report_finding(
            &paths,
            attribution(),
            binary_input(true, "binary:function"),
            &opts,
        )
        .expect("a binary finding with a disasm block satisfies the profile");

        // The asset line was appended, with the full record.
        let graph = crate::asset::read_asset_graph(&paths);
        let assets: Vec<_> = graph.iter().collect();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].kind, "binary:function");
        assert_eq!(assets[0].label, format!("main @ 0x401000"));
        assert_eq!(
            std::fs::read_to_string(&paths.assets)
                .unwrap()
                .lines()
                .count(),
            1
        );

        // The record references it, kind alongside the id.
        let rec = only_record(&paths);
        let r = rec.asset.expect("record.asset stamped");
        assert_eq!(r.id, assets[0].id.0);
        assert_eq!(r.kind, "binary:function");
    }

    #[test]
    fn a_binary_finding_without_a_listing_is_rejected_by_its_completeness_check() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);
        let err = report_finding(
            &paths,
            attribution(),
            binary_input(false, "binary:function"),
            &opts,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("evidence_has_listing"), "{err}");
        assert!(!paths.findings.exists(), "nothing written on rejection");
        assert!(!paths.assets.exists(), "no asset registered on rejection");
    }

    #[test]
    fn completeness_problems_are_listed_with_the_other_report_problems() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);
        let mut i = binary_input(false, "binary:function");
        i.report.as_mut().unwrap().cwe.clear();
        i.report.as_mut().unwrap().classifications.clear();
        i.report.as_mut().unwrap().root_cause = String::new();
        let err = report_finding(&paths, attribution(), i, &opts)
            .unwrap_err()
            .to_string();
        // One error, everything in it: the validator's own problem plus
        // each unmet profile check.
        assert!(err.contains("report.root_cause"), "{err}");
        assert!(err.contains("evidence_has_listing"), "{err}");
        assert!(err.contains("has_root_cause"), "{err}");
        assert!(err.contains("classified"), "{err}");
    }

    #[test]
    fn a_kind_outside_the_active_set_is_rejected_naming_the_active_profiles() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);
        let err = report_finding(
            &paths,
            attribution(),
            binary_input(true, "web:route"),
            &opts,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("web:route"), "{err}");
        assert!(err.contains("binary"), "names the active profiles: {err}");
        assert!(!paths.findings.exists());
        assert!(!paths.assets.exists());
    }

    #[test]
    fn a_kind_its_owner_does_not_declare_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);
        // `binary` is active, but declares no `nope` kind: routing by
        // namespace alone would let this through.
        let err = report_finding(
            &paths,
            attribution(),
            binary_input(true, "binary:nope"),
            &opts,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("binary:nope"), "{err}");
        assert!(err.contains("does not declare"), "{err}");
        assert!(!paths.findings.exists());
        assert!(!paths.assets.exists());
    }

    #[test]
    fn a_finding_with_no_asset_maps_its_file_to_code_file() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["code", "binary"]);
        let mut i = full_input(fixture_report());
        i.scope = FindingScope::File;
        i.file_path = Some("src/routes/notes.rs".into());
        report_finding(&paths, attribution(), i, &opts).expect("code:file, empty completeness");

        let rec = only_record(&paths);
        let r = rec
            .asset
            .expect("the legacy file is registered as an asset");
        assert_eq!(r.kind, "code:file");
        let graph = crate::asset::read_asset_graph(&paths);
        let a = graph.iter().next().unwrap();
        assert_eq!(a.id.0, r.id);
        assert_eq!(a.label, "src/routes/notes.rs");
        assert_eq!(
            a.locator,
            crate::asset::Locator(vec![crate::asset::Coordinate::Path(
                "src/routes/notes.rs".into()
            )])
        );
    }

    #[test]
    fn a_repo_scope_finding_with_no_asset_routes_to_code_but_names_no_asset() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["code", "binary"]);
        report_finding(&paths, attribution(), full_input(fixture_report()), &opts).expect("passes");
        assert!(only_record(&paths).asset.is_none());
        assert!(!paths.assets.exists(), "no file, so no asset to register");
    }

    #[test]
    fn a_legacy_finding_is_rejected_when_code_is_not_active() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);
        let err = report_finding(&paths, attribution(), full_input(fixture_report()), &opts)
            .unwrap_err()
            .to_string();
        assert!(err.contains("code:file"), "{err}");
        assert!(err.contains("binary"), "{err}");
        assert!(!paths.findings.exists());
    }

    #[test]
    fn blocks_and_classification_systems_outside_the_declared_lists_are_not_rejected() {
        // `evidence_blocks` / `classification_systems` are declarations, not
        // allow-lists. A `code` finding (declares text/code_slice/diff and
        // CWE only) that carries a disasm block, a CVE classification AND a
        // CVSS score -- which folds into its classifications -- must record.
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["code", "binary"]);
        let mut r = fixture_report();
        r.evidence[0].blocks = vec![disasm()];
        r.rating.cvss_v3 = "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".into();
        let systems: Vec<String> = r
            .all_classifications()
            .into_iter()
            .map(|c| c.system)
            .collect();
        assert!(
            systems.iter().any(|s| s == "CVE") && systems.iter().any(|s| s == "CVSS"),
            "the fixture exercises both undeclared systems: {systems:?}"
        );
        let mut i = full_input(r);
        i.scope = FindingScope::File;
        i.file_path = Some("src/a.rs".into());
        report_finding(&paths, attribution(), i, &opts)
            .expect("an undeclared block / classification system is not a rejection");
        assert_eq!(only_record(&paths).asset.unwrap().kind, "code:file");
    }

    #[test]
    fn engagement_none_applies_no_profile_gates() {
        // The native path: a disasm block, a CVE classification and a stray
        // asset are all untouched -- no routing, no completeness, no asset.
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = binary_input(true, "web:route");
        i.report.as_mut().unwrap().root_cause = "x".into();
        report_finding(&paths, attribution(), i, &full_opts(store.path())).expect("today's path");
        assert!(only_record(&paths).asset.is_none());
        assert!(!paths.assets.exists());
    }

    #[test]
    fn summary_profile_findings_are_routed_and_register_their_asset_too() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"])
            .with_profile(crate::report::FindingProfile::Summary);

        let mut bad = input(FindingScope::Repo);
        bad.asset = Some(function_asset("web:route"));
        let err = report_finding(&paths, attribution(), bad, &opts)
            .unwrap_err()
            .to_string();
        assert!(err.contains("web:route"), "{err}");
        assert!(!paths.findings.exists());

        let mut good = input(FindingScope::Repo);
        good.asset = Some(function_asset("binary:function"));
        report_finding(&paths, attribution(), good, &opts).expect("summary + asset");
        assert_eq!(
            only_record(&paths).asset.map(|a| a.kind).as_deref(),
            Some("binary:function")
        );
        assert_eq!(crate::asset::read_asset_graph(&paths).iter().count(), 1);
    }

    #[test]
    fn re_reporting_an_asset_keeps_its_depth_and_does_not_re_append_an_unchanged_record() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = engaged_opts(store.path(), &["binary"]);

        // The asset already exists, marked analyzed by an earlier pass.
        let wire = function_asset("binary:function");
        let mut existing =
            crate::asset::Asset::new(wire.kind.clone(), wire.locator.clone(), "main @ 0x401000");
        existing.depth = Some("analyzed".into());
        crate::asset::append_asset(&paths, &existing, &attribution()).unwrap();

        // A finding on the same asset must not clobber the depth (the store
        // is last-write-wins on the whole record) ...
        report_finding(
            &paths,
            attribution(),
            binary_input(true, "binary:function"),
            &opts,
        )
        .unwrap();
        let graph = crate::asset::read_asset_graph(&paths);
        assert_eq!(graph.iter().count(), 1);
        assert_eq!(
            graph.get(&existing.id).unwrap().depth.as_deref(),
            Some("analyzed")
        );
        // ... and, being unchanged, adds no line.
        assert_eq!(
            std::fs::read_to_string(&paths.assets)
                .unwrap()
                .lines()
                .count(),
            1
        );

        // A new label does update the record (full record, merged).
        let mut relabeled = binary_input(true, "binary:function");
        relabeled.asset.as_mut().unwrap().label = Some("entry".into());
        report_finding(&paths, attribution(), relabeled, &opts).unwrap();
        let graph = crate::asset::read_asset_graph(&paths);
        let a = graph.get(&existing.id).unwrap();
        assert_eq!(a.label, "entry");
        assert_eq!(a.depth.as_deref(), Some("analyzed"));
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
