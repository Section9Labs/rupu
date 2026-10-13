//! A flow's live status, as the [`RunStatus`] port behind the lead's
//! `goal.status`, `goal.coverage` and `budget.status` catalog tools
//! (`rupu-tools`, spec W5 §3.3): [`FlowStatus`].
//!
//! The round digest the lead is handed is a snapshot from the START of the
//! round. [`FlowStatus`] re-runs the same pure evaluators ([`GoalEvaluator`],
//! [`CoverageEvaluator`]) over the pooled scope, so the tools reflect findings
//! and assets the lead (or a unit it just joined) JUST banked. The budget is
//! re-read through the SAME [`UsageSource`] the envelope's budget enforcer
//! meters, and staged by a [`BudgetEnforcer`] built from the SAME enforced
//! [`Budget`], so the lead sees the spend and the verdict the next round's
//! check will act on ([`BudgetProbe`]).
//!
//! An evaluation error on one goal is reported on THAT goal's row and never
//! aborts the others.

use crate::budget::{parse_duration, Budget, BudgetEnforcer, BudgetStage, UsageSource};
use crate::coverage::{CoverageEvaluator, CoverageTarget};
use crate::def::Goal;
use crate::goal::{count_matching_findings, requires_verification, GoalEvaluator};
use chrono::{DateTime, Utc};
use rupu_coverage::{read_findings, ActiveSet, CoveragePaths};
use rupu_tools::ports::{
    BudgetReport, BudgetSet, BudgetState, GoalCoverageReport, GoalReport, GoalRow, GoalTally,
    RoundsLine, TokensLine, UsdEnforcement, UsdLine, WallClockLine, WallClockTally,
};
use rupu_tools::{PortError, RunStatus};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A flow's goals and coverage target, the pooled scope they are evaluated
/// over, and its budget's live view: what an agentiflow hands its lead as
/// the run's [`RunStatus`].
pub struct FlowStatus {
    goals: Vec<Goal>,
    coverage: Option<CoverageTarget>,
    paths: CoveragePaths,
    active: Arc<ActiveSet>,
    budget: BudgetProbe,
}

impl FlowStatus {
    /// `paths` is the run's POOLED scope (the one its units' findings are
    /// merged into), `active` the engagement's active profile set, `budget`
    /// the live view of the run's budget (see [`BudgetProbe`]).
    pub fn new(
        goals: Vec<Goal>,
        coverage: Option<CoverageTarget>,
        paths: CoveragePaths,
        active: Arc<ActiveSet>,
        budget: BudgetProbe,
    ) -> Self {
        Self {
            goals,
            coverage,
            paths,
            active,
            budget,
        }
    }
}

impl RunStatus for FlowStatus {
    fn goals(&self) -> Result<GoalReport, PortError> {
        Ok(GoalReport(
            self.goals
                .iter()
                .map(|g| GoalRow {
                    id: g.id.clone(),
                    objective: g.objective.clone(),
                    required: g.required,
                    result: GoalEvaluator::evaluate(g, &self.paths, &self.active)
                        .map(|o| GoalTally {
                            satisfied: o.met,
                            current: o.current,
                            target: o.target,
                            detail: goal_detail(g, &self.paths, &o.detail),
                        })
                        .map_err(|e| e.to_string()),
                })
                .collect(),
        ))
    }

    fn coverage(&self) -> Result<GoalCoverageReport, PortError> {
        let Some(target) = &self.coverage else {
            return Ok(GoalCoverageReport::NoTarget);
        };
        let o = CoverageEvaluator::evaluate(target, &self.paths, &self.active)
            .map_err(|e| PortError::new(format!("could not evaluate coverage: {e}")))?;
        Ok(GoalCoverageReport::Evaluated {
            reach: target.reach,
            depth: target.depth.clone(),
            fraction: o.fraction,
            satisfied: o.met,
            per_kind: o.per_kind,
        })
    }

    fn budget(&self) -> Result<BudgetReport, PortError> {
        Ok(self.budget.report())
    }
}

// ---- the budget's live view -------------------------------------------------

