# `run:` Step Kind Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a deterministic, non-LLM `run:` step kind to `rupu-orchestrator` that executes a declared command as an argv vector and binds its stdout/exit code into downstream templates.

**Architecture:** A new `RunStep` block on `Step`, validated in `validate_step_shape` as a new arm that — unlike `action:`/`panel:`/`parallel:` — is *compatible* with `for_each:`. Execution goes through a new `execute_run_step` in `runner.rs`, modelled on the existing `execute_action_step`. Permission gating reuses the `PermissionMode` triple. Rendering follows the precedent set by gate and action nodes.

**Tech Stack:** Rust 2021, `tokio::process::Command`, `serde`/`serde_yaml`, `thiserror`, `minijinja` (via existing `render_step_prompt`), `insta` for snapshots.

## Global Constraints

- Rust 2021; MSRV pinned in `rust-toolchain.toml`. Do not change it.
- **Workspace deps only.** Versions pinned in root `Cargo.toml`; never in a crate's `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`; `unsafe_code` is forbidden.
- Errors: `thiserror` in libraries, `anyhow` only in `rupu-cli`.
- `rupu-cli` stays thin — arg parsing and delegation only, no business logic.
- **Never run package-wide `cargo fmt`.** `main` is fmt-dirty under the pinned toolchain. Format only the files you touched: `cargo fmt -- <path>`.
- Never use bare `git stash` / `git stash pop` — the stash stack is shared across worktrees.
- No silent no-ops. A capability that cannot run must fail loudly, never degrade quietly.

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-orchestrator/src/workflow.rs` | `RunStep` struct, `Step.run` field, parse-time validation, `STEP_OUTPUT_FIELDS` additions |
| `crates/rupu-orchestrator/src/run_step.rs` (new) | The pure executor: argv assembly, spawn, timeout, exit-code gating, output parsing. No orchestrator knowledge. |
| `crates/rupu-orchestrator/src/runner.rs` | Dispatch arm, `step_kind_for_run_record`, fan-out wiring, `RunWorkflowError` variants |
| `crates/rupu-orchestrator/src/runs.rs` | `StepKind::Run` variant |
| `crates/rupu-config/src/policy_config.rs` | `WorkflowConfig { run_step_enabled, run_step_allowlist }` |
| `crates/rupu-app-canvas/src/git_graph.rs` | `Run` node row rendering |
| `crates/rupu-cp/web/src/components/graph/kindBridge.ts` | `run` kind colour + glyph |
| `docs/workflows.md` | Author-facing `run:` reference |

`run_step.rs` is a new file rather than more mass in `runner.rs` (already 14k lines) because the executor is pure and independently testable: given a `RunStep` and a resolved context it produces a `RunStepOutput`, with no knowledge of runs, steps, or contexts.

---

### Task 1: `RunStep` parse surface

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (add struct near `Distribute`, ~line 860; add field to `Step`, ~line 947)
- Test: `crates/rupu-orchestrator/src/workflow.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub struct RunStep { cmd: String, args: Vec<String>, cwd: Option<String>, env: BTreeMap<String,String>, parse: ParseMode, timeout_seconds: Option<u64>, allow_exit_codes: Vec<i32> }`, `pub enum ParseMode { Raw, Json, Lines }`, and `Step.run: Option<RunStep>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn parses_run_step_with_defaults() {
    let yaml = r#"
name: t
steps:
  - id: score
    run:
      cmd: python3
      args: ["tools/score.py", "{{ item }}"]
"#;
    let wf = Workflow::parse(yaml).expect("parses");
    let run = wf.steps[0].run.as_ref().expect("run block");
    assert_eq!(run.cmd, "python3");
    assert_eq!(run.args, vec!["tools/score.py", "{{ item }}"]);
    assert_eq!(run.parse, ParseMode::Raw);
    assert_eq!(run.allow_exit_codes, vec![0]);
    assert!(run.timeout_seconds.is_none());
}

#[test]
fn rejects_unknown_run_field() {
    let yaml = r#"
name: t
steps:
  - id: score
    run:
      cmd: python3
      shell: true
"#;
    assert!(Workflow::parse(yaml).is_err(), "`shell:` must not parse");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-orchestrator parses_run_step_with_defaults -- --nocapture`
Expected: FAIL — `no field 'run' on type 'Step'`.

- [ ] **Step 3: Write minimal implementation**

Add to `workflow.rs`, immediately after the `Distribute` struct:

```rust
/// Output parsing mode for a `run:` step's stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    /// stdout binds verbatim as a string. Default.
    #[default]
    Raw,
    /// stdout is parsed as JSON; `steps.<id>.output` binds the parsed value.
    Json,
    /// stdout is split on newlines; empty trailing line dropped.
    Lines,
}

fn default_allow_exit_codes() -> Vec<i32> {
    vec![0]
}

/// A deterministic, non-LLM command step. Mutually exclusive with
/// `agent`/`prompt`, `parallel:`, `panel:`, `branch:`, `action:`,
/// `split:`, and `join:` — but deliberately COMPATIBLE with `for_each:`,
/// which fans the same command across items.
///
/// `cmd` and `args` are passed to the OS as an argv vector. There is no
/// shell: no pipes, no redirection, no globbing, no word splitting. A
/// template-rendered value containing shell metacharacters is passed as
/// literal argument text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunStep {
    /// Executable name or path. Rendered as a template.
    pub cmd: String,
    /// Argument vector. Each element is rendered as a template
    /// independently and becomes exactly one argv element.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory. Rendered as a template. Defaults to the run's
    /// workspace root when absent.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables. Values are rendered as templates.
    /// Merged over the inherited environment.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// How to interpret stdout when binding `steps.<id>.output`.
    #[serde(default)]
    pub parse: ParseMode,
    /// Kill the child after this many seconds. `None` ⇒ no timeout.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Exit codes treated as success. Defaults to `[0]`.
    #[serde(default = "default_allow_exit_codes")]
    pub allow_exit_codes: Vec<i32>,
}
```

Add to `Step`, after the `with` field:

```rust
    /// Deterministic command step (Bench Plan 0). See [`RunStep`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunStep>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator run_step -- --nocapture`
