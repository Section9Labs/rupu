//! `rupu agentiflow run` with a `budget:`, end to end, with the mock provider:
//! the meter (`usage.jsonl` folded by the `LedgerUsageSource`) really enforces
//! `budget.tokens`, and the CLI's layered `[pricing]` config really reaches it,
//! so `budget.usd` prices a user-configured model.
//!
//! Unit tests cover the meter, the envelope and the record separately; only a
//! real launch wires them together (the lead's `on_usage` hook writing the
//! ledger, the source folding it each round, the envelope reading the source,
//! the CLI handing `cfg.pricing` to `run_agentiflow`), so this runs the real
//! `rupu` binary (`RUPU_MOCK_PROVIDER_SCRIPT` replaces every provider the
//! factory builds) and looks at what the run left on disk. The lead never
//! dispatches a unit, so there is no child process.
//!
//! Every lead round replays one scripted turn that bills a known number of
//! tokens, so the spend after N rounds is exact and nothing here depends on
//! timing.

use crate::ENV_LOCK;
use assert_cmd::Command as AssertCommand;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Input tokens one lead round bills (the mock reports them on its one turn).
const INPUT_PER_ROUND: u64 = 100;
/// Output tokens one lead round bills.
const OUTPUT_PER_ROUND: u64 = 50;
/// Billable (input + output) tokens per round.
const TOKENS_PER_ROUND: u64 = INPUT_PER_ROUND + OUTPUT_PER_ROUND;

/// The lead's model, as `agents/lead.md` pins it. Not a model the built-in
/// price table knows, so a price for it can only come from the test's config.
const LEAD_MODEL: &str = "claude-lead-mock";

/// One lead round: a text reply (so the round yields) that bills
/// [`INPUT_PER_ROUND`] + [`OUTPUT_PER_ROUND`] tokens.
fn lead_round() -> String {
    format!(
        r#"[{{ "AssistantText": {{ "text": "Round done.", "stop": "end_turn",
                "input_tokens": {INPUT_PER_ROUND}, "output_tokens": {OUTPUT_PER_ROUND} }} }}]"#
    )
}

/// The lead's definition. The one goal is never met (the lead records
/// nothing), so only the budget or the ceiling can stop the run. `ceiling` is
/// a safety net well above where any test here expects to stop: it keeps a
/// budget that is NOT enforced from running forever, and shows up as a
/// `ceiling` stop reason (a failure) rather than a hang. The envelope tests the
/// budget before the ceiling, so it cannot mask a budget stop.
fn def_yaml(budget: &str, ceiling_rounds: u32) -> String {
    format!(
        "name: acme\n\
         description: A tiny agentiflow for the budget end-to-end test.\n\
         lead: lead\n\
         engagement_profiles: [network]\n\
         goals:\n  \
           - id: any-finding\n    \
             objective: \"Record at least one finding.\"\n    \
             target: {{ findings: {{}}, count_gte: 1 }}\n\
         scope: {{ authorized: true }}\n\
         pool: {{ agents: [lead] }}\n\
         budget: {budget}\n\
         round: {{ lead_max_turns: 4, ceiling: {{ rounds: {ceiling_rounds} }} }}\n"
    )
}

/// An isolated rupu home with `acme` and its lead agent installed, and a
/// project directory (no `.rupu/`) to run from.
struct Fixture {
    _tmp: assert_fs::TempDir,
    global: PathBuf,
    project: PathBuf,
}

/// `budget` is the YAML flow mapping of the def's `budget:` (`{ tokens: 200 }`).
fn fixture(budget: &str, ceiling_rounds: u32) -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.path().join(".rupu");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(global.join("agents")).unwrap();
    std::fs::create_dir_all(global.join("agentiflows")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        global.join("agents/lead.md"),
        format!(
            "---\nname: lead\nprovider: anthropic\nmodel: {LEAD_MODEL}\n---\nYou are the lead.\n"
        ),
    )
    .unwrap();
    std::fs::write(
        global.join("agentiflows/acme.yaml"),
        def_yaml(budget, ceiling_rounds),
    )
    .unwrap();
    Fixture {
        _tmp: tmp,
        global,
        project,
    }
}

impl Fixture {
    /// `rupu --format json agentiflow <args>`, hermetic: this fixture's home
    /// and project, no ambient provider credentials, and no stdin.
    fn rupu(&self, args: &[&str]) -> AssertCommand {
        let mut cmd = AssertCommand::cargo_bin("rupu").unwrap();
        cmd.env("RUPU_HOME", &self.global)
            .env("RUPU_MOCK_PROVIDER_SCRIPT", lead_round())
            .current_dir(&self.project)
            .write_stdin("");
        for var in [
            "RUPU_AUTH_FILE",
            "RUPU_ANTHROPIC_API_KEY",
            "RUPU_OPENAI_API_KEY",
            "RUPU_GEMINI_API_KEY",
            "RUPU_COPILOT_API_KEY",
        ] {
            cmd.env_remove(var);
        }
        cmd.args(["--format", "json", "agentiflow"]).args(args);
        cmd
    }

