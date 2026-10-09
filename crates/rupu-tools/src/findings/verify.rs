//! `findings.verify`: record an independent verdict on a finding another
//! run filed.

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_coverage::tools::{verify_finding, VerifyError, VerifyInput};
use rupu_coverage::VerificationStatus;
use serde_json::Value;
use std::time::Instant;

// was `finding.verify`
pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "findings.verify",
    aliases: &[Alias::any("finding.verify")],
    effect: Effect::Record,
    needs: &[Service::Findings],
    uses: &[],
    description: "Record your verdict on a finding that ANOTHER run filed: confirmed (you \
     reproduced or independently established it), disputed (you showed it is wrong), \
     or inconclusive (you could not decide). Your run and agent are recorded \
     automatically. A finding cannot be verified by the run that filed it, and a \
     finding without a full report cannot be verified. A later verdict replaces an \
     earlier one.",
    input_schema: schema,
};

fn schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["finding_id", "status"],
        "properties": {
            "finding_id": {
                "type": "string",
                "description": "The id of the finding to verify (as returned by findings.report)."
            },
            "status": {
                "type": "string",
                "enum": ["confirmed", "disputed", "inconclusive"],
                "description": "Your verdict."
            },
            "notes": {
                "type": "string",
                "description": "What you did and saw: how you reproduced it, or why it does not hold."
            }
        }
    })
}

/// Record an independent verdict on a finding another run filed.
///
/// The verifier's run id and agent name come from the `ToolContext` the
/// runner fills in, NEVER from the tool input: a model that could name its
/// own `by_run` could claim to be any run, which would defeat the
/// self-verification refusal (`verify_finding` compares `by_run` to the run
/// that filed the finding) and the evaluator's independence check. A call
/// from a context with no run id is refused rather than recorded blank.
pub struct FindingsVerifyTool;

#[async_trait]
impl Tool for FindingsVerifyTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let finding_id = input
            .get("finding_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ToolError::InvalidInput("finding_id is required (a non-empty string)".into())
            })?
            .to_string();
        let status = match input.get("status").and_then(Value::as_str) {
            Some("confirmed") => VerificationStatus::Confirmed,
            Some("disputed") => VerificationStatus::Disputed,
            Some("inconclusive") => VerificationStatus::Inconclusive,
            Some(other) => {
                return Err(ToolError::InvalidInput(format!(
                    "status `{other}` is not a verdict; use confirmed, disputed or inconclusive"
                )))
            }
            None => {
                return Err(ToolError::InvalidInput(
                    "status is required (confirmed, disputed or inconclusive)".into(),
                ))
            }
        };
        let notes = match input.get("notes") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ToolError::InvalidInput(
                    "notes must be a string when given".into(),
                ))
            }
        };
        // The verifier's identity is the run executing this call. Any
        // `by_run` / `by_agent` the model put in `input` was not read above
        // and is never consulted.
        let by_run = Some(ctx.identity.run_id.clone())
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| {
                ToolError::Execution(
                    "findings.verify needs the calling run's id, and this context has none; \
                     no verification was recorded"
                        .into(),
                )
            })?;
        let by_agent = Some(ctx.identity.agent.clone()).filter(|a| !a.trim().is_empty());
        let verify = VerifyInput {
            finding_id: finding_id.clone(),
            status,
            by_run,
            by_agent,
            notes,
        };
        // A locked rewrite of the findings ledger: synchronous file I/O, so
        // it runs on the blocking pool like `report_finding`.
        let paths = crate::ledger::paths(ctx);
        let result = tokio::task::spawn_blocking(move || verify_finding(&paths, &verify))
            .await
            .map_err(|join| {
                ToolError::Execution(format!("findings.verify did not complete: {join}"))
            })?;
        match result {
            Ok(()) => Ok(ok(format!(
                "verification recorded: {finding_id} is {}",
                status_word(status)
            ))
            .timed(started)),
            // A refusal the agent can read and act on.
            Err(
                e @ (VerifyError::NotFound
                | VerifyError::Ambiguous
                | VerifyError::NoReport
                | VerifyError::SelfVerification
                | VerifyError::BadStatus
                | VerifyError::MissingVerifier),
            ) => Ok(failed(e.to_string()).timed(started)),
            // The ledger could not be locked or replaced: not the agent's
            // doing, and nothing was recorded.
            Err(VerifyError::Io(e)) => Err(ToolError::Io(e)),
        }
    }
}

