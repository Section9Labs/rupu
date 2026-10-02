//! Strict validation of a [`FindingReport`] under the `full` profile.
//!
//! Collects EVERY problem before returning, so the agent can fix them all in
//! one retry instead of discovering them one call at a time.

use crate::report::types::*;
use std::fmt;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// JSON path, e.g. `report.evidence[0].file`.
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportValidationError(pub Vec<FieldError>);

impl fmt::Display for ReportValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "finding report has {} problem(s); fix all of them and call again:",
            self.0.len()
        )?;
        for e in &self.0 {
            writeln!(f, "- {}: {}", e.path, e.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ReportValidationError {}

pub struct ValidateCtx<'a> {
    /// `fnd_` ids already in this project's ledger; cross-references must
    /// point at one of them.
    pub known_finding_ids: &'a [String],
    /// Serialized-size budget for the whole report.
    pub max_bytes: usize,
}

/// Why `p` is not an acceptable workspace-relative path, if it isn't.
pub(crate) fn rel_path_problem(p: &str) -> Option<&'static str> {
    if p.trim().is_empty() {
        return Some("must not be empty");
    }
    let path = Path::new(p);
    if path.is_absolute() || p.starts_with('/') || p.starts_with('\\') {
        return Some("must be workspace-relative, not absolute");
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Some("must not contain `..`");
    }
    None
}

/// `Not Provided — <justification>`, where the justification starts right
/// after the prefix's single space. Mirrors the schema's
/// `^Not Provided — \S`: a second space (or any whitespace) before the
/// justification is rejected by both.
fn not_provided_ok(s: &str) -> bool {
    s.strip_prefix(NOT_PROVIDED_PREFIX)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| !c.is_whitespace())
}

const NOT_PROVIDED_HINT: &str =
    "provide the content, or exactly `Not Provided — <one-line justification>`; `Unknown` is not accepted here";

struct Check {
    errors: Vec<FieldError>,
}

impl Check {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(FieldError {
            path: path.into(),
            message: message.into(),
        });
    }

    fn text(&mut self, path: &str, v: &str) {
        if v.trim().is_empty() {
            self.err(
                path,
                "must not be empty (use the field's sentinel if it is genuinely unknown)",
            );
        }
    }

    fn opt_text(&mut self, path: &str, v: &Option<String>) {
        if let Some(s) = v {
            self.text(path, s);
        }
    }

    fn rel_path(&mut self, path: &str, v: &Option<String>) {
        if let Some(p) = v {
            if let Some(why) = rel_path_problem(p) {
                self.err(path, why);
            }
        }
    }

    fn lines(&mut self, path: &str, v: &Option<[u32; 2]>) {
        if let Some([a, b]) = v {
            if a > b || *a == 0 {
                self.err(path, "must be [start, end] with 1 <= start <= end");
            }
        }
    }
}

