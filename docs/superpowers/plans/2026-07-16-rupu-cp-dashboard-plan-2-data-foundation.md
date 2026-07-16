# Dashboard Redesign — Plan 2: Data Foundation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the dashboard a multi-host data foundation: a real `rupu run list --format json` CLI surface, a structured `HostConnector::dashboard_summary()` implemented across transports, a fixed SSH `list_runs` that stops under-reporting, and `/api/dashboard` fanned out across hosts.

**Architecture:** Follows the established structured-method pattern (`list_sessions`, `list_autoflow_runs`, `list_agent_runs`) — a trait method with an `Unsupported` default, implemented per transport, aggregated via the existing `fan_out_via` helper. Deliberately **not** `proxy_get_json`, which is structurally HTTP-only and silently drops 3 of 5 transports.

**Tech Stack:** Rust 2021, tokio, axum, serde/serde_json, thiserror, chrono, tracing. Tests: `#[tokio::test]`, `tempfile`, stub `RemoteExec` impls.

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-cp-dashboard-redesign-design.md`

## Global Constraints

- **Workspace deps only.** Versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files. Do not add a dependency to a crate manifest with a version literal.
- `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`. `unsafe_code` is **forbidden** outside `rupu-keychain-acl`.
- **`rupu-cli` is thin.** Subcommands are arg parsing + delegation. Task 1's `run list` is arg parsing + a `RunStore::list()` call + a serializer — no business logic beyond that.
- **Errors:** `thiserror` for libraries; `anyhow` for the CLI binary.
- **rupu-cp stays read-only.** No write paths in this plan.
- **`Unsupported` must never render as `0`.** A host that cannot report is not a host with no runs. Aggregation keeps per-host contributions `Option`-shaped.
- **Never run package-wide `cargo fmt`** — `main` is fmt-dirty under the pinned toolchain. Format only the files you touch: `cargo fmt -- <path>`.
- **Toolchain note:** this worktree may resolve a newer Homebrew rustc than the pinned 1.88. `rupu-cli`'s baseline may be red for reasons unrelated to your changes. Verify a red test existed before your change before chasing it.

---

### Task 0: Lift trigger classification to `rupu-orchestrator` — ✅ COMPLETE (`5b23d6a`)

**Status: DONE.** Recorded here as the interface contract for Tasks 1 and 3. Do not re-implement.

**Why it existed:** Tasks 1 and 3 both classify how a run was triggered, and the only copy lived at `pub(crate) fn trigger_of` in `crates/rupu-cp/src/api/runs.rs:345` — invisible outside `rupu-cp`. Copying it into each caller would have left three definitions of "what counts as a cron trigger", and the dashboard's cycle grouping depends on that answer being consistent.

**What shipped — and what changed from the original plan.** This task was originally written to add a `RunTrigger` enum. It does not. Implementation surfaced two collisions:

- `rupu_runtime::RunTrigger` (`crates/rupu-runtime/src/run_envelope.rs:39`) — a different, struct-shaped type (`{ source: RunTriggerSource, wake_id, event_id }`) already in wide use across `rupu-runtime` and `rupu-cli`.
- `rupu_orchestrator::workflow::TriggerKind` (`crates/rupu-orchestrator/src/workflow.rs:142`) — already `enum { Manual, Cron, Event }` with `name()` returning the same three strings, **in this very crate**, re-exported at the crate root.

A third enum with identical variants would have been noise, so the enum was dropped. `rupu-orchestrator` gained exactly **one method and zero types**:

```rust
impl RunRecord {
    /// How this run came to exist: `"manual"` | `"cron"` | `"event"`.
    pub fn trigger_str(&self) -> &'static str
}
```

`rupu-cp`'s `trigger_of` is now a one-line wrapper calling `r.trigger_str()`, so its callers (`RunListRow` at `api/runs.rs:456`, and `api/projects.rs:156`) read unchanged.

**Interface for Tasks 1 and 3 — use this exact shape:**

```rust
// Returns &'static str. There is NO enum. Do not add one.
let t: &'static str = record.trigger_str();   // "manual" | "cron" | "event"
if record.trigger_str() == "manual" { /* ... */ }
```

**Invariant that must survive:** precedence is **event-before-wake**. An event-triggered run may also carry a `source_wake_id`; flipping the order silently re-buckets those runs. Covered by `event_wins_over_wake_id` in `crates/rupu-orchestrator/src/runs.rs`.

**Why no enum, recorded so nobody re-adds one:** the two sibling types above already occupy the name and the taxonomy. `rupu_runtime::RunTrigger` is what a launcher *declares* on a `RunEnvelope`; `trigger_str()` is what is *inferred* from a persisted `RunRecord`. They are not interchangeable — `trigger_str()` deliberately cannot express `Autoflow` / `IssueCommand`, because those are not derivable from `event` + `source_wake_id`.

---

### Task 1: `rupu run list --format json` CLI surface

**Why this exists:** The SSH fix (Task 5) must shell a CLI command that emits full `RunRecord` fields. `rupu run list --format json` does not exist today, and the only JSON run-listing surface (`rupu workflow runs`) omits `trigger` and `finished_at` and emits non-RFC-3339 timestamps. See spec §4.3.

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs` (add `RunAction::List` + `classify` arm + `list()` handler + DTOs)
- Modify: `crates/rupu-cli/src/lib.rs:318-322` (relax the format gate for the `list` path only)
- Test: `crates/rupu-cli/src/cmd/run.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `rupu_orchestrator::RunStore::list()`, `rupu_orchestrator::runs::{RunRecord, RunStatus}`, `crate::api::runs::trigger_of` logic (reimplemented locally — see Step 3 note)
- Produces: JSON contract `{"kind":"run_list","version":1,"rows":[RunListJsonRow],"summary":{...}}` consumed verbatim by Task 5's `SshHostConnector::list_runs`. **Row field names are load-bearing across tasks.**

**Critical context — do NOT make `list` a clap subcommand.** `Cmd::Run` is deliberately `trailing_var_arg` (`lib.rs:79-83`); its doc comment records that nesting `pause`/`resume` as clap subcommand variants **broke flag-first invocations** (`rupu run --tmp <ref>`). Dispatch instead through `cmd::run::classify` (`run.rs:177`), which hand-matches `argv.first()`. Add a third arm exactly as `pause`/`resume` do.

**Accepted cost:** `list` becomes a reserved first token, so an agent named `list` is unreachable via `rupu run list`. `pause` and `resume` already carry this cost; this extends the set by one. Note it in the doc comment.

- [ ] **Step 1: Write the failing test for `classify`**

Add to the `#[cfg(test)] mod tests` in `crates/rupu-cli/src/cmd/run.rs`:

```rust
    #[test]
    fn classify_routes_list_to_list_action() {
        let action = classify(vec!["list".to_string()]).unwrap();
        assert!(
            matches!(action, RunAction::List { .. }),
            "`rupu run list` must classify as List, not Launch"
        );
    }

    #[test]
    fn classify_list_accepts_limit_flag() {
        let action = classify(vec!["list".to_string(), "--limit".to_string(), "10".to_string()])
            .unwrap();
        match action {
            RunAction::List { limit, .. } => assert_eq!(limit, 10),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn classify_still_launches_bare_agent_name() {
        let action = classify(vec!["my-agent".to_string()]).unwrap();
        assert!(
            matches!(action, RunAction::Launch(_)),
            "a bare agent name must still Launch — `list` is the only new reserved token"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli classify_routes_list_to_list_action`
Expected: FAIL — compile error, `no variant named 'List' found for enum 'RunAction'`

- [ ] **Step 3: Add the `RunAction::List` variant, DTOs, and `classify` arm**

In `crates/rupu-cli/src/cmd/run.rs`, add to the `RunAction` enum:

```rust
    /// `rupu run list` — enumerate the run store as JSON.
    ///
    /// `list` is a reserved first token, like `pause` / `resume`: an agent
    /// literally named `list` is unreachable via `rupu run list`. This is the
    /// same accepted trade-off those two already carry.
    List { limit: usize, status: Option<String> },
```

Add the `classify` arm, immediately before the `_ => Ok(RunAction::Launch(argv))` fallthrough:

```rust
        Some("list") => {
            #[derive(Parser, Debug)]
            #[command(name = "rupu run list")]
            struct ListArgsParser {
                /// Return at most N runs, newest first.
                #[arg(long, default_value_t = 10_000)]
                limit: usize,
                /// Filter by status (`running`, `completed`, `failed`, …).
                #[arg(long)]
                status: Option<String>,
            }
            let parsed = ListArgsParser::try_parse_from(
                std::iter::once("rupu run list".to_string()).chain(argv.into_iter().skip(1)),
            )?;
            Ok(RunAction::List {
                limit: parsed.limit,
                status: parsed.status,
            })
        }
```

