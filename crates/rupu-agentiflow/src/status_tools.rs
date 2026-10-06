//! Live status + steering tools for the agentiflow lead: re-check the goals and
//! the coverage target on demand, and write a standing board directive.
//!
//! The round digest the lead is handed is a snapshot from the START of the
//! round. `goal.status` and `coverage.status` re-run the same pure evaluators
//! ([`GoalEvaluator`], [`CoverageEvaluator`]) over the pooled scope, so they
//! reflect findings and assets the lead (or a unit it just joined) JUST banked.
//! Budget is not a tool here: it is shown in the round digest (a `budget.status`
//! tool is a later plan).
//!
//! `board.directive` is the lead's steering write: a standing
//! [`rupu_fleet::Directive`] that the units' `DirectiveCollector` delivers on
//! every turn.
//!
//! Error discipline matches `tools.rs` / `roster.rs`: a failure on a well-formed
//! call is `Ok(ToolOutput { error: Some(..) })` so the model sees it and can
//! react; `Err(ToolError::InvalidInput)` is reserved for arguments that cannot
//! be parsed. An evaluation error on one goal is reported on THAT goal's row
//! (an `error` field) and never aborts the others.

use crate::coverage::{CoverageEvaluator, CoverageTarget};
use crate::def::Goal;
use crate::goal::GoalEvaluator;
use crate::roster::{done, failed, req_str};
use async_trait::async_trait;
use rupu_coverage::{ActiveSet, CoveragePaths};
use rupu_fleet::{Board, Directive};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::{json, Value};
use std::sync::Arc;

/// The role a `board.directive` is written as.
const LEAD_AUTHOR: &str = "lead";

/// What the status tools share: the definition's goals and coverage target, the
/// pooled scope they are evaluated over, and the board directives go to.
struct StatusCtx {
    goals: Vec<Goal>,
    coverage: Option<CoverageTarget>,
    paths: CoveragePaths,
    active: Arc<ActiveSet>,
    board: Arc<Board>,
}

/// The three status/steering tools (`goal.status`, `coverage.status`,
/// `board.directive`), each holding a clone of the shared context.
///
/// `paths` is the run's POOLED scope (the one its units' findings are merged
/// into), `active` the engagement's active profile set.
pub fn status_tools(
    goals: Vec<Goal>,
    coverage: Option<CoverageTarget>,
    paths: CoveragePaths,
    active: Arc<ActiveSet>,
    board: Arc<Board>,
) -> Vec<Arc<dyn Tool>> {
    let ctx = Arc::new(StatusCtx {
        goals,
        coverage,
        paths,
        active,
        board,
    });
    vec![
        Arc::new(GoalStatus(ctx.clone())),
        Arc::new(CoverageStatus(ctx.clone())),
        Arc::new(BoardDirective(ctx)),
    ]
}

/// An optional string argument: absent, `null` and blank are all `None`; any
/// other non-string value is an error rather than silently ignored.
fn opt_str(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            Ok((!s.is_empty()).then(|| s.to_string()))
        }
        Some(_) => Err(ToolError::InvalidInput(format!(
            "{key} must be a string when given"
        ))),
    }
}

/// Run a blocking evaluation off the async runtime: the evaluators read whole
/// ledgers from disk, which can be large.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ToolOutput> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| failed(format!("status evaluation did not complete: {e}")))
}

// ---- goal.status ------------------------------------------------------------

/// `goal.status` -> `[{id, objective, required, satisfied, current, target,
/// detail}]`, or `{id, objective, required, error}` for a goal that could not be
/// evaluated.
struct GoalStatus(Arc<StatusCtx>);

#[async_trait]
impl Tool for GoalStatus {
    fn name(&self) -> &'static str {
        "goal.status"
    }

    fn description(&self) -> &'static str {
        "Re-check every goal against the pooled evidence right now (findings and \
         assets banked since the round began count). Each goal reports `satisfied`, \
         its `current`/`target` tally and a one-line `detail`; a goal that could not \
         be evaluated carries an `error` instead. Budget is shown in the round \
         digest, not here."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let ctx = self.0.clone();
        let rows = match blocking(move || {
            ctx.goals
                .iter()
                .map(
                    |g| match GoalEvaluator::evaluate(g, &ctx.paths, &ctx.active) {
                        Ok(o) => json!({
                            "id": g.id,
                            "objective": g.objective,
                            "required": g.required,
                            "satisfied": o.met,
                            "current": o.current,
                            "target": o.target,
                            "detail": o.detail,
                        }),
                        Err(e) => json!({
                            "id": g.id,
                            "objective": g.objective,
                            "required": g.required,
                            "error": e.to_string(),
                        }),
                    },
                )
                .collect::<Vec<Value>>()
        })
        .await
        {
            Ok(rows) => rows,
            Err(out) => return Ok(out),
        };
        Ok(done(Value::Array(rows)))
    }
}

