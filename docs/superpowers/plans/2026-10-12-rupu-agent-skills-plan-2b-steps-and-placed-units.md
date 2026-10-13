# Agent Skills Plan 2b: workflow steps and placed units

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A workflow step's `skills:` / `preload_skills:` reach every agent the step launches, locally and on placed units (`host:` / `distribute:`), through `rupu run --skill` / `--preload-skill` and an `agent.skills` peer capability that every connector checks. A connector never silently drops a step's skills.

**Architecture:** Plan 2a gave the assembler `Overrides.skills` / `Overrides.preload_skills` (reason `step`). This plan fills them: locally in `DefaultStepFactory::launch_for_step`, which every substep, panelist, fixer and loop member already reaches through its parent step; remotely through `UnitDispatch` → `AgentLaunchRequest` → `AgentRun` → `--skill` flags, which `rupu run` maps back onto `Overrides`. Delivery follows the `findings_profile` contract: HTTP, tunnel and bucket refuse a peer that didn't advertise `agent.skills`; SSH gates the launch on the remote's `rupu __features` inside the same ssh command (the `resume_command` pattern) and refuses on a marker; local is the same binary.

**Tech Stack:** Rust 2021, existing crates.

**Spec:** `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §2.2, §9, §10, §11. Plan 2a must be merged first: this plan uses `Overrides.skills`, `AssembleError::Skills` and `rupu_skills::parse_grants`.

## Global Constraints

- Workspace deps only. `rupu-orchestrator` gains `rupu-skills = { workspace = true }` (for entry syntax checks).
- Workflow parsing checks entry syntax only. Whether a skill is installed is decided where the agent runs (the assembler), so a workflow's validity never depends on the host parsing it.
- There is no workflow-level `skills:`; `SubStep` gets none (its parent step controls the block, as with `actions:`).
- Every connector delivers a unit's skills or refuses the launch with a message naming the host and "upgrade rupu there". Never a silent drop.
- One spelling per flag in `to_args` (`--skill <name>` repeated, `--preload-skill <name>` repeated); `argv_roundtrip` holds it.
- Format only touched files; never package-wide `cargo fmt`; never bare `git stash`.

## Review Focus

1. **A `parallel:` sub-step, a panelist and a panel's `fix_with` agent** get the parent step's skills. Pinned in Task 1.
2. **A step naming a skill the host doesn't have** fails that step with a recorded, unstarted run whose message names the skill, not a crash and not a run without the skill. Pinned in Task 1 (local) and Task 3 (placed; the remote's own assembler refuses the same way).
3. **An SSH host running an older rupu** refuses a unit with step skills before anything runs, instead of a detached launch dying on an unknown flag with no trace. Pinned in Task 5.
4. **A `distribute:` unit retried on a fallback host** keeps its skills. Pinned in Task 3.
5. **Saving a workflow in the CP editor** keeps a step's `skills:` keys. Pinned in Task 6.

---

### Task 1: Step keys, validation and local delivery

**Files:**
- Modify: `crates/rupu-orchestrator/Cargo.toml`, `src/workflow.rs` (`Step`, `WorkflowParseError`, `Workflow::parse`), `src/runner.rs` (the `Step { .. }` literals at `:4187` and `:6857`), `src/step_factory.rs` (`launch_for_step`), `src/generate.rs` (`WORKFLOW_SYSTEM_PROMPT` step keys), `src/testing.rs` (`MockRun` if it mirrors step fields)
- Test: `workflow.rs` unit tests, `step_factory.rs` unit tests, `crates/rupu-orchestrator/tests/it/step_failure_stub.rs`

**Interfaces:**
- Produces: `Step.skills: Vec<String>`, `Step.preload_skills: Vec<String>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`); `WorkflowParseError::{SkillsOnNonAgentStep { step: String }, BadSkillEntry { step: String, message: String }}`.

- [ ] **Step 1: Write the failing tests**

`workflow.rs` tests (next to the findings-profile ones):

```rust
#[test]
fn step_skills_parse_and_round_trip() {
    let wf = Workflow::parse(
        "name: w\nsteps:\n  - id: a\n    agent: reviewer\n    prompt: go\n    skills: [code-review, \"project:*\"]\n    preload_skills: [report-style]\n",
    )
    .unwrap();
    assert_eq!(wf.steps[0].skills, vec!["code-review".to_string(), "project:*".to_string()]);
    assert_eq!(wf.steps[0].preload_skills, vec!["report-style".to_string()]);
    let yaml = serde_yaml::to_string(&wf.steps[0]).unwrap();
    assert!(yaml.contains("skills:"), "{yaml}");
}