pub fn validate_report(r: &FindingReport, ctx: &ValidateCtx) -> Result<(), ReportValidationError> {
    let mut c = Check { errors: Vec::new() };

    c.text("report.title", &r.title);
    c.text("report.ownership.owner", &r.ownership.owner);
    c.text("report.ownership.product", &r.ownership.product);
    c.text(
        "report.ownership.affected_component",
        &r.ownership.affected_component,
    );
    c.text(
        "report.ownership.source_repository",
        &r.ownership.source_repository,
    );

    match &r.tickets {
        OrSentinel::Sentinel(s) if s == "None Provided" || s == "Unknown" => {}
        OrSentinel::Sentinel(_) => c.err(
            "report.tickets",
            "must be `None Provided`, `Unknown`, or a list of tickets",
        ),
        OrSentinel::Value(list) if list.is_empty() => c.err(
            "report.tickets",
            "an empty list is not allowed; use `None Provided`",
        ),
        OrSentinel::Value(list) => {
            for (i, t) in list.iter().enumerate() {
                c.text(&format!("report.tickets[{i}].type"), &t.kind);
                c.text(&format!("report.tickets[{i}].identifier"), &t.identifier);
            }
        }
    }

    c.text("report.rating.cvss_v3", &r.rating.cvss_v3);
    c.text("report.category", &r.category);
    c.text("report.attack_vector", &r.attack_vector);
    for (i, cwe) in r.cwe.iter().enumerate() {
        let ok = cwe
            .strip_prefix("CWE-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            c.err(format!("report.cwe[{i}]"), "must look like `CWE-306`");
        }
    }
    // Engagement-profile classifications: system + id non-empty (mirrors the
    // schema's `minLength: 1`). The taxonomy itself is open (CVE/CAPEC/ATT&CK/…).
    for (i, cls) in r.classifications.iter().enumerate() {
        if cls.system.is_empty() {
            c.err(
                format!("report.classifications[{i}].system"),
                "must not be empty",
            );
        }
        if cls.id.is_empty() {
            c.err(
                format!("report.classifications[{i}].id"),
                "must not be empty",
            );
        }
    }
    c.text("report.description", &r.description);
    c.text("report.impact", &r.impact);
    c.text("report.location.input", &r.location.input);
    c.text("report.location.output", &r.location.output);
    c.text("report.root_cause", &r.root_cause);

    match &r.call_chain {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.call_chain", NOT_PROVIDED_HINT),
        OrSentinel::Value(hops) if hops.is_empty() => {
            c.err("report.call_chain", "must list at least one hop")
        }
        OrSentinel::Value(hops) => {
            for (i, h) in hops.iter().enumerate() {
                c.text(&format!("report.call_chain[{i}].label"), &h.label);
                c.rel_path(&format!("report.call_chain[{i}].file"), &h.file);
                c.lines(&format!("report.call_chain[{i}].lines"), &h.lines);
            }
        }
    }

    if r.evidence.is_empty() {
        c.err("report.evidence", "must contain at least one claim");
    }
    for (i, e) in r.evidence.iter().enumerate() {
        c.text(&format!("report.evidence[{i}].claim"), &e.claim);
        c.rel_path(&format!("report.evidence[{i}].file"), &e.file);
        c.lines(&format!("report.evidence[{i}].lines"), &e.lines);
    }

    c.text("report.remediation", &r.remediation);

    match &r.recommended_patch {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.recommended_patch", NOT_PROVIDED_HINT),
        OrSentinel::Value(p) => c.text("report.recommended_patch.diff", &p.diff),
    }
    match &r.ci_cd_detection {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.ci_cd_detection", NOT_PROVIDED_HINT),
        OrSentinel::Value(d) => {
            c.text("report.ci_cd_detection.stage", &d.stage);
            c.text("report.ci_cd_detection.body", &d.body);
            c.opt_text("report.ci_cd_detection.command", &d.command);
            c.text("report.ci_cd_detection.expect", &d.expect);
        }
    }
    match &r.regression_test {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.regression_test", NOT_PROVIDED_HINT),
        OrSentinel::Value(t) => {
            c.text("report.regression_test.body", &t.body);
            c.text("report.regression_test.command", &t.command);
            c.text(
                "report.regression_test.expect_vulnerable",
                &t.expect_vulnerable,
            );
            c.text("report.regression_test.expect_patched", &t.expect_patched);
        }
    }

    if r.replication_steps.is_empty() {
        c.err("report.replication_steps", "must contain at least one step");
    }
    for (i, s) in r.replication_steps.iter().enumerate() {
        c.text(&format!("report.replication_steps[{i}]"), s);
    }

    match &r.cross_references {
        OrSentinel::Sentinel(s) if s == "None" => {}
        OrSentinel::Sentinel(_) => c.err(
            "report.cross_references",
            "must be `None` or a list of related findings",
        ),
        OrSentinel::Value(list) if list.is_empty() => c.err(
            "report.cross_references",
            "an empty list is not allowed; use `None`",
        ),
        OrSentinel::Value(list) => {
            for (i, x) in list.iter().enumerate() {
                if !ctx.known_finding_ids.iter().any(|k| k == &x.finding_id) {
                    c.err(
                        format!("report.cross_references[{i}].finding_id"),
                        format!(
                            "`{}` is not a finding in this project; use the id a previous report_finding call returned",
                            x.finding_id
                        ),
                    );
                }
            }
        }
    }

    c.text("report.references", &r.references);

    for (i, a) in r.artifacts.iter().enumerate() {
        if let Some(why) = rel_path_problem(&a.path) {
            c.err(format!("report.artifacts[{i}].path"), why);
        } else if crate::report::artifacts::names_workspace_root(&a.path) {
            c.err(
                format!("report.artifacts[{i}].path"),
                "names the workspace root; list the files or directories inside it that prove the finding",
            );
        }
    }

    // A block's file follows the same path rules as `artifacts`; it names
    // exactly one file, which `report_finding` checks against the disk.
    for (i, b) in r.blocks.iter().enumerate() {
        let Some(a) = b.artifact() else { continue };
        let field = format!("report.blocks[{i}].artifact.path");
        if let Some(why) = rel_path_problem(&a.path) {
            c.err(field, why);
        } else if crate::report::artifacts::names_workspace_root(&a.path) {
            c.err(
                field,
                "names the workspace root; name the one file this block shows",
            );
        }
    }

    // Size last, and only when nothing else is wrong: a flood of field
    // errors plus "too big" buries the actionable ones.
    if c.errors.is_empty() {
        let size = serde_json::to_vec(r).map(|v| v.len()).unwrap_or(usize::MAX);
        if size > ctx.max_bytes {
            c.err(
                "report",
                format!(
                    "serialized report is {size} bytes, over the {} byte budget; trim excerpts and move long output into `artifacts`",
                    ctx.max_bytes
                ),
            );
        }
    }

    if c.errors.is_empty() {
        Ok(())
    } else {
        Err(ReportValidationError(c.errors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn ctx() -> ValidateCtx<'static> {
        ValidateCtx {
            known_finding_ids: &[],
            max_bytes: 262_144,
        }
    }

    fn problems(r: &FindingReport) -> Vec<String> {
        match validate_report(r, &ctx()) {
            Ok(()) => vec![],
            Err(e) => e.0.into_iter().map(|f| f.path).collect(),
        }
    }

    #[test]
    fn fixture_is_valid() {
        assert_eq!(problems(&valid()), Vec::<String>::new());
    }

    #[test]
    fn empty_required_strings_are_rejected_and_all_reported_at_once() {
        let mut r = valid();
        r.root_cause = "  ".into();
        r.description = String::new();
        r.ownership.owner = String::new();
        let p = problems(&r);
        assert!(p.contains(&"report.root_cause".to_string()), "{p:?}");
        assert!(p.contains(&"report.description".to_string()), "{p:?}");
        assert!(p.contains(&"report.ownership.owner".to_string()), "{p:?}");
    }

    #[test]
    fn unknown_is_not_accepted_for_patch_ci_or_regression() {
        let mut r = valid();
        r.recommended_patch = OrSentinel::Sentinel("Unknown".into());
        r.ci_cd_detection = OrSentinel::Sentinel("Unknown".into());
        r.regression_test = OrSentinel::Sentinel("Unknown".into());
        let p = problems(&r);
        for f in [
            "report.recommended_patch",
            "report.ci_cd_detection",
            "report.regression_test",
        ] {
            assert!(p.contains(&f.to_string()), "{f} missing from {p:?}");
        }
    }

    #[test]
    fn not_provided_needs_a_justification() {
        let mut r = valid();
        r.regression_test = OrSentinel::Sentinel("Not Provided — ".into());
        assert_eq!(problems(&r), vec!["report.regression_test".to_string()]);
        r.regression_test = OrSentinel::Sentinel("Not Provided —  double space".into());
        assert_eq!(problems(&r), vec!["report.regression_test".to_string()]);
        r.regression_test =
            OrSentinel::Sentinel("Not Provided — requires hardware we do not have".into());
        assert_eq!(problems(&r), Vec::<String>::new());
    }

    #[test]
    fn ticket_sentinels_are_exact() {
        let mut r = valid();
        r.tickets = OrSentinel::Sentinel("none provided".into());
        assert_eq!(problems(&r), vec!["report.tickets".to_string()]);
        r.tickets = OrSentinel::Sentinel("Unknown".into());
        assert_eq!(problems(&r), Vec::<String>::new());
        r.tickets = OrSentinel::Value(vec![]);
        assert_eq!(problems(&r), vec!["report.tickets".to_string()]);
    }

    #[test]
    fn cross_references_accept_only_none_or_known_ids() {
        let mut r = valid();
        r.cross_references = OrSentinel::Sentinel("none".into());
        assert_eq!(problems(&r), vec!["report.cross_references".to_string()]);

        r.cross_references = OrSentinel::Value(vec![CrossRef {
            finding_id: "fnd_UNKNOWN".into(),
            relation: Relation::Sibling,
            note: None,
        }]);
        assert_eq!(
            problems(&r),
            vec!["report.cross_references[0].finding_id".to_string()]
        );

        let known = vec!["fnd_UNKNOWN".to_string()];
        let ok = validate_report(
            &r,
            &ValidateCtx {
                known_finding_ids: &known,
                max_bytes: 262_144,
            },
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn cwe_entries_must_be_cwe_n() {
        let mut r = valid();
        r.cwe = vec!["306".into()];
        assert_eq!(problems(&r), vec!["report.cwe[0]".to_string()]);
    }

    #[test]
    fn paths_must_be_workspace_relative_and_lines_ordered() {
        let mut r = valid();
        r.evidence[0].file = Some("/etc/passwd".into());
        r.evidence[0].lines = Some([10, 2]);
        if let OrSentinel::Value(hops) = &mut r.call_chain {
            hops[0].file = Some("../outside.c".into());
        }
        let p = problems(&r);
        assert!(p.contains(&"report.evidence[0].file".to_string()), "{p:?}");
        assert!(p.contains(&"report.evidence[0].lines".to_string()), "{p:?}");
        assert!(
            p.contains(&"report.call_chain[0].file".to_string()),
            "{p:?}"
        );
    }

    #[test]
    fn lists_that_must_be_non_empty() {
        let mut r = valid();
        r.evidence.clear();
        r.replication_steps.clear();
        r.call_chain = OrSentinel::Value(vec![]);
        let p = problems(&r);
        for f in [
            "report.evidence",
            "report.replication_steps",
            "report.call_chain",
        ] {
            assert!(p.contains(&f.to_string()), "{f} missing from {p:?}");
        }
    }

    #[test]
    fn an_artifact_naming_the_workspace_root_is_rejected() {
        let mut r = valid();
        for p in [".", "./", "./."] {
            r.artifacts = vec![ArtifactRef {
                path: p.into(),
                sha256: String::new(),
                size: 0,
                kind: None,
                stored: None,
                host: None,
            }];
            assert_eq!(
                problems(&r),
                vec!["report.artifacts[0].path".to_string()],
                "{p}"
            );
        }
    }

    #[test]
    fn a_block_artifact_path_follows_the_artifact_path_rules() {
        let img = |path: &str| EvidenceBlock::Image {
            artifact: ArtifactRef {
                path: path.into(),
                sha256: String::new(),
                size: 0,
                kind: None,
                stored: None,
                host: None,
            },
            caption: None,
        };
        let mut r = valid();
        // Block index 1, behind a text block: the field path names the index.
        for bad in ["../x.png", ".", "./"] {
            r.blocks = vec![EvidenceBlock::Text { text: "t".into() }, img(bad)];
            assert_eq!(
                problems(&r),
                vec!["report.blocks[1].artifact.path".to_string()],
                "{bad}"
            );
        }
        r.blocks = vec![
            EvidenceBlock::Text { text: "t".into() },
            img("shots/login.png"),
        ];
        assert_eq!(problems(&r), Vec::<String>::new());
    }

    #[test]
    fn size_budget_is_enforced() {
        let mut r = valid();
        r.description = "x".repeat(300_000);
        assert_eq!(problems(&r), vec!["report".to_string()]);
    }

    #[test]
    fn display_lists_every_problem() {
        let mut r = valid();
        r.root_cause = String::new();
        r.remediation = String::new();
        let msg = validate_report(&r, &ctx()).unwrap_err().to_string();
        assert!(msg.contains("2 problem"), "{msg}");
        assert!(msg.contains("report.root_cause"), "{msg}");
        assert!(msg.contains("report.remediation"), "{msg}");
    }
}
