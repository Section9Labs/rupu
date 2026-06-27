# CP Web Run — PR3: agent Run

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add a Run experience to `AgentDetail` (prompt + mode + the same target picker as workflows), spawning `rupu run <agent>` via a new `AgentLauncher` port. Cancel works unchanged (runs land in the shared run store).

**Architecture:** Mirror the existing `RunLauncher` (workflow) port end-to-end for agents: `rupu-cp` defines `AgentLauncher` + `POST /api/agents/:name/run`; `rupu-cli`'s `cp serve` installs a `SubprocessAgentLauncher` that spawns `rupu run <agent> …` detached. Reuses PR1/PR2 frontend pieces (`Combobox`, `DirectoryPicker`, target-mode).

**Tech Stack:** Rust + axum (rupu-cp), Rust (rupu-cli), React 18 + TS + Vitest + Tailwind.

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-web-run-design.md` → "PR3 — Agent Run".

## Global Constraints
- `#![deny(clippy::all)]`; per-file `rustfmt` only. `rupu-cp` gains no SCM/auth deps.
- Mirror the existing `RunLauncher` pattern exactly: `crates/rupu-cp/src/launcher.rs`, its `AppState.launcher`/`with_launcher` wiring, and `crates/rupu-cli/src/cp_launcher.rs` (which already spawns detached: own process group + null stdio + optional `current_dir`).
- web vitest `globals: false`: component tests `// @vitest-environment jsdom` + `afterEach(cleanup)`; pure-logic tests node env.

---

## Task 1: `--run-id` on `rupu run` (`rupu-cli`)

**Files:** `crates/rupu-cli/src/cmd/run.rs`.

- [ ] **Step 1:** Add a `--run-id` flag to the `Args` struct (after `tmp`):
```rust
    /// Pre-assign the run id (so a caller can reference the run before it starts).
    #[arg(long)]
    pub run_id: Option<String>,
```
- [ ] **Step 2:** Use it where the run id is currently generated (run.rs ~line 146 `let run_id = format!("run_{}", Ulid::new());`):
```rust
    let run_id = args.run_id.clone().unwrap_or_else(|| format!("run_{}", Ulid::new()));
```
- [ ] **Step 3:** `cargo build -p rupu-cli` → compiles. (No new unit test — it's a passthrough flag mirroring `workflow run --run-id`; the adapter test in Task 3 covers argv.)
- [ ] **Step 4: Commit** — `git add -A && git commit -m "feat(cli): rupu run --run-id (pre-assign agent run id)"`

---

## Task 2: `AgentLauncher` port + endpoint (`rupu-cp`)

**Files:** Create `crates/rupu-cp/src/agent_launcher.rs`; modify `lib.rs`, `state.rs`, `crates/rupu-cp/src/api/agents.rs`.

**Interfaces (produces):**
```rust
pub struct AgentLaunchRequest {
    pub agent: String,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    pub working_dir: Option<String>,
}
pub enum AgentLaunchError { Invalid(String), Spawn(String) }
#[async_trait] pub trait AgentLauncher { async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError>; }
```
plus `AppState.agent_launcher: Option<Arc<dyn AgentLauncher>>` + `with_agent_launcher`, and `POST /api/agents/:name/run`.

- [ ] **Step 1: Create the port** — `crates/rupu-cp/src/agent_launcher.rs`, mirroring `launcher.rs` (the four-field request, the two-variant error, the trait). Add `pub mod agent_launcher;` to `lib.rs`; wire an `agent_launcher` option field in the lib options struct + `.with_agent_launcher(opts.agent_launcher)` (mirror how `launcher` is wired). Add `AppState.agent_launcher` + `with_agent_launcher` + `agent_launcher: None` default (mirror `launcher`).

- [ ] **Step 2: Failing test** — add to `crates/rupu-cp/src/api/agents.rs` a test module with a `MockAgentLauncher` capturing the request:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
    use std::sync::{Arc, Mutex};

    struct MockAgent { last: Mutex<Option<AgentLaunchRequest>> }
    #[async_trait::async_trait]
    impl AgentLauncher for MockAgent {
        async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("run_A".into())
        }
    }

    #[tokio::test]
    async fn run_agent_forwards_request() {
        let mock = Arc::new(MockAgent { last: Mutex::new(None) });
        let body = AgentRunBody {
            prompt: Some("do it".into()),
            mode: Some("bypass".into()),
            target: None,
            working_dir: Some("/tmp/p".into()),
        };
        let run_id = run_agent_with("triage", body, mock.clone()).await.expect("ok");
        assert_eq!(run_id, "run_A");
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.agent, "triage");
        assert_eq!(got.prompt.as_deref(), Some("do it"));
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/p"));
    }

    #[tokio::test]
    async fn missing_launcher_is_not_available() {
        let err = run_agent_with("triage", AgentRunBody::default(), )
            ; // see note
    }
}
```
(Make `run_agent_with(name, body, launcher: Arc<dyn AgentLauncher>)` the testable core, and add a separate `#[tokio::test]` that calls the axum handler `run_agent` with an `AppState` whose `agent_launcher` is `None` and asserts 501 — mirror how `crates/rupu-cp/src/api/workflows.rs` tests `launch_run_without_launcher_is_not_implemented` with `test_state`. Derive `Default` on `AgentRunBody`.)