#[test]
fn step_skills_are_refused_where_no_agent_runs_and_on_bad_syntax() {
    let gate = "name: w\nsteps:\n  - id: g\n    approval: { prompt: ok? }\n    skills: [x]\n";
    assert!(matches!(Workflow::parse(gate), Err(WorkflowParseError::SkillsOnNonAgentStep { .. })));
    let bad = "name: w\nsteps:\n  - id: a\n    agent: r\n    prompt: go\n    skills: [Bad_Name]\n";
    assert!(matches!(Workflow::parse(bad), Err(WorkflowParseError::BadSkillEntry { .. })));
    let placed = "name: w\nsteps:\n  - id: a\n    agent: r\n    prompt: go\n    host: mini\n    skills: [x]\n";
    assert!(Workflow::parse(placed).is_ok(), "skills travel to placed units; no refusal");
}
```

Adjust the gate YAML to whatever `approval:`-only step shape the existing `FindingsProfileOnNonAgentStep` tests use.

`step_factory.rs` tests (siblings of `step_actions_reach_the_run_with_the_agent_grant` at `:265`): build a workflow whose step has `skills: [code-review]`, `preload_skills: [report-style]`, plus a `parallel:` block, a `panel:` with `fix_with`; for each `StepRequest` shape the runner sends (`step_id` = the parent step's id, `agent_name` = the sub-agent / panelist / fixer), call `launch_for_step` and assert `spec.overrides.skills == ["code-review"]` and `spec.overrides.preload_skills == ["report-style"]`.

`tests/it/step_failure_stub.rs`: a step with `skills: [missing]` on an agent that exists. The step fails, its recorded transcript ends in the unstarted-run shape the file already pins (`run_start`, `model_limits` notice, …, a `Preflight` close) and the message contains `unknown skill "missing"`.

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-orchestrator --lib workflow:: step_factory::`, `cargo test -p rupu-orchestrator --test it step_failure_stub::`
Expected: compile errors, then failures.

- [ ] **Step 3: Implement**

`Step` (after `findings_profile`):

```rust
    /// Skills every agent this step launches may load, on top of each
    /// agent's own (spec §2.2). Syntax is checked here; existence where the
    /// agent runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Skills whose bodies are in every launched agent's system prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preload_skills: Vec<String>,
```

Errors:

```rust
    #[error("step `{step}`: skills: apply only to steps that run an agent")]
    SkillsOnNonAgentStep { step: String },
    #[error("step `{step}`: skills: {message}")]
    BadSkillEntry { step: String, message: String },
```

Validation, called from `Workflow::parse` next to `validate_step_actions(step)?` and for `on_reject` subs where `validate_step_actions` is:

```rust
fn validate_step_skills(step: &Step) -> Result<(), WorkflowParseError> {
    if step.skills.is_empty() && step.preload_skills.is_empty() {
        return Ok(());
    }
    if runs_no_agent(step) || step.action.is_some() {
        return Err(WorkflowParseError::SkillsOnNonAgentStep { step: step.id.clone() });
    }
    let all: Vec<String> = step.skills.iter().chain(&step.preload_skills).cloned().collect();
    rupu_skills::parse_grants(&all).map_err(|e| WorkflowParseError::BadSkillEntry {
        step: step.id.clone(),
        message: e.to_string(),
    })?;
    Ok(())
}
```

