# Arc 1 — Config Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close `ISSUES.md` **I-6 … I-21** — every declared config key either reaches its runtime consumer or is deleted; `rupu config set` stops destroying `config.toml`; `[policy].lock` becomes a real control on the CLI.

**Architecture:** Three shapes of work, in dependency order. (a) Two standalone CLI defects (`config set` corruption, dead pricing-error swallow). (b) One resolver change that makes `[policy].lock` apply to CLI loads. (c) A sweep over inert config keys where each key is either **wired to its consumer** or **deleted along with its docs and UI field** — an honestly-absent knob beats a knob that lies. Every fix is validated by a test that observes the value *at the consumer*, never merely that it parses.

**Tech Stack:** Rust (rupu-config, rupu-cli, rupu-runtime, rupu-providers, rupu-scm, rupu-orchestrator, rupu-app), TypeScript (rupu-cp web ConfigEditor), `cargo test`, `vitest`.

## Global Constraints

- **Validation bar (from the charter §3.2):** a parse test cannot close any of these. Each fix needs a test asserting the configured value is observable **at the consumer**. I-1 was invisible to both parse tests and layering tests for its entire lifetime.
- **Delete is a legitimate fix.** For any inert key, wiring it and removing it are both acceptable outcomes — but removal must also delete its docs entry and its CP Settings field in the same PR, or the lie persists.
- Workspace deps only (versions pinned in the root `Cargo.toml`); `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- Errors: `thiserror` in libraries, `anyhow` in `rupu-cli`.
- Never run package-wide `cargo fmt` — per-file only. `main` is fmt-dirty under the pinned toolchain.
- **Known-red baseline, do not chase:** 4 `linear_runner.rs` tests (ISSUES.md I-4), local clippy under Homebrew 1.95 vs pinned 1.88 (I-5), rupu-cli ANSI/session-test redness, `rupu-mcp` `schema_snapshot` drift. Compare against a clean checkout before debugging anything that looks pre-existing.
- **Close the issue in the same PR that fixes it:** move the `ISSUES.md` row to `## Fixed` with the PR number *and the validation that proves it* (test name, or command + output). Never delete a row.
- Line refs are `main` at `9031d848`; re-locate by the quoted code if drifted.

---

