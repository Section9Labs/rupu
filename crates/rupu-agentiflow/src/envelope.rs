//! The envelope round loop: the deterministic supervisor that, once per
//! round, drains the operator's control queue, recaps the operator's
//! messages to the lead, evaluates every goal / coverage / budget
//! stop condition over the evidence on disk, and either winds the flow down or
//! hands the lead a [`Digest`] and lets it run one round.
//!
//! The lead is a [`LeadDriver`] port (the real, LLM-backed driver is Plan 3).
//! The envelope never trusts the lead to say it is done: stop conditions are
//! all computed here, from evidence.
//!
//! # Stop disjunction
//!
//! Checked at the top of every round, **first match wins**, in this order:
//!
//! 1. [`StopReason::GoalsMet`] -- at least one goal is `required`, and every
//!    `required` goal is met. Non-required goals never gate.
//! 2. [`StopReason::CoverageReached`] -- a coverage target is set and met.
//! 3. [`StopReason::BudgetExhausted`] -- the budget is [`BudgetStage::Hard`].
//! 4. [`StopReason::OperatorStop`] -- the control queue held a stop this
//!    round.
//! 5. [`StopReason::Ceiling`] -- `round >= ceiling_rounds`, or the wall-clock
//!    elapsed since `started` is `>= ceiling_wall_clock`.
//!
//! Because the check precedes the lead's turn, a flow whose goals are already
//! met runs zero rounds.
//!
//! # No required goals
//!
//! `GoalsMet` requires **at least one** required goal. A flow with no required
//! goals (empty, or all `required: false`) cannot end as `GoalsMet`: the
//! all-quantifier would be vacuously true and the flow would stop on round 0
//! having done nothing. Such a flow is governed by its coverage target, budget,
//! operator, and ceilings instead. A flow with no required goal, no coverage
//! target, no budget cap, and no ceiling has no automatic stop at all -- that is
//! a caller error (the def validator is where it belongs), and only an operator
//! `stop` message can end it.
//!
//! # Evaluation errors fail closed
//!
//! A goal or coverage evaluation that errors (unreadable ledger, a kind no
//! active profile owns, ...) is reported as **not met**, never as met, so a
//! broken evaluator can never falsely end the flow as a success. The error is
//! logged, handed to the lead that round in [`Digest::warnings`], and surfaced
//! in the wind-down summary; the budget and ceilings still bound the flow. An
//! operator-queue or message-log read failure is likewise logged, warned, and
//! retried next round.

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use rupu_coverage::{ActiveSet, CoveragePaths};

use crate::budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
use crate::coverage::{CoverageEvaluator, CoverageOutcome, CoverageTarget};
use crate::def::Goal;
use crate::goal::{GoalEvaluator, GoalOutcome};
use crate::operator::{Control, OperatorQueue};
use rupu_fleet::{address, Address, Bus, LogLine, LogMessage, MessageKind, OperatorVia};

/// What the envelope tells the lead at the start of a round: where every stop
/// condition stands, plus a recap of what the operator told it.
#[derive(Debug, Clone)]
pub struct Digest {
    /// One outcome per configured goal, in configuration order.
    pub goals: Vec<GoalOutcome>,
    /// The coverage outcome, when a coverage target is configured.
    pub coverage: Option<CoverageOutcome>,
    /// The budget stage. Never `Hard` here: a hard cap stops the flow before
    /// the lead is consulted.
    pub budget: BudgetStage,
    /// True when the budget is `Soft`: the lead should converge (wrap up the
    /// highest-value work) rather than open new lines of inquiry.
    pub converge: bool,
    /// The operator messages delivered to the lead since the previous round,
    /// oldest first. A recap: the lead's collectors hand it each one before
    /// its next model call.
    pub operator_messages: Vec<LogMessage>,
    /// Non-fatal problems found while assessing this round (a goal or coverage
    /// evaluation error, an unreadable operator queue or message log).
    /// Evaluation errors read as "not met" in `goals` / `coverage`; this is how
    /// the lead learns *why* rather than seeing only a bare unmet outcome.
    pub warnings: Vec<String>,
}

/// The context handed to [`LeadDriver::run_round`].
#[derive(Debug, Clone)]
pub struct RoundContext {
    /// Zero-based index of this round (== rounds the lead has already run).
    pub round: u32,
    pub digest: Digest,
}