`runs_no_agent` is the predicate the findings-profile check uses (`workflow.rs:1990-2028`); reuse it, or extract it if it is inline. `step.action` stands for the field that marks an `action:` step; use its real name.

The two `Step { .. }` literals in `runner.rs`: add `skills: Vec::new(), preload_skills: Vec::new(),`.

`step_factory.rs` `launch_for_step`, after `spec.overrides.findings_profile = step.findings_profile;`:

```rust
        spec.overrides.skills = step.skills.clone();
        spec.overrides.preload_skills = step.preload_skills.clone();
```

`generate.rs` `WORKFLOW_SYSTEM_PROMPT`: in the step-keys list add `skills: []  # optional: skills every agent of this step may load` and `preload_skills: []  # optional: skills always in their prompt`.

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-orchestrator --lib`, `cargo test -p rupu-orchestrator --test it`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator Cargo.lock
git commit -m "feat(orchestrator): step skills and preload_skills reach every agent a step launches"
```

---

### Task 2: `rupu run --skill` / `--preload-skill` and the argv

**Files:**
- Modify: `crates/rupu-runtime/src/argv.rs` (`AgentRun`, `AgentRun::new`, `to_args` Agent arm, unit test `an_agent_run_renders_one_spelling_per_flag`)
- Modify: `crates/rupu-cli/src/cmd/run.rs` (`Args`, the `Overrides { .. }` at `:896`, test literals at `:1338` / `:2033`)
- Test: `crates/rupu-cli/tests/it/argv_roundtrip.rs`, `cmd/run.rs` parse tests

**Interfaces:**
- Produces: `AgentRun.skills: Vec<String>`, `AgentRun.preload_skills: Vec<String>`; `rupu run --skill <s>` (repeatable) and `--preload-skill <s>` (repeatable) → `Overrides.skills` / `Overrides.preload_skills`.

- [ ] **Step 1: Write the failing tests**

`argv.rs`: extend `an_agent_run_renders_one_spelling_per_flag` so the placed agent has `skills: vec!["pdf".into(), "project:notes".into()]`, `preload_skills: vec!["house-style".into()]`, and the expected args contain, right after the engagement flags, `"--skill", "pdf", "--skill", "project:notes", "--preload-skill", "house-style"`.

`argv_roundtrip.rs`: widen `agent_runs()`'s mask from `1 << 7` to `1 << 8`; bit 7 sets `skills = vec!["pdf".into(), "project:notes".into()]` and `preload_skills = vec!["house-style".into()]`. In `back()`, map `skills: a.skills, preload_skills: a.preload_skills`.

`run.rs` parse test:

```rust
#[test]
fn skill_flags_parse_repeated_in_either_position() {
    let a = parse_launch_args(&["rupu", "run", "--skill", "pdf", "agent", "--preload-skill", "house-style", "--skill", "project:notes", "go"]).unwrap();
    assert_eq!(a.skills, vec!["pdf".to_string(), "project:notes".to_string()]);
    assert_eq!(a.preload_skills, vec!["house-style".to_string()]);
}
```