- [ ] **Step 3:** `cargo test -p rupu-cp --lib api::agents` → FAILS.

- [ ] **Step 4: Implement** in `crates/rupu-cp/src/api/agents.rs` — add the route `POST /api/agents/:name/run`, the body, the testable core, and the handler (mirror `workflows.rs::launch_run`):
```rust
use crate::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
use std::sync::Arc;
// ...
#[derive(serde::Deserialize, Default)]
struct AgentRunBody {
    #[serde(default)] prompt: Option<String>,
    #[serde(default)] mode: Option<String>,
    #[serde(default)] target: Option<String>,
    #[serde(default)] working_dir: Option<String>,
}

async fn run_agent_with(
    name: &str,
    body: AgentRunBody,
    launcher: Arc<dyn AgentLauncher>,
) -> Result<String, ApiError> {
    let req = AgentLaunchRequest {
        agent: name.to_string(),
        prompt: body.prompt,
        mode: body.mode,
        target: body.target,
        working_dir: body.working_dir,
    };
    launcher.launch(req).await.map_err(|e| match e {
        AgentLaunchError::Invalid(m) => ApiError::bad_request(m),
        AgentLaunchError::Spawn(m) => ApiError::internal(m),
    })
}

async fn run_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<AgentRunBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let launcher = s
        .agent_launcher
        .clone()
        .ok_or_else(|| ApiError::not_available("launching agents requires `rupu cp serve`"))?;
    let run_id = run_agent_with(&name, body.map(|b| b.0).unwrap_or_default(), launcher).await?;
    Ok(Json(serde_json::json!({ "run_id": run_id })))
}
```
Register `.route("/api/agents/:name/run", post(run_agent))` (add `post` + `Json`/`State`/`Path` to imports as needed; check the current `agents.rs` imports).

- [ ] **Step 5:** `cargo test -p rupu-cp --lib api::agents` → PASS; clippy clean; per-file rustfmt.
- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(cp): AgentLauncher port + POST /api/agents/:name/run"`

---

## Task 3: `cp serve` agent adapter (`rupu-cli`)

**Files:** Create `crates/rupu-cli/src/cp_agent_launcher.rs`; modify `lib.rs` + `cmd/cp.rs`.

**Interfaces (consumes):** `rupu_cp::agent_launcher::{AgentLaunchRequest, AgentLauncher, AgentLaunchError}`. Mirrors `crates/rupu-cli/src/cp_launcher.rs`.

- [ ] **Step 1: Failing test** — argv builder, in `crates/rupu-cli/src/cp_agent_launcher.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::build_agent_argv;
    use rupu_cp::agent_launcher::AgentLaunchRequest;

    #[test]
    fn argv_with_target_prompt_mode() {
        let req = AgentLaunchRequest {
            agent: "triage".into(),
            prompt: Some("look at PR".into()),
            mode: Some("bypass".into()),
            target: Some("github:o/r".into()),
            working_dir: None,
        };
        let argv = build_agent_argv(&req, "run_X");
        assert_eq!(
            argv,
            vec!["run", "triage", "github:o/r", "look at PR",
                 "--run-id", "run_X", "--mode", "bypass", "--tmp"]
        );
    }

    #[test]
    fn argv_minimal() {
        let req = AgentLaunchRequest {
            agent: "triage".into(), prompt: None, mode: None, target: None, working_dir: None,
        };
        assert_eq!(build_agent_argv(&req, "run_X"), vec!["run", "triage", "--run-id", "run_X"]);
    }
}
```