/// The live view of a run's budget that `budget.status` reads.
///
/// It holds the run's own meter and clock, not copies of them, so the report
/// cannot drift from what the envelope enforces:
///
/// - `usage` is the [`UsageSource`] the envelope's enforcer is handed (the
///   ledger-backed one in production), queried fresh on every call;
/// - the stage comes from a [`BudgetEnforcer`] built from the same ENFORCED
///   [`Budget`] the envelope runs, started at the same instant, and is computed
///   from the very spend figures the report prints (one read of the meter);
/// - `round` is the counter the launch site updates as each round begins (it
///   is the round the lead is in, i.e. the rounds already completed, which is
///   what the enforcer was handed at the top of this round), and `now` is the
///   envelope's clock.
///
/// `declared` is the budget as the definition wrote it and `enforced` the one
/// the envelope runs: they differ only when a `usd` cap is dropped because
/// nothing can price the lead's model (the report says so rather than showing a
/// cap that can never fire as if it were live).
pub struct BudgetProbe {
    declared: Option<Budget>,
    enforcer: BudgetEnforcer,
    /// The enforced budget's `soft_at`, so a per-dimension verdict uses the
    /// same threshold as the overall stage.
    soft_at: Option<f64>,
    usd_enforced: bool,
    started: DateTime<Utc>,
    usage: Arc<dyn UsageSource>,
    round: Arc<AtomicU32>,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

/// One read of the meter, replayed to the enforcer so the stage and the
/// reported spend always agree. A dimension the budget does not set is not read
/// (it is `0` here): the enforcer ignores it anyway, and reading it would cost a
/// ledger fold and, for usd on an unpriced model, a spurious "counted as $0"
/// warning for a flow that has no usd budget.
struct SpendSnapshot {
    usd: f64,
    tokens: u64,
}

impl UsageSource for SpendSnapshot {
    fn spent_usd(&self) -> f64 {
        self.usd
    }
    fn spent_tokens(&self) -> u64 {
        self.tokens
    }
}

impl BudgetProbe {
    pub fn new(
        declared: Option<Budget>,
        enforced: Budget,
        started: DateTime<Utc>,
        usage: Arc<dyn UsageSource>,
        round: Arc<AtomicU32>,
        now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    ) -> Self {
        Self {
            declared,
            soft_at: enforced.soft_at,
            usd_enforced: enforced.usd.is_some(),
            enforcer: BudgetEnforcer::new(enforced, started),
            started,
            usage,
            round,
            now,
        }
    }