Expected: PASS (both tests).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-orchestrator/src/workflow.rs
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(orchestrator): add run: step parse surface"
```

---

### Task 2: Validation and template bindings

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` — `WorkflowParseError` (~line 21), `validate_step_shape` (~line 1462), `STEP_OUTPUT_FIELDS` (~line 1845)
- Modify: `crates/rupu-orchestrator/src/runs.rs` — `StepKind` (~line 486)
- Modify: `crates/rupu-orchestrator/src/runner.rs` — `step_kind_for_run_record` (~line 4586)

**Interfaces:**
- Consumes: `RunStep`, `Step.run` from Task 1.
- Produces: `WorkflowParseError::{RunMutuallyExclusive, RunEmptyCmd, RunInvalidExitCodes}`; `StepKind::Run`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn rejects_run_with_agent() {
    let yaml = r#"
name: t
steps:
  - id: s
    agent: writer
    prompt: hi
    run: { cmd: echo }
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(matches!(err, WorkflowParseError::RunMutuallyExclusive { .. }));
}

#[test]
fn accepts_run_with_for_each() {
    // for_each + run: is the fan-out benchmark shape and MUST parse.
    let yaml = r#"
name: t
steps:
  - id: score
    for_each: "{{ inputs.jobs }}"
    max_parallel: 4
    run:
      cmd: python3
      args: ["score.py", "{{ item.id }}"]
"#;
    let wf = Workflow::parse(yaml).expect("for_each + run must parse");
    assert!(wf.steps[0].run.is_some());
    assert!(wf.steps[0].for_each.is_some());
}

#[test]
fn rejects_run_with_empty_cmd() {
    let yaml = r#"
name: t
steps:
  - id: s
    run: { cmd: "   " }
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(matches!(err, WorkflowParseError::RunEmptyCmd { .. }));
}

#[test]
fn rejects_run_with_empty_allow_exit_codes() {
    let yaml = r#"
name: t
steps:
  - id: s
    run: { cmd: echo, allow_exit_codes: [] }
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(matches!(err, WorkflowParseError::RunInvalidExitCodes { .. }));
}

#[test]
fn run_step_output_fields_are_referencable() {
    // A downstream template referencing exit_code must validate.
    let yaml = r#"
name: t
steps:
  - id: probe
    run: { cmd: echo, args: ["hi"] }
  - id: report
    agent: writer
    prompt: "code={{ steps.probe.exit_code }} out={{ steps.probe.stdout }}"
"#;
    Workflow::parse(yaml).expect("exit_code/stdout are valid step fields");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator run_ -- --nocapture`
Expected: FAIL — `RunMutuallyExclusive` does not exist; `accepts_run_with_for_each` fails with `MissingStepField { field: "agent" }`.

- [ ] **Step 3: Write the implementation**

Add to `WorkflowParseError`:

```rust
    #[error(
        "step `{step}`: `run:` is mutually exclusive with `agent`/`prompt`, `parallel:`, `panel:`, `branch:`, `action:`, `split:`, and `join:` (it IS compatible with `for_each:`)"
    )]
    RunMutuallyExclusive { step: String },
    #[error("step `{step}`: `run.cmd` must not be empty")]
    RunEmptyCmd { step: String },
    #[error("step `{step}`: `run.allow_exit_codes` must list at least one exit code")]
    RunInvalidExitCodes { step: String },