// ---- coverage.status --------------------------------------------------------

/// `coverage.status` -> `{reach, depth, fraction, satisfied, per_kind}`, or
/// `{coverage: null}` when the definition has no coverage target.
struct CoverageStatus(Arc<StatusCtx>);

#[async_trait]
impl Tool for CoverageStatus {
    fn name(&self) -> &'static str {
        "coverage.status"
    }

    fn description(&self) -> &'static str {
        "Re-check the engagement coverage target against the pooled assets right \
         now. `reach` is the required fraction and `depth` the rung assets must \
         have reached (null: the ladder's terminal rung); `fraction` is what has \
         been reached so far, `satisfied` whether it meets `reach`, `per_kind` the \
         per-asset-kind fractions. `{coverage: null}` when the flow sets no \
         coverage target. Budget is shown in the round digest, not here."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let Some(target) = self.0.coverage.clone() else {
            return Ok(done(json!({ "coverage": null })));
        };
        let ctx = self.0.clone();
        let evaluated = match blocking(move || {
            CoverageEvaluator::evaluate(&target, &ctx.paths, &ctx.active).map(|o| {
                let per_kind: serde_json::Map<String, Value> = o
                    .per_kind
                    .iter()
                    .map(|(k, f)| (k.clone(), json!(f)))
                    .collect();
                json!({
                    "reach": target.reach,
                    "depth": target.depth,
                    "fraction": o.fraction,
                    "satisfied": o.met,
                    "per_kind": per_kind,
                })
            })
        })
        .await
        {
            Ok(r) => r,
            Err(out) => return Ok(out),
        };
        Ok(match evaluated {
            Ok(v) => done(v),
            Err(e) => failed(format!("could not evaluate coverage: {e}")),
        })
    }
}

// ---- board.directive --------------------------------------------------------

/// `board.directive { body, addressed_to? }` -> `{ok: true}`: write a standing
/// directive authored by the lead.
struct BoardDirective(Arc<StatusCtx>);