Add the DTOs near the bottom of the file:

```rust
/// One run, carrying every field `rupu-cp`'s `RunListRow` needs.
///
/// Contract note: `started_at` / `finished_at` are `DateTime<Utc>` serialized by
/// serde, which emits RFC-3339 with a `Z` suffix. The fan-out merge in rupu-cp
/// sorts these with a **lexicographic string compare**, so the format must
/// byte-match rupu-cp's `RunListRow`. Two ways to break this, both silent:
/// a human-readable format (`rupu workflow runs` uses `%Y-%m-%d %H:%M:%S`, and
/// its rows consequently cannot be merge-sorted at all), or `.to_rfc3339()`,
/// whose `+00:00` offset sorts before `Z`.
#[derive(serde::Serialize)]
struct RunListJsonRow {
    run_id: String,
    workflow_name: String,
    status: String,
    /// Serialized by serde, NOT `.to_rfc3339()`.
    ///
    /// MUST byte-match how rupu-cp's `RunListRow` serializes this field: rupu-cp
    /// merges local and remote rows with a LEXICOGRAPHIC compare on it
    /// (`sort_values_newest_first`). serde emits `...Z`; `.to_rfc3339()` emits
    /// `...+00:00`, and `'+'` (0x2B) sorts before `'Z'` (0x5A) — so a
    /// to_rfc3339 row silently sorts as OLDER than it is. Do not "tidy" this
    /// into a String.
    started_at: chrono::DateTime<chrono::Utc>,
    /// Same constraint as `started_at`. `None` while the run is non-terminal.
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `"manual"` | `"cron"` | `"event"` — mirrors `rupu-cp`'s `trigger_of`.
    trigger: &'static str,
    workspace_id: Option<String>,
    parent_run_id: Option<String>,
    awaiting_step_id: Option<String>,
    active_step_id: Option<String>,
    error_message: Option<String>,
}

#[derive(serde::Serialize)]
struct RunListSummary {
    count: usize,
    limit: usize,
    status_filter: Option<String>,
}

#[derive(serde::Serialize)]
struct RunListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<RunListJsonRow>,
    summary: RunListSummary,
}
```

Trigger classification comes from **`RunRecord::trigger_str()`** (Task 0), which
returns `&'static str` — `"manual"` / `"cron"` / `"event"`. Do **not** define a
local copy, and do **not** expect an enum: Task 0 deliberately added no new type,
because `rupu_orchestrator::workflow::TriggerKind` and `rupu_runtime::RunTrigger`
already exist and a third would be noise.

- [ ] **Step 4: Run test to verify classify passes**

Run: `cargo test -p rupu-cli classify_`
Expected: PASS (3 tests)

- [ ] **Step 5: Write the failing test for the JSON contract**

```rust
    #[test]
    fn run_list_row_serializes_rfc3339_and_trigger() {
        let row = RunListJsonRow {
            run_id: "run_01".into(),
            workflow_name: "nightly".into(),
            status: "completed".into(),
            started_at: "2026-07-16T14:02:11Z".into(),
            finished_at: Some("2026-07-16T14:09:02Z".into()),
            trigger: "cron",
            workspace_id: Some("ws_1".into()),
            parent_run_id: None,
            awaiting_step_id: None,
            active_step_id: None,
            error_message: None,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["run_id"], "run_01");
        assert_eq!(v["trigger"], "cron");
        // RFC-3339 is required for the lexicographic merge sort in rupu-cp.
        assert!(
            v["started_at"].as_str().unwrap().contains('T'),
            "started_at must be RFC-3339, not space-separated"
        );
    }
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -p rupu-cli run_list_row_serializes`
Expected: FAIL — `RunListJsonRow` not in scope in the test module, or field mismatch

Fix by adding `use super::*;` if absent. Re-run: PASS.

- [ ] **Step 7: Implement the `list()` handler**

Add to `crates/rupu-cli/src/cmd/run.rs`:

```rust
/// `rupu run list` — enumerate the run store.
///
/// Sorts **before** truncating. (`rupu workflow runs` does the reverse —
/// `.take(limit)` on unsorted `store.list()` output — so a small `--limit`
/// there returns an arbitrary subset rather than the newest N. Do not
/// replicate that.)
async fn list(
    limit: usize,
    status: Option<String>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = rupu_workspace::paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));

    let mut all: Vec<_> = store
        .list()?
        .into_iter()
        .filter(|r| match &status {
            None => true,
            Some(s) => r.status.as_str() == s.as_str(),
        })
        .collect();

    all.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    all.truncate(limit);

    let rows: Vec<RunListJsonRow> = all
        .iter()
        .map(|r| RunListJsonRow {
            run_id: r.id.clone(),
            workflow_name: r.workflow_name.clone(),
            status: r.status.as_str().to_string(),
            started_at: r.started_at,
            finished_at: r.finished_at,
            trigger: r.trigger_str(),
            workspace_id: r.workspace_id.clone(),
            parent_run_id: r.parent_run_id.clone(),
            awaiting_step_id: r.awaiting_step_id.clone(),
            active_step_id: r.active_step_id.clone(),
            error_message: r.error_message.clone(),
        })
        .collect();

    let report = RunListReport {
        kind: "run_list",
        version: 1,
        summary: RunListSummary {
            count: rows.len(),
            limit,
            status_filter: status,
        },
        rows,
    };

    // `rupu run` has no table renderer for this view; JSON is the contract
    // consumed by rupu-cp's SshHostConnector::list_runs.
    match global_format.unwrap_or(crate::output::formats::OutputFormat::Table) {
        crate::output::formats::OutputFormat::Json => {
            println!("{}", serde_json::to_string(&report)?);
        }
        _ => {
            for row in &report.rows {
                println!(
                    "{}  {}  {}  {}",
                    row.run_id, row.status, row.trigger, row.started_at
                );
            }
        }
    }
    Ok(())
}
```

Wire it into `handle()`, alongside the existing `Pause`/`Resume` arms:

```rust
        Ok(RunAction::List { limit, status }) => {
            match list(limit, status, global_format).await {
                Ok(()) => ExitCode::from(0),
                Err(e) => crate::output::diag::fail(e),
            }
        }
```

**Note:** `handle()` currently takes only `argv`. It needs `global_format: Option<OutputFormat>` threaded in from `lib.rs`. Update the call site in `lib.rs` (`Cmd::Run { argv } => cmd::run::handle(argv, cli.format).await`) and the signature together.

- [ ] **Step 8: Relax the format gate for the `list` path**

`crates/rupu-cli/src/lib.rs:318-322` currently rejects every non-Table format for all of `Cmd::Run`. Change it to peek at the first token:

```rust
        Cmd::Run { argv } => {
            // `rupu run list` emits JSON (it is rupu-cp's SSH run-listing
            // contract); every other `rupu run` form is Table-only.
            let allowed: &[output::formats::OutputFormat] =
                if argv.first().map(String::as_str) == Some("list") {
                    &[
                        output::formats::OutputFormat::Table,
                        output::formats::OutputFormat::Json,
                    ]
                } else {
                    &[output::formats::OutputFormat::Table]
                };
            output::formats::ensure_supported("run", format, allowed)
        }
```

- [ ] **Step 9: Verify end-to-end against the real store**

Run: `cargo run -q -p rupu-cli -- --format json run list --limit 3 | head -c 400`

**Note the flag order** — `--format` MUST precede `run`. `Cmd::Run` is `trailing_var_arg`, so it swallows everything after `run`; `rupu run list --format json` fails with `unexpected argument '--format' found`.
Expected: JSON beginning `{"kind":"run_list","version":1,"rows":[` with RFC-3339 `started_at` values containing `T` and a `trigger` field on each row.

Run: `cargo run -p rupu-cli -- run --help`
Expected: still works — the format gate change must not break the launcher.

Run: `cargo test -p rupu-cli`
Expected: PASS. Any pre-existing failures unrelated to `run` are the known toolchain baseline (see Global Constraints).

- [ ] **Step 10: Format and commit**

```bash
cargo fmt -- crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/src/lib.rs
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/src/lib.rs
git commit -m "feat(cli): rupu run list --format json

Emits the full RunRecord fields rupu-cp needs (trigger, finished_at,
RFC-3339 timestamps), versioned kind: run_list / version: 1.

Routed through cmd::run::classify like pause/resume rather than a clap
subcommand — nesting broke flag-first invocations (see Cmd::Run doc).
'list' joins pause/resume as a reserved first token.

Sorts before truncating, unlike 'workflow runs' which takes an arbitrary
subset when --limit is small."
```

---

### Task 2: `DashboardSummary` DTOs + `HostConnector::dashboard_summary()` trait method