(Mirror `findings_profile_flag_parses_in_either_position`'s exact call shape.)

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-runtime --lib argv`, `cargo test -p rupu-cli --test it argv_roundtrip::`, `cargo test -p rupu-cli --lib skill_flags`
Expected: compile errors.

- [ ] **Step 3: Implement**

`argv.rs` `AgentRun`: add the two fields (defaults `Vec::new()` in `new`); in the Agent arm of `to_args`, after the engagement loop:

```rust
            for s in &r.skills {
                a.flag_value("--skill", s);
            }
            for s in &r.preload_skills {
                a.flag_value("--preload-skill", s);
            }
```

`for_peer` is unchanged: skills change how a run behaves, so a connector refuses a peer that can't take them rather than downgrading (its doc already says so).

`run.rs` `Args`:

```rust
    /// A skill this run may load, on top of the agent's own (repeatable).
    /// A coordinator passes a workflow step's `skills:` this way.
    #[arg(long = "skill", value_name = "SKILL")]
    pub skills: Vec<String>,

    /// A skill whose body goes in this run's system prompt (repeatable).
    #[arg(long = "preload-skill", value_name = "SKILL")]
    pub preload_skills: Vec<String>,
```

and in the `Overrides { .. }` at `:896`: `skills: args.skills.clone(), preload_skills: args.preload_skills.clone(),`. Fix the test literals the compiler reports.

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-runtime --lib`, `cargo test -p rupu-cli --test it argv_roundtrip::`, `cargo test -p rupu-cli --lib`
Expected: pass; `argv_roundtrip` still asserts more than 300 renderings.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-runtime/src/argv.rs crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/tests/it/argv_roundtrip.rs
git commit -m "feat(cli): rupu run --skill and --preload-skill"
```

---

### Task 3: The capability, `UnitDispatch` and `AgentLaunchRequest`

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs` (`CAP_AGENT_SKILLS`, `node_capabilities`, `host_features`, `bucket_worker_capabilities`, `RunSpec`, `RunSpec::run_argv`), `crates/rupu-cp/src/agent_launcher.rs` (`AgentLaunchRequest`, `agent_run`)
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`UnitDispatch` + its construction in `dispatch_placed_step` `:7860`, `run_fanout_step` `:8937` / `:9134` / `:9241`), `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (`:338` mapping, `make_unit` `:1204`), `crates/rupu-cli/src/cmd/node.rs` (executor tests)
- Modify: every `AgentLaunchRequest { .. }` / `RunSpec { .. }` literal the compiler reports (ssh.rs, tunnel.rs, bucket/connector.rs, local.rs, cp_agent_launcher.rs, api/agents.rs, tests): add `skills: Vec::new(), preload_skills: Vec::new(),` (or the request's values where it carries them)
- Test: `crates/rupu-orchestrator/tests/it/remote_skills.rs` (new, add to `main.rs`), `protocol.rs` tests, `fleet_unit_dispatcher.rs` tests, `node.rs` tests

**Interfaces:**
- Produces: `CAP_AGENT_SKILLS: &str = "agent.skills"`; `UnitDispatch.skills`, `UnitDispatch.preload_skills`; `AgentLaunchRequest.skills`, `AgentLaunchRequest.preload_skills`; `RunSpec.skills`, `RunSpec.preload_skills` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`).

- [ ] **Step 1: Write the failing tests**

`remote_skills.rs`, a copy of `tests/it/remote_findings_profile.rs`'s structure (`ProfileRecorder` → a `SkillsRecorder` that records `(step_id, index, host, skills, preload_skills)` from `dispatch_unit`; `PanicFactory` so no local build happens):
- `placed_steps_carry_the_steps_skills`: a `host:` step with `skills: [pdf]`, `preload_skills: [house-style]` → the recorded unit has exactly those.
- `a_placed_step_without_skills_sends_none`: empty vectors.
- `every_distributed_unit_carries_the_steps_skills`: a `distribute:` step over three hosts; all units carry them.
- `a_distributed_units_retry_on_the_fallback_host_keeps_the_skills`: mirror `a_distributed_units_retry_on_the_fallback_host_keeps_the_profile`.

`protocol.rs`:
- Update `a_bucket_worker_advertises_no_tunnel_only_capability`'s exact list to include `CAP_AGENT_SKILLS`.
- Add `every_executor_surface_advertises_skills_support`, modelled on `every_executor_surface_advertises_engagement_profile_support` (`:585`).
- Extend the `RunSpec` round-trip and `run_argv` tests: an Agent spec with skills renders `--skill`; a Workflow spec with skills is refused ("applies to agent runs only").

`cmd/node.rs`: update `the_bucket_worker_does_not_advertise_the_tunnel_artifact_pull`'s exact list; extend `node_honours_supplied_identity` to assert skills parse back from the spawned argv.