    /// stdout of a command that must have succeeded, parsed as the JSON
    /// report it prints; the failure message carries both streams.
    fn json(cmd: &mut AssertCommand) -> Value {
        let out = cmd.output().unwrap();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        );
        assert!(out.status.success(), "command failed: {stderr}\n{stdout}");
        serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {stdout}\n{stderr}"))
    }

    /// Launch the run and return the report it prints.
    fn run(&self) -> Value {
        Self::json(&mut self.rupu(&["run", "acme"]))
    }

    /// `agentiflow status <id>`, for the persisted spend.
    fn status(&self, id: &str) -> Value {
        Self::json(&mut self.rupu(&["status", id]))
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.global.join("agentiflows").join(id)
    }
}

/// The lead's usage ledger rows.
fn ledger_rows(run_dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(run_dir.join("usage.jsonl"))
        .unwrap_or_else(|e| panic!("no usage.jsonl under {run_dir:?}: {e}"))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn events(run_dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(run_dir.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// `budget.tokens` enforces for real. A round bills 150 tokens and the cap is
/// 200: the first round ends under it (150 < 200, and under the 80% soft mark
/// of 160), the second ends at 300 and trips it. The ceiling is 6 rounds, so a
/// meter that did not reach the envelope would end the run as `ceiling`.
#[tokio::test(flavor = "multi_thread")]
async fn a_tokens_cap_stops_the_run_with_budget_exhausted() {
    let _guard = ENV_LOCK.lock().await;
    let cap = 200;
    assert!(TOKENS_PER_ROUND < cap && 2 * TOKENS_PER_ROUND >= cap);
    let fx = fixture(&format!("{{ tokens: {cap} }}"), 6);

    let run = fx.run();
    assert_eq!(run["kind"], "agentiflow_run");
    assert_eq!(
        run["stop_reason"], "budget_exhausted:tokens",
        "the tokens cap did not stop the run (a `ceiling` here means it was not enforced): {run}"
    );
    assert_eq!(
        run["rounds"], 2,
        "round 0 under the cap, round 1 over it: {run}"
    );
    let id = run["id"].as_str().unwrap().to_string();
    let dir = fx.run_dir(&id);

    // The lead's spend landed in the run's own ledger: one row per round.
    let rows = ledger_rows(&dir);
    assert_eq!(rows.len(), 2, "one ledger row per lead round: {rows:?}");
    let ledger_tokens: u64 = rows
        .iter()
        .map(|r| r["input_tokens"].as_u64().unwrap() + r["output_tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(ledger_tokens, 2 * TOKENS_PER_ROUND, "{rows:?}");

    // The persisted record agrees: stopped by the budget, spend over the cap.
    let record = rupu_agentiflow::AgentiflowRecord::read(&dir).unwrap();
    assert_eq!(record.status, "completed");
    assert_eq!(
        record.stop_reason.as_deref(),
        Some("budget_exhausted:tokens")
    );
    assert_eq!(record.rounds, 2);
    assert_eq!(record.spent_tokens, 2 * TOKENS_PER_ROUND);
    assert!(record.spent_tokens >= cap, "{record:?}");

    // The event stream says so too, and the last round reports the spend.
    let evs = events(&dir);
    let stopped = evs.last().unwrap();
    assert_eq!(stopped["kind"], "run_stopped", "{evs:?}");
    assert_eq!(stopped["stop_reason"], "budget_exhausted:tokens", "{evs:?}");
    let rounds: Vec<&Value> = evs.iter().filter(|e| e["kind"] == "round").collect();
    assert_eq!(rounds.len(), 2, "{evs:?}");
    assert_eq!(rounds[0]["spent_tokens"], TOKENS_PER_ROUND, "{evs:?}");
    assert_eq!(rounds[1]["spent_tokens"], 2 * TOKENS_PER_ROUND, "{evs:?}");

    // `status` reports the same spend and the cap it ran against.
    let status = fx.status(&id);
    assert_eq!(status["stop_reason"], "budget_exhausted:tokens", "{status}");
    assert_eq!(
        status["budget"]["spent_tokens"],
        2 * TOKENS_PER_ROUND,
        "{status}"
    );
    assert_eq!(status["budget"]["caps"]["tokens"], cap, "{status}");
}

/// The control for the cap above: with a cap the lead never reaches, the same
/// run is stopped only by its ceiling, and its spend is exactly what the
/// rounds billed. A tokens cap that "worked" because the meter over-counted
/// (or tripped on every run) would fail here.
#[tokio::test(flavor = "multi_thread")]
async fn a_tokens_cap_above_the_spend_leaves_the_ceiling_to_stop_the_run() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture("{ tokens: 1000000 }", 3);

    let run = fx.run();
    assert_eq!(run["stop_reason"], "ceiling", "{run}");
    assert_eq!(run["rounds"], 3, "{run}");
    let id = run["id"].as_str().unwrap();
    let dir = fx.run_dir(id);

    assert_eq!(ledger_rows(&dir).len(), 3);
    let record = rupu_agentiflow::AgentiflowRecord::read(&dir).unwrap();
    assert_eq!(record.spent_tokens, 3 * TOKENS_PER_ROUND);
}

/// The pin for the CLI's pricing wiring: `rupu agentiflow run` hands the
/// layered `[pricing]` config to the meter, so a model the config prices
/// accrues USD (and `budget.usd` can enforce on it). The same run without the
/// price meters `$0`, which is what shows the config, and nothing else, made
/// the difference.
#[tokio::test(flavor = "multi_thread")]
async fn a_configured_price_reaches_the_meter_through_the_cli() {
    let _guard = ENV_LOCK.lock().await;
    // $3 / Mtok in, $15 / Mtok out: 100 * 3e-6 + 50 * 15e-6 = 1.05e-3 per round.
    let usd_per_round = 1.05e-3;

    // ---- unpriced: the control ----------------------------------------------
    let unpriced = fixture("{ tokens: 1000000 }", 2);
    let run = unpriced.run();
    assert_eq!(run["rounds"], 2, "{run}");
    let record =
        rupu_agentiflow::AgentiflowRecord::read(&unpriced.run_dir(run["id"].as_str().unwrap()))
            .unwrap();
    assert_eq!(record.spent_tokens, 2 * TOKENS_PER_ROUND);
    assert_eq!(
        record.spent_usd,
        Some(0.0),
        "a model with no price counts as $0: {record:?}"
    );

    // ---- priced through `$RUPU_HOME/config.toml` ----------------------------
    let priced = fixture("{ tokens: 1000000 }", 2);
    std::fs::write(
        priced.global.join("config.toml"),
        format!(
            "[pricing.anthropic.\"{LEAD_MODEL}\"]\n\
             input_per_mtok = 3.0\n\
             output_per_mtok = 15.0\n"
        ),
    )
    .unwrap();
    let run = priced.run();
    assert_eq!(run["rounds"], 2, "{run}");
    let id = run["id"].as_str().unwrap().to_string();
    let record = rupu_agentiflow::AgentiflowRecord::read(&priced.run_dir(&id)).unwrap();
    assert_eq!(record.spent_tokens, 2 * TOKENS_PER_ROUND);
    let spent = record.spent_usd.expect("a metered run records its USD");
    assert!(
        spent > 0.0,
        "the configured price did not reach the meter: {record:?}"
    );
    assert!(
        (spent - 2.0 * usd_per_round).abs() < 1e-9,
        "two rounds at {usd_per_round} USD each, got {spent}"
    );

    // `status` surfaces it.
    let status = priced.status(&id);
    let status_usd = status["budget"]["spent_usd"].as_f64().unwrap();
    assert!((status_usd - spent).abs() < 1e-12, "{status}");
}

/// `budget.usd` enforces on that price: a cap between one round's spend
/// (1.05e-3) and two rounds' (2.1e-3) lets round 0 finish and trips after
/// round 1.
#[tokio::test(flavor = "multi_thread")]
async fn a_usd_cap_stops_the_run_on_a_configured_price() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture("{ usd: 0.0015 }", 6);
    std::fs::write(
        fx.global.join("config.toml"),
        format!(
            "[pricing.anthropic.\"{LEAD_MODEL}\"]\n\
             input_per_mtok = 3.0\n\
             output_per_mtok = 15.0\n"
        ),
    )
    .unwrap();

    let run = fx.run();
    assert_eq!(
        run["stop_reason"], "budget_exhausted:usd",
        "the usd cap did not stop the run: {run}"
    );
    assert_eq!(run["rounds"], 2, "{run}");
    let record =
        rupu_agentiflow::AgentiflowRecord::read(&fx.run_dir(run["id"].as_str().unwrap())).unwrap();
    assert!(record.spent_usd.unwrap() >= 0.0015, "{record:?}");
}