```

In `validate_step_shape`, add a new arm **before** the `else if let Some(action)` arm (so a step with both `run:` and `action:` reports the `run:` error, which names both):

```rust
    } else if let Some(run) = &step.run {
        // NOTE: `for_each` is deliberately absent from this list — a
        // `for_each:` + `run:` step fans the same command across items,
        // which is the whole point of the primitive.
        if step.agent.is_some()
            || step.prompt.is_some()
            || step.parallel.is_some()
            || step.panel.is_some()
            || step.branch.is_some()
            || step.action.is_some()
            || step.split.is_some()
            || step.join.is_some()
        {
            return Err(WorkflowParseError::RunMutuallyExclusive {
                step: step.id.clone(),
            });
        }
        if run.cmd.trim().is_empty() {
            return Err(WorkflowParseError::RunEmptyCmd {
                step: step.id.clone(),
            });
        }
        if run.allow_exit_codes.is_empty() {
            return Err(WorkflowParseError::RunInvalidExitCodes {
                step: step.id.clone(),
            });
        }
```

Extend `STEP_OUTPUT_FIELDS`:

```rust
const STEP_OUTPUT_FIELDS: &[&str] = &[
    "output",
    "success",
    "skipped",
    "results",
    "sub_results",
    "findings",
    "max_severity",
    "iterations",
    "resolved",
    "decision",
    // `run:` step fields (Bench Plan 0)
    "stdout",
    "stderr",
    "exit_code",
    "duration_ms",
];
```

Add to `StepKind` in `runs.rs`:

```rust
    /// `run:` deterministic command node (Bench Plan 0).
    Run,
```

Add to `step_kind_for_run_record` in `runner.rs`, **before** the `for_each` arm (a `for_each` + `run` step is a Run node, not a ForEach node — the fan-out is an attribute of it):

```rust
    } else if step.run.is_some() {
        crate::runs::StepKind::Run
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator -- --nocapture 2>&1 | tail -20`
Expected: PASS. Fix any non-exhaustive `match` on `StepKind` the new variant surfaces — the compiler will name each site.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-orchestrator/src/workflow.rs crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/src/runner.rs
git add -u
git commit -m "feat(orchestrator): validate run: steps and bind their output fields"
```

---

### Task 3: The pure executor

**Files:**
- Create: `crates/rupu-orchestrator/src/run_step.rs`
- Modify: `crates/rupu-orchestrator/src/lib.rs` (add `pub mod run_step;`)

**Interfaces:**
- Consumes: `RunStep`, `ParseMode` from Task 1.
- Produces: `pub struct ResolvedRun { cmd: String, args: Vec<String>, cwd: PathBuf, env: BTreeMap<String,String>, parse: ParseMode, timeout: Option<Duration>, allow_exit_codes: Vec<i32> }`; `pub struct RunStepOutput { stdout: String, stderr: String, exit_code: i32, duration_ms: u64, success: bool, parsed: serde_json::Value }`; `pub async fn execute(resolved: &ResolvedRun) -> Result<RunStepOutput, RunStepError>`; `pub enum RunStepError { Spawn, Timeout, ParseJson }`.

This module knows nothing about workflows, steps, or contexts — templates are already rendered by the caller. That is what makes it testable without a runner.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-orchestrator/src/run_step.rs` with only a test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn resolved(cmd: &str, args: &[&str]) -> ResolvedRun {
        ResolvedRun {
            cmd: cmd.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            parse: ParseMode::Raw,
            timeout: None,
            allow_exit_codes: vec![0],
        }
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let out = execute(&resolved("echo", &["hello"])).await.unwrap();
        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.exit_code, 0);
        assert!(out.success);
    }

    #[tokio::test]
    async fn argv_is_never_shell_interpreted() {
        // THE security property: a metacharacter-laden argument is passed
        // as literal text, not executed. If this ever regresses, a
        // template-rendered fixture path could execute arbitrary code.
        let evil = "; touch /tmp/rupu_pwned_marker";
        let out = execute(&resolved("echo", &[evil])).await.unwrap();
        assert_eq!(out.stdout.trim(), evil);
        assert!(
            !std::path::Path::new("/tmp/rupu_pwned_marker").exists(),
            "shell metacharacters must not be interpreted"
        );
    }

    #[tokio::test]
    async fn disallowed_exit_code_is_not_success() {
        let out = execute(&resolved("false", &[])).await.unwrap();
        assert_eq!(out.exit_code, 1);
        assert!(!out.success, "exit 1 is not in allow_exit_codes [0]");
    }

    #[tokio::test]
    async fn custom_allow_exit_codes_accepts_nonzero() {
        let mut r = resolved("false", &[]);
        r.allow_exit_codes = vec![0, 1];
        let out = execute(&r).await.unwrap();
        assert!(out.success);
    }

    #[tokio::test]
    async fn timeout_kills_the_child() {
        let mut r = resolved("sleep", &["30"]);
        r.timeout = Some(std::time::Duration::from_millis(200));
        let started = std::time::Instant::now();
        let err = execute(&r).await.unwrap_err();
        assert!(matches!(err, RunStepError::Timeout { .. }));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn json_parse_mode_binds_a_value() {
        let mut r = resolved("echo", &[r#"{"score": 91}"#]);
        r.parse = ParseMode::Json;
        let out = execute(&r).await.unwrap();
        assert_eq!(out.parsed["score"], 91);
    }

    #[tokio::test]
    async fn json_parse_mode_fails_loudly_on_bad_json() {
        let mut r = resolved("echo", &["not json"]);
        r.parse = ParseMode::Json;
        assert!(matches!(
            execute(&r).await.unwrap_err(),
            RunStepError::ParseJson { .. }
        ));
    }

    #[tokio::test]
    async fn lines_parse_mode_drops_trailing_empty() {
        let mut r = resolved("printf", &["a\nb\n"]);
        r.parse = ParseMode::Lines;
        let out = execute(&r).await.unwrap();
        assert_eq!(out.parsed, serde_json::json!(["a", "b"]));
    }

    #[tokio::test]
    async fn missing_executable_is_a_spawn_error() {
        let err = execute(&resolved("rupu_no_such_binary_xyz", &[])).await.unwrap_err();
        assert!(matches!(err, RunStepError::Spawn { .. }));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator --lib run_step`
Expected: FAIL to compile — `ResolvedRun`, `execute`, `RunStepError` undefined.

- [ ] **Step 3: Write the implementation**

Prepend to `run_step.rs`:

```rust
//! Pure executor for `run:` steps. Templates are already rendered by the
//! caller; this module turns a fully-resolved command into a captured
//! result. No shell is involved at any point.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::workflow::ParseMode;

/// A `run:` step with every template already rendered.
#[derive(Debug, Clone)]
pub struct ResolvedRun {
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub parse: ParseMode,
    pub timeout: Option<Duration>,
    pub allow_exit_codes: Vec<i32>,
}

/// Captured result of one command execution.
#[derive(Debug, Clone)]
pub struct RunStepOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    /// True when `exit_code` is in `allow_exit_codes`.
    pub success: bool,
    /// stdout interpreted per `ParseMode`. `Raw` ⇒ a JSON string.
    pub parsed: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum RunStepError {
    #[error("failed to spawn `{cmd}`: {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{cmd}` exceeded its {timeout_secs}s timeout and was killed")]
    Timeout { cmd: String, timeout_secs: u64 },
    #[error("`{cmd}` stdout is not valid JSON (parse: json): {source}")]
    ParseJson {
        cmd: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Execute a resolved command. Never invokes a shell: `cmd` and `args`
/// go to the OS as an argv vector, so shell metacharacters in an
/// argument are inert literal text.
pub async fn execute(r: &ResolvedRun) -> Result<RunStepOutput, RunStepError> {
    let started = Instant::now();

    let mut command = tokio::process::Command::new(&r.cmd);
    command
        .args(&r.args)
        .current_dir(&r.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in &r.env {
        command.env(k, v);
    }

    let child = command.spawn().map_err(|source| RunStepError::Spawn {
        cmd: r.cmd.clone(),
        source,
    })?;

    let output = match r.timeout {
        Some(t) => match tokio::time::timeout(t, child.wait_with_output()).await {
            Ok(res) => res.map_err(|source| RunStepError::Spawn {
                cmd: r.cmd.clone(),
                source,
            })?,
            // `kill_on_drop(true)` reaps the child when the future drops.
            Err(_) => {
                return Err(RunStepError::Timeout {
                    cmd: r.cmd.clone(),
                    timeout_secs: t.as_secs(),
                })
            }
        },
        None => child
            .wait_with_output()
            .await
            .map_err(|source| RunStepError::Spawn {
                cmd: r.cmd.clone(),
                source,
            })?,
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // A signal-terminated child has no code; -1 is distinguishable from
    // any real exit status and is never in a sane allow-list.
    let exit_code = output.status.code().unwrap_or(-1);
    let success = r.allow_exit_codes.contains(&exit_code);

    let parsed = match r.parse {
        ParseMode::Raw => serde_json::Value::String(stdout.clone()),
        ParseMode::Json => {
            serde_json::from_str(stdout.trim()).map_err(|source| RunStepError::ParseJson {
                cmd: r.cmd.clone(),
                source,
            })?
        }
        ParseMode::Lines => serde_json::Value::Array(
            stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| serde_json::Value::String(l.to_string()))
                .collect(),
        ),
    };

    Ok(RunStepOutput {
        stdout,
        stderr,
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
        success,
        parsed,
    })
}
```

Add to `crates/rupu-orchestrator/src/lib.rs`:

```rust
pub mod run_step;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --lib run_step -- --nocapture`
Expected: PASS, all nine tests.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-orchestrator/src/run_step.rs crates/rupu-orchestrator/src/lib.rs
git add crates/rupu-orchestrator/src/run_step.rs crates/rupu-orchestrator/src/lib.rs
git commit -m "feat(orchestrator): pure argv executor for run: steps"
```

---

### Task 4: Config gate and executable allowlist

**Files:**
- Modify: `crates/rupu-config/src/policy_config.rs`
- Test: `crates/rupu-config/src/policy_config.rs` (inline tests)

**Interfaces:**
- Produces: `pub struct WorkflowConfig { run_step_enabled: bool, run_step_allowlist: Vec<String> }` with `WorkflowConfig::default()` ⇒ `{ run_step_enabled: false, run_step_allowlist: vec![] }`, and `pub fn allows(&self, cmd: &str) -> bool`.

**Default is `false`.** `run:` executes arbitrary commands, so it is opt-in per workspace. A workflow containing a `run:` step fails to load when the toggle is off — it does not skip the step.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn run_step_is_disabled_by_default() {
    let c = WorkflowConfig::default();
    assert!(!c.run_step_enabled, "run: must be opt-in");
}

#[test]
fn empty_allowlist_permits_any_executable_when_enabled() {
    let c = WorkflowConfig {
        run_step_enabled: true,
        run_step_allowlist: vec![],
    };
    assert!(c.allows("python3"));
}

#[test]
fn allowlist_matches_on_basename_not_path() {
    // `cmd: /usr/bin/python3` and `cmd: python3` must gate identically,
    // otherwise the allowlist is trivially bypassed by absolute path.
    let c = WorkflowConfig {
        run_step_enabled: true,
        run_step_allowlist: vec!["python3".into()],
    };
    assert!(c.allows("python3"));
    assert!(c.allows("/usr/bin/python3"));
    assert!(!c.allows("bash"));
    assert!(!c.allows("/bin/bash"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-config workflow_config`
Expected: FAIL — `WorkflowConfig` undefined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// `[workflow]` config block. Gates the `run:` step kind, which executes
/// declared commands and is therefore opt-in per workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowConfig {
    /// Whether `run:` steps may execute. Default `false`. A workflow
    /// containing a `run:` step fails to LOAD when this is off — it does
    /// not silently skip the step.
    pub run_step_enabled: bool,
    /// Optional allowlist of permitted executables, matched on basename.
    /// Empty ⇒ any executable is permitted (when `run_step_enabled`).
    pub run_step_allowlist: Vec<String>,
}

impl WorkflowConfig {
    /// True if `cmd` may be executed. Matches on basename so an absolute
    /// path cannot bypass the list.
    pub fn allows(&self, cmd: &str) -> bool {
        if self.run_step_allowlist.is_empty() {
            return true;
        }
        let base = std::path::Path::new(cmd)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd);
        self.run_step_allowlist.iter().any(|a| a == base)
    }
}
```

Wire `workflow: WorkflowConfig` into the top-level config struct alongside the existing `cp: CpConfig` field, following the same `#[serde(default)]` pattern.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-config`
Expected: PASS.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-config/src/policy_config.rs
git add -u
git commit -m "feat(config): add [workflow] run_step gate and allowlist"
```

---

### Task 5: Permission gating

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`RunWorkflowError`, ~line 114)
- Create: `crates/rupu-orchestrator/src/run_step.rs` — add `gate` fn to the existing module
- Test: inline in `run_step.rs`

**Interfaces:**
- Consumes: `PermissionMode` from `rupu_tools::permission`, `WorkflowConfig` from Task 4.
- Produces: `pub enum RunGateDecision { Allow, NeedsOperatorDecision, Denied(RunDenyReason) }`, `pub enum RunDenyReason { ReadonlyMode, ConfigDisabled, NotAllowlisted }`, `pub fn gate(mode: PermissionMode, cfg: &WorkflowConfig, cmd: &str) -> RunGateDecision`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod gate_tests {
    use super::*;
    use rupu_config::policy_config::WorkflowConfig;
    use rupu_tools::permission::PermissionMode;

    fn enabled() -> WorkflowConfig {
        WorkflowConfig { run_step_enabled: true, run_step_allowlist: vec![] }
    }

    #[test]
    fn readonly_denies_run_steps() {
        assert!(matches!(
            gate(PermissionMode::Readonly, &enabled(), "python3"),
            RunGateDecision::Denied(RunDenyReason::ReadonlyMode)
        ));
    }

    #[test]
    fn ask_requires_an_operator_decision() {
        assert!(matches!(
            gate(PermissionMode::Ask, &enabled(), "python3"),
            RunGateDecision::NeedsOperatorDecision
        ));
    }

    #[test]
    fn bypass_allows() {
        assert!(matches!(
            gate(PermissionMode::Bypass, &enabled(), "python3"),
            RunGateDecision::Allow
        ));
    }

    #[test]
    fn config_disabled_denies_even_under_bypass() {
        // The config gate is not overridable by permission mode.
        let cfg = WorkflowConfig::default();
        assert!(matches!(
            gate(PermissionMode::Bypass, &cfg, "python3"),
            RunGateDecision::Denied(RunDenyReason::ConfigDisabled)
        ));
    }

    #[test]
    fn non_allowlisted_executable_denies_under_bypass() {
        let cfg = WorkflowConfig {
            run_step_enabled: true,
            run_step_allowlist: vec!["python3".into()],
        };
        assert!(matches!(
            gate(PermissionMode::Bypass, &cfg, "/bin/bash"),
            RunGateDecision::Denied(RunDenyReason::NotAllowlisted)
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator gate_tests`
Expected: FAIL to compile — `gate` undefined.

- [ ] **Step 3: Write the implementation**

Add to `run_step.rs`:

```rust
use rupu_config::policy_config::WorkflowConfig;
use rupu_tools::permission::PermissionMode;

/// Why a `run:` step was refused. Each variant is a hard stop — no
/// operator decision changes the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDenyReason {
    /// `--mode readonly`: `run:` executes, so it is write-class.
    ReadonlyMode,
    /// `[workflow].run_step_enabled = false`.
    ConfigDisabled,
    /// `[workflow].run_step_allowlist` does not contain this executable.
    NotAllowlisted,
}

/// Outcome of gating one `run:` step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunGateDecision {
    Allow,
    /// `ask` mode: the CLI prompts the operator with the fully-resolved
    /// argv, cwd, and env keys before this runs.
    NeedsOperatorDecision,
    Denied(RunDenyReason),
}

/// Pure gate for a `run:` step. Config is checked BEFORE permission
/// mode, so `--mode bypass` cannot override a workspace that has not
/// opted in.
pub fn gate(mode: PermissionMode, cfg: &WorkflowConfig, cmd: &str) -> RunGateDecision {
    if !cfg.run_step_enabled {
        return RunGateDecision::Denied(RunDenyReason::ConfigDisabled);
    }
    if !cfg.allows(cmd) {
        return RunGateDecision::Denied(RunDenyReason::NotAllowlisted);
    }
    match mode {
        PermissionMode::Readonly => RunGateDecision::Denied(RunDenyReason::ReadonlyMode),
        PermissionMode::Ask => RunGateDecision::NeedsOperatorDecision,
        PermissionMode::Bypass => RunGateDecision::Allow,
    }
}
```

Add to `RunWorkflowError` in `runner.rs`:

```rust
    #[error("run step `{step}` refused: {reason}")]
    RunStepDenied { step: String, reason: String },
    #[error("run step `{step}` failed: {source}")]
    RunStep {
        step: String,
        #[source]
        source: crate::run_step::RunStepError,
    },
```

Add `rupu-config` and `rupu-tools` to `rupu-orchestrator`'s `Cargo.toml` dependencies if not already present, using `workspace = true`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator gate_tests`
Expected: PASS, all five.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-orchestrator/src/run_step.rs crates/rupu-orchestrator/src/runner.rs
git add -u
git commit -m "feat(orchestrator): gate run: steps by mode, config, and allowlist"
```

---

### Task 6: Runner dispatch for linear `run:` steps

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` — dispatch chain (~line 4395), add `execute_run_step` near `execute_action_step` (~line 4827)
- Test: `crates/rupu-orchestrator/tests/run_step_workflow.rs` (new integration test)

**Interfaces:**
- Consumes: `execute`, `gate`, `ResolvedRun` from Tasks 3 and 5; `render_step_prompt` and `StepContext` from existing runner code.
- Produces: `async fn execute_run_step(step: &Step, ctx: &StepContext, mode: RenderMode, perm: PermissionMode, cfg: &WorkflowConfig, continue_on_error: bool, workspace_root: &Path) -> Result<StepResult, RunWorkflowError>`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-orchestrator/tests/run_step_workflow.rs`:

```rust
//! End-to-end: a `run:` step executes and binds its output downstream.

#[tokio::test]
async fn run_step_binds_output_to_next_step() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run:
      cmd: echo
      args: ["{\"score\": 91}"]
      parse: json
  - id: gate
    run:
      cmd: test
      args: ["91", "-eq", "{{ steps.probe.output.score }}"]
"#;
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).expect("parses");
    let outcome = run_with_bypass_and_run_enabled(&wf).await.expect("runs");
    assert!(outcome.step("probe").success);
    assert_eq!(outcome.step("probe").exit_code, Some(0));
    assert!(
        outcome.step("gate").success,
        "steps.probe.output.score must render as 91"
    );
}

#[tokio::test]
async fn failing_run_step_fails_the_run() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: boom
    run: { cmd: "false" }
"#;
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).expect("parses");
    let err = run_with_bypass_and_run_enabled(&wf).await.unwrap_err();
    assert!(
        format!("{err}").contains("boom"),
        "a disallowed exit code must fail the run, not pass silently"
    );
}

#[tokio::test]
async fn readonly_mode_refuses_a_run_step() {
    let yaml = r#"
name: bench-smoke
steps:
  - id: probe
    run: { cmd: echo, args: ["hi"] }
"#;
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).expect("parses");
    let err = run_with_readonly(&wf).await.unwrap_err();
    assert!(format!("{err}").contains("refused"));
}
```

Write the `run_with_bypass_and_run_enabled` / `run_with_readonly` helpers against the crate's existing `run_workflow` entry point, mirroring the harness used by the existing action-step integration tests. Grep for an existing `tests/` file that drives `run_workflow` and copy its option construction.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-orchestrator --test run_step_workflow`
Expected: FAIL — the `run:` step is not dispatched; the step falls through to the linear arm and errors on a missing agent.

- [ ] **Step 3: Write the implementation**

Add `execute_run_step` next to `execute_action_step`:

```rust
async fn execute_run_step(
    step: &Step,
    ctx: &StepContext,
    mode: RenderMode,
    perm: rupu_tools::permission::PermissionMode,
    cfg: &rupu_config::policy_config::WorkflowConfig,
    continue_on_error: bool,
    workspace_root: &Path,
) -> Result<StepResult, RunWorkflowError> {
    use crate::run_step::{execute, gate, ParseModeExt, ResolvedRun, RunGateDecision};

    let spec = step
        .run
        .as_ref()
        .expect("execute_run_step called for a non-run step");

    // Render every template BEFORE gating, so the operator prompt and the
    // allowlist both see the real command rather than the template.
    let render = |s: &str| -> Result<String, RunWorkflowError> {
        render_step_prompt(s, ctx, mode).map_err(|source| RunWorkflowError::Render {
            step: step.id.clone(),
            source,
        })
    };

    let cmd = render(&spec.cmd)?;
    let mut args = Vec::with_capacity(spec.args.len());
    for a in &spec.args {
        // Each arg renders independently and becomes exactly one argv
        // element — a rendered value is never re-split on whitespace.
        args.push(render(a)?);
    }
    let cwd = match &spec.cwd {
        Some(c) => PathBuf::from(render(c)?),
        None => workspace_root.to_path_buf(),
    };
    let mut env = std::collections::BTreeMap::new();
    for (k, v) in &spec.env {
        env.insert(k.clone(), render(v)?);
    }

    match gate(perm, cfg, &cmd) {
        RunGateDecision::Allow => {}
        RunGateDecision::NeedsOperatorDecision => {
            // `ask` is resolved by the CLI before the runner is entered.
            // Reaching here means no decider was wired — refuse rather
            // than assume approval.
            return Err(RunWorkflowError::RunStepDenied {
                step: step.id.clone(),
                reason: "ask mode requires an operator decision, but no decider is wired"
                    .to_string(),
            });
        }
        RunGateDecision::Denied(reason) => {
            return Err(RunWorkflowError::RunStepDenied {
                step: step.id.clone(),
                reason: format!("{reason:?}"),
            });
        }
    }

    let resolved = ResolvedRun {
        cmd,
        args,
        cwd,
        env,
        parse: spec.parse,
        timeout: spec.timeout_seconds.map(std::time::Duration::from_secs),
        allow_exit_codes: spec.allow_exit_codes.clone(),
    };

    let out = execute(&resolved).await.map_err(|source| RunWorkflowError::RunStep {
        step: step.id.clone(),
        source,
    })?;

    if !out.success && !continue_on_error {
        return Err(RunWorkflowError::RunStepDenied {
            step: step.id.clone(),
            reason: format!(
                "exit code {} not in allow_exit_codes; stderr: {}",
                out.exit_code,
                out.stderr.trim()
            ),
        });
    }

    Ok(step_result_from_run_output(step, &out))
}
```

Write `step_result_from_run_output` to populate the existing `StepResult` shape: `output` = `out.parsed` serialised (or `out.stdout` for `Raw`), `success` = `out.success`, and the new `stdout` / `stderr` / `exit_code` / `duration_ms` fields.

Add a dispatch arm in the chain at ~line 4395, **before** the `else if step.action.is_some()` arm:

```rust
    } else if step.run.is_some() && step.for_each.is_none() {
        execute_run_step(
            step,
            ctx,
            render_mode(opts.strict_templates),
            opts.permission_mode,
            &opts.workflow_config,
            effective_continue_on_error,
            &opts.workspace_root,
        )
        .await
```

Add `permission_mode`, `workflow_config`, and `workspace_root` to the runner's options struct if not already present, defaulting to `PermissionMode::Ask`, `WorkflowConfig::default()`, and the run's workspace root.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --test run_step_workflow -- --nocapture`
Expected: PASS, all three.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/tests/run_step_workflow.rs
git add -u crates/rupu-orchestrator
git commit -m "feat(orchestrator): dispatch linear run: steps"
```

---

### Task 7: `for_each` + `run:` fan-out

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` — fan-out path (`run_steps_over` / the `FanoutStepOutcome` producer)
- Test: `crates/rupu-orchestrator/tests/run_step_workflow.rs`

**Interfaces:**
- Consumes: `execute_run_step` from Task 6.
- Produces: per-unit `ItemResultRecord`s where `output`/`success` come from the command, and `unit_checkpoints.jsonl` entries so a partially-completed fan-out resumes.

This is the shape both benchmarks depend on: score N items concurrently, resume the failures.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn for_each_run_fans_out_per_item() {
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '["alpha", "beta", "gamma"]'
    max_parallel: 3
    run:
      cmd: echo
      args: ["{{ item }}"]
"#;
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).expect("parses");
    let outcome = run_with_bypass_and_run_enabled(&wf).await.expect("runs");
    let items = outcome.step("score").items;
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].output.trim(), "alpha");
    assert_eq!(items[1].output.trim(), "beta");
    assert_eq!(items[2].output.trim(), "gamma");
    assert!(items.iter().all(|i| i.success));
}

#[tokio::test]
async fn for_each_run_continue_on_error_records_the_failure_and_proceeds() {
    // A benchmark must not lose 199 results because item 3 exits nonzero.
    let yaml = r#"
name: bench-fanout
steps:
  - id: score
    for_each: '["0", "1", "0"]'
    max_parallel: 1
    continue_on_error: true
    run:
      cmd: sh
      args: ["-c", "exit {{ item }}"]
"#;
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).expect("parses");
    let outcome = run_with_bypass_and_run_enabled(&wf).await.expect("runs");
    let items = outcome.step("score").items;
    assert_eq!(items.len(), 3);
    assert!(items[0].success);
    assert!(!items[1].success, "the failing unit is recorded, not dropped");
    assert!(items[2].success, "later units still dispatch");
}
```

Note: the second test uses `sh -c` deliberately — that is an *explicitly authored* shell invocation, which is allowed. What the executor must never do is wrap an author's `cmd`/`args` in a shell implicitly.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-orchestrator --test run_step_workflow for_each_run`
Expected: FAIL — the fan-out path assumes an agent dispatch per unit.

- [ ] **Step 3: Write the implementation**

In the fan-out unit dispatcher, branch on `step.run.is_some()`: instead of dispatching an agent, build the per-unit `StepContext` (binding `item` and `loop.index` exactly as the agent path does) and call `execute_run_step` with `continue_on_error` forced true at the unit level, converting the result into an `ItemResultRecord`:

```rust
let record = ItemResultRecord {
    index,
    item: item_value.clone(),
    sub_id: String::new(),
    rendered_prompt: String::new(), // run: steps have no prompt
    run_id: run_id.to_string(),
    transcript_path: unit_transcript_path.clone(),
    output: unit_output,
    success: unit_success,
};
```

Write the command's stdout/stderr/exit code as a single JSONL record to `unit_transcript_path` so the CP and app can display a `run:` unit the same way they display an agent unit. Append the `UnitCheckpoint` on completion exactly as the agent path does, so resume skips already-successful units.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --test run_step_workflow -- --nocapture`
Expected: PASS, all five tests in the file.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/tests/run_step_workflow.rs
git add -u crates/rupu-orchestrator
git commit -m "feat(orchestrator): fan out run: steps over for_each"
```

---

### Task 8: CLI `ask`-mode prompt

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (wherever run options are constructed)
- Test: `crates/rupu-cli/tests/` — a test asserting the prompt text contains the resolved argv

**Interfaces:**
- Consumes: `RunGateDecision::NeedsOperatorDecision` from Task 5.
- Produces: an operator prompt showing the **fully-resolved** argv, cwd, and env *keys* (never env values — they carry API keys).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn run_step_prompt_shows_resolved_argv_and_hides_env_values() {
    let prompt = render_run_step_prompt(
        "score",
        "python3",
        &["tools/score.py".into(), "fixtures/kr/001".into()],
        std::path::Path::new("/work"),
        &["CYBERBENCH_RELEASE_KEY".into()],
    );
    assert!(prompt.contains("python3 tools/score.py fixtures/kr/001"));
    assert!(prompt.contains("/work"));
    assert!(prompt.contains("CYBERBENCH_RELEASE_KEY"));
    assert!(
        !prompt.contains("release-secret"),
        "env VALUES must never be printed"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli run_step_prompt`
Expected: FAIL — `render_run_step_prompt` undefined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// Operator-facing description of a pending `run:` step. Shows the
/// resolved argv so the operator approves what will ACTUALLY execute,
/// not the template. Env keys are listed; values never are.
pub fn render_run_step_prompt(
    step_id: &str,
    cmd: &str,
    args: &[String],
    cwd: &std::path::Path,
    env_keys: &[String],
) -> String {
    let argv = std::iter::once(cmd.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let mut s = format!("run step `{step_id}` will execute:\n  {argv}\n  cwd: {}\n", cwd.display());
    if !env_keys.is_empty() {
        s.push_str(&format!("  env: {}\n", env_keys.join(", ")));
    }
    s
}
```

Wire it into the workflow-run path so `NeedsOperatorDecision` prompts with this text and maps a decline to `RunWorkflowError::RunStepDenied`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cli run_step_prompt`
Expected: PASS.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -- crates/rupu-cli/src/cmd/workflow.rs
git add -u crates/rupu-cli
git commit -m "feat(cli): prompt with resolved argv before running a run: step"
```

---

### Task 9: Renderers and author docs

**Files:**
- Modify: `crates/rupu-app-canvas/src/git_graph.rs` (`render_rows`)
- Create: `crates/rupu-app-canvas/tests/snapshots/` entry via `insta`
- Modify: `crates/rupu-cp/web/src/components/graph/kindBridge.ts`
- Create: `docs/workflows.md` section (or modify if it exists)

**Interfaces:**
- Consumes: `StepKind::Run` from Task 2.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn renders_a_run_node_row() {
    let wf = Workflow::parse(
        r#"
name: t
steps:
  - id: score
    run: { cmd: python3, args: ["score.py"] }
"#,
    )
    .unwrap();
    let rows = render_rows(&wf, |_| NodeStatus::Waiting);
    insta::assert_debug_snapshot!(rows);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app-canvas renders_a_run_node_row`
Expected: FAIL — no snapshot yet, and the row carries no `run` meta label.

- [ ] **Step 3: Write minimal implementation**

In `render_rows`, add a `run:` arm alongside the existing `is_approval_gate(step)` and action arms, emitting a `GraphCell::Meta` of the form `run · python3`. Accept the insta snapshot with `cargo insta accept`.

In `kindBridge.ts`, add a `run` entry to the per-kind palette map, following the existing `gate`/`action` entries. Give it a distinct colour and glyph so a deterministic node is visually separable from an agent node.

Add a `docs/workflows.md` section documenting: the full `run:` field set, that there is no shell, the three `parse:` modes, the `steps.<id>.stdout/.stderr/.exit_code/.duration_ms` bindings, the `for_each` + `run:` fan-out shape, and the `[workflow].run_step_enabled` opt-in with its allowlist.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-app-canvas && cargo test --workspace 2>&1 | tail -20`
Expected: PASS across the workspace.

- [ ] **Step 5: Verify lints and commit**

```bash
cargo clippy --workspace --all-targets 2>&1 | tail -20   # expect no warnings
cargo fmt -- crates/rupu-app-canvas/src/git_graph.rs
git add -u
git add crates/rupu-app-canvas/tests/snapshots
git commit -m "feat: render run: nodes in canvas + CP, document the step kind"
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `run:` field set (`cmd`/`args`/`cwd`/`env`/`parse`/`timeout_seconds`/`allow_exit_codes`) | 1 |
| Argv vector, never a shell string | 3 (asserted by `argv_is_never_shell_interpreted`) |
| Bindings `output`/`stdout`/`stderr`/`exit_code`/`success`/`duration_ms` | 2, 6 |
| Per-unit bindings under `for_each` | 7 |
| Composes with `for_each` + `max_parallel` | 2 (validation), 7 (execution) |
| Mutual exclusivity with other step kinds | 2 |
| `readonly` refuses / `ask` prompts with resolved argv / `bypass` executes | 5, 8 |
| `[workflow].run_step_enabled` + allowlist | 4 |
| Workflow with `run:` fails to LOAD when toggle off | 4, 5 (`ConfigDisabled` denies even under bypass) |
| Renders in canvas `GraphRow` and CP | 9 |
| Inherits `unit_checkpoints.jsonl` resume | 7 |
| Non-goal: no shell/pipes/redirection | 3 (no `sh -c` wrapping anywhere in `execute`) |
| Non-goal: no interactive stdin | 3 (`Stdio::null()`) |

**Deferred with reason:** `distribute:` (fleet placement) for `run:` steps is *not* implemented here. The spec's workflows declare `distribute:` on the `solve` agent step, not on `run:` steps, and Plan 0's scope is local execution. Remote `run:` placement is a follow-on; validation should therefore reject `distribute:` on a `run:` step rather than silently ignore it — **add this to Task 2** as a fourth error variant `RunWithDistribute` if the existing `validate_rejects_distribute_without_for_each` check does not already cover it.

**Type consistency:** `ParseMode` is defined in `workflow.rs` (Task 1) and imported by `run_step.rs` (Task 3) — one definition, no duplicate. `RunStepOutput.parsed` is `serde_json::Value` in Task 3 and consumed as such by `step_result_from_run_output` in Task 6. `WorkflowConfig::allows` is defined in Task 4 and called by `gate` in Task 5 with the same signature.

**Known gap for the implementer:** Task 6's `step_result_from_run_output` requires `StepResult`/`StepResultRecord` to carry `stdout`, `stderr`, `exit_code`, and `duration_ms`. Those fields do not exist today (`StepResultRecord` is at `crates/rupu-orchestrator/src/runs.rs:561`). Add them as `#[serde(default, skip_serializing_if = "Option::is_none")] Option<...>` so existing on-disk `step_results.jsonl` round-trips byte-for-byte, matching the precedent set by `loop_iteration` on that same struct.