- [ ] **Step 2:** `cargo test -p rupu-cli --lib cp_agent_launcher` → FAILS.

- [ ] **Step 3: Implement** `crates/rupu-cli/src/cp_agent_launcher.rs`:
```rust
//! `cp serve` adapter for rupu-cp's `AgentLauncher`. Spawns a detached
//! `rupu run <agent> …` child per request (own process group + null stdio).
use rupu_cp::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub struct SubprocessAgentLauncher {
    pub exe: PathBuf,
}

/// argv after the exe: `run <agent> [target] [prompt] --run-id <id> [--mode m]
/// [--tmp]`. `--tmp` is added when a target is present so a repo/PR clone lands
/// in an auto-deleted tmpdir instead of polluting / refusing in cwd.
pub(crate) fn build_agent_argv(req: &AgentLaunchRequest, run_id: &str) -> Vec<String> {
    let mut argv = vec!["run".to_string(), req.agent.clone()];
    if let Some(t) = &req.target {
        argv.push(t.clone());
    }
    if let Some(p) = &req.prompt {
        argv.push(p.clone());
    }
    argv.push("--run-id".to_string());
    argv.push(run_id.to_string());
    if let Some(m) = &req.mode {
        argv.push("--mode".to_string());
        argv.push(m.clone());
    }
    if req.target.is_some() {
        argv.push("--tmp".to_string());
    }
    argv
}

#[async_trait::async_trait]
impl AgentLauncher for SubprocessAgentLauncher {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        let run_id = format!("run_{}", ulid::Ulid::new());
        let argv = build_agent_argv(&req, &run_id);
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        cmd.process_group(0);
        if let Some(dir) = req.working_dir.as_deref() {
            cmd.current_dir(dir);
        }
        cmd.spawn().map_err(|e| AgentLaunchError::Spawn(e.to_string()))?;
        Ok(run_id)
    }
}
```
(Confirm `ulid` is a dep of rupu-cli — `cp_launcher.rs` already uses `ulid::Ulid::new()`, so it is.)

- [ ] **Step 4:** Add `pub mod cp_agent_launcher;` to `crates/rupu-cli/src/lib.rs`. In `cmd/cp.rs` `serve`, build + install the adapter next to the workflow launcher + repo lister:
```rust
    let agent_launcher: Option<Arc<dyn rupu_cp::agent_launcher::AgentLauncher>> =
        Some(Arc::new(crate::cp_agent_launcher::SubprocessAgentLauncher { exe: exe.clone() }));
```
and add `.with_agent_launcher(agent_launcher)` to the AppState builder chain (reuse the same `exe` resolved for `SubprocessLauncher`).

- [ ] **Step 5:** `cargo test -p rupu-cli --lib cp_agent_launcher` → PASS; `cargo build -p rupu-cli` → ok; clippy on new file clean.
- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(cp): cp serve agent-launcher adapter"`

---

## Task 4: Frontend api (`web`)

**Files:** `crates/rupu-cp/web/src/lib/api.ts`.

- [ ] **Step 1:** Add:
```ts
  launchAgent(
    agent: string,
    opts: { prompt?: string; mode?: LaunchMode; target?: string; working_dir?: string } = {},
  ): Promise<LaunchResult> {
    return request<LaunchResult>(`/api/agents/${encodeURIComponent(agent)}/run`, {
      method: 'POST',
      body: JSON.stringify({ prompt: opts.prompt, mode: opts.mode, target: opts.target, working_dir: opts.working_dir }),
    });
  },
```
(`LaunchResult` already exists.)
- [ ] **Step 2:** `npx tsc --noEmit` clean. Commit `feat(cp/web): api.launchAgent`.

---

## Task 5: `AgentLauncherSheet` + Run button on AgentDetail (`web`)

**Files:** Create `crates/rupu-cp/web/src/components/AgentLauncherSheet.tsx` (+ test); modify `crates/rupu-cp/web/src/pages/AgentDetail.tsx`.

**Interfaces (produces):** `<AgentLauncherSheet agent onClose />`; a prompt `<textarea>` + Mode select + the same target-mode selector (Workspace/Directory/Repository) reusing `Combobox` (repos via `getRepos`) and `DirectoryPicker`. Exposes pure `buildAgentLaunch(prompt, mode, targetMode, target, workingDir)` for testing.