/// How a lead round ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundOutcome {
    /// The lead finished its turn normally.
    Yielded,
    /// The lead ran out of its per-round turn budget. For the envelope this is
    /// the same as `Yielded` -- the next round continues the work.
    TurnBudgetHit,
    /// The lead's round failed. Recorded in the wind-down summary but it does
    /// **not** itself stop the loop: a flapping lead is bounded by the budget
    /// and ceilings, not by one bad round.
    Error(String),
}

/// The lead port. A synchronous trait: Plan 2 needs no async runtime, so it
/// avoids an `async-trait` dependency. The real Plan-3 driver does its async
/// work (a session turn) internally and blocks until the round completes.
pub trait LeadDriver: Send {
    fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome;
}

/// Why the envelope stopped. See the module docs for the check order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    GoalsMet,
    CoverageReached,
    BudgetExhausted { dimension: String },
    OperatorStop,
    Ceiling,
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::GoalsMet => f.write_str("all required goals met"),
            StopReason::CoverageReached => f.write_str("coverage target reached"),
            StopReason::BudgetExhausted { dimension } => {
                write!(f, "budget exhausted ({dimension})")
            }
            StopReason::OperatorStop => f.write_str("operator requested stop"),
            StopReason::Ceiling => f.write_str("round / wall-clock ceiling reached"),
        }
    }
}

/// The envelope's configuration, distilled from the `AgentiflowDef` by the
/// caller (the budget is passed to [`Envelope::new`] separately).
#[derive(Debug, Clone)]
pub struct EnvelopeConfig {
    pub goals: Vec<Goal>,
    pub coverage: Option<CoverageTarget>,
    /// Stop when this many rounds have run (`round >= ceiling_rounds`).
    pub ceiling_rounds: Option<u32>,
    /// Stop when this much wall-clock time has elapsed since `started`.
    pub ceiling_wall_clock: Option<Duration>,
}

/// The result of a completed envelope run.
#[derive(Debug, Clone)]
pub struct EnvelopeOutcome {
    pub stop: StopReason,
    /// Goal outcomes as of the final evaluation.
    pub goals: Vec<GoalOutcome>,
    /// Coverage outcome as of the final evaluation, when configured.
    pub coverage: Option<CoverageOutcome>,
    /// How many rounds the lead ran.
    pub rounds: u32,
    /// Human-readable wind-down summary: stop reason, per-goal met/unmet,
    /// coverage, rounds, and any lead / evaluation errors.
    pub summary: String,
}

/// One round's evaluation of every stop input.
struct Assessment {
    goals: Vec<GoalOutcome>,
    coverage: Option<CoverageOutcome>,
    budget: BudgetStage,
    /// The control queue held a stop.
    stop: bool,
    operator_messages: Vec<LogMessage>,
    /// Non-fatal problems this round (evaluator / queue / log errors).
    warnings: Vec<String>,
}

pub struct Envelope {
    paths: CoveragePaths,
    active: ActiveSet,
    cfg: EnvelopeConfig,
    budget: BudgetEnforcer,
    operator: OperatorQueue,
    /// The operator's bus on the flow's message space.
    space: Bus,
    /// The last log message the recap has read past.
    recapped: Option<String>,
    started: DateTime<Utc>,
}

impl Envelope {
    /// `space` is the operator's bus on the flow's message space
    /// (`Bus::open(run_dir, address::OPERATOR)`).
    pub fn new(
        paths: CoveragePaths,
        active: ActiveSet,
        cfg: EnvelopeConfig,
        budget: Budget,
        operator: OperatorQueue,
        space: Bus,
        started: DateTime<Utc>,
    ) -> Self {
        // One instant: the budget's wall-clock dimension and the envelope's
        // wall-clock ceiling both measure from `started`, so a caller cannot
        // hand them mismatched zeros.
        let budget = BudgetEnforcer::new(budget, started);
        Self {
            paths,
            active,
            cfg,
            budget,
            operator,
            space,
            recapped: None,
            started,
        }
    }