#[async_trait]
impl Tool for BoardDirective {
    fn name(&self) -> &'static str {
        "board.directive"
    }

    fn description(&self) -> &'static str {
        "Steer the fleet: write a standing directive to the board. Units see it on \
         every turn for the rest of the run -- there is no retraction, so \
         directives accumulate; post sparingly and keep each short and actionable. \
         Omit `addressed_to` to address everyone; name a unit or role to address only it."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["body"],
            "properties": {
                "body": { "type": "string", "description": "The directive text" },
                "addressed_to": {
                    "type": "string",
                    "description": "A unit name or role to address; omit for everyone"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let body = req_str(&input, "body")?.to_string();
        let addressed_to = opt_str(&input, "addressed_to")?;
        let directive = Directive {
            author: LEAD_AUTHOR.to_string(),
            ts: chrono::Utc::now().to_rfc3339(),
            body,
            addressed_to: addressed_to.clone(),
        };
        Ok(match self.0.board.put_directive(&directive) {
            Ok(()) => done(json!({ "ok": true, "addressed_to": addressed_to })),
            Err(e) => failed(format!("could not write directive: {e}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{AssetSelector, FindingSelector, GoalTarget};
    use chrono::Utc;
    use rupu_coverage::{
        append_record, Asset, Attribution, Coordinate, FindingEvidence, FindingProfile,
        FindingRecord, FindingScope, Ledger, Locator, Severity, Surface,
    };
    use std::collections::BTreeMap;

    fn active() -> ActiveSet {
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".to_string()])
            .unwrap()
    }

    /// "At least `n` findings of any classification."
    fn finding_goal(id: &str, n: u64) -> Goal {
        Goal {
            id: id.into(),
            objective: "Record findings.".into(),
            target: GoalTarget {
                findings: Some(FindingSelector {
                    classification: None,
                }),
                asset: None,
                count_gte: Some(n),
                depth_at_least: None,
                verified: false,
                verify_check: None,
            },
            required: true,
            verify_with: None,
        }
    }

    /// An asset goal over a kind no active profile owns: evaluation errors.
    fn unowned_asset_goal(id: &str) -> Goal {
        Goal {
            id: id.into(),
            objective: "Reach a thing.".into(),
            target: GoalTarget {
                findings: None,
                asset: Some(AssetSelector {
                    kind: "nope:thing".into(),
                    locator: BTreeMap::new(),
                }),
                count_gte: Some(1),
                depth_at_least: Some("tested".into()),
                verified: false,
                verify_check: None,
            },
            required: false,
            verify_with: None,
        }
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        paths: CoveragePaths,
        board_root: std::path::PathBuf,
    }

    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(&tmp.path().join("ws"), "pooled");
        paths.ensure_dir().unwrap();
        let board_root = tmp.path().join("fleet");
        Fx {
            _tmp: tmp,
            paths,
            board_root,
        }
    }

    /// Seed one benign (Summary-profile) finding into the pooled ledger.
    fn seed_finding(paths: &CoveragePaths, id: &str) {
        let rec = FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: "seeded".into(),
            severity: Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_seed".into(),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc::now(),
            profile: FindingProfile::Summary,
            report: None,
        };
        append_record(paths, Ledger::Findings, &rec).unwrap();
    }

    fn tools(fx: &Fx, goals: Vec<Goal>, coverage: Option<CoverageTarget>) -> Vec<Arc<dyn Tool>> {
        status_tools(
            goals,
            coverage,
            fx.paths.clone(),
            Arc::new(active()),
            Arc::new(Board::new(&fx.board_root)),
        )
    }

    async fn call(tools: &[Arc<dyn Tool>], name: &str, input: Value) -> ToolOutput {
        tools
            .iter()
            .find(|t| t.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
            .invoke(input, &ToolContext::default())
            .await
            .unwrap()
    }

    fn json_of(out: &ToolOutput) -> Value {
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        serde_json::from_str(&out.stdout).unwrap()
    }

    #[test]
    fn exposes_the_three_tools() {
        let fx = fx();
        let mut names: Vec<_> = tools(&fx, vec![], None).iter().map(|t| t.name()).collect();
        names.sort_unstable();
        assert_eq!(names, ["board.directive", "coverage.status", "goal.status"]);
    }

    #[tokio::test]
    async fn goal_status_flips_to_satisfied_once_the_finding_is_banked() {
        let fx = fx();
        let ts = tools(&fx, vec![finding_goal("any", 1)], None);

        let before = json_of(&call(&ts, "goal.status", json!({})).await);
        let row = &before.as_array().unwrap()[0];
        assert_eq!(row["id"], "any");
        assert_eq!(row["objective"], "Record findings.");
        assert_eq!(row["satisfied"], false);
        assert_eq!(row["current"], 0);
        assert_eq!(row["target"], 1);
        assert_eq!(row["detail"], "0/1 findings");

        // The evaluator is live: a finding banked AFTER the tools were built
        // (what the lead just recorded) is seen by the very next call.
        seed_finding(&fx.paths, "fnd_a");
        let after = json_of(&call(&ts, "goal.status", json!({})).await);
        let row = &after.as_array().unwrap()[0];
        assert_eq!(row["satisfied"], true);
        assert_eq!(row["current"], 1);
        assert_eq!(row["detail"], "1/1 findings");
    }

    #[tokio::test]
    async fn goal_status_reports_every_goal_and_isolates_an_eval_error() {
        let fx = fx();
        seed_finding(&fx.paths, "fnd_a");
        let ts = tools(
            &fx,
            vec![
                finding_goal("one", 1),
                unowned_asset_goal("broken"),
                finding_goal("two", 2),
            ],
            None,
        );
        let out = call(&ts, "goal.status", json!({})).await;
        // A goal that cannot be evaluated is a row with an `error`, never a
        // tool-level failure.
        let v = json_of(&out);
        let rows = v.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["satisfied"], true);
        assert_eq!(rows[1]["id"], "broken");
        assert!(rows[1]["error"].as_str().unwrap().contains("unowned"));
        assert!(rows[1].get("satisfied").is_none());
        assert_eq!(rows[1]["required"], false);
        // The failing goal did not stop the one after it.
        assert_eq!(rows[2]["id"], "two");
        assert_eq!(rows[2]["satisfied"], false);
        assert_eq!(rows[2]["detail"], "1/2 findings");
    }

    #[tokio::test]
    async fn goal_status_with_no_goals_is_an_empty_list() {
        let fx = fx();
        let v = json_of(&call(&tools(&fx, vec![], None), "goal.status", json!({})).await);
        assert_eq!(v, json!([]));
    }

    #[tokio::test]
    async fn coverage_status_is_null_without_a_target() {
        let fx = fx();
        let v = json_of(&call(&tools(&fx, vec![], None), "coverage.status", json!({})).await);
        assert_eq!(v, json!({ "coverage": null }));
    }

    #[tokio::test]
    async fn coverage_status_reports_reach_depth_and_the_fraction_so_far() {
        let fx = fx();
        let target = CoverageTarget {
            reach: 0.5,
            depth: Some("enumerated".into()),
            kinds: Some(vec!["host".into()]),
        };
        let ts = tools(&fx, vec![], Some(target));

        // Nothing discovered yet: 0/0 is 0.0, never "satisfied".
        let v = json_of(&call(&ts, "coverage.status", json!({})).await);
        assert_eq!(v["reach"], 0.5);
        assert_eq!(v["depth"], "enumerated");
        assert_eq!(v["fraction"], 0.0);
        assert_eq!(v["satisfied"], false);

        // Two hosts, one of them at the required depth.
        let host = |h: &str, depth: Option<&str>| {
            let mut a = Asset::new(
                "network:host",
                Locator(vec![Coordinate::Host(h.into())]),
                h,
                None,
            );
            a.depth = depth.map(String::from);
            serde_json::to_string(&a).unwrap() + "\n"
        };
        std::fs::write(
            &fx.paths.assets,
            host("10.0.0.1", Some("enumerated")) + &host("10.0.0.2", None),
        )
        .unwrap();
        let v = json_of(&call(&ts, "coverage.status", json!({})).await);
        assert_eq!(v["fraction"], 0.5);
        assert_eq!(v["satisfied"], true);
        assert_eq!(v["per_kind"]["network:host"], 0.5);
    }

    #[tokio::test]
    async fn board_directive_writes_a_lead_directive_a_fresh_board_reads() {
        let fx = fx();
        let ts = tools(&fx, vec![], None);
        let out = call(&ts, "board.directive", json!({ "body": "focus on auth" })).await;
        let v = json_of(&out);
        assert_eq!(v["ok"], true);

        let ds = Board::new(&fx.board_root).read_directives().unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].author, "lead");
        assert_eq!(ds[0].body, "focus on auth");
        assert_eq!(ds[0].addressed_to, None);
        assert!(chrono::DateTime::parse_from_rfc3339(&ds[0].ts).is_ok());
    }

    #[tokio::test]
    async fn board_directive_can_be_addressed_and_blank_means_everyone() {
        let fx = fx();
        let ts = tools(&fx, vec![], None);
        call(
            &ts,
            "board.directive",
            json!({ "body": "stand down", "addressed_to": "scanner" }),
        )
        .await;
        call(
            &ts,
            "board.directive",
            json!({ "body": "regroup", "addressed_to": "  " }),
        )
        .await;
        let ds = Board::new(&fx.board_root).read_directives().unwrap();
        assert_eq!(ds[0].addressed_to.as_deref(), Some("scanner"));
        assert_eq!(ds[1].addressed_to, None);
    }

    #[tokio::test]
    async fn board_directive_rejects_a_blank_body_and_a_non_string_target() {
        let fx = fx();
        let ts = tools(&fx, vec![], None);
        let tool = ts.iter().find(|t| t.name() == "board.directive").unwrap();
        for bad in [
            json!({}),
            json!({ "body": "   " }),
            json!({ "body": "x", "addressed_to": 7 }),
        ] {
            let err = tool
                .invoke(bad.clone(), &ToolContext::default())
                .await
                .unwrap_err();
            assert!(matches!(err, ToolError::InvalidInput(_)), "{bad}");
        }
        assert!(Board::new(&fx.board_root)
            .read_directives()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn board_directive_reports_a_store_failure_to_the_model() {
        let fx = fx();
        // The board root's parent is a FILE, so the directive log cannot be created.
        let blocker = fx._tmp.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let ts = status_tools(
            vec![],
            None,
            fx.paths.clone(),
            Arc::new(active()),
            Arc::new(Board::new(blocker.join("fleet"))),
        );
        let out = call(&ts, "board.directive", json!({ "body": "x" })).await;
        let err = out.error.expect("a store failure is reported, not raised");
        assert!(err.starts_with("could not write directive:"), "{err}");
    }
}