- [ ] **Step 1: Failing test** — `AgentLauncherSheet.test.tsx`:
```tsx
import { describe, it, expect } from 'vitest';
import { buildAgentLaunch } from './AgentLauncherSheet';

describe('buildAgentLaunch', () => {
  it('directory mode sends working_dir only', () => {
    expect(buildAgentLaunch('hi', 'ask', 'directory', 'github:o/r', '/tmp/x')).toEqual({
      prompt: 'hi', mode: 'ask', working_dir: '/tmp/x',
    });
  });
  it('repo mode sends target only; blank prompt omitted', () => {
    expect(buildAgentLaunch('  ', 'bypass', 'repo', 'github:o/r', '')).toEqual({
      mode: 'bypass', target: 'github:o/r',
    });
  });
  it('workspace mode sends neither target nor dir', () => {
    expect(buildAgentLaunch('go', 'ask', 'workspace', '', '')).toEqual({ prompt: 'go', mode: 'ask' });
  });
});
```

- [ ] **Step 2:** `npx vitest run src/components/AgentLauncherSheet.test.tsx` → FAILS.

- [ ] **Step 3: Implement** `AgentLauncherSheet.tsx` — model it on `LauncherSheet.tsx` (the modal shell, mode select, target-mode selector with `Combobox`+`DirectoryPicker`), but replace the workflow inputs with a prompt `<textarea>` and call `api.launchAgent`. Provide the pure helper:
```tsx
export type AgentTargetMode = 'workspace' | 'directory' | 'repo';
export interface AgentLaunch { prompt?: string; mode: string; target?: string; working_dir?: string; }
export function buildAgentLaunch(
  prompt: string, mode: string, targetMode: AgentTargetMode, target: string, workingDir: string,
): AgentLaunch {
  const out: AgentLaunch = { mode };
  const p = prompt.trim();
  if (p) out.prompt = p;
  if (targetMode === 'repo') { const t = target.trim(); if (t) out.target = t; }
  if (targetMode === 'directory') { const d = workingDir.trim(); if (d) out.working_dir = d; }
  return out;
}
```
On launch: `const res = await api.launchAgent(agent, buildAgentLaunch(...)); navigate(`/runs/${res.run_id}`);`. Reuse the modal styling/ARIA from `LauncherSheet` (Esc-to-close, backdrop click, `LaunchMode` select). Fetch `getRepos()` for the repo combobox like LauncherSheet does.

- [ ] **Step 4:** `npx vitest run src/components/AgentLauncherSheet.test.tsx` → PASS.

- [ ] **Step 5: Wire into AgentDetail** (`crates/rupu-cp/web/src/pages/AgentDetail.tsx`): add `const [runOpen, setRunOpen] = useState(false)`, a **Run** button in the header (mirror how `WorkflowDetail` renders its Run button), and render `{runOpen && <AgentLauncherSheet agent={agent.name} onClose={() => setRunOpen(false)} />}` (use the loaded agent's name field). Read the file first to match its header structure + the agent name variable.

- [ ] **Step 6:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green.
- [ ] **Step 7: Commit** — `git add -A && git commit -m "feat(cp/web): agent Run modal + button on AgentDetail"`

---

## Task 6: Verify + PR

- [ ] `cargo test -p rupu-cp --lib 2>&1 | grep "test result"` → ok; `cargo clippy -p rupu-cp --all-targets` clean; `cargo build -p rupu-cli` ok (rupu-cli full suite has the known pre-existing 1.95 session-test failure — confirm it's the same, not new).
- [ ] `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → green.
- [ ] Manual: `make cp-web && rupu cp serve`; open an agent → Run → enter a prompt, pick a target (workspace/dir/repo), Launch → navigates to the new run; Cancel works.
- [ ] `gh pr create --title "feat(cp): run an agent from the web UI" --body "…"`

## Self-review notes
- Spec PR3 coverage: CLI `--run-id` (T1), AgentLauncher port + endpoint (T2), adapter (T3), api (T4), sheet+button (T5).
- The agent run reuses the shared run store → existing RunDetail + Cancel work with no changes.
- `--tmp` is auto-added for repo/PR targets so a server-side clone doesn't refuse-on-exists; directory targets use `current_dir`; workspace uses cp-serve cwd.