**Files:**
- Create: `crates/rupu-cp/src/host/dashboard_summary.rs` (DTOs)
- Modify: `crates/rupu-cp/src/host/mod.rs` (add `pub mod dashboard_summary;`)
- Modify: `crates/rupu-cp/src/host/connector.rs` (add the trait method with `Unsupported` default)
- Test: `crates/rupu-cp/src/host/dashboard_summary.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `rupu_orchestrator::runs::RunStatus`, `crate::usage::UsageSummary`, `HostConnectorError` from Task 0 (existing)
- Produces: `DashboardSummary`, `DashboardRange`, `ActiveCounts`, `TerminalBucket`, `ActiveRunBar`, `CycleRollup`, `RecentRun` — consumed by Tasks 3, 4, 5, 6 and serialized to the wire for Plan 1.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/src/host/dashboard_summary.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parses_from_wire_strings() {
        assert_eq!(DashboardRange::parse("7d"), Some(DashboardRange::Days7));
        assert_eq!(DashboardRange::parse("30d"), Some(DashboardRange::Days30));
        assert_eq!(DashboardRange::parse("all"), Some(DashboardRange::All));
        assert_eq!(DashboardRange::parse("bogus"), None);
    }

    #[test]
    fn active_counts_default_to_zero() {
        let a = ActiveCounts::default();
        assert_eq!(a.running, 0);
        assert_eq!(a.awaiting_approval, 0);
        assert_eq!(a.paused, 0);
        assert_eq!(a.pending, 0);
    }

    #[test]
    fn summary_serializes_captured_at_as_rfc3339() {
        let s = DashboardSummary {
            active: ActiveCounts::default(),
            terminal_buckets: vec![],
            active_runs: vec![],
            cycles: vec![],
            recent_manual: vec![],
            findings_open: 0,
            captured_at: chrono::Utc::now(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(
            v["captured_at"].as_str().unwrap().contains('T'),
            "captured_at must be RFC-3339 — the freshness strip parses it"
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p rupu-cp range_parses_from_wire_strings`
Expected: FAIL — `cannot find type 'DashboardRange' in this scope`

- [ ] **Step 3: Implement the DTOs**

Prepend to `crates/rupu-cp/src/host/dashboard_summary.rs`:

```rust
//! DTOs for [`HostConnector::dashboard_summary`].
//!
//! One host's entire contribution to the dashboard, fetched in ONE round-trip.
//! Deliberately coarse: SSH hosts pay a full ssh handshake per call (no
//! ControlMaster multiplexing — see `host/ssh.rs` `RemoteExec::run`), so this
//! must not decompose into per-panel calls.

#![deny(clippy::all)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The dashboard's time window. Mirrors the UI's segmented control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardRange {
    Days7,
    Days30,
    All,
}

impl DashboardRange {
    /// Parse the wire form (`"7d"` / `"30d"` / `"all"`). `None` on anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "7d" => Some(Self::Days7),
            "30d" => Some(Self::Days30),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// The cutoff instant, or `None` for [`DashboardRange::All`].
    pub fn since(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::Days7 => Some(now - chrono::Duration::days(7)),
            Self::Days30 => Some(now - chrono::Duration::days(30)),
            Self::All => None,
        }
    }

    /// CLI flag form, for shelling to a remote host.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::All => "all",
        }
    }
}

impl Default for DashboardRange {
    fn default() -> Self {
        Self::Days30
    }
}

/// Live, non-terminal run counts. These are the states that answer
/// "is anything stuck right now".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActiveCounts {
    pub running: u64,
    pub awaiting_approval: u64,
    pub paused: u64,
    pub pending: u64,
}

/// One time bucket of terminal outcomes, for the trend area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalBucket {
    pub ts: DateTime<Utc>,
    pub completed: u64,
    pub failed: u64,
    pub rejected: u64,
    pub cancelled: u64,
}

/// One bar in the live swimlane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveRunBar {
    pub run_id: String,
    pub workflow_name: String,
    /// `RunStatus::as_str()` form.
    pub status: String,
    pub started_at: DateTime<Utc>,
    /// `"manual"` | `"cron"` | `"event"`.
    pub trigger: String,
    /// `None` for manual runs; set when the run belongs to an autoflow cycle.
    pub cycle_id: Option<String>,
}

/// One run inside a cycle.
///
/// Carries `status`, not just an id, because the `+N clean` pill needs to know
/// what folds. `AutoflowCycleRow` supplies only ids, so the status is joined
/// server-side in `build_summary` — which already holds every run. Making the
/// client fetch a run per id would turn one expanded cycle into N requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRun {
    pub run_id: String,
    /// `RunStatus::as_str()` form. `"unknown"` when the cycle references a run
    /// this host cannot resolve — never silently omitted, or the cycle's run
    /// count would disagree with its own row.
    pub status: String,
}

/// One autoflow cycle, collapsed. The activity feed's primary row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRollup {
    pub cycle_id: String,
    pub worker_name: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub ran: u64,
    pub skipped: u64,
    pub failed: u64,
    pub runs: Vec<CycleRun>,
}

/// A manual-trigger run. Never grouped — always rendered individually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentRun {
    pub id: String,
    pub workflow_name: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub trigger: String,
}

/// One host's complete dashboard contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub active: ActiveCounts,
    pub terminal_buckets: Vec<TerminalBucket>,
    pub active_runs: Vec<ActiveRunBar>,
    pub cycles: Vec<CycleRollup>,
    pub recent_manual: Vec<RecentRun>,
    pub findings_open: u64,
    /// When this host's data was actually read. Drives the per-host freshness
    /// strip — a host 30s stale must not render as "live". Never synthesized
    /// at the aggregation layer; always set by the connector that read it.
    pub captured_at: DateTime<Utc>,
}
```

Register the module in `crates/rupu-cp/src/host/mod.rs`:

```rust
pub mod dashboard_summary;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cp dashboard_summary`
Expected: PASS (3 tests)

- [ ] **Step 5: Add the trait method**

In `crates/rupu-cp/src/host/connector.rs`, add to the `HostConnector` trait, in the defaulted-methods section alongside `list_sessions` / `list_autoflow_runs`:

```rust
    /// Aggregate dashboard state for this host, in ONE round-trip.
    ///
    /// Deliberately coarse. SSH hosts pay a full ssh handshake per call — there
    /// is no ControlMaster multiplexing in `RemoteExec::run` — so this must not
    /// decompose into per-panel calls.
    ///
    /// The default is `Unsupported`, and callers MUST render that as
    /// "unavailable", never as zero: a host that cannot report is not a host
    /// with no runs.
    async fn dashboard_summary(
        &self,
        _range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        Err(HostConnectorError::Unsupported)
    }
```

- [ ] **Step 6: Verify the workspace still builds**

Run: `cargo build -p rupu-cp`
Expected: SUCCESS — the default means no impl is forced to change yet.

Run: `cargo clippy -p rupu-cp --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/host/dashboard_summary.rs crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/mod.rs
git add crates/rupu-cp/src/host/dashboard_summary.rs crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/mod.rs
git commit -m "feat(cp): DashboardSummary DTOs + HostConnector::dashboard_summary()

Structured per-view method with an Unsupported default, following the
list_sessions / list_autoflow_runs pattern. NOT proxy_get_json, which is
structurally HTTP-only and would silently drop SSH/Tunnel/Bucket.

captured_at is per-host and set by the reading connector, so the freshness
strip can tell a live host from a stale one."
```

---

### Task 3: `LocalHostConnector::dashboard_summary()`

**Files:**
- Modify: `crates/rupu-cp/src/host/local.rs`
- Create: `crates/rupu-cp/src/host/summary_build.rs` (shared pure builder)
- Modify: `crates/rupu-cp/src/host/mod.rs`
- Test: `crates/rupu-cp/src/host/summary_build.rs` (inline tests)

**Interfaces:**
- Consumes: `DashboardSummary` DTOs (Task 2), `rupu_orchestrator::RunStore`
- Produces: `build_summary(runs: &[RunRecord], cycles: &[CycleRollup], findings_open: u64, range: DashboardRange, now: DateTime<Utc>) -> DashboardSummary` — a **pure function**, reused by Task 4 (HTTP passthrough validation) and testable without any I/O.