    /// Run the round loop to a stop and return the wind-down outcome.
    ///
    /// Each round reads `now()` exactly once and uses that instant for both the
    /// budget stage and the wall-clock ceiling, so one round sees one
    /// consistent time. The clock is injected so tests control time.
    ///
    /// This performs **no real sleeping or waiting** between rounds: it is the
    /// pure round loop. The daemon's wait-on-signal between rounds (and the
    /// real async lead) are layered on in Plan 3.
    pub fn run(
        &mut self,
        lead: &mut dyn LeadDriver,
        usage: &dyn UsageSource,
        now: &dyn Fn() -> DateTime<Utc>,
    ) -> EnvelopeOutcome {
        let mut round: u32 = 0;
        let mut lead_errors: u32 = 0;
        let mut last_lead_error: Option<String> = None;

        loop {
            let t = now();
            let a = self.assess(usage, round, t);
            if let Some(stop) = self.decide(&a, round, t) {
                return self.wind_down(stop, a, round, lead_errors, last_lead_error);
            }

            let converge = a.budget == BudgetStage::Soft;
            let ctx = RoundContext {
                round,
                digest: Digest {
                    goals: a.goals,
                    coverage: a.coverage,
                    budget: a.budget,
                    converge,
                    operator_messages: a.operator_messages,
                    warnings: a.warnings,
                },
            };
            if let RoundOutcome::Error(msg) = lead.run_round(&ctx) {
                tracing::warn!(round, error = %msg, "agentiflow lead round failed");
                lead_errors += 1;
                last_lead_error = Some(format!("round {round}: {msg}"));
            }
            round = round.saturating_add(1);
        }
    }

    /// Drain the control queue, recap the operator's messages, and evaluate
    /// goals, coverage, and budget for `round`. Evaluation failures are
    /// reported as "not met" (fail closed) plus a warning; they never abort
    /// the loop.
    fn assess(&mut self, usage: &dyn UsageSource, round: u32, now: DateTime<Utc>) -> Assessment {
        let mut warnings = Vec::new();

        let mut stop = false;
        match self.operator.drain() {
            Ok(controls) => {
                for control in controls {
                    match control {
                        Control::Stop { .. } => stop = true,
                        Control::Steering { ts, body, urgent } => {
                            self.resend_steering(&ts, body, urgent, &mut warnings)
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "agentiflow operator queue drain failed");
                warnings.push(format!("operator queue unreadable: {e}"));
            }
        }
        let operator_messages = self.recap().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "agentiflow message log read failed");
            warnings.push(format!("message log unreadable: {e}"));
            Vec::new()
        });