fn status_word(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Confirmed => "confirmed",
        VerificationStatus::Disputed => "disputed",
        VerificationStatus::Inconclusive => "inconclusive",
        VerificationStatus::Unverified => "unverified",
    }
}

#[cfg(test)]
mod finding_verify_tests {
    use super::*;
    use rupu_coverage::{
        read_findings, report_finding, CoveragePaths, FindingProfile, FindingReport, FindingScope,
        FindingWriteOptions, ReportFindingInput,
    };
    use rupu_coverage::{Attribution, Surface};

    /// File a full-report finding as `run_id` (the same write path
    /// `report_finding` uses) and return its id.
    fn seed_full(paths: &CoveragePaths, run_id: &str) -> String {
        let report: FindingReport = serde_json::from_str(include_str!(
            "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        let input = ReportFindingInput {
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
            tags: Vec::new(),
        };
        let attribution = Attribution {
            run_id: run_id.into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        };
        let opts = FindingWriteOptions::default().with_profile(FindingProfile::Full);
        report_finding(paths, attribution, input, &opts).unwrap().id
    }

    /// A call from `run_id` / `agent` in workspace `ws`, scoped `t`.
    fn ctx_in(ws: &std::path::Path, run_id: Option<&str>, agent: Option<&str>) -> ToolContext {
        let mut ctx = ToolContext::in_workspace(ws);
        let id = ctx.identity_mut();
        id.run_id = run_id.unwrap_or_default().to_string();
        id.agent = agent.unwrap_or_default().to_string();
        id.scope_name = Some("t".into());
        ctx
    }

    /// The ledger the tool writes for workspace `ws`.
    fn paths_in(ws: &std::path::Path) -> CoveragePaths {
        crate::ledger::paths(&ctx_in(ws, None, None))
    }

    fn verification_of(paths: &CoveragePaths, id: &str) -> Option<rupu_coverage::Verification> {
        read_findings(paths)
            .unwrap()
            .into_iter()
            .find(|f| f.id == id)
            .unwrap()
            .report
            .unwrap()
            .verification
    }

    #[test]
    fn schema_requires_id_and_status_and_offers_only_verdicts() {
        let tool = FindingsVerifyTool;
        assert_eq!(tool.name(), "findings.verify");
        let s = tool.input_schema();
        let req: Vec<&str> = s["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(req, ["finding_id", "status"]);
        let verdicts: Vec<&str> = s["properties"]["status"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(verdicts, ["confirmed", "disputed", "inconclusive"]);
        assert!(s["properties"]["notes"].is_object());
        // The verifier's identity is never an input.
        assert!(s["properties"].get("by_run").is_none());
        assert!(s["properties"].get("by_agent").is_none());
    }

    #[tokio::test]
    async fn confirmed_records_the_verdict_with_the_callers_run_and_agent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let id = seed_full(&paths, "run_A");
        let tool = FindingsVerifyTool;

        let out = tool
            .invoke(
                serde_json::json!({
                    "finding_id": id,
                    "status": "confirmed",
                    "notes": "reproduced with the PoC",
                }),
                &ctx_in(tmp.path(), Some("run_B"), Some("exploit-verifier")),
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        assert!(out.stdout.contains(&id), "{}", out.stdout);

        let v = verification_of(&paths, &id).expect("verification recorded");
        assert_eq!(v.status, VerificationStatus::Confirmed);
        assert_eq!(v.by_run.as_deref(), Some("run_B"));
        assert_eq!(v.by_agent.as_deref(), Some("exploit-verifier"));
        assert_eq!(v.notes.as_deref(), Some("reproduced with the PoC"));
    }

    #[tokio::test]
    async fn a_by_run_smuggled_in_the_input_is_ignored() {
        // The model tries to pose as another run. The identity recorded is
        // the context's, and the context's run is judged for independence.
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let id = seed_full(&paths, "run_A");
        let tool = FindingsVerifyTool;

        // Posing as someone else while really being the filing run: refused.
        let out = tool
            .invoke(
                serde_json::json!({
                    "finding_id": id, "status": "confirmed",
                    "by_run": "run_Z", "by_agent": "someone-else",
                }),
                &ctx_in(tmp.path(), Some("run_A"), Some("reporter")),
            )
            .await
            .unwrap();
        assert!(out.error.is_some(), "self-verification must be refused");
        assert!(verification_of(&paths, &id).is_none());

        // Posing as the filing run while really being run_B: recorded as run_B.
        let out = tool
            .invoke(
                serde_json::json!({
                    "finding_id": id, "status": "disputed",
                    "by_run": "run_A", "by_agent": "someone-else",
                }),
                &ctx_in(tmp.path(), Some("run_B"), Some("exploit-verifier")),
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        let v = verification_of(&paths, &id).unwrap();
        assert_eq!(v.status, VerificationStatus::Disputed);
        assert_eq!(v.by_run.as_deref(), Some("run_B"));
        assert_eq!(v.by_agent.as_deref(), Some("exploit-verifier"));
    }

    #[tokio::test]
    async fn no_run_id_in_the_context_is_refused_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let id = seed_full(&paths, "run_A");
        let before = std::fs::read(&paths.findings).unwrap();
        let tool = FindingsVerifyTool;

        for run_id in [None, Some(""), Some("   ")] {
            let err = tool
                .invoke(
                    serde_json::json!({ "finding_id": id, "status": "confirmed" }),
                    &ctx_in(tmp.path(), run_id, Some("exploit-verifier")),
                )
                .await
                .expect_err("a call with no run id must fail");
            assert!(matches!(err, ToolError::Execution(_)), "{err:?}");
        }
        assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
        assert!(verification_of(&paths, &id).is_none());
    }

    #[tokio::test]
    async fn verifying_a_finding_the_caller_filed_is_a_refusal_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let id = seed_full(&paths, "run_B");
        let before = std::fs::read(&paths.findings).unwrap();
        let tool = FindingsVerifyTool;

        let out = tool
            .invoke(
                serde_json::json!({ "finding_id": id, "status": "confirmed" }),
                &ctx_in(tmp.path(), Some("run_B"), Some("exploit-verifier")),
            )
            .await
            .unwrap();
        let err = out.error.expect("self-verification is a refusal");
        assert!(err.contains("filed it"), "{err}");
        assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    }

    #[tokio::test]
    async fn an_unknown_finding_is_a_refusal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        seed_full(&paths, "run_A");
        let tool = FindingsVerifyTool;

        let out = tool
            .invoke(
                serde_json::json!({ "finding_id": "f_nope", "status": "confirmed" }),
                &ctx_in(tmp.path(), Some("run_B"), None),
            )
            .await
            .unwrap();
        assert!(out.error.unwrap().contains("no finding"));
    }

    #[tokio::test]
    async fn bad_input_is_invalid_input_not_a_write() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let id = seed_full(&paths, "run_A");
        let before = std::fs::read(&paths.findings).unwrap();
        let tool = FindingsVerifyTool;
        let c = ctx_in(tmp.path(), Some("run_B"), Some("exploit-verifier"));

        for bad in [
            // `unverified` is the absence of a verdict, not a verdict.
            serde_json::json!({ "finding_id": id, "status": "unverified" }),
            serde_json::json!({ "finding_id": id, "status": "Confirmed" }),
            serde_json::json!({ "finding_id": id }),
            serde_json::json!({ "status": "confirmed" }),
            serde_json::json!({ "finding_id": "", "status": "confirmed" }),
            serde_json::json!({ "finding_id": "  ", "status": "confirmed" }),
            serde_json::json!({ "finding_id": id, "status": "confirmed", "notes": 7 }),
        ] {
            let err = tool
                .invoke(bad.clone(), &c)
                .await
                .expect_err(&format!("{bad} must be invalid"));
            assert!(matches!(err, ToolError::InvalidInput(_)), "{bad}: {err:?}");
        }
        assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    }
}