**Design note:** the bucketing and counting live in a pure function so they can be unit-tested against fixtures rather than against a live store, and so Local and any future in-process transport share one implementation.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/src/host/summary_build.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn rec(id: &str, status: rupu_orchestrator::runs::RunStatus, mins_ago: i64)
        -> rupu_orchestrator::runs::RunRecord
    {
        let mut r = rupu_orchestrator::runs::RunRecord::default();
        r.id = id.to_string();
        r.workflow_name = "wf".to_string();
        r.status = status;
        r.started_at = chrono::Utc::now() - chrono::Duration::minutes(mins_ago);
        r
    }

    #[test]
    fn active_counts_tally_non_terminal_states_only() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![
            rec("r1", Running, 1),
            rec("r2", Running, 2),
            rec("r3", AwaitingApproval, 3),
            rec("r4", Paused, 4),
            rec("r5", Pending, 5),
            rec("r6", Completed, 6),
            rec("r7", Failed, 7),
        ];
        let s = build_summary(&runs, &[], 0, DashboardRange::All, chrono::Utc::now());
        assert_eq!(s.active.running, 2);
        assert_eq!(s.active.awaiting_approval, 1);
        assert_eq!(s.active.paused, 1);
        assert_eq!(s.active.pending, 1);
    }

    #[test]
    fn terminal_buckets_exclude_active_runs() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![rec("r1", Completed, 10), rec("r2", Failed, 10), rec("r3", Running, 10)];
        let s = build_summary(&runs, &[], 0, DashboardRange::All, chrono::Utc::now());
        let completed: u64 = s.terminal_buckets.iter().map(|b| b.completed).sum();
        let failed: u64 = s.terminal_buckets.iter().map(|b| b.failed).sum();
        assert_eq!(completed, 1);
        assert_eq!(failed, 1);
    }

    #[test]
    fn range_filters_out_older_runs() {
        use rupu_orchestrator::runs::RunStatus::*;
        // 10 days ago — outside a 7d window.
        let runs = vec![rec("old", Completed, 60 * 24 * 10), rec("new", Completed, 5)];
        let s = build_summary(&runs, &[], 0, DashboardRange::Days7, chrono::Utc::now());
        let total: u64 = s.terminal_buckets.iter().map(|b| b.completed).sum();
        assert_eq!(total, 1, "the 10-day-old run must fall outside the 7d range");
    }

    #[test]
    fn buckets_are_contiguous_so_charts_do_not_lie_about_gaps() {
        use rupu_orchestrator::runs::RunStatus::*;
        // Two runs 3 days apart; the days between must still appear as zeroed
        // buckets, or the trend area silently closes the gap.
        let runs = vec![rec("a", Completed, 60 * 24 * 4), rec("b", Completed, 60 * 24 * 1)];
        let s = build_summary(&runs, &[], 0, DashboardRange::Days7, chrono::Utc::now());
        assert!(
            s.terminal_buckets.len() >= 4,
            "expected a filled bucket grid, got {} buckets",
            s.terminal_buckets.len()
        );
    }

    #[test]
    fn manual_runs_are_separated_from_cycle_runs() {
        use rupu_orchestrator::runs::RunStatus::*;
        let mut cron = rec("r_cron", Completed, 1);
        cron.source_wake_id = Some("wake_1".into());
        let manual = rec("r_manual", Completed, 1);
        let s = build_summary(&[cron, manual], &[], 0, DashboardRange::All, chrono::Utc::now());
        assert_eq!(s.recent_manual.len(), 1, "only the manual run belongs in recent_manual");
        assert_eq!(s.recent_manual[0].id, "r_manual");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp active_counts_tally_non_terminal`
Expected: FAIL — `cannot find function 'build_summary' in this scope`

- [ ] **Step 3: Implement `build_summary`**

Prepend to `crates/rupu-cp/src/host/summary_build.rs`:

```rust
//! Pure builder for [`DashboardSummary`].
//!
//! Kept free of I/O so the bucketing and tallying can be tested against
//! fixtures. `LocalHostConnector` is the caller; SSH builds its own summary
//! from CLI JSON (see `host/ssh.rs`).

#![deny(clippy::all)]

use crate::host::dashboard_summary::{
    ActiveCounts, ActiveRunBar, CycleRollup, DashboardRange, DashboardSummary, RecentRun,
    TerminalBucket,
};
use chrono::{DateTime, Duration, Timelike, Utc};
use rupu_orchestrator::runs::{RunRecord, RunStatus};
use std::collections::BTreeMap;

/// Truncate to the start of the UTC day — the bucket key.
fn day_key(t: DateTime<Utc>) -> DateTime<Utc> {
    t.with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t)
}