### Task 1: I-6 — `rupu config set` corrupts, then wipes, `config.toml`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/config.rs` (`get` ~`:42`, `set` ~`:49-70`, `Action` doc comments `:9-15`)
- Test: `crates/rupu-cli/src/cmd/config.rs` (in-file `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn get_path<'a>(v: &'a Value, key: &str) -> Option<&'a Value>` and `fn set_path(v: &mut Value, key: &str, val: Value) -> anyhow::Result<()>` — both split `key` on `.` and descend/create intermediate tables. Later tasks do not consume these.

Two independent defects, one task because they share a file and a test harness:
1. `set` inserts the dotted key literally (`t.insert(key.to_string(), parsed)`), producing `"ui.theme" = "dracula"` at top level. `Config` is `#[serde(default, deny_unknown_fields)]` (`crates/rupu-config/src/config.rs:22`), so the file no longer loads.
2. `set` reads the existing file with `toml::from_str(&text).unwrap_or_else(|_| Value::Table(Default::default()))` — on any parse failure it starts from an **empty table** and writes it back, erasing every remaining setting.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rupu-cli/src/cmd/config.rs`. These call the path helpers directly (the public `get`/`set` read `~/.rupu`, which a test must not touch):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_path_descends_into_a_nested_table() {
        let mut v: Value = toml::from_str("[ui]\ntheme = \"old\"\n").unwrap();
        set_path(&mut v, "ui.theme", Value::String("dracula".into())).unwrap();
        let rendered = toml::to_string_pretty(&v).unwrap();
        // The dotted key must NOT appear as a literal top-level key.
        assert!(!rendered.contains("\"ui.theme\""), "wrote a literal dotted key:\n{rendered}");
        assert_eq!(
            v.get("ui").and_then(|u| u.get("theme")).and_then(|t| t.as_str()),
            Some("dracula")
        );
    }

    #[test]
    fn set_path_creates_missing_intermediate_tables() {
        let mut v = Value::Table(Default::default());
        set_path(&mut v, "bash.timeout_secs", Value::Integer(30)).unwrap();
        assert_eq!(
            v.get("bash").and_then(|b| b.get("timeout_secs")).and_then(|t| t.as_integer()),
            Some(30)
        );
    }

    #[test]
    fn set_path_refuses_to_overwrite_a_scalar_with_a_table() {
        let mut v: Value = toml::from_str("log_level = \"warn\"\n").unwrap();
        let err = set_path(&mut v, "log_level.nested", Value::Integer(1)).unwrap_err();
        assert!(err.to_string().contains("log_level"), "got: {err}");
    }

    #[test]
    fn get_path_reads_a_nested_key() {
        let v: Value = toml::from_str("[ui]\ntheme = \"dracula\"\n").unwrap();
        assert_eq!(get_path(&v, "ui.theme").and_then(|t| t.as_str()), Some("dracula"));
        assert!(get_path(&v, "ui.missing").is_none());
        assert!(get_path(&v, "nope.nope").is_none());
    }

    #[test]
    fn a_top_level_key_still_works() {
        let mut v = Value::Table(Default::default());
        set_path(&mut v, "log_level", Value::String("debug".into())).unwrap();
        assert_eq!(get_path(&v, "log_level").and_then(|t| t.as_str()), Some("debug"));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cli --lib cmd::config 2>&1 | tail -5`
Expected: FAIL — `set_path` / `get_path` are not defined.

- [ ] **Step 3: Implement the path helpers and the fail-loud read**

Add to `crates/rupu-cli/src/cmd/config.rs`:

```rust
/// Read a dotted key (`ui.theme`) by descending nested tables.
fn get_path<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in key.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// Write a dotted key (`ui.theme`), creating intermediate tables as needed.
/// Refuses to replace an existing non-table with a table — that would silently
/// discard the user's value.
fn set_path(v: &mut Value, key: &str, val: Value) -> anyhow::Result<()> {
    let segs: Vec<&str> = key.split('.').collect();
    let (last, parents) = segs.split_last().expect("split always yields one segment");
    let mut cur = v;
    for seg in parents {
        if !matches!(cur.get(*seg), Some(Value::Table(_))) {
            if cur.get(*seg).is_some() {
                anyhow::bail!(
                    "cannot set `{key}`: `{seg}` is already a value, not a table"
                );
            }
            let table = cur
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("cannot set `{key}`: `{seg}` has no parent table"))?;
            table.insert((*seg).to_string(), Value::Table(Default::default()));
        }
        cur = cur.get_mut(*seg).expect("just inserted");
    }
    cur.as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("cannot set `{key}`: parent is not a table"))?
        .insert((*last).to_string(), val);
    Ok(())
}
```

Then in `set`, replace the lossy read — a malformed file must abort, never be silently replaced:

```rust
    let mut v: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "refusing to write: {} is not valid TOML ({e}). \
                 Fix or move the file first — writing would discard its contents.",
                path.display()
            )
        })?
    } else {
        Value::Table(Default::default())
    };
```

…and swap the insert for `set_path(&mut v, key, parsed)?;`. In `get`, swap `v.get(key)` for `get_path(&v, key)`. Update both `Action` doc comments (`:9-15`) to say dotted keys are supported (drop "top-level" and the hand-edit advice).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib cmd::config 2>&1 | tail -5` → 5 passed.
Run: `cargo build -p rupu-cli` → clean.

- [ ] **Step 5: Close I-6 and commit**

Move the I-6 row in `ISSUES.md` to `## Fixed` with the PR number and validation (`cmd::config::tests`, 5 tests). Move its Tier-2 write-up under `## Fixed` too.

```bash
git add crates/rupu-cli/src/cmd/config.rs ISSUES.md
git commit -m "fix(cli): config set writes dotted keys and never wipes config.toml (I-6)"
```

---

### Task 2: I-7 — enforce `[policy].lock` on every CLI config load

**Files:**
- Modify: `crates/rupu-config/src/layer.rs` (add the lock-aware entry point), `crates/rupu-config/src/lib.rs` (export it)
- Test: `crates/rupu-config/tests/parse.rs`

**Interfaces:**
- Consumes: `rupu_config::resolve` (`crates/rupu-config/src/resolve.rs:144`), whose `is_locked` logic (`resolve.rs:180-197`) is the existing lock semantics — global wins for a locked key.
- Produces: `pub fn layer_files_locked(global: Option<&Path>, project: Option<&Path>) -> Result<Config, LayerError>` — same signature and return type as the existing `layer_files`, but a key listed in the global `[policy].lock` takes its **global** value even when the project file overrides it.

Scope note: this task adds and proves the lock-aware loader. Migrating all 43 `layer_files` call sites is Task 3 — split so a reviewer can reject the migration without rejecting the semantics.

- [ ] **Step 1: Write the failing test**

Add to `crates/rupu-config/tests/parse.rs` (mirror how the file's existing tests write temp TOML — read one first):

```rust
#[test]
fn locked_global_key_survives_a_project_override() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global.toml");
    let project = dir.path().join("project.toml");
    std::fs::write(
        &global,
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
    )
    .unwrap();
    std::fs::write(&project, "permission_mode = \"bypass\"\n").unwrap();

    // Unlocked loader: project wins (today's behavior, unchanged).
    let plain = rupu_config::layer_files(Some(&global), Some(&project)).unwrap();
    assert_eq!(plain.permission_mode.as_deref(), Some("bypass"));

    // Lock-aware loader: the global lock holds.
    let locked = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(
        locked.permission_mode.as_deref(),
        Some("readonly"),
        "a locked global key must not be overridable by a project config"
    );
}

#[test]
fn unlocked_keys_still_layer_project_over_global() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global.toml");
    let project = dir.path().join("project.toml");
    std::fs::write(&global, "default_model = \"g\"\n[policy]\nlock = [\"permission_mode\"]\n").unwrap();
    std::fs::write(&project, "default_model = \"p\"\n").unwrap();

    let locked = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(locked.default_model.as_deref(), Some("p"));
}
```

Verify `permission_mode` and `default_model` are the real field names on `Config` (`crates/rupu-config/src/config.rs`) before writing; substitute the actual ones if not.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-config locked_global 2>&1 | tail -5`
Expected: FAIL — `layer_files_locked` not found.

- [ ] **Step 3: Implement**

In `crates/rupu-config/src/layer.rs`, add the lock-aware loader. Reuse `resolve()` rather than reimplementing lock precedence — it already owns `is_locked` and the dotted-key encoding rules:

```rust
/// Like [`layer_files`], but a key named in the **global** `[policy].lock`
/// keeps its global value even when the project config sets it.
///
/// `layer_files` performs plain project-over-global layering and is correct for
/// non-policy reads. Every path that honors operator policy must use this
/// instead — see ISSUES.md I-7, where lock enforcement existed only inside
/// `resolve()` (6 call sites, all in rupu-cp) while 43 CLI loads bypassed it.
pub fn layer_files_locked(
    global: Option<&Path>,
    project: Option<&Path>,
) -> Result<Config, LayerError> {
    let resolved = crate::resolve::resolve(global, project, &std::collections::BTreeMap::new())?;
    resolved_into_config(&resolved)
}
```

Read `resolve()`'s `Resolved` type first and implement `resolved_into_config` to rebuild a `Config` from the resolved key/value map — or, if `Resolved` already carries a `Config`, return that field directly and drop the helper. Prefer whichever the existing type makes natural; do not duplicate lock logic.

Export from `crates/rupu-config/src/lib.rs` next to the existing `layer_files` export.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-config 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-config/src/layer.rs crates/rupu-config/src/lib.rs crates/rupu-config/tests/parse.rs
git commit -m "feat(config): lock-aware layer_files_locked for policy-bearing loads (I-7 part 1)"
```

---

### Task 3: I-7 — migrate CLI config loads to the lock-aware loader

**Files:**
- Modify: every `rupu-cli` site that loads config for a policy-bearing decision. Enumerate with:
  `grep -rn "layer_files(" --include="*.rs" crates/rupu-cli crates/rupu-orchestrator | grep -v test`
  Known sites include `crates/rupu-cli/src/resume.rs:193`, `cmd/run.rs:473`, `cmd/workflow.rs:2614,3372,3973`, `cmd/session.rs:1256,6652`, `cmd/auth.rs:523`, `cmd/cron.rs:391`.
- Test: `crates/rupu-cli/tests/` (new integration test) — see Step 1.

**Interfaces:**
- Consumes: `rupu_config::layer_files_locked` (Task 2).

Judgment call the implementer must make explicitly and record in the report: **not every load is policy-bearing.** A load that only reads UI preferences (theme, view mode) can stay on `layer_files`. A load that feeds `permission_mode`, provider/model resolution, or anything an operator would lock **must** move. When unsure, move it — the lock-aware loader is a superset and costs one extra file read.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/policy_lock.rs`. This must exercise a real CLI path, not the config crate:

```rust
// A locked global `permission_mode` must not be overridable by a project
// config on a CLI code path. Regression test for ISSUES.md I-7, where lock
// enforcement lived only inside rupu-cp's `resolve()` call sites.
#[test]
fn cli_config_load_honors_a_locked_global_key() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("config.toml");
    let project_dir = dir.path().join("proj/.rupu");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project = project_dir.join("config.toml");
    std::fs::write(
        &global,
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
    )
    .unwrap();
    std::fs::write(&project, "permission_mode = \"bypass\"\n").unwrap();

    let cfg = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(cfg.permission_mode.as_deref(), Some("readonly"));
}
```

If `rupu-cli` exposes a testable config-loading helper (check for a shared loader before writing this), assert through **that** instead — it proves the migration, not just the library. State in the report which you used and why.

- [ ] **Step 2: Run to verify it fails or passes trivially**

Run: `cargo test -p rupu-cli --test policy_lock 2>&1 | tail -5`
If it passes immediately, that only proves the library; the migration below is still required and is verified by Step 4's grep.

- [ ] **Step 3: Migrate the call sites**

Replace `layer_files(` with `layer_files_locked(` at every policy-bearing site found in Step 0's grep. Leave UI-preference-only sites alone and add a one-line comment at each site you deliberately leave, e.g. `// UI prefs only — lock does not apply (I-7)`.

- [ ] **Step 4: Verify the migration is complete**

Run: `grep -rn "layer_files(" --include="*.rs" crates/rupu-cli | grep -v test`
Every remaining hit must carry the deliberate-skip comment. Paste the output into the report.
Run: `cargo build --workspace` → clean. `cargo test -p rupu-cli --test policy_lock` → pass.

- [ ] **Step 5: Close I-7 and commit**

Move I-7 to `## Fixed` with the PR number and validation (the test name + the migration grep showing no unannotated sites).

```bash
git add -A && git commit -m "fix(cli): honor [policy].lock on every policy-bearing config load (I-7)"
```

---

### Task 4: I-8 — route sub-agent dispatch through the shared provider/model resolvers

**Files:**
- Modify: `crates/rupu-cli/src/cmd/dispatch.rs` (`CliAgentDispatcher` struct `:26-30`, its constructor `:63-75`, the resolution block `:186-197`), plus the construction sites found by
  `grep -rn "CliAgentDispatcher::new\|CliAgentDispatcher {" --include="*.rs" crates | grep -v "^crates/rupu-cli/src/cmd/dispatch.rs"`
- Test: `crates/rupu-cli/src/cmd/dispatch.rs` (in-file tests — the file already has them at `:401`, `:476`, `:569`, `:622`)

**Interfaces:**
- Consumes: `rupu_runtime::provider_factory::{resolve_provider_name, resolve_model, build_for_provider_with_config, openai_compatible_params}` — the same four the three already-fixed sites use (`cmd/run.rs`, `cmd/session.rs`, `step_factory.rs`). Signatures: `resolve_provider_name(spec_provider: Option<&str>, cfg_default: Option<&str>) -> String`; `resolve_model(spec_model: Option<&str>, cfg_default: Option<&str>, provider_default: Option<&str>) -> String`.
- Produces: `CliAgentDispatcher` gains config-derived fields; its constructor signature changes, so every construction site must be updated.

The defect (`dispatch.rs:186-190`):
```rust
let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
let model = spec.model.clone().unwrap_or_else(|| "claude-sonnet-4-6".into());
```
This is the fourth I-1/I-2 site. `ISSUES.md` records those as fixed "at all three call sites" — read `cmd/run.rs`'s resolution block first and mirror it exactly, including how it obtains `openai_compatible` params.

- [ ] **Step 1: Write the failing test**

Add to `dispatch.rs`'s existing test module. Model the harness on the tests at `:476`/`:569` (they already build a `CliAgentDispatcher` and a child agent file):

```rust
#[tokio::test]
async fn dispatch_honors_config_default_provider_and_model() {
    // Regression for ISSUES.md I-8: dispatch.rs was the 4th I-1/I-2 site and
    // hardcoded anthropic / claude-sonnet-4-6, ignoring config entirely.
    // Child agent declares NEITHER provider nor model, so config must supply both.
    // …build the dispatcher exactly as the neighbouring tests do, but with a
    // Config carrying default_provider + default_model, and a child agent file
    // whose frontmatter omits provider: and model:.
    // Assert on the values the dispatcher resolved — via the mock provider
    // script (RUPU_MOCK_PROVIDER_SCRIPT, see build_for_provider_with_config)
    // or by asserting the child's persisted RunRecord/transcript records the
    // configured model rather than "claude-sonnet-4-6".
}
```

The implementer writes this concretely against the file's real harness. The binding assertion: with `default_model = "cfg-model"` in config and no `model:` in the child's frontmatter, the resolved model is `cfg-model`, **not** `claude-sonnet-4-6`. If the existing harness cannot observe the resolved model, add the smallest seam that lets it (e.g. return the resolved name from a helper) rather than asserting on a mock's internals — and say so in the report.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli --lib cmd::dispatch 2>&1 | tail -6`
Expected: FAIL — resolved model is `claude-sonnet-4-6`.

- [ ] **Step 3: Implement**

Add config-derived fields to `CliAgentDispatcher` (mirror what `DefaultStepFactory` carries — `crates/rupu-orchestrator/src/step_factory.rs:36-50` — since that's the already-fixed workflow equivalent): `default_provider: Option<String>`, `default_model: Option<String>`, and the `openai_compatible` map. Thread them through the constructor and every construction site. Replace `:186-197` with the `resolve_provider_name` / `resolve_model` / `build_for_provider_with_config` sequence copied from `cmd/run.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cli --lib cmd::dispatch 2>&1 | tail -6` → all pass, including the pre-existing dispatch tests.
Run: `cargo build --workspace` → clean (constructor signature changed).

- [ ] **Step 5: Close I-8 and commit**

Move I-8 to `## Fixed`. **Also correct the I-1/I-2 entries** in the `## Fixed` section: they claim "all three call sites"; add a note that a fourth existed and was closed by I-8.

```bash
git add -A && git commit -m "fix(cli): sub-agent dispatch honors default_provider/default_model (I-8)"
```

---

### Task 5: I-18 — thread `[bash]` config into workflow steps

**Files:**
- Modify: `crates/rupu-orchestrator/src/step_factory.rs` (`DefaultStepFactory` fields `:36-50`, the hardcode at `:245-246`), plus its construction sites (`grep -rn "DefaultStepFactory" --include="*.rs" crates | grep -v test`)
- Test: `crates/rupu-orchestrator/src/step_factory.rs` (in-file tests — the file already has them, e.g. `step_actions_narrows_the_agent_grant` at `:809`)

**Interfaces:**
- Consumes: `rupu_config::Config`'s `bash` section (`crates/rupu-config/src/config.rs:101-107` — `timeout_secs`, `env_allowlist`).
- Produces: `DefaultStepFactory` gains `bash_timeout_secs: u64` and `bash_env_allowlist: Vec<String>`; its constructor signature changes.

The defect: `step_factory.rs:245-246` hardcodes `bash_env_allowlist: Vec::new(), bash_timeout_secs: 120`. `cmd/session.rs:6705-6706` and `cmd/run.rs:574-575` read the real values — so the same agent behaves differently as a workflow step. Same shape as I-2.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn bash_config_reaches_the_step_opts() {
    // Regression for ISSUES.md I-18: the workflow path hardcoded a 120s bash
    // timeout and an empty env allowlist, so [bash] config silently applied
    // under `rupu run`/`session` but not under `rupu workflow run`.
    // Build a DefaultStepFactory carrying bash_timeout_secs = 42 and
    // env_allowlist = ["FOO"], call build_opts_for_step(...) the way the
    // neighbouring tests do, and assert the returned AgentRunOpts carries
    // BOTH values through — not 120 / empty.
}
```

Write concretely against the file's real harness (read `step_actions_narrows_the_agent_grant` first for the construction pattern). The binding assertion: `opts.bash_timeout_secs == 42` and the allowlist contains `"FOO"`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator --lib step_factory 2>&1 | tail -5`
Expected: FAIL — got 120 / empty.

- [ ] **Step 3: Implement**

Add the two fields to `DefaultStepFactory`, thread them from `Config` at every construction site (the CLI sites already have a `Config` in scope — they build the factory right after loading it), and use them at `:245-246`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator --lib 2>&1 | grep "test result"` → pass.
Run: `cargo build --workspace` → clean.

- [ ] **Step 5: Close I-18 and commit**

```bash
git add -A && git commit -m "fix(orchestrator): workflow steps honor [bash] config (I-18)"
```

---

### Task 6: I-15 — honor `[scm.default]` / `[issues.default]`

**Files:**
- Modify: `crates/rupu-scm/src/registry.rs` (`default_platform` `:206-218`, `default_tracker` `:221-230`, and `Registry::discover` `:38-100` if the config values need capturing at construction)
- Test: `crates/rupu-scm/src/registry.rs` (in-file tests) — check for an existing `mod tests` first; the crate has `Registry::empty()` behind the `test-helpers` feature (`registry.rs:235`) and `insert_repo_connector` (`:190`) for fakes.

**Interfaces:**
- Consumes: `rupu_config::ScmDefault` (`crates/rupu-config/src/scm_config.rs:24-31`, fields `platform`/`owner`/`repo`) and `IssuesDefault` (`:34-38`, fields `tracker`/`project`). `Registry::discover` already takes `cfg: &Config`, so the values are reachable without a signature change — verify.
- Produces: `default_platform()` / `default_tracker()` consult config first, falling back to the current registration-order preference only when unset.

The defect: both methods hardcode a GitHub-then-GitLab preference and never read config. The code admits it (`registry.rs:207-208`: *"Wiring to `[scm.default]` config lands in Task 19; this is the v0 'first registered' fallback"*). `[scm.default]` is written into every `rupu init` config (`crates/rupu-cli/src/templates.rs:155`), documented as functional (`docs/scm.md:100-107`), and **named in the error users see when it's missing** (`crates/rupu-mcp/src/tools/scm_repos.rs:73`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn default_platform_prefers_the_configured_value() {
    // ISSUES.md I-15: a GitLab-primary shop that sets
    // [scm.default] platform = "gitlab" still got GitHub for every tool call
    // that omitted `platform`.
    // Build a Registry with BOTH github and gitlab repo connectors inserted,
    // and a config whose [scm.default].platform = "gitlab".
    // Assert default_platform() == Some(Platform::Gitlab).
}

#[test]
fn default_platform_falls_back_when_config_is_unset() {
    // With no [scm.default], preserve today's registration-order preference.
}

#[test]
fn default_tracker_includes_linear() {
    // ISSUES.md I-15 (second half): default_tracker() only considered
    // Github/Gitlab, so a Linear-only setup got None even with Linear wired.
}
```

Write concretely using `Registry::empty()` + `insert_repo_connector` (and the issue-connector equivalent — find its name). If `default_platform` currently takes no config, the fix requires storing it on `Registry` at `discover` time; assert through whatever surface that produces.

- [ ] **Step 2: Run to verify they fail**, **Step 3: implement**, **Step 4: run to verify they pass**

Run: `cargo test -p rupu-scm --lib 2>&1 | grep "test result"`.
Also update the stale in-code comment at `registry.rs:207-208` — it will otherwise keep telling readers this is unwired.

- [ ] **Step 5: Close I-15 and commit**

```bash
git add -A && git commit -m "fix(scm): honor [scm.default]/[issues.default]; include Linear in tracker fallback (I-15)"
```

---

### Task 7: I-9 … I-14, I-16, I-17, I-20 — resolve every remaining inert key

**Files:**
- Modify: `crates/rupu-config/src/provider_config.rs`, `crates/rupu-config/src/config.rs`, `crates/rupu-config/src/scm_config.rs`, `crates/rupu-config/src/resolve.rs`, plus each key's consumer (or its docs + `crates/rupu-cp/web/src/components/ConfigEditor.tsx` if deleting)
- Modify (docs, when deleting): `docs/providers.md`, `docs/providers/openai.md`, `docs/providers/gemini.md`, `docs/scm.md`
- Test: per key, at its consumer

**Interfaces:** none shared — each key is independent. This is one task because the decision procedure is identical and a reviewer judges them as a set.

**For each key below: wire it, or delete it (key + docs + UI field). Record the choice and the reason in the report.** The default recommendation is given, but the implementer decides and justifies.

| ID | Key | Declared | Recommendation |
|---|---|---|---|
| I-9 | `[providers.*].timeout_ms` | `provider_config.rs:23` | **Wire** — pass into the provider HTTP client builder. Documented with a specific default (`docs/providers.md:111`). |
| I-10 | `[providers.*].max_retries` | `provider_config.rs:25` | **Wire** — the real budget is a hardcoded 1 (`anthropic.rs:213`) while docs promise 5. Either wire it or correct the doc to say 1; do not leave both. |
| I-11 | `[providers.*].max_concurrency` | `provider_config.rs:27` | **Wire** — `semaphore_for` (`concurrency.rs:34`) exists and is used by SCM clients only; no LLM call acquires a permit, so `default_permits()` is unreachable for LLM traffic. |
| I-12 | `[providers.*].org_id`, `.region` | `provider_config.rs:19,21` | **Wire** for the providers that have a real header/endpoint use (OpenAI org header, Vertex region); **delete** for those that don't. Docs actively instruct setting them. |
| I-13 | `[retry]` section | `config.rs:113-116` | **Delete** — no consumer, no docs, superseded by per-provider retry (I-10). Remove the struct, its field on `Config`, and the `lib.rs` re-export. |
| I-14 | `log_level` | `config.rs:27` | **Wire** — make it the fallback when `RUPU_LOG` is unset (`crates/rupu-cli/src/logging.rs:25`), env still winning. It has an editable, *lockable* field in CP Settings (`ConfigEditor.tsx:199-207`), so deleting means removing that too. |
| I-16 | `[scm.*].clone_protocol` | `scm_config.rs:51` | **Wire** — honor `ssh` in both clone paths (`connectors/gitlab/repo.rs:477-479`, `github/repo.rs:442`). It has a dedicated dropdown at `ConfigEditor.tsx:374,463`. |
| I-17 | `[scm.*].timeout_ms` | `scm_config.rs:46` | **Wire** alongside I-9, or delete with its `docs/scm.md:109-119` entry. |
| I-20 | `resolve()` env tier | `resolve.rs:144-171` | **Delete** the parameter and `KeySource::Env`, or populate it from the process env at both callers. Both callers pass an empty map, so `KeySource::Env` is unreachable. |

- [ ] **Step 1: For each key, write the failing consumer-level test first**

Every test must observe the value **at the consumer**. Examples of the required shape (adapt to each key's real consumer):

```rust
// I-14: log_level is the fallback when RUPU_LOG is unset.
#[test]
fn log_level_config_is_the_fallback_filter() {
    // Assert the EnvFilter built by logging::init (or the helper it calls)
    // reflects cfg.log_level when RUPU_LOG is absent, and that RUPU_LOG
    // still wins when present. Refactor logging.rs to expose a pure
    // `filter_from(cfg_level: Option<&str>, env: Option<&str>) -> EnvFilter`
    // if that is what makes it testable.
}
```

```rust
// I-16: clone_protocol = "ssh" produces an ssh clone URL.
#[test]
fn clone_url_honors_ssh_protocol() {
    // Extract the URL construction from the clone path into a pure fn
    // (e.g. `clone_url(base_url, owner, repo, protocol, token) -> String`)
    // and assert "ssh" yields git@host:owner/repo.git while the default
    // yields the https form.
}
```

- [ ] **Step 2: Run to verify each fails**, **Step 3: implement (wire or delete)**, **Step 4: run to verify each passes**

For any key **deleted**, the proof is different: the key is gone from `crates/rupu-config/src/`, gone from `docs/`, gone from `ConfigEditor.tsx`, and `cargo build --workspace` + `npm run test` (cwd `crates/rupu-cp/web`) are clean. Record the greps proving absence.

- [ ] **Step 5: Verify no inert keys remain**

For every field in `crates/rupu-config/src/**`, confirm a non-test consumer exists:
```bash
grep -rn '\bfield_name\b' --include='*.rs' crates | grep -v '^crates/rupu-config/' | grep -v test
```
Paste a table of key → consumer (or → deleted) into the report. This table is the deliverable that proves the arc's premise.

- [ ] **Step 6: Close I-9…I-14, I-16, I-17, I-20 and commit**

```bash
git add -A && git commit -m "fix(config): wire or remove every inert config key (I-9..I-14, I-16, I-17, I-20)"
```

---

### Task 8: I-19, I-21 — desktop-app config and the pricing-error swallow

**Files:**
- Modify: `crates/rupu-app/src/executor/mod.rs` (`build_executor` `:109-118`, `:210`), `crates/rupu-cli/src/cmd/run.rs` (`:339`, `:404`)
- Test: `crates/rupu-app/src/executor/mod.rs` (in-file), `crates/rupu-cli/src/cmd/run.rs` (in-file)

**Interfaces:**
- Consumes: `rupu_config::layer_files_locked` (Task 2) — the desktop app is a policy-bearing surface.

**I-19:** `build_executor` passes `Config::default()` (`executor/mod.rs:210`) and `openai_compatible: HashMap::new(), default_provider: None, default_model: None` (`:116-118`), self-admitted at `:109-115`. Every user config is inert in the GUI — I-1 still live on that surface.

**I-21:** `cmd/run.rs:339,404` use `layer_files(...).unwrap_or_default()`, discarding a real `LayerError::Parse` and feeding `cfg.pricing` into `query_run_detail` — so a typo in `[pricing]` prints wrong dollar figures silently. The identical pattern at `cmd/workflow.rs:993` and `cmd/cron.rs:281` affects only UI preferences and is fine; leave those.

- [ ] **Step 1: Write the failing tests**

```rust
// I-19: the desktop executor loads real config, not Config::default().
#[test]
fn build_executor_loads_the_layered_config() {
    // Assert the executor's resolved default_provider/default_model reflect a
    // config file rather than None. If build_executor is not directly testable,
    // extract the config-loading step into a small pure helper and test that,
    // then have build_executor call it.
}
```

```rust
// I-21: a malformed [pricing] must not silently yield default rates.
#[test]
fn malformed_config_surfaces_on_the_pricing_path() {
    // Point the loader at a config.toml with invalid TOML and assert the
    // pricing path returns/logs an error rather than silently using defaults.
}
```

- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**

For I-21, surfacing the error is enough — at minimum a `tracing::warn!` naming the file and the parse error, so the wrong number is never presented as authoritative. Prefer failing the command if that matches the surrounding style; state which you chose.

- [ ] **Step 5: Close I-19, I-21 and commit**

```bash
git add -A && git commit -m "fix(app,cli): desktop app honors user config; pricing parse errors surface (I-19, I-21)"
```

---

### Task 9: Arc close-out — verification, docs, and the issue ledger

**Files:**
- Modify: `ISSUES.md` (verify every Arc 1 row moved), `CLAUDE.md` (one line if any config surface changed shape)

- [ ] **Step 1: Full verification**

```bash
cargo test -p rupu-config -p rupu-scm -p rupu-orchestrator -p rupu-cli 2>&1 | grep "test result"
cargo build --workspace
cargo clippy -p rupu-config -p rupu-scm -p rupu-runtime 2>&1 | grep -c "^error"
cd crates/rupu-cp/web && npm run test && npx tsc -b && npm run build
```
Expect green modulo the known-red baseline in Global Constraints. Compare any failure against a clean checkout before treating it as yours.

- [ ] **Step 2: Confirm every Arc 1 issue is closed with evidence**

Every one of I-6 … I-21 is either in `## Fixed` with a PR number **and** its validation, or still in the triage table with a recorded reason for deferral. No row may be silently deleted. Verify with:
```bash
grep -cE "^\| I-(6|7|8|9|1[0-9]|2[01]) \|" ISSUES.md    # rows remaining in triage
```

- [ ] **Step 3: Write the config-key ledger into the report**

The table from Task 7 Step 5 (every config key → its consumer, or → deleted) is Arc 1's proof of completion. It is also the input to Arc 6's config reference page (I-58), so it must be complete and accurate.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "docs: close Arc 1 (config integrity) in ISSUES.md"
```

---

## Deferred out of this arc

- **I-58 (config reference page)** — Arc 6. Task 7's ledger is its input, which is why the ledger must be complete.
- **Self-hosted clone URLs** (clone paths ignore `base_url`) — overlaps I-16 but is a separate defect; tracked in `TODO.md` under recovered orphans.
- **`[cp].agent_authoring_ui` / `workflow_editor_ui`** — deliberately untouched here; Arc 3 deletes both keys along with the classic renderers.