    /// The `budget.status` report. Reads ledgers from disk: call it off the
    /// async runtime.
    fn report(&self) -> BudgetReport {
        let Some(declared) = self.declared.as_ref().filter(|b| sets_a_dimension(b)) else {
            return BudgetReport::NoBudget;
        };
        let spend = SpendSnapshot {
            usd: if declared.usd.is_some() {
                self.usage.spent_usd()
            } else {
                0.0
            },
            tokens: if declared.tokens.is_some() {
                self.usage.spent_tokens()
            } else {
                0
            },
        };
        let round = self.round.load(Ordering::Relaxed);
        let now = (self.now)();

        // One dimension's own verdict, from the enforcer's own logic (an
        // enforcer over just that dimension), so it can never disagree with the
        // overall stage about what "soft" or "hard" means.
        let state = |only: fn(&mut Budget, &Budget)| -> BudgetState {
            let mut b = Budget {
                soft_at: self.soft_at,
                ..Budget::default()
            };
            only(&mut b, declared);
            stage_state(&BudgetEnforcer::new(b, self.started).stage(&spend, round, now))
        };

        let stage = self.enforcer.stage(&spend, round, now);
        BudgetReport::Set(BudgetSet {
            stage: stage_state(&stage),
            tripped: match &stage {
                BudgetStage::Hard { dimension } => Some(dimension.clone()),
                _ => None,
            },
            soft_at: declared.soft_at,
            usd: declared.usd.map(|cap| UsdLine {
                cap,
                spent: spend.usd,
                enforcement: if self.usd_enforced {
                    UsdEnforcement::Enforced {
                        state: state(|b, d| b.usd = d.usd),
                        remaining: (cap - spend.usd).max(0.0),
                    }
                } else {
                    UsdEnforcement::NotEnforced {
                        note: "not enforced: the lead's model has no price, so spend reads as $0 \
                               (only priced models' spend is counted here); budget.tokens, \
                               budget.rounds and budget.wall_clock still apply"
                            .into(),
                    }
                },
            }),
            tokens: declared.tokens.map(|cap| TokensLine {
                cap,
                spent: spend.tokens,
                remaining: cap.saturating_sub(spend.tokens),
                state: state(|b, d| b.tokens = d.tokens),
            }),
            rounds: declared.rounds.map(|cap| RoundsLine {
                cap,
                elapsed: round,
                remaining: cap.saturating_sub(round),
                state: state(|b, d| b.rounds = d.rounds),
            }),
            wall_clock: declared.wall_clock.as_ref().map(|cap| {
                let elapsed = (now - self.started).num_seconds().max(0);
                WallClockLine {
                    cap: cap.clone(),
                    tally: match parse_duration(cap) {
                        Ok(d) => Ok(WallClockTally {
                            cap_secs: d.num_seconds(),
                            elapsed_secs: elapsed,
                            remaining_secs: (d.num_seconds() - elapsed).max(0),
                            state: state(|b, d| b.wall_clock = d.wall_clock.clone()),
                        }),
                        // `Budget::validate` rejects this at launch; never reached.
                        Err(e) => Err(e),
                    },
                }
            }),
        })
    }
}

/// Whether `b` sets any dimension (a `budget:` block with nothing in it is no
/// budget at all).
fn sets_a_dimension(b: &Budget) -> bool {
    b.usd.is_some() || b.tokens.is_some() || b.rounds.is_some() || b.wall_clock.is_some()
}

fn stage_state(stage: &BudgetStage) -> BudgetState {
    match stage {
        BudgetStage::Ok => BudgetState::Ok,
        BudgetStage::Soft => BudgetState::Soft,
        BudgetStage::Hard { .. } => BudgetState::Hard,
    }
}

/// The line `goal.status` reports for a goal the evaluator scored as `scored`.
///
/// A findings goal that gates on verification (`verified: true`, or a
/// `verify_with`) gets the matched-vs-verified split appended, so the lead can
/// tell "no findings yet" from "findings filed, nobody has verified them" and,
/// in the second case, which agent to dispatch:
/// `... (3 matched, 1 verified; 2 awaiting verification by <agent>)`. Both
/// counts come from ONE read of the ledger, through the evaluator's own counter
/// (gate off, then on), so they cannot disagree with how the goal is scored.
/// Any other goal -- and a goal whose ledger cannot be read, which
/// [`GoalEvaluator::evaluate`] already reported -- keeps `scored` as is.
fn goal_detail(goal: &Goal, paths: &CoveragePaths, scored: &str) -> String {
    let verify_with = goal.verify_with.as_deref();
    let Some(sel) = goal.target.findings.as_ref() else {
        return scored.to_string();
    };
    if !requires_verification(goal.target.verified, verify_with) {
        return scored.to_string();
    }
    let Ok(records) = read_findings(paths) else {
        return scored.to_string();
    };
    let check = goal.target.verify_check.unwrap_or_default();
    let matched = count_matching_findings(&records, sel, false, None, check);
    let verified = count_matching_findings(&records, sel, true, verify_with, check);
    let awaiting = matched.saturating_sub(verified);
    let ask = match (awaiting, verify_with) {
        (0, _) => String::new(),
        (n, Some(agent)) => format!("; {n} awaiting verification by {agent}"),
        (n, None) => format!("; {n} awaiting independent verification"),
    };
    format!("{scored} ({matched} matched, {verified} verified{ask})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{AssetSelector, FindingSelector, GoalTarget};
    use crate::usage::LedgerUsageSource;
    use crate::verify_fixtures::full_finding;
    use chrono::Utc;
    use rupu_config::{ModelPricing, PricingConfig};
    use rupu_coverage::tools::{verify_finding, VerifyInput};
    use rupu_coverage::{
        append_record, Asset, Attribution, Coordinate, FindingEvidence, FindingProfile,
        FindingRecord, FindingScope, Ledger, Locator, Severity, Surface, VerificationStatus,
    };
    use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
    use rupu_tools::{ToolContext, ToolOutput};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicI64;

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

    /// The instant the test run began (the budget's wall-clock zero).
    fn t0() -> DateTime<Utc> {
        "2026-10-06T12:00:00Z".parse().unwrap()
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        paths: CoveragePaths,
        /// The flow's ledger, `<run dir>/usage.jsonl`: the lead's spend and
        /// every unit's.
        lead_usage: PathBuf,
        /// The round the lead is in, as `run_agentiflow` publishes it.
        round: Arc<AtomicU32>,
        /// Seconds past [`t0`] the injected clock reads.
        clock: Arc<AtomicI64>,
    }

    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(&tmp.path().join("ws"), "pooled");
        paths.ensure_dir().unwrap();
        let run_dir = tmp.path().join("af_run");
        std::fs::create_dir_all(&run_dir).unwrap();
        Fx {
            _tmp: tmp,
            paths,
            lead_usage: run_dir.join("usage.jsonl"),
            round: Arc::new(AtomicU32::new(0)),
            clock: Arc::new(AtomicI64::new(0)),
        }
    }

    /// `[pricing.priced."m"]`: $2 / Mtok in, $10 / Mtok out.
    fn pricing() -> PricingConfig {
        let mut cfg = PricingConfig::default();
        cfg.models.entry("priced".into()).or_default().insert(
            "m".into(),
            ModelPricing {
                input_per_mtok: 2.0,
                output_per_mtok: 10.0,
                cached_input_per_mtok: None,
                cache_write_per_mtok: None,
            },
        );
        cfg
    }

    fn ledger_row(id: &str, input: u64, output: u64) -> LedgerRow {
        LedgerRow {
            v: LEDGER_VERSION,
            id: id.into(),
            at: Utc::now(),
            kind: LedgerKind::Turn,
            step_id: None,
            unit_index: None,
            unit_key: None,
            agent_run_id: "ar_1".into(),
            parent_agent_run_id: None,
            transcript: PathBuf::from("/t/x.jsonl"),
            agent: "recon".into(),
            provider: "priced".into(),
            model: "m".into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: 0,
            rollup: false,
        }
    }

    fn append_row(path: &std::path::Path, row: &LedgerRow) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        let mut line = serde_json::to_vec(row).unwrap();
        line.push(b'\n');
        f.write_all(&line).unwrap();
    }