`fleet_unit_dispatcher.rs`: add `dispatch_passes_the_units_skills_to_launch_agent`, modelled on `dispatch_passes_the_units_findings_profile_to_launch_agent` (`:2756`).

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-orchestrator --test it remote_skills::`, `cargo test -p rupu-cp --lib node::protocol`, `cargo test -p rupu-cli --lib fleet_unit_dispatcher node::`
Expected: compile errors.

- [ ] **Step 3: Implement**

`protocol.rs`:

```rust
/// A host whose `rupu run` takes `--skill` / `--preload-skill`, so a placed
/// unit's workflow-step skills reach it. A connector refuses a unit with
/// step skills on a peer that doesn't advertise it.
pub const CAP_AGENT_SKILLS: &str = "agent.skills";
```

Add it to `node_capabilities()`, `host_features()` and `bucket_worker_capabilities()`. `RunSpec` gains the two fields; `run_argv` refuses them on a Workflow spec exactly as it refuses `findings_profile`, and for an Agent spec sets `run.skills = self.skills.clone(); run.preload_skills = self.preload_skills.clone();`.

`agent_launcher.rs`: `AgentLaunchRequest` gains

```rust
    /// The placed unit's workflow-step skills. Every connector delivers them
    /// as `rupu run --skill` / `--preload-skill` or refuses the launch.
    pub skills: Vec<String>,
    pub preload_skills: Vec<String>,
```

and `agent_run` copies them onto the `AgentRun`.

`runner.rs` `UnitDispatch` gains the same two fields (doc: "The dispatcher must deliver them as `rupu run --skill` / `--preload-skill` or refuse the launch."). At each construction site set `skills: step.skills.clone(), preload_skills: step.preload_skills.clone(),` (for `run_fanout_step`, compute once per step next to `unit_findings_profile` and clone per task and per retry).

`fleet_unit_dispatcher.rs:338`: `skills: unit.skills, preload_skills: unit.preload_skills, // Every connector delivers these or refuses the launch.`

Fill every other literal the compiler reports.

- [ ] **Step 4: Run and pass**

Run: the Step 2 commands, plus `cargo test -p rupu-cli --test it host_features::`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp crates/rupu-orchestrator crates/rupu-cli
git commit -m "feat(fleet): placed units carry their step's skills behind agent.skills"
```

---

### Task 4: HTTP, tunnel and bucket refuse a peer without `agent.skills`

**Files:**
- Modify: `crates/rupu-cp/src/host/http.rs` (`launch_agent` `:330-400`), `crates/rupu-cp/src/api/agents.rs` (`AgentRunBody`, `run_agent_with`, the remote-proxy branch), `crates/rupu-cp/src/host/tunnel.rs` (`:174-232`), `crates/rupu-cp/src/host/bucket/connector.rs` (`:247-291`)
- Test: `crates/rupu-cp/tests/it/host_http.rs`, `node_tunnel.rs`, `bucket/connector.rs` tests, `api/agents.rs` tests, `host/local.rs` tests

- [ ] **Step 1: Write the failing tests**

For each of HTTP, tunnel and bucket, two tests modelled on that connector's existing findings-profile pair:
- **delivers**: a peer advertising `CAP_AGENT_SKILLS` receives the skills (HTTP: the JSON body has `"skills": ["pdf"]` and `"preload_skills": ["house-style"]`; tunnel: the `RunSpec` sent carries them; bucket: the queued spec carries them).
- **refuses**: a peer not advertising it gets `HostConnectorError::Unsupported` whose message contains `skills` and `upgrade rupu`, and nothing is launched (no mirror run created; for HTTP no POST).

`api/agents.rs`: a body with `"skills": ["pdf"]` reaches the local launcher's `AgentLaunchRequest.skills` (model on the engagement test at `:1406`).

`local.rs`: the launched argv contains `--skill pdf` (model on `:1246`).

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-cp --test it host_http:: node_tunnel::`, `cargo test -p rupu-cp --lib bucket api::agents host::local`
Expected: failures.

- [ ] **Step 3: Implement**

HTTP `launch_agent`: include `!req.skills.is_empty() || !req.preload_skills.is_empty()` in the condition that reads `peer_info()`, then next to the engagement check:

```rust
        if !req.skills.is_empty() || !req.preload_skills.is_empty() {
            if let Some(info) = &info {
                Self::require_in(
                    info,
                    CAP_AGENT_SKILLS,
                    "this run's workflow-step skills cannot be delivered",
                    &self.base_url,
                )?;
            }
        }
```

(Match how the engagement check treats a missing `info`.) In the body: `if !req.skills.is_empty() { body["skills"] = json!(req.skills); }` and the same for `preload_skills`.

`api/agents.rs` `AgentRunBody`: `#[serde(default)] skills: Vec<String>, #[serde(default)] preload_skills: Vec<String>,` carried into the `AgentLaunchRequest` in `run_agent_with` and the remote-proxy branch.

Tunnel, before `mirror.create_run`:

```rust
        if (!req.skills.is_empty() || !req.preload_skills.is_empty()) && !conn.supports(CAP_AGENT_SKILLS) {
            return Err(HostConnectorError::Unsupported(format!(
                "node {} (rupu {}) does not support workflow-step skills on agent launches; upgrade rupu on that node",
                node_id, node_version,
            )));
        }
```

where `node_id` and `node_version` are the same two expressions the findings-profile refusal just above formats; and put `skills` / `preload_skills` on the `RunSpec`.

Bucket, next to the findings-profile check:

```rust
        if !req.skills.is_empty() || !req.preload_skills.is_empty() {
            self.require_worker_capability(CAP_AGENT_SKILLS, "this run's workflow-step skills cannot be delivered").await?;
        }
```

- [ ] **Step 4: Run and pass**

Run: the Step 2 commands.
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp
git commit -m "feat(cp): http, tunnel and bucket deliver step skills or refuse the peer"
```

---

### Task 5: SSH gates the launch on the remote's `agent.skills`

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs` (`detach_launch_argv` `:1913`, `launch_agent` `:2661`, a new marker const next to `LAUNCH_CODENAME_ENV` `:65`)
- Test: `ssh.rs` tests, modelled on `resume_command_sends_if_unfinished_only_to_a_remote_advertising_it` (`:6939`) and `launch_agent_passes_the_findings_profile_to_the_remote_rupu_run` (`:9072`)

**Interfaces:**
- Produces: `const LAUNCH_SKILLS_UNSUPPORTED: &str = "rupu-launch: skills-unsupported";`. A launch whose argv carries skills runs only when the remote's `rupu __features` lists `"agent.skills"`, in the same ssh command; otherwise the remote prints the marker and `launch_agent` returns `HostConnectorError::Unsupported` and marks the mirror run failed.

- [ ] **Step 1: Write the failing tests**

A table test like `:6939`'s, running the real remote command through the existing fake-exec harness, with three remotes: a current rupu (features JSON including `agent.skills`) → the unit launches and the launch contains `'--skill' 'pdf'`; a rupu listing other features → `Unsupported` mentioning `skills` and `upgrade rupu`, nothing detached; a rupu predating `__features` (the `old_clap` echo) → `Unsupported`. A fourth case: a unit without skills sends no gate (the command does not mention `agent.skills`).

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: failures.

- [ ] **Step 3: Implement**

In `detach_launch_argv`, wrap whatever fragment it builds (the plain launch, or the codename gate) in a skills gate when the argv carries skills:

```rust
    fn detach_launch_argv(argv: &RunArgv, run_id: &str, working_dir: Option<&str>) -> Result<String, HostConnectorError> {
        let current = argv.to_shell("rupu");
        let (legacy, _) = argv.for_peer(&FeatureSet::shell_peer(Vec::<String>::new()));
        let legacy = legacy.to_shell("rupu");
        let probe = build_remote_command(&["rupu".to_string(), FEATURES_SUBCOMMAND.to_string()]);
        let needs_skills = matches!(argv, RunArgv::Agent(a) if !a.skills.is_empty() || !a.preload_skills.is_empty());
        Self::detach_launch_with(run_id, working_dir, |redirect| {
            let launch = if legacy == current {
                detach_fragment(&current, redirect)
            } else {
                let feature = shell_escape(&format!("\"{CAP_RUN_CODENAME_FLAG}\""));
                format!(
                    "if {probe} 2>/dev/null | grep -qF {feature}; then {}; else echo {}; {}; fi",
                    detach_fragment(&current, redirect),
                    shell_escape(LAUNCH_CODENAME_ENV),
                    detach_fragment(&legacy, redirect),
                )
            };
            if !needs_skills {
                return launch;
            }
            let skills = shell_escape(&format!("\"{CAP_AGENT_SKILLS}\""));
            format!(
                "if {probe} 2>/dev/null | grep -qF {skills}; then {launch}; else echo {}; fi",
                shell_escape(LAUNCH_SKILLS_UNSUPPORTED),
            )
        })
    }
```

