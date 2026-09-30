use crate::catalog::types::{Severity, TouchStrength};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub run_id: String,
    pub model: String,
    pub surface: Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Workflow,
    Agent,
    Autoflow,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FileTouchEvent {
    Read {
        path: String,
        line_range: [u32; 2],
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Grep {
        path: String,
        pattern: String,
        match_count: u32,
        matched_lines: Vec<u32>,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Glob {
        path: String,
        pattern: String,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Edit {
        path: String,
        line_range: [u32; 2],
        lines_changed: u32,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Cmd {
        path: String,
        command: String,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Unknown {
        tool: String,
        arg_hash: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
}

impl FileTouchEvent {
    pub fn strength(&self) -> Option<TouchStrength> {
        match self {
            FileTouchEvent::Edit { .. } => Some(TouchStrength::Edit),
            FileTouchEvent::Read { .. } => Some(TouchStrength::Read),
            FileTouchEvent::Grep { .. } => Some(TouchStrength::Grep),
            FileTouchEvent::Cmd { .. } => Some(TouchStrength::Cmd),
            FileTouchEvent::Glob { .. } => Some(TouchStrength::Glob),
            FileTouchEvent::Unknown { .. } => None,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            FileTouchEvent::Read { path, .. }
            | FileTouchEvent::Grep { path, .. }
            | FileTouchEvent::Glob { path, .. }
            | FileTouchEvent::Edit { path, .. }
            | FileTouchEvent::Cmd { path, .. } => Some(path),
            FileTouchEvent::Unknown { .. } => None,
        }
    }

    pub fn attribution(&self) -> &Attribution {
        match self {
            FileTouchEvent::Read { attribution, .. }
            | FileTouchEvent::Grep { attribution, .. }
            | FileTouchEvent::Glob { attribution, .. }
            | FileTouchEvent::Edit { attribution, .. }
            | FileTouchEvent::Cmd { attribution, .. }
            | FileTouchEvent::Unknown { attribution, .. } => attribution,
        }
    }

    pub fn at(&self) -> DateTime<Utc> {
        match self {
            FileTouchEvent::Read { at, .. }
            | FileTouchEvent::Grep { at, .. }
            | FileTouchEvent::Glob { at, .. }
            | FileTouchEvent::Edit { at, .. }
            | FileTouchEvent::Cmd { at, .. }
            | FileTouchEvent::Unknown { at, .. } => *at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    Clean,
    Finding,
    Examined,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub summary: String,
    #[serde(default)]
    pub line_ranges: Vec<[u32; 2]>,
    #[serde(default)]
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernAssertion {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
    pub evidence: Evidence,
    pub declared_by: Attribution,
    pub declared_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingScope {
    /// A specific line range in a file. Requires `file_path` + `line_range`.
    Line,
    /// A whole file. Requires `file_path`.
    File,
    /// The repository or project as a whole. Needs no locator.
    Repo,
    /// A network host or IP. Requires `target_ref`.
    Host,
    /// A specific service endpoint (a URL). Requires `target_ref`.
    Endpoint,
    /// A cloud or platform resource named by its own scheme - OCID, ARN,
    /// URN. Requires `target_ref`.
    Resource,
}

impl FindingScope {
    /// Which locator this scope needs in order to point at anything.
    ///
    /// The code scopes locate a finding with `file_path`/`line_range`; the
    /// target scopes locate it with `target_ref`. `Repo` locates nothing --
    /// it IS the whole project.
    pub fn locator(&self) -> ScopeLocator {
        match self {
            Self::Line => ScopeLocator::FileAndLine,
            Self::File => ScopeLocator::File,
            Self::Repo => ScopeLocator::None,
            Self::Host | Self::Endpoint | Self::Resource => ScopeLocator::Target,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::File => "file",
            Self::Repo => "repo",
            Self::Host => "host",
            Self::Endpoint => "endpoint",
            Self::Resource => "resource",
        }
    }
}

/// What a [`FindingScope`] requires in order to point at anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeLocator {
    None,
    File,
    FileAndLine,
    Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingEvidence {
    #[serde(default)]
    pub code_excerpt: Option<String>,
    pub rationale: String,
    #[serde(default)]
    pub references: Vec<String>,
}

/// One line of `findings.jsonl`.
///
/// Deserialized through [`FindingRecordWire`] so that `report` is read
/// leniently: the report types are strict (`deny_unknown_fields`, closed
/// enums) because they validate TOOL INPUT, but a ledger line written by a
/// newer rupu may carry a report field or enum variant this build does not
/// know. Failing the whole line would silently drop the finding from every
/// older reader; instead the record loads with `report: None` (its summary,
/// severity and evidence intact) and a warning names the finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "FindingRecordWire")]
pub struct FindingRecord {
    pub id: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    /// Locator for a non-code scope: the host, endpoint or resource this
    /// finding is about. `None` for the code scopes, which use `file_path`.
    ///
    /// Added ALONGSIDE `file_path` rather than replacing `scope` with a
    /// tagged union carrying its own locator. That would be the tidier
    /// model, but `scope` is consumed as a plain string by the CP DTO (which
    /// `#[serde(flatten)]`s this record) and by the macOS client
    /// (`FindingsModels.swift`: `let scope: String`), so changing its shape
    /// breaks every reader while adding a variant does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    pub scope: FindingScope,
    pub summary: String,
    pub severity: Severity,
    #[serde(default)]
    pub concern_id: Option<String>,
    pub evidence: FindingEvidence,
    pub declared_by: Attribution,
    pub declared_at: DateTime<Utc>,
    /// Contract this finding was recorded under. Absent on ledger lines that
    /// predate profiles, which were all summary records.
    #[serde(default = "crate::report::FindingProfile::legacy")]
    pub profile: crate::report::FindingProfile,
    /// The full report. `Some` exactly when `profile` is `full` — except on
    /// a ledger line whose report this build cannot parse (see the type
    /// docs), which loads with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::report::FindingReport>,
}

/// Deserialization shape of [`FindingRecord`]: identical except that
/// `report` is kept as raw JSON until the record's id is known, so a report
/// that fails to parse can be dropped with a warning naming the finding
/// rather than failing the line. Keep the two field lists in step.
#[derive(Deserialize)]
struct FindingRecordWire {
    id: String,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    line_range: Option<[u32; 2]>,
    #[serde(default)]
    target_ref: Option<String>,
    scope: FindingScope,
    summary: String,
    severity: Severity,
    #[serde(default)]
    concern_id: Option<String>,
    evidence: FindingEvidence,
    declared_by: Attribution,
    declared_at: DateTime<Utc>,
    #[serde(default = "crate::report::FindingProfile::legacy")]
    profile: crate::report::FindingProfile,
    #[serde(default)]
    report: Option<serde_json::Value>,
}

impl From<FindingRecordWire> for FindingRecord {
    fn from(w: FindingRecordWire) -> Self {
        let report = match w.report {
            None | Some(serde_json::Value::Null) => None,
            Some(raw) => match serde_json::from_value::<crate::report::FindingReport>(raw) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::warn!(
                        finding_id = %w.id,
                        error = %e,
                        "finding report in the ledger could not be parsed by this rupu version \
                         (written by a newer one?); loading the finding without its report"
                    );
                    None
                }
            },
        };
        Self {
            id: w.id,
            file_path: w.file_path,
            line_range: w.line_range,
            target_ref: w.target_ref,
            scope: w.scope,
            summary: w.summary,
            severity: w.severity,
            concern_id: w.concern_id,
            evidence: w.evidence,
            declared_by: w.declared_by,
            declared_at: w.declared_at,
            profile: w.profile,
            report,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_01KS19A4MQXP".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn file_touch_event_exposes_attribution_and_at() {
        let at = Utc::now();
        let ev = FileTouchEvent::Read {
            path: "src/a.rs".to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attribution(),
            at,
        };
        assert_eq!(ev.attribution().run_id, "run_01KS19A4MQXP");
        assert_eq!(ev.at(), at);

        // Unknown has no path but still carries attribution + timestamp.
        let unknown = FileTouchEvent::Unknown {
            tool: "mystery".to_string(),
            arg_hash: "deadbeef".to_string(),
            attribution: attribution(),
            at,
        };
        assert_eq!(unknown.attribution().model, "claude-sonnet-4-6");
        assert_eq!(unknown.at(), at);
    }

    #[test]
    fn file_touch_read_event_round_trips_jsonl() {
        let event = FileTouchEvent::Read {
            path: "src/handlers/users.rs".to_string(),
            line_range: [1, 240],
            tool: "read_file".to_string(),
            attribution: attribution(),
            at: DateTime::parse_from_rfc3339("2026-05-23T14:01:32Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: FileTouchEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
        assert_eq!(event.strength(), Some(TouchStrength::Read));
        assert_eq!(event.path(), Some("src/handlers/users.rs"));
    }

    #[test]
    fn concern_assertion_round_trips_jsonl() {
        let assertion = ConcernAssertion {
            concern_id: "stride:spoofing".to_string(),
            file_path: "src/auth/login.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "Token check covers all entry points.".to_string(),
                line_ranges: vec![[1, 80]],
                finding_ids: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        let json = serde_json::to_string(&assertion).unwrap();
        let decoded: ConcernAssertion = serde_json::from_str(&json).unwrap();
        assert_eq!(assertion, decoded);
    }

    #[test]
    fn finding_record_round_trips_jsonl_with_null_concern() {
        let record = FindingRecord {
            id: "fnd_01KS19A3".to_string(),
            file_path: Some("src/config.rs".to_string()),
            line_range: Some([20, 28]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: "Hardcoded API key.".to_string(),
            severity: Severity::High,
            concern_id: None, // serendipitous
            evidence: FindingEvidence {
                code_excerpt: Some("const STRIPE_KEY = \"sk_live_...\"".to_string()),
                rationale: "Key should come from env.".to_string(),
                references: vec!["https://cwe.mitre.org/data/definitions/798.html".to_string()],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        let decoded: FindingRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn legacy_finding_line_deserializes_as_summary_without_report() {
        let line = r#"{"id":"fnd_1","scope":"repo","summary":"s","severity":"high",
            "evidence":{"rationale":"r"},
            "declared_by":{"run_id":"run_1","model":"m","surface":"workflow"},
            "declared_at":"2026-09-01T00:00:00Z"}"#;
        let rec: FindingRecord = serde_json::from_str(line).unwrap();
        assert_eq!(rec.profile, crate::report::FindingProfile::Summary);
        assert!(rec.report.is_none());
    }

    #[test]
    fn a_report_this_build_cannot_parse_keeps_the_rest_of_the_finding() {
        // A newer rupu may add a report field or an enum variant. The report
        // types are strict (they validate tool input), but the ledger read
        // must not drop the whole finding over it.
        let fixture = include_str!("../../tests/fixtures/finding_report/valid_full.json");
        let mut report: serde_json::Value = serde_json::from_str(fixture).unwrap();
        report["exploit_maturity"] = serde_json::json!("weaponized");
        report["rating"]["likelihood"] = serde_json::json!("Certain");
        let line = serde_json::json!({
            "id": "fnd_future",
            "scope": "repo",
            "summary": "Note lookup skips the ownership check",
            "severity": "critical",
            "evidence": { "rationale": "no owner comparison" },
            "declared_by": { "run_id": "run_1", "model": "m", "surface": "workflow" },
            "declared_at": "2026-09-01T00:00:00Z",
            "profile": "full",
            "report": report,
        })
        .to_string();

        let rec: FindingRecord = serde_json::from_str(&line).expect("line must still load");
        assert_eq!(rec.id, "fnd_future");
        assert_eq!(rec.summary, "Note lookup skips the ownership check");
        assert_eq!(rec.severity, Severity::Critical);
        assert_eq!(rec.evidence.rationale, "no owner comparison");
        assert_eq!(rec.profile, crate::report::FindingProfile::Full);
        assert!(rec.report.is_none());

        // And through the ledger reader, next to a line it can fully parse.
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = crate::ledger::paths::CoveragePaths::new(tmp.path(), "t");
        paths.ensure_dir().unwrap();
        let ok = r#"{"id":"fnd_ok","scope":"repo","summary":"s","severity":"low","evidence":{"rationale":"r"},"declared_by":{"run_id":"r","model":"m","surface":"workflow"},"declared_at":"2026-09-01T00:00:00Z"}"#;
        std::fs::write(&paths.findings, format!("{line}\n{ok}\n")).unwrap();
        let all = crate::ledger::read_findings(&paths).unwrap();
        let ids: Vec<&str> = all.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["fnd_future", "fnd_ok"]);
    }

    #[test]
    fn full_record_round_trips_with_its_report() {
        let fixture = include_str!("../../tests/fixtures/finding_report/valid_full.json");
        let report: crate::report::FindingReport = serde_json::from_str(fixture).unwrap();
        let rec = FindingRecord {
            id: "fnd_2".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: report.title.clone(),
            severity: crate::catalog::types::Severity::Critical,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: report.root_cause.clone(),
                references: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
            profile: crate::report::FindingProfile::Full,
            report: Some(report),
        };
        let back: FindingRecord =
            serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        assert_eq!(back, rec);
    }
}