    impl Fx {
        /// The lead spent `input` / `output` tokens on `priced` / `m`.
        fn lead_spends(&self, id: &str, input: u64, output: u64) {
            append_row(&self.lead_usage, &ledger_row(id, input, output));
        }

        /// A unit spent `input` tokens, copied to the flow's ledger as a
        /// rollup row (as `--usage-root` copies it).
        fn unit_spends(&self, _participant: &str, id: &str, input: u64) {
            let row = LedgerRow {
                rollup: true,
                ..ledger_row(id, input, 0)
            };
            append_row(&self.lead_usage, &row);
        }

        /// The ledger-backed meter `run_agentiflow` hands the envelope.
        fn source(&self) -> Arc<LedgerUsageSource> {
            Arc::new(LedgerUsageSource::new(self.lead_usage.clone(), pricing()))
        }

        fn at(&self, round: u32, secs_after_start: i64) {
            self.round.store(round, Ordering::Relaxed);
            self.clock.store(secs_after_start, Ordering::Relaxed);
        }

        fn probe(&self, declared: Option<Budget>, enforced: Budget) -> BudgetProbe {
            let clock = self.clock.clone();
            BudgetProbe::new(
                declared,
                enforced,
                t0(),
                self.source(),
                self.round.clone(),
                Arc::new(move || t0() + chrono::Duration::seconds(clock.load(Ordering::Relaxed))),
            )
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
            tags: Vec::new(),
        };
        append_record(paths, Ledger::Findings, &rec).unwrap();
    }

    /// The lead's status tools over a [`FlowStatus`], built the way a lead
    /// round builds them: catalog bodies over a context carrying the port.
    struct Kit(ToolContext);

    fn kit(status: FlowStatus) -> Kit {
        let mut ctx = ToolContext::default();
        ctx.services.run_status = Some(Arc::new(status));
        Kit(ctx)
    }

    fn tools(fx: &Fx, goals: Vec<Goal>, coverage: Option<CoverageTarget>) -> Kit {
        kit(FlowStatus::new(
            goals,
            coverage,
            fx.paths.clone(),
            Arc::new(active()),
            fx.probe(None, Budget::default()),
        ))
    }

    /// The tools of a run whose definition (and enforcer) carry `budget`.
    fn budget_tools(fx: &Fx, budget: Budget) -> Kit {
        kit(FlowStatus::new(
            vec![],
            None,
            fx.paths.clone(),
            Arc::new(active()),
            fx.probe(Some(budget.clone()), budget),
        ))
    }

    async fn budget_report(ts: &Kit) -> Value {
        json_of(&call(ts, "budget.status", json!({})).await)
    }

    async fn call(kit: &Kit, name: &str, input: Value) -> ToolOutput {
        rupu_tools::bodies::body(name, &kit.0)
            .unwrap_or_else(|| panic!("no tool {name}"))
            .invoke(input, &kit.0)
            .await
            .unwrap()
    }

    fn json_of(out: &ToolOutput) -> Value {
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        serde_json::from_str(&out.stdout).unwrap()
    }