This replaces the early `return Self::detach_launch(&current, run_id, working_dir)`; confirm `detach_launch` is exactly `detach_launch_with(run_id, wd, |r| detach_fragment(&current, r))` (if it does more, keep that behaviour in the `legacy == current` arm). Existing tests for the codename gate must still pass unchanged.

In `launch_agent`, after `if !out.success { … }`:

```rust
        if out.stdout.lines().any(|l| l.trim() == LAUNCH_SKILLS_UNSUPPORTED) {
            let _ = self.mirror.finish(&run_id, &self.host_id, "failed").await;
            return Err(HostConnectorError::Unsupported(format!(
                "host {} runs a rupu without workflow-step skills support; this unit's skills cannot be delivered; upgrade rupu there",
                self.host_id
            )));
        }
```

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: pass, including the existing codename-gate and findings-profile launch tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(ssh): launch a unit with step skills only on a remote that advertises agent.skills"
```

---

### Task 6: The CP editor keeps step skills; docs

**Files:**
- Inspect / modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` (`:112-117` mirrors `Step.findings_profile`), its tests
- Modify: `docs/skills.md`, the workflow format doc (the file that documents `actions:` / `findings_profile` step keys; find it with `grep -rln "findings_profile" docs/*.md`), `CLAUDE.md` (`rupu-orchestrator`: step `skills:` / `preload_skills:`; `rupu-cp`: `agent.skills` + the SSH gate; `rupu-runtime` argv: `--skill`), the spec §9 (SSH gates on `__features` in the launch command)

- [ ] **Step 1:** Write a web test that loads a workflow YAML whose step has `skills: [pdf]` and `preload_skills: [house-style]` into the editor's graph model and serialises it back, asserting both keys survive. Run it with the web test runner the repo uses (`npm test` / `vitest` under `crates/rupu-cp/web`; see its `package.json`).
- [ ] **Step 2:** If it fails, carry the two fields through `workflowGraph.ts` the way `findings_profile` is carried (data only; no form UI in this plan; that is Plan 4). Re-run until it passes.
- [ ] **Step 3:** Docs. `docs/skills.md` gains "Skills on workflow steps" (the keys, that they add to each agent's own, that they reach substeps, panelists and fixers, that placed units need a host advertising `agent.skills`). The workflow format doc lists the two keys. CLAUDE.md and spec §9 as above.
- [ ] **Step 4:** `cargo test -p rupu-cli --test it test_layout::`, and run `make cp-web` only if a TS file changed (the release embeds `web/dist`).
- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web docs CLAUDE.md
git commit -m "docs(skills): workflow-step skills; the CP editor keeps them"
```

---

## Done when

- `cargo test -p rupu-orchestrator --lib` and `--test it`, `-p rupu-runtime --lib`, `-p rupu-cp --lib` and `--test it`, `-p rupu-cli --lib` and `--test it` pass; the web test passes; clippy is clean on touched crates.
- By hand: a local workflow whose step has `skills: [pdf]` runs and its agent's transcript shows the `skill_index` with reason `launch`. If a second host is available (SSH to a host running this build), a `host:` step with skills launches there and shows the same; against a host on an older rupu, the step fails with the `agent.skills` refusal before anything runs. (Respect the SSH connection-burst rule: one launch, no probe loops.)