/// Build one host's dashboard contribution from its runs + cycles.
pub fn build_summary(
    runs: &[RunRecord],
    cycles: &[CycleRollup],
    findings_open: u64,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> DashboardSummary {
    let since = range.since(now);
    let in_range = |t: DateTime<Utc>| since.map(|s| t >= s).unwrap_or(true);

    let mut active = ActiveCounts::default();
    let mut active_runs = Vec::new();
    let mut recent_manual = Vec::new();
    let mut buckets: BTreeMap<DateTime<Utc>, TerminalBucket> = BTreeMap::new();

    // Runs belonging to a cycle are grouped under it in the feed; only manual
    // runs are listed individually (spec §5.5).
    let cycle_of: std::collections::HashMap<&str, &str> = cycles
        .iter()
        .flat_map(|c| c.runs.iter().map(move |r| (r.run_id.as_str(), c.cycle_id.as_str())))
        .collect();

    // Join each cycle's runs to their status. The `+N clean` pill needs it, and
    // we already hold every run here — the client should not fetch N runs to
    // expand one cycle.
    let status_of: std::collections::HashMap<&str, &str> =
        runs.iter().map(|r| (r.id.as_str(), r.status.as_str())).collect();
    let cycles: Vec<CycleRollup> = cycles
        .iter()
        .map(|c| {
            let mut c = c.clone();
            for run in c.runs.iter_mut() {
                // "unknown" rather than dropping the run: a cycle whose run list
                // silently shrank would disagree with its own `ran` count.
                run.status = status_of
                    .get(run.run_id.as_str())
                    .copied()
                    .unwrap_or("unknown")
                    .to_string();
            }
            c
        })
        .collect();

    for r in runs {
        if !in_range(r.started_at) {
            continue;
        }
        match r.status {
            RunStatus::Running => active.running += 1,
            RunStatus::AwaitingApproval => active.awaiting_approval += 1,
            RunStatus::Paused => active.paused += 1,
            RunStatus::Pending => active.pending += 1,
            _ => {}
        }

        // Non-terminal runs become swimlane bars. Paused is deliberately
        // included: is_terminal() excludes it because a paused run expects a
        // resume, so it is still live work.
        if !r.status.is_terminal() {
            active_runs.push(ActiveRunBar {
                run_id: r.id.clone(),
                workflow_name: r.workflow_name.clone(),
                status: r.status.as_str().to_string(),
                started_at: r.started_at,
                trigger: r.trigger_str().to_string(),
                cycle_id: cycle_of.get(r.id.as_str()).map(|c| c.to_string()),
            });
        }

        if r.status.is_terminal() {
            let key = day_key(r.started_at);
            let b = buckets.entry(key).or_insert(TerminalBucket {
                ts: key,
                completed: 0,
                failed: 0,
                rejected: 0,
                cancelled: 0,
            });
            match r.status {
                RunStatus::Completed => b.completed += 1,
                RunStatus::Failed => b.failed += 1,
                RunStatus::Rejected => b.rejected += 1,
                RunStatus::Cancelled => b.cancelled += 1,
                _ => {}
            }
        }

        if r.trigger_str() == "manual" {
            recent_manual.push(RecentRun {
                id: r.id.clone(),
                workflow_name: r.workflow_name.clone(),
                status: r.status.as_str().to_string(),
                started_at: r.started_at,
                finished_at: r.finished_at,
                trigger: "manual".to_string(),
            });
        }
    }

    // Fill the bucket grid. Without this the trend area silently closes gaps
    // and reads as continuous activity across days that had none.
    let terminal_buckets = fill_bucket_grid(buckets, range, now);

    active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

    DashboardSummary {
        active,
        terminal_buckets,
        active_runs,
        cycles,
        recent_manual,
        findings_open,
        captured_at: now,
    }
}

/// Emit a contiguous day-by-day grid, zero-filling days with no terminal runs.
fn fill_bucket_grid(
    mut buckets: BTreeMap<DateTime<Utc>, TerminalBucket>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> Vec<TerminalBucket> {
    let start = match range.since(now) {
        Some(s) => day_key(s),
        // `All`: start at the earliest bucket we actually have.
        None => match buckets.keys().next() {
            Some(k) => *k,
            None => return Vec::new(),
        },
    };
    let end = day_key(now);
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        out.push(buckets.remove(&cursor).unwrap_or(TerminalBucket {
            ts: cursor,
            completed: 0,
            failed: 0,
            rejected: 0,
            cancelled: 0,
        }));
        cursor += Duration::days(1);
    }
    out
}
```

Register in `crates/rupu-cp/src/host/mod.rs`:

```rust
pub mod summary_build;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cp summary_build`
Expected: PASS (5 tests)

If `RunRecord::default()` does not exist, construct the fixture with explicit field initialization instead — check `rupu-orchestrator/src/runs.rs:99` for the actual field set and whether `#[derive(Default)]` is present. Adjust the `rec()` helper accordingly; do not add a `Default` impl to the domain crate for a test's convenience.

- [ ] **Step 5: Wire `LocalHostConnector::dashboard_summary`**

In `crates/rupu-cp/src/host/local.rs`, add to the `impl HostConnector for LocalHostConnector` block:

```rust
    async fn dashboard_summary(
        &self,
        range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        let runs = self
            .run_store
            .list()
            .map_err(|e| HostConnectorError::Invalid(format!("run store list failed: {e}")))?;
        let cycles = crate::host::local::collect_cycle_rollups(&self.global_dir)
            .unwrap_or_default();
        let findings_open = crate::host::local::count_open_findings(&self.global_dir)
            .unwrap_or(0);
        Ok(crate::host::summary_build::build_summary(
            &runs,
            &cycles,
            findings_open,
            range,
            chrono::Utc::now(),
        ))
    }
```

**Implementer note — `collect_cycle_rollups` and `count_open_findings` do not exist yet; build them in this step. Reuse, do not re-derive:**

- **Cycles:** `crates/rupu-cp/src/api/run_streams.rs:15` already imports `AutoflowCycleRecord` / `AutoflowHistoryStore`, and **`run_streams.rs:48` already has `impl From<AutoflowCycleRecord> for AutoflowCycleRow`**. Read the history through `AutoflowHistoryStore` exactly as `list_autoflow_runs` (`run_streams.rs:522`) does, then map each record to a `CycleRollup`. Do **not** write a second history-reading path, and do **not** re-parse the raw history JSON — `AutoflowCycleRecord` is already the parsed form.
- **`CycleRun.status`:** leave `"unknown"` here. `build_summary` fills it in from the runs it already holds (see Step 3) — this helper must not do per-run store reads.
- **Findings:** use whatever `crates/rupu-cp/src/api/findings.rs` already calls to produce `.summary.total`; call that function rather than re-walking the findings store.
- If `LocalHostConnector` lacks a `global_dir` field, thread it in from `AppState` at construction.
- Keep both helpers private to `local.rs`.

- [ ] **Step 6: Verify**

Run: `cargo test -p rupu-cp`
Expected: PASS.

Run: `cargo clippy -p rupu-cp --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/host/summary_build.rs crates/rupu-cp/src/host/local.rs crates/rupu-cp/src/host/mod.rs
git add crates/rupu-cp/src/host/summary_build.rs crates/rupu-cp/src/host/local.rs crates/rupu-cp/src/host/mod.rs
git commit -m "feat(cp): LocalHostConnector::dashboard_summary + pure build_summary

build_summary is I/O-free so bucketing and tallying are testable against
fixtures. Fills the bucket grid so the trend area does not silently close
gaps over days with no runs.

Paused counts as a swimlane bar: is_terminal() excludes it because a paused
run expects a resume, so it is still live work."
```

---

### Task 4: `HttpHostConnector::dashboard_summary()` + bounded timeout

**Files:**
- Modify: `crates/rupu-cp/src/host/http.rs`
- Test: `crates/rupu-cp/tests/host_http.rs`

**Interfaces:**
- Consumes: `DashboardSummary` (Task 2), the existing `proxy_get_json` (HTTP-only, which is fine *here* — this is the HTTP connector)
- Produces: nothing new; satisfies the trait for HTTP hosts.

- [ ] **Step 1: Write the failing test**

Add to `crates/rupu-cp/tests/host_http.rs`, following the existing two-real-servers pattern in that file:

```rust
#[tokio::test]
async fn http_dashboard_summary_proxies_remote_and_preserves_captured_at() {
    // Spin a remote CP with one seeded run, register it as a remote of a
    // second CP, and assert the summary comes back through the connector.
    let remote_dir = tempfile::tempdir().unwrap();
    seed_standalone_meta(remote_dir.path(), "run_remote_1");
    let remote = spawn_server(remote_dir.path()).await;

    let conn = crate::host::http::HttpHostConnector::new("host_remote", &remote.base_url, None);
    let summary = conn
        .dashboard_summary(rupu_cp::host::dashboard_summary::DashboardRange::Days30)
        .await
        .expect("http host must serve dashboard_summary");

    // captured_at must come from the host that read the data, not be
    // synthesized locally.
    assert!(summary.captured_at <= chrono::Utc::now());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp http_dashboard_summary_proxies_remote`
Expected: FAIL — the default trait impl returns `Unsupported`, so `.expect(...)` panics with `Unsupported`.

- [ ] **Step 3: Implement**

Add to the `impl HostConnector for HttpHostConnector` block in `crates/rupu-cp/src/host/http.rs`:

```rust
    async fn dashboard_summary(
        &self,
        range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        // `host=local` scopes the remote CP to ITS OWN data — without it the
        // remote would fan out to its own remotes and we would double-count
        // any host registered on both sides.
        let path = format!("/api/dashboard?host=local&range={}", range.as_str());
        let v = self.proxy_get_json(&path).await?;
        serde_json::from_value(v)
            .map_err(|e| HostConnectorError::Invalid(format!("bad dashboard summary: {e}")))
    }
```

- [ ] **Step 4: Bound the client timeout**

`HttpHostConnector::new()` currently builds `reqwest::Client::new()`, whose timeout is effectively unbounded on the normal `?host=` path — only the probe path bounds it via `resolve_for_probe`. Fan-out makes this much easier to hit: one unreachable HTTP host stalls the whole dashboard on the OS TCP connect timeout. Spec §8 flags it.

In `HttpHostConnector::new`, replace `reqwest::Client::new()` with:

```rust
        // Bounded so one unreachable host cannot stall a fan-out on the OS TCP
        // connect timeout. Fan-out is concurrent (join_all), so wall-clock is
        // the slowest host — which must therefore be bounded.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-cp host_http`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/host/http.rs crates/rupu-cp/tests/host_http.rs
git add crates/rupu-cp/src/host/http.rs crates/rupu-cp/tests/host_http.rs
git commit -m "feat(cp): HttpHostConnector::dashboard_summary + bounded timeouts

Proxies /api/dashboard?host=local&range= — host=local scopes the remote to
its own data so a host registered on both sides is not double-counted.

Also bounds the reqwest client (5s connect / 30s total). It was effectively
unbounded on the normal ?host= path; fan-out makes that reachable, where one
unreachable host would stall the whole dashboard."
```

---

### Task 5: SSH `list_runs` fix — mirror-read → remote CLI

> **This task is its own PR.** It is a behavior change to an existing shipped path, it is the highest-risk change in this plan, and it must be revertable alone.

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs`
- Test: `crates/rupu-cp/src/host/ssh.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 1's `{"kind":"run_list","version":1,"rows":[...]}` contract
- Produces: `SshHostConnector::list_runs` returning rows shaped like `RunListRow`

**The bug being fixed:** `SshHostConnector::list_runs` reads a **local mirror** (`mirror_list_runs`, `connector.rs:433`). The mirror is populated by `spawn_tail_pump` (`ssh.rs:616`), an in-memory `tokio::spawn` created inside `launch_run` (`ssh.rs:897`) and `launch_agent` (`ssh.rs:977`) — and nowhere else. So a run started directly on the remote box, or launched by a *previous* `cp serve` process, is **permanently invisible**. The panel under-reports and cannot know it.

**Do not touch `stream_run_events`.** It legitimately reads the mirror: tailing a known path on a live run is a different problem from enumerating the store. The pump stays as-is.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/rupu-cp/src/host/ssh.rs`. Copy the `StubExec` shape from `list_sessions_shells_rupu_session_list_and_parses_rows` (ssh.rs:1419) verbatim:

```rust
    #[tokio::test]
    async fn list_runs_shells_rupu_run_list_not_the_mirror() {
        struct StubExec {
            json: String,
            last_cmd: std::sync::Mutex<String>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                *self.last_cmd.lock().unwrap() = remote.to_string();
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by list_runs")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by list_runs")
            }
        }

        let json = r#"{"kind":"run_list","version":1,"rows":[
            {"run_id":"run_a","workflow_name":"nightly","status":"completed",
             "started_at":"2026-07-16T14:02:11Z","finished_at":"2026-07-16T14:09:02Z",
             "trigger":"cron","workspace_id":"ws_1","parent_run_id":null,
             "awaiting_step_id":null,"active_step_id":null,"error_message":null}
        ],"summary":{"count":1,"limit":10000,"status_filter":null}}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            last_cmd: std::sync::Mutex::new(String::new()),
        });
        // The mirror is EMPTY — this is the point. Before the fix, list_runs
        // read the mirror and would return zero rows here.
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let rows = conn
            .list_runs(RunListQuery {
                kind: RunKind::All,
                offset: 0,
                limit: 100,
                lifecycle: None,
            })
            .await
            .unwrap();

        assert_eq!(rows.len(), 1, "must return the CLI's row, not the empty mirror");
        assert_eq!(rows[0]["id"], "run_a");
        assert_eq!(rows[0]["trigger"], "cron", "trigger must survive — cycle grouping depends on it");

        let cmd = stub.last_cmd.lock().unwrap().clone();
        assert!(
            cmd.contains("run") && cmd.contains("list") && cmd.contains("json"),
            "must shell `rupu run list --format json`: {cmd}"
        );
    }

    #[tokio::test]
    async fn list_runs_preserves_rfc3339_for_merge_sort() {
        // rupu-cp's fan_out merge does a LEXICOGRAPHIC string compare on
        // started_at. A space-separated timestamp (' ' = 0x20 < 'T' = 0x54)
        // would sort every remote row after every local row at the same
        // instant. Guard the format.
        struct StubExec {
            json: String,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, _remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }
        let json = r#"{"kind":"run_list","version":1,"rows":[
            {"run_id":"run_a","workflow_name":"w","status":"completed",
             "started_at":"2026-07-16T14:02:11Z","finished_at":null,"trigger":"manual",
             "workspace_id":null,"parent_run_id":null,"awaiting_step_id":null,
             "active_step_id":null,"error_message":null}
        ],"summary":{"count":1,"limit":1,"status_filter":null}}"#;
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(StubExec { json: json.into() }));
        let rows = conn
            .list_runs(RunListQuery { kind: RunKind::All, offset: 0, limit: 100, lifecycle: None })
            .await
            .unwrap();
        let started = rows[0]["started_at"].as_str().unwrap();
        assert!(started.contains('T'), "started_at must stay RFC-3339: {started}");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp list_runs_shells_rupu_run_list`
Expected: FAIL — `assertion failed: rows.len() == 1`, actual `0`. The mirror is empty and the current impl reads it. **This failure is the bug, reproduced.**

- [ ] **Step 3: Implement**

Replace `SshHostConnector::list_runs` (`ssh.rs:~1010` region) with:

```rust
    /// List runs by shelling the remote CLI.
    ///
    /// Was: `mirror_list_runs`. The mirror is populated only by
    /// `spawn_tail_pump`, which runs solely on the launch path — so runs
    /// started directly on the box, or launched by a PREVIOUS `cp serve`
    /// process, were permanently invisible. Enumerating via the CLI is the
    /// same pattern `list_sessions` / `list_autoflow_runs` / `list_agent_runs`
    /// already use.
    ///
    /// `stream_run_events` still reads the mirror, deliberately: tailing a
    /// known path on a live run is a different problem from enumerating.
    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let rows = self
            .remote_json_rows(&["--format", "json", "run", "list", "--limit", "10000"])
            .await?;

        let mut out: Vec<serde_json::Value> = rows
            .iter()
            .filter_map(run_list_row_to_wire)
            .filter(|r| match params.kind {
                RunKind::All => true,
                // Workflow-only means manual-triggered only, mirroring
                // query_run_rows' `event.is_none() && source_wake_id.is_none()`.
                RunKind::Workflow => r["trigger"] == "manual",
            })
            .filter(|r| match params.lifecycle.as_deref() {
                None => true,
                Some("active") => !matches!(
                    r["status"].as_str().unwrap_or(""),
                    "completed" | "failed" | "rejected" | "cancelled"
                ),
                Some("completed") => r["status"] == "completed",
                Some("failed") => r["status"] == "failed",
                Some(_) => true,
            })
            .collect();

        // The CLI already sorts newest-first, but re-sort so this is correct
        // regardless of remote CLI version.
        out.sort_by(|a, b| {
            let ta = a["started_at"].as_str().unwrap_or("");
            let tb = b["started_at"].as_str().unwrap_or("");
            tb.cmp(ta)
        });

        Ok(out
            .into_iter()
            .skip(params.offset)
            .take(params.limit)
            .collect())
    }
```

Add the mapper near the other row mappers (alongside `transcript_row_to_agent_run`, ~`ssh.rs:135`):

```rust
/// Map one `rupu run list --format json` row to the `RunListRow` wire shape.
///
/// Field renames only — the CLI contract (Task 1) was built to carry every
/// field this needs, so nothing is synthesized or defaulted-to-wrong here.
fn run_list_row_to_wire(row: &serde_json::Value) -> Option<serde_json::Value> {
    let run_id = row.get("run_id")?.as_str()?;
    Some(serde_json::json!({
        "id": run_id,
        "workflow_name": row.get("workflow_name").and_then(|v| v.as_str()).unwrap_or(""),
        "status": row.get("status").and_then(|v| v.as_str()).unwrap_or("pending"),
        "started_at": row.get("started_at").and_then(|v| v.as_str()).unwrap_or(""),
        "finished_at": row.get("finished_at").cloned().unwrap_or(serde_json::Value::Null),
        "trigger": row.get("trigger").and_then(|v| v.as_str()).unwrap_or("manual"),
        "workspace_id": row.get("workspace_id").cloned().unwrap_or(serde_json::Value::Null),
        "error_message": row.get("error_message").cloned().unwrap_or(serde_json::Value::Null),
    }))
}
```

**CRITICAL — flag order.** `--format json` MUST come **before** `run`:
`rupu --format json run list --limit 10000`. `Cmd::Run` is `trailing_var_arg`
(`rupu-cli/src/lib.rs:79-83`), so it swallows everything after `run` before
clap extracts global flags — `rupu run list --format json` fails outright with
`unexpected argument '--format' found`. This differs from `list_sessions` /
`list_autoflow_runs`, whose subcommands ARE real clap subcommands and so accept
a trailing `--format json`. Do not copy their argument order.

**Implementer note:** `remote_json_rows` is the existing helper `list_sessions` uses to build the ssh command, run it, and pull `.rows` out. Reuse it — do not build the command string by hand. Its `.get("rows")` contract already matches Task 1's report shape.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cp list_runs_`
Expected: PASS (2 tests).

Run: `cargo test -p rupu-cp ssh`
Expected: PASS — in particular the existing pump/launch tests must still pass, proving `stream_run_events` is untouched.

- [ ] **Step 5: Version-gate old hosts**

A remote host whose `rupu` predates Task 1 will fail `run list` with a clap error. That must surface as unavailable-with-reason, never as zero runs (spec §4.3).

In `list_runs`, map the failure:

```rust
        let rows = match self
            .remote_json_rows(&["--format", "json", "run", "list", "--limit", "10000"])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // An old remote rupu has no `run list`; it parses as "launch an
                // agent named list" and errors. Surface it as Unsupported so the
                // freshness strip renders "needs rupu >= 0.49" rather than
                // silently reporting zero runs.
                tracing::warn!(
                    host_id = %self.host_id,
                    error = %e,
                    "list_runs: remote `rupu run list` failed; host may predate the command"
                );
                return Err(HostConnectorError::Unsupported);
            }
        };
```

- [ ] **Step 6: Verify against a REAL SSH host**

**A mock cannot validate this fix.** The bug is that the mirror is populated only on the launch path — a stub `RemoteExec` would happily fake either behavior. This needs a real host.

```bash
# On a real remote host with rupu >= the version built in Task 1:
rupu host add ssh <user>@<host>
# Start a run DIRECTLY on the remote box (not via CP) — this is the run that
# was invisible before the fix.
ssh <user>@<host> 'rupu workflow run <some-workflow>'
# Then, from CP:
curl -s 'http://127.0.0.1:7878/api/runs?host=<host_id>' | jq '.[].id'
```

Expected: the directly-started run **appears**. Before this fix it would not.

If no real SSH host is available, **stop and report** rather than marking this task complete on stub tests alone.

- [ ] **Step 7: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/host/ssh.rs
cargo clippy -p rupu-cp --all-targets -- -D warnings
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "fix(cp): SSH list_runs shells the remote CLI instead of reading the mirror

The mirror is populated only by spawn_tail_pump, which runs solely on the
launch path. So runs started directly on the remote box -- or launched by a
PREVIOUS cp serve process, since the pump is an in-memory tokio::spawn not
restarted on boot -- were permanently invisible. The panel under-reported and
could not know it.

Now shells 'rupu run list --format json', matching list_sessions /
list_autoflow_runs / list_agent_runs. stream_run_events still reads the
mirror, deliberately: tailing a known path on a live run is a different
problem from enumerating the store.

Hosts predating 'rupu run list' return Unsupported, which renders as
unavailable-with-reason -- never as zero runs."
```

---

### Task 6: `SshHostConnector::dashboard_summary()`

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs`
- Test: `crates/rupu-cp/src/host/ssh.rs` (inline tests)

**Interfaces:**
- Consumes: Task 1's `run_list` contract; the existing `list_autoflow_runs` (`rupu autoflow history --format json`)
- Produces: `DashboardSummary` for SSH hosts

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn ssh_dashboard_summary_sets_captured_at_and_tallies_active() {
        struct StubExec {
            runs_json: String,
            cycles_json: String,
            cmds: std::sync::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                self.cmds.lock().unwrap().push(remote.to_string());
                let stdout = if remote.contains("autoflow") {
                    self.cycles_json.clone()
                } else {
                    self.runs_json.clone()
                };
                Ok(RemoteOutput { stdout, stderr: String::new(), success: true })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }

        let runs_json = r#"{"kind":"run_list","version":1,"rows":[
            {"run_id":"r1","workflow_name":"w","status":"running",
             "started_at":"2026-07-16T14:02:11Z","finished_at":null,"trigger":"manual",
             "workspace_id":null,"parent_run_id":null,"awaiting_step_id":null,
             "active_step_id":null,"error_message":null},
            {"run_id":"r2","workflow_name":"w","status":"awaiting_approval",
             "started_at":"2026-07-16T14:03:11Z","finished_at":null,"trigger":"cron",
             "workspace_id":null,"parent_run_id":null,"awaiting_step_id":"s1",
             "active_step_id":null,"error_message":null}
        ],"summary":{"count":2,"limit":10000,"status_filter":null}}"#;
        let cycles_json = r#"{"kind":"autoflow_history","version":1,"rows":[]}"#;

        let stub = std::sync::Arc::new(StubExec {
            runs_json: runs_json.into(),
            cycles_json: cycles_json.into(),
            cmds: std::sync::Mutex::new(Vec::new()),
        });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let before = chrono::Utc::now();
        let s = conn
            .dashboard_summary(crate::host::dashboard_summary::DashboardRange::Days30)
            .await
            .unwrap();

        assert_eq!(s.active.running, 1);
        assert_eq!(s.active.awaiting_approval, 1);
        assert_eq!(s.active_runs.len(), 2, "both non-terminal runs become swimlane bars");
        assert!(
            s.captured_at >= before,
            "captured_at must be stamped when the host was actually read"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp ssh_dashboard_summary_sets_captured_at`
Expected: FAIL — panics with `Unsupported` (the trait default).

- [ ] **Step 3: Implement**

Add to `impl HostConnector for SshHostConnector`:

```rust
    async fn dashboard_summary(
        &self,
        range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        use crate::host::dashboard_summary::*;

        // Two round-trips, not more. Every RemoteExec::run spawns a fresh ssh
        // process with a full handshake (no ControlMaster multiplexing), so
        // this must stay coarse.
        let run_rows = self
            .remote_json_rows(&["--format", "json", "run", "list", "--limit", "10000"])
            .await
            .map_err(|e| {
                tracing::warn!(host_id = %self.host_id, error = %e, "dashboard_summary: run list failed");
                HostConnectorError::Unsupported
            })?;
        let cycle_rows = self.list_autoflow_runs().await.unwrap_or_default();

        let now = chrono::Utc::now();
        let since = range.since(now);
        let in_range = |t: chrono::DateTime<chrono::Utc>| since.map(|s| t >= s).unwrap_or(true);

        let cycles: Vec<CycleRollup> = cycle_rows
            .iter()
            .filter_map(|c| {
                Some(CycleRollup {
                    cycle_id: c.get("cycle_id")?.as_str()?.to_string(),
                    worker_name: c.get("worker_name").and_then(|v| v.as_str()).map(String::from),
                    started_at: c
                        .get("started_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&chrono::Utc))?,
                    finished_at: c
                        .get("finished_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&chrono::Utc)),
                    ran: c.get("ran_cycles").and_then(|v| v.as_u64()).unwrap_or(0),
                    skipped: c.get("skipped_cycles").and_then(|v| v.as_u64()).unwrap_or(0),
                    failed: c.get("failed_cycles").and_then(|v| v.as_u64()).unwrap_or(0),
                    // Status is filled in below, once the run rows are indexed.
                    runs: c
                        .get("run_ids")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .map(|id| CycleRun {
                                    run_id: id.to_string(),
                                    status: "unknown".to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .filter(|c| in_range(c.started_at))
            .collect();

        // Index the CLI's run rows so each cycle's runs can carry a status —
        // the `+N clean` pill needs it, and this costs no extra round-trip.
        let status_of: std::collections::HashMap<&str, &str> = run_rows
            .iter()
            .filter_map(|r| {
                Some((
                    r.get("run_id")?.as_str()?,
                    r.get("status")?.as_str()?,
                ))
            })
            .collect();
        let mut cycles = cycles;
        for c in cycles.iter_mut() {
            for run in c.runs.iter_mut() {
                if let Some(st) = status_of.get(run.run_id.as_str()) {
                    run.status = st.to_string();
                }
            }
        }

        let cycle_of: std::collections::HashMap<String, String> = cycles
            .iter()
            .flat_map(|c| c.runs.iter().map(|r| (r.run_id.clone(), c.cycle_id.clone())))
            .collect();

        let mut active = ActiveCounts::default();
        let mut active_runs = Vec::new();
        let mut recent_manual = Vec::new();
        let mut buckets: std::collections::BTreeMap<String, TerminalBucket> = Default::default();

        for row in &run_rows {
            let (Some(id), Some(status), Some(started)) = (
                row.get("run_id").and_then(|v| v.as_str()),
                row.get("status").and_then(|v| v.as_str()),
                row.get("started_at").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(started) else {
                continue;
            };
            let started_at = started_at.with_timezone(&chrono::Utc);
            if !in_range(started_at) {
                continue;
            }
            let trigger = row.get("trigger").and_then(|v| v.as_str()).unwrap_or("manual");
            let workflow_name =
                row.get("workflow_name").and_then(|v| v.as_str()).unwrap_or("").to_string();

            match status {
                "running" => active.running += 1,
                "awaiting_approval" => active.awaiting_approval += 1,
                "paused" => active.paused += 1,
                "pending" => active.pending += 1,
                _ => {}
            }

            let terminal = matches!(status, "completed" | "failed" | "rejected" | "cancelled");
            if !terminal {
                active_runs.push(ActiveRunBar {
                    run_id: id.to_string(),
                    workflow_name: workflow_name.clone(),
                    status: status.to_string(),
                    started_at,
                    trigger: trigger.to_string(),
                    cycle_id: cycle_of.get(id).cloned(),
                });
            } else {
                let key = started_at.format("%Y-%m-%d").to_string();
                let b = buckets.entry(key).or_insert(TerminalBucket {
                    ts: started_at,
                    completed: 0,
                    failed: 0,
                    rejected: 0,
                    cancelled: 0,
                });
                match status {
                    "completed" => b.completed += 1,
                    "failed" => b.failed += 1,
                    "rejected" => b.rejected += 1,
                    "cancelled" => b.cancelled += 1,
                    _ => {}
                }
            }

            // A run belonging to a cycle is grouped under that cycle in the
            // feed even when it has no trigger provenance of its own — it must
            // never ALSO leak into recent_manual, or the same run renders twice
            // (once under its cycle, once standalone). That double-listing is
            // the exact autoflow-flooding bug this redesign exists to fix.
            // The local build_summary has the identical guard.
            if trigger == "manual" && !cycle_of.contains_key(id) {
                recent_manual.push(RecentRun {
                    id: id.to_string(),
                    workflow_name,
                    status: status.to_string(),
                    started_at,
                    finished_at: row
                        .get("finished_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&chrono::Utc)),
                    trigger: "manual".to_string(),
                });
            }
        }

        active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

        Ok(DashboardSummary {
            active,
            terminal_buckets: buckets.into_values().collect(),
            active_runs,
            cycles,
            recent_manual,
            // Findings are not exposed by the CLI; 0 here means "not reported by
            // this host", and the aggregate sums only hosts that report.
            findings_open: 0,
            captured_at: now,
        })
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cp ssh_dashboard_summary`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/host/ssh.rs
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(cp): SshHostConnector::dashboard_summary

Two ssh round-trips (run list + autoflow history), no more: every
RemoteExec::run is a fresh ssh process with a full handshake, so the summary
must stay coarse.

Hosts predating 'rupu run list' return Unsupported -> rendered as unavailable,
never as zero."
```

---

### Task 7: Fan out `/api/dashboard` across hosts

**Files:**
- Modify: `crates/rupu-cp/src/api/dashboard.rs`
- Test: `crates/rupu-cp/tests/dashboard.rs`

**Interfaces:**
- Consumes: `dashboard_summary()` (Tasks 2–6), `fan_out_via` (`api/host_fanout.rs`)
- Produces: `GET /api/dashboard?range=&host=` returning `DashboardResponse { hosts: Vec<HostFreshness>, active, terminal_buckets, active_runs, cycles, recent_manual, findings_open }` — **the contract Plan 1's page consumes.**

- [ ] **Step 1: Write the failing test**

Add to `crates/rupu-cp/tests/dashboard.rs`:

```rust
#[tokio::test]
async fn dashboard_reports_per_host_freshness_and_never_zeroes_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert!(!hosts.is_empty(), "local must always appear");
    let local = &hosts[0];
    assert_eq!(local["host_id"], "local");
    assert_eq!(local["state"], "ok");
    assert!(
        local["captured_at"].as_str().unwrap().contains('T'),
        "captured_at must be RFC-3339 for the freshness strip"
    );
}

#[tokio::test]
async fn dashboard_rejects_unknown_range() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/dashboard?range=bogus", srv.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "an unparseable range must 400, not silently default");
}

#[tokio::test]
async fn dashboard_unavailable_host_renders_unavailable_not_zero() {
    // A host that cannot report is NOT a host with no runs. Register an
    // unreachable remote and assert it surfaces as a distinct state.
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().unwrap();
    let remote = hosts
        .iter()
        .find(|h| h["host_id"] != "local")
        .expect("the unreachable remote must still appear in the freshness strip");
    assert_ne!(
        remote["state"], "ok",
        "an unreachable host must not report ok"
    );
    assert!(
        remote["captured_at"].is_null(),
        "an unreachable host has no captured_at — it never reported"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp dashboard_reports_per_host_freshness`
Expected: FAIL — `hosts array required`; the current response has no `hosts` key.

- [ ] **Step 3: Rewrite the handler**

Replace the DTOs and handler in `crates/rupu-cp/src/api/dashboard.rs`:

```rust
/// One host's reporting state, for the freshness strip.
///
/// `state` is deliberately three-valued. A host that cannot report is NOT a
/// host with no runs, so `unavailable` and `offline` must never collapse into
/// zeroed counts.
#[derive(Serialize)]
struct HostFreshness {
    host_id: String,
    name: String,
    transport_kind: String,
    /// `"ok"` | `"offline"` | `"unavailable"`.
    state: &'static str,
    /// Present only when `state == "ok"`.
    captured_at: Option<DateTime<Utc>>,
    /// Human-readable cause when `state != "ok"`, e.g. "needs rupu >= 0.49".
    reason: Option<String>,
}

#[derive(Serialize)]
struct DashboardResponse {
    hosts: Vec<HostFreshness>,
    active: ActiveCounts,
    terminal_buckets: Vec<TerminalBucket>,
    active_runs: Vec<ActiveRunBar>,
    cycles: Vec<CycleRollup>,
    recent_manual: Vec<RecentRun>,
    findings_open: u64,
}

#[derive(serde::Deserialize)]
struct DashboardQuery {
    range: Option<String>,
    host: Option<String>,
}

async fn get_dashboard(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<DashboardQuery>,
) -> ApiResult<Json<DashboardResponse>> {
    let range = match q.range.as_deref() {
        None => DashboardRange::default(),
        Some(r) => DashboardRange::parse(r).ok_or_else(|| {
            ApiError::bad_request(format!("unknown range {r:?}; expected 7d | 30d | all"))
        })?,
    };

    // Which hosts to ask: one named host, or every registered host.
    let targets: Vec<_> = match q.host.as_deref() {
        Some(id) => vec![s.hosts.host_view(id).ok_or_else(|| {
            ApiError::not_found(format!("unknown host {id}"))
        })?],
        None => s.hosts.list_hosts(),
    };

    let futs = targets.into_iter().map(|h| {
        let registry = std::sync::Arc::clone(&s.hosts);
        let host_id = h.id.clone();
        let name = h.name.clone();
        let transport_kind = h.transport_kind.clone();
        async move {
            let conn = match registry.resolve(&host_id) {
                Ok(c) => c,
                Err(e) => {
                    return (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.to_string()),
                        },
                        None,
                    )
                }
            };
            match conn.dashboard_summary(range).await {
                Ok(sum) => (
                    HostFreshness {
                        host_id,
                        name,
                        transport_kind,
                        state: "ok",
                        captured_at: Some(sum.captured_at),
                        reason: None,
                    },
                    Some(sum),
                ),
                Err(HostConnectorError::Unsupported) => (
                    HostFreshness {
                        host_id,
                        name,
                        transport_kind,
                        state: "unavailable",
                        captured_at: None,
                        reason: Some(
                            "host does not report dashboard data (needs a newer rupu)".into(),
                        ),
                    },
                    None,
                ),
                Err(e) => {
                    tracing::warn!(host_id = %host_id, error = %e, "dashboard_summary failed");
                    (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.to_string()),
                        },
                        None,
                    )
                }
            }
        }
    });

    let results = futures_util::future::join_all(futs).await;

    // Merge ONLY hosts that actually reported. A non-reporting host contributes
    // nothing rather than zeros — its state is carried in `hosts` instead.
    let mut resp = DashboardResponse {
        hosts: Vec::new(),
        active: ActiveCounts::default(),
        terminal_buckets: Vec::new(),
        active_runs: Vec::new(),
        cycles: Vec::new(),
        recent_manual: Vec::new(),
        findings_open: 0,
    };
    let mut bucket_merge: std::collections::BTreeMap<DateTime<Utc>, TerminalBucket> =
        Default::default();

    for (freshness, summary) in results {
        resp.hosts.push(freshness);
        let Some(sum) = summary else { continue };
        resp.active.running += sum.active.running;
        resp.active.awaiting_approval += sum.active.awaiting_approval;
        resp.active.paused += sum.active.paused;
        resp.active.pending += sum.active.pending;
        resp.findings_open += sum.findings_open;
        resp.active_runs.extend(sum.active_runs);
        resp.cycles.extend(sum.cycles);
        resp.recent_manual.extend(sum.recent_manual);
        for b in sum.terminal_buckets {
            let e = bucket_merge.entry(b.ts).or_insert(TerminalBucket {
                ts: b.ts,
                completed: 0,
                failed: 0,
                rejected: 0,
                cancelled: 0,
            });
            e.completed += b.completed;
            e.failed += b.failed;
            e.rejected += b.rejected;
            e.cancelled += b.cancelled;
        }
    }

    resp.terminal_buckets = bucket_merge.into_values().collect();
    resp.active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    resp.cycles.sort_by_key(|c| std::cmp::Reverse(c.started_at));
    resp.recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

    Ok(Json(resp))
}
```

**Implementer note:** `s.hosts.host_view(id)` may not exist — `list_hosts()` does. If not, filter `list_hosts()` by id and 404 when absent, rather than adding a registry method for one caller.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cp dashboard`
Expected: PASS (3 new tests + existing).

- [ ] **Step 5: Verify by hand**

```bash
cargo run -p rupu-cli -- cp serve &
sleep 3
curl -s 'http://127.0.0.1:7878/api/dashboard?range=30d' | jq '{hosts, active, cycles: (.cycles|length), bars: (.active_runs|length)}'
curl -s -o /dev/null -w '%{http_code}\n' 'http://127.0.0.1:7878/api/dashboard?range=bogus'
```

Expected: first prints a `hosts` array with `local` in `state: "ok"` and an RFC-3339 `captured_at`; second prints `400`.

- [ ] **Step 6: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/api/dashboard.rs crates/rupu-cp/tests/dashboard.rs
cargo clippy -p rupu-cp --all-targets -- -D warnings
git add crates/rupu-cp/src/api/dashboard.rs crates/rupu-cp/tests/dashboard.rs
git commit -m "feat(cp): fan /api/dashboard out across hosts with per-host freshness

Was the one list-ish view in CP that never learned about hosts -- it read
s.run_store directly, so every number silently meant 'local only'.

Non-reporting hosts contribute NOTHING rather than zeros; their state lives in
the hosts[] array as ok | offline | unavailable with a reason. A host that
cannot report is not a host with no runs.

?range= is validated, not silently defaulted."
```

---

## Plan 2 Definition of Done

- [ ] `rupu run list --format json` emits `kind: "run_list"`, `version: 1`, RFC-3339 timestamps, and a `trigger` on every row.
- [ ] `cargo test -p rupu-cp` and `cargo test -p rupu-cli` pass (modulo the known toolchain baseline).
- [ ] `cargo clippy -p rupu-cp -p rupu-cli --all-targets -- -D warnings` is clean.
- [ ] `GET /api/dashboard?range=30d` returns a `hosts[]` freshness array; `?range=bogus` returns 400.
- [ ] **A run started directly on a real SSH host appears in `GET /api/runs?host=<id>`.** Verified against a real host, not a stub (Task 5, Step 6).
- [ ] An unreachable or too-old host renders `state: "offline"` / `"unavailable"` with a reason — never zeroed counts.
- [ ] Task 5 is a separate PR from the rest of the plan.