        let goals = self
            .cfg
            .goals
            .iter()
            .map(
                |g| match GoalEvaluator::evaluate(g, &self.paths, &self.active) {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::warn!(goal = %g.id, error = %e, "agentiflow goal evaluation failed");
                        warnings.push(format!("goal `{}` evaluation error: {e}", g.id));
                        GoalOutcome {
                            id: g.id.clone(),
                            met: false,
                            current: 0,
                            target: g.target.count_gte.unwrap_or(1),
                            detail: format!("evaluation error: {e}"),
                        }
                    }
                },
            )
            .collect();

        let coverage = self.cfg.coverage.as_ref().map(|target| {
            match CoverageEvaluator::evaluate(target, &self.paths, &self.active) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(error = %e, "agentiflow coverage evaluation failed");
                    warnings.push(format!("coverage evaluation error: {e}"));
                    CoverageOutcome {
                        met: false,
                        fraction: 0.0,
                        per_kind: Vec::new(),
                    }
                }
            }
        });

        let budget = self.budget.stage(usage, round, now);
        Assessment {
            goals,
            coverage,
            budget,
            stop,
            operator_messages,
            warnings,
        }
    }

    /// Deliver a steering message an older binary queued to the lead the way
    /// `rupu agentiflow send` now does, through the flow's message space.
    fn resend_steering(&self, ts: &str, body: String, urgent: bool, warnings: &mut Vec<String>) {
        let msg = rupu_fleet::OperatorMessage {
            to: Address::Lead,
            body,
            urgent,
            via: OperatorVia::Cli,
            viewer: None,
            client: None,
        };
        match self.space.operator_send(msg) {
            Ok(receipt) => tracing::info!(
                id = receipt.id(),
                queued_at = ts,
                "re-sent a queued steering message to the lead through the message space"
            ),
            Err(e) => {
                tracing::warn!(error = %e, queued_at = ts, "could not re-send a queued steering message to the lead");
                warnings.push(format!(
                    "an operator message queued at {ts} could not be delivered: {e}"
                ));
            }
        }
    }

    /// The operator messages delivered to the lead that the log holds past
    /// the last recap.
    fn recap(&mut self) -> Result<Vec<LogMessage>, rupu_fleet::BusError> {
        let page = self.space.read_log(self.recapped.as_deref(), usize::MAX)?;
        let mut out = Vec::new();
        for line in page.lines {
            let LogLine::Msg(m) = line else { continue };
            self.recapped = Some(m.id.clone());
            if m.kind == MessageKind::Operator
                && m.delivery.resolved.iter().any(|r| r == address::LEAD)
            {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// The stop disjunction, first match wins. See the module docs.
    fn decide(&self, a: &Assessment, round: u32, now: DateTime<Utc>) -> Option<StopReason> {
        // `cfg.goals` and `a.goals` are index-aligned (one outcome per goal).
        let mut required = self
            .cfg
            .goals
            .iter()
            .zip(&a.goals)
            .filter(|(g, _)| g.required)
            .map(|(_, o)| o.met)
            .peekable();
        // At least one required goal, and all of them met.
        if required.peek().is_some() && required.all(|met| met) {
            return Some(StopReason::GoalsMet);
        }
        if a.coverage.as_ref().is_some_and(|c| c.met) {
            return Some(StopReason::CoverageReached);
        }
        if let BudgetStage::Hard { dimension } = &a.budget {
            return Some(StopReason::BudgetExhausted {
                dimension: dimension.clone(),
            });
        }
        if a.stop {
            return Some(StopReason::OperatorStop);
        }
        let rounds_up = self.cfg.ceiling_rounds.is_some_and(|cap| round >= cap);
        let time_up = self
            .cfg
            .ceiling_wall_clock
            .is_some_and(|cap| now - self.started >= cap);
        if rounds_up || time_up {
            return Some(StopReason::Ceiling);
        }
        None
    }

    /// Build the wind-down outcome and its summary.
    fn wind_down(
        &self,
        stop: StopReason,
        a: Assessment,
        rounds: u32,
        lead_errors: u32,
        last_lead_error: Option<String>,
    ) -> EnvelopeOutcome {
        let mut summary = format!("Stopped: {stop} after {rounds} round(s).");
        if !a.goals.is_empty() {
            summary.push_str("\nGoals:");
            for (g, o) in self.cfg.goals.iter().zip(&a.goals) {
                summary.push_str(&format!(
                    "\n  - {} [{}]: {} ({})",
                    o.id,
                    if g.required { "required" } else { "optional" },
                    if o.met { "met" } else { "unmet" },
                    o.detail,
                ));
            }
        }
        if let Some(c) = &a.coverage {
            summary.push_str(&format!(
                "\nCoverage: {:.1}% ({})",
                c.fraction * 100.0,
                if c.met { "reached" } else { "not reached" },
            ));
        }
        if lead_errors > 0 {
            summary.push_str(&format!("\nLead round errors: {lead_errors}"));
            if let Some(last) = last_lead_error {
                summary.push_str(&format!(" (last: {last})"));
            }
        }
        for w in &a.warnings {
            summary.push_str(&format!("\nWarning: {w}"));
        }
        EnvelopeOutcome {
            stop,
            goals: a.goals,
            coverage: a.coverage,
            rounds,
            summary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{AssetSelector, GoalTarget};
    use chrono::TimeZone;
    use rupu_coverage::{Asset, Coordinate, Locator};
    use std::cell::Cell;

    // ---- mocks -----------------------------------------------------------

    /// Counts rounds and always yields.
    struct MockLead {
        rounds: u32,
    }
    impl LeadDriver for MockLead {
        fn run_round(&mut self, _ctx: &RoundContext) -> RoundOutcome {
            self.rounds += 1;
            RoundOutcome::Yielded
        }
    }

    /// Records every digest it is handed.
    #[derive(Default)]
    struct CapturingLead {
        seen: Vec<RoundContext>,
    }
    impl LeadDriver for CapturingLead {
        fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome {
            self.seen.push(ctx.clone());
            RoundOutcome::Yielded
        }
    }

    /// Always errors.
    struct FailingLead {
        rounds: u32,
    }
    impl LeadDriver for FailingLead {
        fn run_round(&mut self, _ctx: &RoundContext) -> RoundOutcome {
            self.rounds += 1;
            RoundOutcome::Error("boom".into())
        }
    }

    struct FixedUsage {
        usd: f64,
        tokens: u64,
    }
    impl UsageSource for FixedUsage {
        fn spent_usd(&self) -> f64 {
            self.usd
        }
        fn spent_tokens(&self) -> u64 {
            self.tokens
        }
    }
    const NO_SPEND: FixedUsage = FixedUsage {
        usd: 0.0,
        tokens: 0,
    };

    // ---- fixtures --------------------------------------------------------

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
    }

    fn host_asset(host: &str, depth: Option<&str>) -> Asset {
        let mut a = Asset::new(
            "network:host",
            Locator(vec![Coordinate::Host(host.into())]),
            host,
            None,
        );
        a.depth = depth.map(|d| d.to_string());
        a
    }

    /// A tempdir workspace with `assets` written as the asset ledger. The
    /// operator queue lives under the same tempdir.
    fn seed(assets: &[Asset]) -> (tempfile::TempDir, CoveragePaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(dir.path(), "t");
        paths.ensure_dir().unwrap();
        let body: String = assets
            .iter()
            .map(|a| serde_json::to_string(a).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.assets, body).unwrap();
        (dir, paths)
    }

    fn active() -> ActiveSet {
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".to_string()])
            .unwrap()
    }

    /// A goal met by >= 1 `network:host` asset at depth >= "tested".
    fn host_goal(id: &str, required: bool) -> Goal {
        Goal {
            id: id.into(),
            objective: "reach a host".into(),
            target: GoalTarget {
                findings: None,
                asset: Some(AssetSelector {
                    kind: "network:host".into(),
                    locator: Default::default(),
                }),
                count_gte: Some(1),
                depth_at_least: Some("tested".into()),
                verified: false,
                verify_check: None,
            },
            required,
            verify_with: None,
        }
    }

    fn no_budget() -> Budget {
        Budget {
            usd: None,
            tokens: None,
            wall_clock: None,
            rounds: None,
            soft_at: None,
        }
    }

    fn cfg(goals: Vec<Goal>) -> EnvelopeConfig {
        EnvelopeConfig {
            goals,
            coverage: None,
            ceiling_rounds: None,
            ceiling_wall_clock: None,
        }
    }

    fn envelope(
        dir: &tempfile::TempDir,
        paths: CoveragePaths,
        cfg: EnvelopeConfig,
        budget: Budget,
    ) -> Envelope {
        Envelope::new(
            paths,
            active(),
            cfg,
            budget,
            OperatorQueue::new(dir.path()),
            Bus::open(dir.path(), address::OPERATOR),
            t0(),
        )
    }

    /// The lead, joined to the space under `dir` (as the flow joins it).
    fn lead_bus(dir: &tempfile::TempDir) -> Bus {
        joined(dir, address::LEAD)
    }

    fn joined(dir: &tempfile::TempDir, participant: &str) -> Bus {
        let bus = Bus::open(dir.path(), participant);
        bus.join(rupu_fleet::Member {
            codename: None,
            agent: participant.into(),
            run_id: format!("run_{participant}"),
            step_id: None,
            unit: None,
            kind: "flow_lead".into(),
            parent_run_id: None,
            pid: None,
        })
        .unwrap();
        bus
    }

    fn operator_send(dir: &std::path::Path, to: Address, body: &str) {
        Bus::open(dir, address::OPERATOR)
            .operator_send(rupu_fleet::OperatorMessage {
                to,
                body: body.into(),
                urgent: false,
                via: OperatorVia::Cli,
                viewer: None,
                client: None,
            })
            .unwrap();
    }

    fn bodies(ms: &[LogMessage]) -> Vec<&str> {
        ms.iter().map(|m| m.body.as_str()).collect()
    }

    /// A frozen clock.
    fn fixed() -> impl Fn() -> DateTime<Utc> {
        || t0()
    }

    // ---- tests -----------------------------------------------------------

    #[test]
    fn stops_when_all_required_goals_met() {
        let (dir, paths) = seed(&[host_asset("10.0.0.1", Some("exploited"))]);
        // An unmet optional goal must not gate the stop.
        let c = cfg(vec![host_goal("g1", true), {
            let mut g = host_goal("g2", false);
            g.target.depth_at_least = Some("exploited".into());
            g.target.count_gte = Some(99);
            g
        }]);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::GoalsMet);
        assert_eq!(lead.rounds, 0, "stop is checked before the lead runs");
        assert_eq!(out.rounds, 0);
        assert!(out.goals[0].met && !out.goals[1].met);
        assert!(
            out.summary.contains("all required goals met"),
            "{}",
            out.summary
        );
        assert!(
            out.summary.contains("g1 [required]: met"),
            "{}",
            out.summary
        );
        assert!(
            out.summary.contains("g2 [optional]: unmet"),
            "{}",
            out.summary
        );
    }

    #[test]
    fn stops_on_budget_hard_cap() {
        let (dir, paths) = seed(&[]);
        let budget = Budget {
            usd: Some(10.0),
            ..no_budget()
        };
        let mut env = envelope(&dir, paths, cfg(vec![host_goal("g1", true)]), budget);
        let mut lead = MockLead { rounds: 0 };
        let usage = FixedUsage {
            usd: 12.0,
            tokens: 0,
        };
        let out = env.run(&mut lead, &usage, &fixed());
        assert_eq!(
            out.stop,
            StopReason::BudgetExhausted {
                dimension: "usd".into()
            }
        );
        assert_eq!(lead.rounds, 0);
        assert!(
            out.summary.contains("budget exhausted (usd)"),
            "{}",
            out.summary
        );
    }

    #[test]
    fn stops_on_operator_stop_message() {
        let (dir, paths) = seed(&[]);
        let mut env = envelope(&dir, paths, cfg(vec![host_goal("g1", true)]), no_budget());
        OperatorQueue::new(dir.path()).request_stop().unwrap();
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::OperatorStop);
        assert_eq!(lead.rounds, 0);
    }

    #[test]
    fn stops_on_round_ceiling() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(3);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(lead.rounds, 3);
        assert_eq!(out.rounds, 3);
        assert!(out.summary.contains("after 3 round(s)"), "{}", out.summary);
        assert!(
            out.summary.contains("g1 [required]: unmet"),
            "{}",
            out.summary
        );
    }

    #[test]
    fn soft_budget_sets_converge_in_digest_without_stopping() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(2);
        let budget = Budget {
            usd: Some(100.0),
            soft_at: Some(0.8),
            ..no_budget()
        };
        let mut env = envelope(&dir, paths, c, budget);
        let mut lead = CapturingLead::default();
        let usage = FixedUsage {
            usd: 85.0,
            tokens: 0,
        };
        let out = env.run(&mut lead, &usage, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(lead.seen.len(), 2, "soft budget does not stop the flow");
        assert!(lead.seen.iter().all(|c| c.digest.converge));
        assert!(lead
            .seen
            .iter()
            .all(|c| c.digest.budget == BudgetStage::Soft));
    }

    #[test]
    fn digest_is_not_converge_when_budget_is_ok() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(1);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = CapturingLead::default();
        env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(lead.seen.len(), 1);
        assert!(!lead.seen[0].digest.converge);
        assert_eq!(lead.seen[0].round, 0);
        assert_eq!(lead.seen[0].digest.goals.len(), 1);
    }

    #[test]
    fn a_queued_steering_message_is_resent_to_the_lead_through_the_space() {
        let (dir, paths) = seed(&[]);
        let lead_bus = lead_bus(&dir);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(2);
        let mut env = envelope(&dir, paths, c, no_budget());
        crate::operator::tests::queue_legacy_steering(dir.path(), "focus on 10.0.0.1", true);
        let mut lead = CapturingLead::default();
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);

        // One operator message to the lead, keeping its `--now` mark...
        let log = lead_bus.read_log(None, usize::MAX).unwrap().lines;
        let [LogLine::Msg(m)] = log.as_slice() else {
            panic!("{log:?}")
        };
        assert_eq!(m.kind, MessageKind::Operator);
        assert_eq!(m.from, "operator");
        assert_eq!(m.body, "focus on 10.0.0.1");
        assert!(m.urgent);
        assert_eq!(m.via, Some(OperatorVia::Cli));
        assert_eq!(m.delivery.resolved, ["lead"]);
        // ...in the lead's inbox, for its collector...
        let inbox = lead_bus.mailbox.drain("lead").unwrap();
        assert_eq!(inbox.len(), 1);
        assert!(inbox[0].urgent);
        // ...recapped in the round it was re-sent before, and only that one.
        assert_eq!(
            bodies(&lead.seen[0].digest.operator_messages),
            ["focus on 10.0.0.1"]
        );
        assert!(lead.seen[1].digest.operator_messages.is_empty());
        assert!(OperatorQueue::new(dir.path()).drain().unwrap().is_empty());
    }

    /// Sends an operator message to the lead, one to everyone and one to a
    /// unit while each listed round runs.
    struct SendingLead {
        dir: std::path::PathBuf,
        during: Vec<u32>,
        seen: Vec<RoundContext>,
    }
    impl LeadDriver for SendingLead {
        fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome {
            self.seen.push(ctx.clone());
            if self.during.contains(&ctx.round) {
                operator_send(&self.dir, Address::Lead, &format!("during {}", ctx.round));
                operator_send(&self.dir, Address::Broadcast, "to everyone");
                operator_send(&self.dir, Address::Named("lynx#1".into()), "to a unit");
            }
            RoundOutcome::Yielded
        }
    }

    #[test]
    fn the_recap_lists_the_operator_messages_to_the_lead_since_the_last_round() {
        let (dir, paths) = seed(&[]);
        let _lead = lead_bus(&dir);
        let _unit = joined(&dir, "lynx#1");
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(3);
        let mut env = envelope(&dir, paths, c, no_budget());
        operator_send(dir.path(), Address::Lead, "before the flow");
        let mut lead = SendingLead {
            dir: dir.path().to_path_buf(),
            during: vec![0],
            seen: Vec::new(),
        };
        env.run(&mut lead, &NO_SPEND, &fixed());
        let recaps: Vec<Vec<&str>> = lead
            .seen
            .iter()
            .map(|c| bodies(&c.digest.operator_messages))
            .collect();
        assert_eq!(
            recaps,
            [
                vec!["before the flow"],
                vec!["during 0", "to everyone"],
                vec![]
            ],
            "the unit's message never reached the lead"
        );
    }

    #[test]
    fn goals_met_wins_over_budget_hard_and_ceiling() {
        let (dir, paths) = seed(&[host_asset("10.0.0.1", Some("tested"))]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(0);
        let budget = Budget {
            usd: Some(1.0),
            ..no_budget()
        };
        let mut env = envelope(&dir, paths, c, budget);
        let usage = FixedUsage {
            usd: 5.0,
            tokens: 0,
        };
        let out = env.run(&mut MockLead { rounds: 0 }, &usage, &fixed());
        assert_eq!(out.stop, StopReason::GoalsMet);
    }

    #[test]
    fn operator_stop_wins_over_ceiling() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(0);
        let mut env = envelope(&dir, paths, c, no_budget());
        OperatorQueue::new(dir.path()).request_stop().unwrap();
        let out = env.run(&mut MockLead { rounds: 0 }, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::OperatorStop);
    }

    #[test]
    fn budget_hard_wins_over_operator_stop() {
        let (dir, paths) = seed(&[]);
        let budget = Budget {
            usd: Some(1.0),
            ..no_budget()
        };
        let mut env = envelope(&dir, paths, cfg(vec![host_goal("g1", true)]), budget);
        OperatorQueue::new(dir.path()).request_stop().unwrap();
        let usage = FixedUsage {
            usd: 5.0,
            tokens: 0,
        };
        let out = env.run(&mut MockLead { rounds: 0 }, &usage, &fixed());
        assert!(matches!(out.stop, StopReason::BudgetExhausted { .. }));
    }

    #[test]
    fn stops_when_coverage_reached() {
        // Every enumerated-kind asset is at the ladder terminal.
        let (dir, paths) = seed(&[
            host_asset("a", Some("exploited")),
            host_asset("b", Some("exploited")),
        ]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        // g1 is satisfied too; make it unmeetable so coverage is what fires.
        c.goals[0].target.count_gte = Some(99);
        c.coverage = Some(CoverageTarget {
            reach: 0.9,
            depth: None,
            kinds: None,
        });
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::CoverageReached);
        assert_eq!(lead.rounds, 0);
        assert!(out.coverage.as_ref().is_some_and(|c| c.met));
        assert!(
            out.summary.contains("Coverage: 100.0% (reached)"),
            "{}",
            out.summary
        );
    }

    #[test]
    fn no_required_goals_never_declares_goals_met() {
        // The only goal is optional AND already met; with no required goal
        // and no coverage target the flow must not end as GoalsMet on round 0.
        let (dir, paths) = seed(&[host_asset("10.0.0.1", Some("exploited"))]);
        let mut c = cfg(vec![host_goal("g1", false)]);
        c.ceiling_rounds = Some(2);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(lead.rounds, 2);
    }

    #[test]
    fn empty_goals_with_no_coverage_runs_to_the_ceiling() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![]);
        c.ceiling_rounds = Some(1);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(lead.rounds, 1);
    }

    #[test]
    fn stops_on_wall_clock_ceiling_with_an_injected_clock() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_wall_clock = Some(Duration::minutes(25));
        let mut env = envelope(&dir, paths, c, no_budget());
        // Each call to `now()` advances ten minutes: 0, 10, 20, 30.
        let calls = Cell::new(0i64);
        let clock = || {
            let n = calls.get();
            calls.set(n + 1);
            t0() + Duration::minutes(10 * n)
        };
        let mut lead = MockLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &clock);
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(lead.rounds, 3, "elapsed 0/10/20 run; 30 >= 25 stops");
    }

    #[test]
    fn lead_error_is_recorded_but_does_not_stop_the_loop() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(2);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = FailingLead { rounds: 0 };
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(lead.rounds, 2, "an error does not end the loop");
        assert!(
            out.summary.contains("Lead round errors: 2"),
            "{}",
            out.summary
        );
        assert!(out.summary.contains("round 1: boom"), "{}", out.summary);
    }

    #[test]
    fn goal_evaluation_error_fails_closed_as_unmet() {
        // `depth_at_least` names a rung that is not on the network ladder:
        // the evaluator errors. The goal must read as UNMET, not met.
        let (dir, paths) = seed(&[host_asset("10.0.0.1", Some("exploited"))]);
        let mut g = host_goal("g1", true);
        g.target.depth_at_least = Some("not-a-rung".into());
        let mut c = cfg(vec![g]);
        c.ceiling_rounds = Some(1);
        let mut env = envelope(&dir, paths, c, no_budget());
        let out = env.run(&mut MockLead { rounds: 0 }, &NO_SPEND, &fixed());
        assert_eq!(out.stop, StopReason::Ceiling);
        assert!(!out.goals[0].met);
        assert!(out.goals[0].detail.contains("evaluation error"));
        assert!(
            out.summary.contains("g1 [required]: unmet"),
            "{}",
            out.summary
        );
    }

    #[test]
    fn goal_evaluation_error_reaches_the_lead_as_a_digest_warning() {
        // The evaluator errors (`not-a-rung`): the goal reads as unmet AND the
        // lead is told this round, not just in the final summary.
        let (dir, paths) = seed(&[host_asset("10.0.0.1", Some("exploited"))]);
        let mut g = host_goal("g1", true);
        g.target.depth_at_least = Some("not-a-rung".into());
        let mut c = cfg(vec![g]);
        c.ceiling_rounds = Some(2);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = CapturingLead::default();
        let out = env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(lead.seen.len(), 2);
        for ctx in &lead.seen {
            assert!(!ctx.digest.warnings.is_empty(), "{:?}", ctx.digest);
            assert!(
                ctx.digest.warnings.iter().any(|w| w.contains("g1")),
                "warning names the goal: {:?}",
                ctx.digest.warnings
            );
        }
        // Still surfaced in the wind-down summary.
        assert!(out.summary.contains("Warning:"), "{}", out.summary);
        assert!(out.summary.contains("g1"), "{}", out.summary);
    }

    #[test]
    fn coverage_evaluation_error_reaches_the_lead_as_a_digest_warning() {
        // An unreadable asset ledger (a directory where the file should be)
        // makes the coverage evaluation error.
        let (dir, paths) = seed(&[]);
        std::fs::remove_file(&paths.assets).unwrap();
        std::fs::create_dir(&paths.assets).unwrap();
        let mut c = cfg(vec![]);
        c.coverage = Some(CoverageTarget {
            reach: 0.9,
            depth: None,
            kinds: None,
        });
        c.ceiling_rounds = Some(1);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = CapturingLead::default();
        env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(lead.seen.len(), 1);
        assert!(
            lead.seen[0]
                .digest
                .warnings
                .iter()
                .any(|w| w.contains("coverage evaluation error")),
            "{:?}",
            lead.seen[0].digest.warnings
        );
    }

    #[test]
    fn digest_warnings_are_empty_when_every_evaluation_succeeds() {
        let (dir, paths) = seed(&[]);
        let mut c = cfg(vec![host_goal("g1", true)]);
        c.ceiling_rounds = Some(1);
        let mut env = envelope(&dir, paths, c, no_budget());
        let mut lead = CapturingLead::default();
        env.run(&mut lead, &NO_SPEND, &fixed());
        assert_eq!(lead.seen.len(), 1);
        assert!(lead.seen[0].digest.warnings.is_empty());
    }
}