    #[test]
    fn the_legacy_coverage_status_name_resolves_to_goal_coverage_in_a_lead() {
        // W1: the lead's goal-coverage tool was `coverage.status` before it
        // became `goal.coverage`; a lead prompt naming the old tool still
        // reaches it (a flow-lead-scoped alias).
        let fx = fx();
        let ts = tools(&fx, vec![], None);
        let mut reg = rupu_agent::ToolRegistry::with_alias_scope(rupu_tools::AliasScope::FlowLead);
        reg.insert(rupu_tools::bodies::body("goal.coverage", &ts.0).unwrap());
        assert_eq!(reg.get("coverage.status").unwrap().name(), "goal.coverage");
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

    // ---- verification-gated findings goals ---------------------------------

    /// A findings goal over `CWE-94`, needing `n`, gated on verification
    /// through `verified` and/or `verify_with`.
    fn verified_goal(n: u64, verified: bool, verify_with: Option<&str>) -> Goal {
        let mut g = finding_goal("rce", n);
        g.target.findings = Some(FindingSelector {
            classification: Some("CWE-94".into()),
        });
        g.target.verified = verified;
        g.verify_with = verify_with.map(String::from);
        g
    }

    /// File a Full finding (filed by `run_seed`) into the pooled ledger.
    fn file_full(paths: &CoveragePaths, id: &str, classification: &str) {
        append_record(
            paths,
            Ledger::Findings,
            &full_finding(id, "run_seed", classification),
        )
        .unwrap();
    }

    /// Record a verdict through the real `finding.verify` write path.
    fn verify(paths: &CoveragePaths, id: &str, status: VerificationStatus, by: (&str, &str)) {
        verify_finding(
            paths,
            &VerifyInput {
                finding_id: id.into(),
                status,
                by_run: by.0.into(),
                by_agent: Some(by.1.into()),
                notes: None,
            },
        )
        .unwrap();
    }

    async fn only_row(fx: &Fx, goal: Goal) -> Value {
        let v = json_of(&call(&tools(fx, vec![goal], None), "goal.status", json!({})).await);
        v.as_array().unwrap()[0].clone()
    }

    #[tokio::test]
    async fn goal_status_splits_matched_from_verified_and_names_the_verifier() {
        let fx = fx();
        // Three CWE-94 findings, all filed by `run_seed`; one more of another class.
        for id in ["fnd_a", "fnd_b", "fnd_c"] {
            file_full(&fx.paths, id, "CWE-94");
        }
        file_full(&fx.paths, "fnd_other", "CWE-79");
        // a: confirmed by the named verifier, from another run -> verified.
        verify(
            &fx.paths,
            "fnd_a",
            VerificationStatus::Confirmed,
            ("run_v", "verifier"),
        );
        // b: never verified. c: confirmed, but by the wrong agent.
        verify(
            &fx.paths,
            "fnd_c",
            VerificationStatus::Confirmed,
            ("run_v", "somebody-else"),
        );

        let row = only_row(&fx, verified_goal(2, true, Some("verifier"))).await;
        assert_eq!(row["satisfied"], false);
        assert_eq!(row["current"], 1);
        assert_eq!(row["target"], 2);
        assert_eq!(
            row["detail"],
            "1/2 verified findings [CWE-94] \
             (3 matched, 1 verified; 2 awaiting verification by verifier)"
        );

        // Verifying the second one meets the goal, and nothing is left to ask for.
        verify(
            &fx.paths,
            "fnd_c",
            VerificationStatus::Confirmed,
            ("run_v", "verifier"),
        );
        let row = only_row(&fx, verified_goal(2, true, Some("verifier"))).await;
        assert_eq!(row["satisfied"], true);
        assert_eq!(row["current"], 2);
        assert_eq!(
            row["detail"],
            "2/2 verified findings [CWE-94] (3 matched, 2 verified; 1 awaiting verification by verifier)"
        );
    }

    #[tokio::test]
    async fn goal_status_without_a_named_verifier_asks_for_independent_verification() {
        let fx = fx();
        file_full(&fx.paths, "fnd_a", "CWE-94");
        file_full(&fx.paths, "fnd_b", "CWE-94");
        verify(
            &fx.paths,
            "fnd_a",
            VerificationStatus::Confirmed,
            ("run_v", "anyone"),
        );
        let row = only_row(&fx, verified_goal(2, true, None)).await;
        assert_eq!(
            row["detail"],
            "1/2 verified findings [CWE-94] (2 matched, 1 verified; 1 awaiting independent verification)"
        );
    }

    #[tokio::test]
    async fn goal_status_a_verify_with_alone_gates_and_names_the_verifier() {
        let fx = fx();
        file_full(&fx.paths, "fnd_a", "CWE-94");
        // `verified` is false, but naming a verifier is itself a gate.
        let row = only_row(&fx, verified_goal(1, false, Some("verifier"))).await;
        assert_eq!(row["satisfied"], false);
        assert_eq!(
            row["detail"],
            "0/1 verified findings [CWE-94] (1 matched, 0 verified; 1 awaiting verification by verifier)"
        );
    }

    #[tokio::test]
    async fn goal_status_with_everything_verified_has_nothing_left_to_ask_for() {
        let fx = fx();
        file_full(&fx.paths, "fnd_a", "CWE-94");
        verify(
            &fx.paths,
            "fnd_a",
            VerificationStatus::Confirmed,
            ("run_v", "verifier"),
        );
        let row = only_row(&fx, verified_goal(1, true, Some("verifier"))).await;
        assert_eq!(row["satisfied"], true);
        assert_eq!(
            row["detail"],
            "1/1 verified findings [CWE-94] (1 matched, 1 verified)"
        );
    }

    #[tokio::test]
    async fn goal_status_leaves_an_ungated_goal_and_its_detail_alone() {
        let fx = fx();
        file_full(&fx.paths, "fnd_a", "CWE-94");
        // Neither `verified` nor `verify_with`: verification is ignored, and the
        // detail carries no matched/verified split.
        let row = only_row(&fx, verified_goal(1, false, None)).await;
        assert_eq!(row["satisfied"], true);
        assert_eq!(row["detail"], "1/1 findings [CWE-94]");
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
        let v = json_of(&call(&tools(&fx, vec![], None), "goal.coverage", json!({})).await);
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
        let v = json_of(&call(&ts, "goal.coverage", json!({})).await);
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
        let v = json_of(&call(&ts, "goal.coverage", json!({})).await);
        assert_eq!(v["fraction"], 0.5);
        assert_eq!(v["satisfied"], true);
        assert_eq!(v["per_kind"]["network:host"], 0.5);
    }

    // ---- budget.status -------------------------------------------------------

    /// Seeds $5.00 / 1.7M billable tokens: the lead 1M in + 200k out ($2 + $2),
    /// one launched unit 500k in ($1).
    fn seed_spend(fx: &Fx) {
        fx.lead_spends("01A", 1_000_000, 200_000);
        fx.unit_spends("recon#1", "01B", 500_000);
    }

    fn all_caps() -> Budget {
        Budget {
            usd: Some(10.0),
            tokens: Some(10_000_000),
            rounds: Some(5),
            wall_clock: Some("1h".into()),
            soft_at: None,
        }
    }

    fn near(v: &Value, want: f64) {
        let got = v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"));
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }

    #[tokio::test]
    async fn budget_status_reports_spend_against_every_set_cap_while_under_budget() {
        let fx = fx();
        seed_spend(&fx);
        fx.at(1, 600); // one round done, ten minutes in
        let v = budget_report(&budget_tools(&fx, all_caps())).await;

        assert_eq!(v["stage"], "ok");
        assert!(v.get("tripped").is_none());
        near(&v["usd"]["cap"], 10.0);
        near(&v["usd"]["spent"], 5.0);
        near(&v["usd"]["remaining"], 5.0);
        assert_eq!(v["usd"]["enforced"], true);
        assert_eq!(v["usd"]["state"], "ok");
        // Tokens are billable (input + output), the lead's plus the unit's.
        assert_eq!(v["tokens"]["cap"], 10_000_000);
        assert_eq!(v["tokens"]["spent"], 1_700_000);
        assert_eq!(v["tokens"]["remaining"], 8_300_000);
        assert_eq!(v["tokens"]["state"], "ok");
        assert_eq!(v["rounds"]["cap"], 5);
        assert_eq!(v["rounds"]["elapsed"], 1);
        assert_eq!(v["rounds"]["remaining"], 4);
        assert_eq!(v["rounds"]["state"], "ok");
        assert_eq!(v["wall_clock"]["cap"], "1h");
        assert_eq!(v["wall_clock"]["cap_secs"], 3600);
        assert_eq!(v["wall_clock"]["elapsed_secs"], 600);
        assert_eq!(v["wall_clock"]["remaining_secs"], 3000);
        assert_eq!(v["wall_clock"]["state"], "ok");
    }

    #[tokio::test]
    async fn budget_status_reports_only_the_dimensions_the_flow_sets() {
        let fx = fx();
        seed_spend(&fx);
        let only_tokens = Budget {
            tokens: Some(10_000_000),
            ..Budget::default()
        };
        let v = budget_report(&budget_tools(&fx, only_tokens)).await;
        assert_eq!(v["stage"], "ok");
        assert_eq!(v["tokens"]["spent"], 1_700_000);
        for unset in ["usd", "rounds", "wall_clock", "soft_at"] {
            assert!(v.get(unset).is_none(), "{unset} is unset: {v}");
        }
    }

    #[tokio::test]
    async fn budget_status_goes_soft_when_a_dimension_crosses_the_threshold() {
        let fx = fx();
        seed_spend(&fx);
        // 1.7M of 2M tokens is 85%: past the default 80% soft threshold.
        let b = Budget {
            tokens: Some(2_000_000),
            ..all_caps()
        };
        let v = budget_report(&budget_tools(&fx, b)).await;
        assert_eq!(v["stage"], "soft");
        assert!(v.get("tripped").is_none());
        assert_eq!(v["tokens"]["state"], "soft");
        assert_eq!(v["tokens"]["remaining"], 300_000);
        // The others are still comfortably under.
        assert_eq!(v["usd"]["state"], "ok");
        assert_eq!(v["rounds"]["state"], "ok");

        // An explicit `soft_at` moves the line (and is reported).
        let b = Budget {
            tokens: Some(2_000_000),
            soft_at: Some(0.9),
            ..all_caps()
        };
        let v = budget_report(&budget_tools(&fx, b)).await;
        assert_eq!(v["stage"], "ok");
        near(&v["soft_at"], 0.9);
        assert_eq!(v["tokens"]["state"], "ok");
    }

    #[tokio::test]
    async fn budget_status_goes_hard_and_names_the_dimension_that_tripped() {
        let fx = fx();
        seed_spend(&fx);

        // usd: $5.00 spent against a $5.00 cap.
        let b = Budget {
            usd: Some(5.0),
            ..all_caps()
        };
        let v = budget_report(&budget_tools(&fx, b)).await;
        assert_eq!(v["stage"], "hard");
        assert_eq!(v["tripped"], "usd");
        assert_eq!(v["usd"]["state"], "hard");
        near(&v["usd"]["remaining"], 0.0);
        assert_eq!(v["tokens"]["state"], "ok");

        // tokens: 1.7M spent against a 1M cap; `remaining` never goes negative.
        let b = Budget {
            tokens: Some(1_000_000),
            ..all_caps()
        };
        let v = budget_report(&budget_tools(&fx, b)).await;
        assert_eq!(v["stage"], "hard");
        assert_eq!(v["tripped"], "tokens");
        assert_eq!(v["tokens"]["remaining"], 0);

        // rounds and wall-clock, from the injected round counter and clock.
        let ts = budget_tools(&fx, all_caps());
        fx.at(5, 0);
        let v = budget_report(&ts).await;
        assert_eq!(
            (v["stage"].as_str(), v["tripped"].as_str()),
            (Some("hard"), Some("rounds"))
        );
        assert_eq!(v["rounds"]["elapsed"], 5);
        assert_eq!(v["rounds"]["remaining"], 0);
        fx.at(0, 3600);
        let v = budget_report(&ts).await;
        assert_eq!(
            (v["stage"].as_str(), v["tripped"].as_str()),
            (Some("hard"), Some("wall_clock"))
        );
        assert_eq!(v["wall_clock"]["remaining_secs"], 0);
    }

    #[tokio::test]
    async fn budget_status_fails_closed_on_a_zero_cap_like_the_enforcer() {
        let fx = fx();
        let b = Budget {
            tokens: Some(0),
            ..Budget::default()
        };
        let v = budget_report(&budget_tools(&fx, b)).await;
        assert_eq!(v["stage"], "hard");
        assert_eq!(v["tripped"], "tokens");
    }

    #[tokio::test]
    async fn budget_status_agrees_with_the_enforcer_over_the_same_source() {
        // The tool and a separately built enforcer, over the SAME ledger-backed
        // source, must never disagree about the stage.
        let fx = fx();
        seed_spend(&fx);
        let source = fx.source();
        let budgets = [
            all_caps(),
            Budget {
                tokens: Some(2_000_000),
                ..all_caps()
            },
            Budget {
                usd: Some(5.0),
                ..all_caps()
            },
            Budget {
                usd: Some(6.0),
                ..all_caps()
            },
        ];
        for b in budgets {
            let ts = budget_tools(&fx, b.clone());
            for (round, secs) in [(0, 0), (2, 1800), (4, 3000), (5, 3500), (1, 3600)] {
                fx.at(round, secs);
                let now = t0() + chrono::Duration::seconds(secs);
                let want = BudgetEnforcer::new(b.clone(), t0()).stage(&*source, round, now);
                let v = budget_report(&ts).await;
                assert_eq!(
                    v["stage"],
                    stage_state(&want).as_str(),
                    "{b:?} round {round} +{secs}s"
                );
                match want {
                    BudgetStage::Hard { dimension } => assert_eq!(v["tripped"], dimension),
                    _ => assert!(v.get("tripped").is_none()),
                }
            }
        }
    }

    #[tokio::test]
    async fn budget_status_is_live_not_a_snapshot_from_when_the_tools_were_built() {
        let fx = fx();
        let ts = budget_tools(&fx, all_caps());
        let before = budget_report(&ts).await;
        assert_eq!(before["tokens"]["spent"], 0);
        near(&before["usd"]["spent"], 0.0);

        // The lead's own call and a unit's call land AFTER the tools exist.
        seed_spend(&fx);
        let after = budget_report(&ts).await;
        assert_eq!(after["tokens"]["spent"], 1_700_000);
        near(&after["usd"]["spent"], 5.0);
    }

    #[tokio::test]
    async fn budget_status_says_so_when_the_flow_sets_no_budget() {
        let fx = fx();
        seed_spend(&fx);
        let expect = json!({ "budget": null, "detail": "no budget set" });
        // No `budget:` block at all...
        let ts = tools(&fx, vec![], None);
        assert_eq!(budget_report(&ts).await, expect);
        // ...and a `budget:` block that sets nothing.
        let ts = budget_tools(&fx, Budget::default());
        assert_eq!(budget_report(&ts).await, expect);
        let ts = budget_tools(
            &fx,
            Budget {
                soft_at: Some(0.5),
                ..Budget::default()
            },
        );
        assert_eq!(budget_report(&ts).await, expect);
    }

    #[tokio::test]
    async fn budget_status_marks_a_usd_cap_nothing_can_price_as_not_enforced() {
        let fx = fx();
        seed_spend(&fx);
        // The definition set `usd`, but the envelope dropped it (the lead's model
        // has no price): it is declared, not enforced. A $0 cap would otherwise
        // read as hard; here it must not trip the stage.
        let declared = Budget {
            usd: Some(0.0),
            tokens: Some(10_000_000),
            ..Budget::default()
        };
        let enforced = Budget {
            usd: None,
            ..declared.clone()
        };
        let ts = kit(FlowStatus::new(
            vec![],
            None,
            fx.paths.clone(),
            Arc::new(active()),
            fx.probe(Some(declared), enforced),
        ));
        let v = budget_report(&ts).await;
        assert_eq!(v["stage"], "ok");
        assert_eq!(v["usd"]["enforced"], false);
        assert!(v["usd"].get("state").is_none(), "{v}");
        assert!(v["usd"]["note"].as_str().unwrap().contains("not enforced"));
        // The enforced dimensions are reported normally.
        assert_eq!(v["tokens"]["state"], "ok");
    }

    #[tokio::test]
    async fn budget_status_ignores_input_arguments() {
        let fx = fx();
        let ts = budget_tools(&fx, all_caps());
        let out = call(&ts, "budget.status", json!({ "anything": 1 })).await;
        assert!(out.error.is_none(), "{:?}", out.error);
    }

    #[tokio::test]
    async fn budget_status_reads_only_the_dimensions_the_budget_declares() {
        use std::sync::atomic::AtomicUsize;

        /// A meter that counts how often each dimension is read.
        #[derive(Default)]
        struct Counting {
            usd: AtomicUsize,
            tokens: AtomicUsize,
        }
        impl UsageSource for Counting {
            fn spent_usd(&self) -> f64 {
                self.usd.fetch_add(1, Ordering::Relaxed);
                1.0
            }
            fn spent_tokens(&self) -> u64 {
                self.tokens.fetch_add(1, Ordering::Relaxed);
                7
            }
        }

        let fx = fx();
        let reads = |declared: Option<Budget>| {
            let meter = Arc::new(Counting::default());
            let probe = BudgetProbe::new(
                declared.clone(),
                declared.unwrap_or_default(),
                t0(),
                meter.clone(),
                fx.round.clone(),
                Arc::new(t0),
            );
            let report = probe.report().to_json();
            (
                meter.usd.load(Ordering::Relaxed),
                meter.tokens.load(Ordering::Relaxed),
                report,
            )
        };

        // tokens only: usd is never read (so an unpriced model is never warned about).
        let (usd, tokens, report) = reads(Some(Budget {
            tokens: Some(100),
            ..Budget::default()
        }));
        assert_eq!((usd, tokens), (0, 1));
        assert_eq!(report["tokens"]["spent"], 7);
        assert!(report.get("usd").is_none(), "{report}");
        assert_eq!(report["stage"], "ok");

        // usd only: tokens are not folded.
        let (usd, tokens, report) = reads(Some(Budget {
            usd: Some(10.0),
            ..Budget::default()
        }));
        assert_eq!((usd, tokens), (1, 0));
        near(&report["usd"]["spent"], 1.0);

        // rounds / wall-clock only, and no budget at all: the meter is untouched.
        let (usd, tokens, _) = reads(Some(Budget {
            rounds: Some(3),
            wall_clock: Some("1h".into()),
            ..Budget::default()
        }));
        assert_eq!((usd, tokens), (0, 0));
        let (usd, tokens, _) = reads(None);
        assert_eq!((usd, tokens), (0, 0));

        // A skipped dimension never changes the stage: tokens over their cap
        // still trip while usd (unset) is not consulted.
        let (usd, _, report) = reads(Some(Budget {
            tokens: Some(5),
            ..Budget::default()
        }));
        assert_eq!(usd, 0);
        assert_eq!(report["stage"], "hard");
        assert_eq!(report["tripped"], "tokens");
    }
}
