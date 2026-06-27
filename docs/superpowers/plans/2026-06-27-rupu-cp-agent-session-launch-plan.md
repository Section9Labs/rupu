# Agent Single-run vs Session launch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add a Single-run / Session choice to the CP web agent Run modal; Session mode runs `rupu session start` and lands on the live SessionDetail chat — giving the web a way to create sessions.

**Architecture:** Mirror the existing `AgentLauncher` port (run) with a parallel `SessionStarter` port (rupu-cp trait + `cp serve` subprocess adapter), a `POST /api/agents/:name/session` endpoint, and an `api.startSession` + a launch-kind toggle in `AgentLauncherSheet`.

**Tech Stack:** Rust + axum (rupu-cp), Rust (rupu-cli), React 18 + TS + Vitest (web).

Spec: `docs/superpowers/specs/2026-06-27-rupu-cp-agent-session-launch-design.md`.

## Global Constraints
- `#![deny(clippy::all)]`; per-file `rustfmt` only. `rupu-cp` gains no SCM/auth deps.
- Mirror these existing files exactly: `crates/rupu-cp/src/agent_launcher.rs`, `crates/rupu-cp/src/api/agents.rs` (`run_agent`/`run_agent_with`/route/501), `crates/rupu-cli/src/cp_agent_launcher.rs`, the `cmd/cp.rs` install block, and `crates/rupu-cli/src/cp_session_sender.rs` (for the `output()`-and-parse pattern).
- web vitest `globals: false`: component tests `// @vitest-environment jsdom` + `afterEach(cleanup)`; pure-logic tests node env.
- `rupu-cli`'s full `--lib` suite has a known pre-existing failure under Homebrew rustc 1.95 — gate Rust tasks on `cargo build -p rupu-cli` + the specific new tests + `cargo test -p rupu-cp`.

---

## Task 1: `rupu session start --prompt` (`rupu-cli`)

**Files:** `crates/rupu-cli/src/cmd/session.rs`.

- [ ] **Step 1:** In `StartArgs` add a flag (after `prompt`):
```rust
    /// Initial user message via flag (preferred over the positional `prompt`;
    /// lets a caller pass a prompt without it being parsed as a target).
    #[arg(long = "prompt")]
    pub prompt_flag: Option<String>,
```
- [ ] **Step 2:** In the `start` handler, compute the effective prompt preferring the flag. Find where the positional `args.prompt` is read/passed into the launch (the start path calls `launch_turn`/`enqueue_turn_request` with a prompt that defaults to `"go"`). Replace that read with:
```rust
    let effective_prompt = args.prompt_flag.clone().or_else(|| args.prompt.clone());
```
and use `effective_prompt` everywhere `args.prompt` fed the turn message (keep the `"go"` default downstream). Leave target disambiguation using the POSITIONAL `target` only.
- [ ] **Step 3:** `cargo build -p rupu-cli` compiles. If any struct-literal construction of `StartArgs` exists elsewhere (grep `StartArgs {`), add `prompt_flag: None`.
- [ ] **Step 4: Commit** — `git add -A && git commit -m "feat(cli): rupu session start --prompt flag"`

---

## Task 2: `SessionStarter` port (`rupu-cp`)

**Files:** Create `crates/rupu-cp/src/session_starter.rs`; modify `lib.rs`, `state.rs`.

**Interfaces (produces):** `SessionStartRequest { agent, prompt, mode, target, working_dir }`, `SessionStartError { Invalid(String), Spawn(String) }`, `trait SessionStarter { async fn start(&self, req) -> Result<String, SessionStartError> }`, `AppState.session_starter` + `with_session_starter`.

- [ ] **Step 1:** Create `session_starter.rs` by copying `agent_launcher.rs` and renaming: `AgentLaunchRequest`→`SessionStartRequest`, `AgentLaunchError`→`SessionStartError`, `AgentLauncher`→`SessionStarter`, `launch`→`start`. Keep the same five fields (`agent, prompt, mode, target, working_dir`) and the doc comment adjusted to "starts sessions".
- [ ] **Step 2:** `lib.rs`: add `pub mod session_starter;`; add a `session_starter` field to `ServeOpts` (next to `agent_launcher`) and `.with_session_starter(opts.session_starter)` in the builder chain (mirror `agent_launcher` exactly).
- [ ] **Step 3:** `state.rs`: add `pub session_starter: Option<Arc<dyn crate::session_starter::SessionStarter>>`, default `None`, and `with_session_starter` (mirror `with_agent_launcher`). Update any `AppState` struct-literal test helpers that need the new field (grep `AppState {`; or they use `AppState::new`/builders).
- [ ] **Step 4:** `cargo build -p rupu-cp` compiles; `cargo test -p rupu-cp --lib` still green (test helpers updated).
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp): SessionStarter port + AppState.session_starter"`

---

## Task 3: `POST /api/agents/:name/session` (`rupu-cp`)

**Files:** `crates/rupu-cp/src/api/agents.rs`.

- [ ] **Step 1: Failing test** — add to the agents test module a `MockStarter` + tests, mirroring the `run_agent` tests:
```rust
    use crate::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};

    struct MockStarter { last: std::sync::Mutex<Option<SessionStartRequest>> }
    #[async_trait::async_trait]
    impl SessionStarter for MockStarter {
        async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("ses_TEST".into())
        }
    }

    #[tokio::test]
    async fn start_session_forwards_request() {
        let mock = std::sync::Arc::new(MockStarter { last: std::sync::Mutex::new(None) });
        let body = SessionStartBody {
            prompt: Some("hi".into()), mode: Some("ask".into()),
            target: None, working_dir: Some("/tmp/p".into()),
        };
        let id = start_session_with("triage", body, mock.clone()).await.expect("ok");
        assert_eq!(id, "ses_TEST");
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.agent, "triage");
        assert_eq!(got.prompt.as_deref(), Some("hi"));
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/p"));
    }

    #[tokio::test]
    async fn start_session_without_starter_is_not_available() {
        let tmp = tempfile::tempdir().unwrap();
        let s = test_state(&tmp); // session_starter: None
        let err = start_session(State(s), Path("triage".into()), None).await.expect_err("no starter");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
```
(Use the SAME `test_state` helper the existing `run_agent` no-launcher test uses. `SessionStartBody` derives `Default`.)

- [ ] **Step 2:** `cargo test -p rupu-cp --lib api::agents` → FAILS.

- [ ] **Step 3: Implement** in `agents.rs` (mirror `run_agent`/`run_agent_with`):
```rust
use crate::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};

#[derive(serde::Deserialize, Default)]
struct SessionStartBody {
    #[serde(default)] prompt: Option<String>,
    #[serde(default)] mode: Option<String>,
    #[serde(default)] target: Option<String>,
    #[serde(default)] working_dir: Option<String>,
}

async fn start_session_with(
    name: &str, body: SessionStartBody, starter: std::sync::Arc<dyn SessionStarter>,
) -> Result<String, ApiError> {
    let req = SessionStartRequest {
        agent: name.to_string(),
        prompt: body.prompt, mode: body.mode, target: body.target, working_dir: body.working_dir,
    };
    starter.start(req).await.map_err(|e| match e {
        SessionStartError::Invalid(m) => ApiError::bad_request(m),
        SessionStartError::Spawn(m) => ApiError::internal(m),
    })
}

async fn start_session(
    State(s): State<AppState>, Path(name): Path<String>, body: Option<Json<SessionStartBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let starter = s.session_starter.clone()
        .ok_or_else(|| ApiError::not_available("starting sessions requires `rupu cp serve`"))?;
    let id = start_session_with(&name, body.map(|b| b.0).unwrap_or_default(), starter).await?;
    Ok(Json(serde_json::json!({ "session_id": id })))
}
```
Register `.route("/api/agents/:name/session", post(start_session))` next to the `/run` route.

- [ ] **Step 4:** `cargo test -p rupu-cp --lib api::agents` → PASS; `cargo clippy -p rupu-cp --all-targets` clean; per-file rustfmt.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp): POST /api/agents/:name/session via SessionStarter port"`

---

## Task 4: `cp serve` session-starter adapter (`rupu-cli`)

**Files:** Create `crates/rupu-cli/src/cp_session_starter.rs`; modify `lib.rs` + `cmd/cp.rs`.

- [ ] **Step 1: Failing test** — argv builder + session-id parse, in `cp_session_starter.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::{build_session_start_argv, parse_session_id};
    use rupu_cp::session_starter::SessionStartRequest;

    fn req(target: Option<&str>, prompt: Option<&str>, mode: Option<&str>, wd: Option<&str>) -> SessionStartRequest {
        SessionStartRequest {
            agent: "triage".into(),
            prompt: prompt.map(Into::into), mode: mode.map(Into::into),
            target: target.map(Into::into), working_dir: wd.map(Into::into),
        }
    }

    #[test]
    fn argv_workspace_prompt_mode() {
        let a = build_session_start_argv(&req(None, Some("hi"), Some("ask"), None), None);
        assert_eq!(a, vec!["session","start","triage","--detach","--mode","ask","--prompt","hi"]);
    }
    #[test]
    fn argv_repo_adds_into() {
        let a = build_session_start_argv(&req(Some("github:o/r"), Some("hi"), None, None), Some("/clones/x"));
        assert_eq!(a, vec!["session","start","triage","github:o/r","--detach","--prompt","hi","--into","/clones/x"]);
    }
    #[test]
    fn argv_minimal() {
        assert_eq!(build_session_start_argv(&req(None, None, None, None), None),
            vec!["session","start","triage","--detach"]);
    }
    #[test]
    fn parse_session_id_finds_line() {
        assert_eq!(parse_session_id("session: ses_01XYZ\nrun: run_1\n"), Some("ses_01XYZ".into()));
        assert_eq!(parse_session_id("run: run_1\n"), None);
    }
}
```

- [ ] **Step 2:** `cargo test -p rupu-cli --lib cp_session_starter` → FAILS.

- [ ] **Step 3: Implement** `cp_session_starter.rs` (mirror `cp_session_sender.rs` for `output()`-and-parse, `cp_agent_launcher.rs` for `current_dir`):
```rust
//! `cp serve` adapter for rupu-cp's `SessionStarter` port. Spawns
//! `rupu session start … --detach`, which enqueues the first turn + spawns the
//! session worker and prints `session: <id>`; we parse that and return it.
use rupu_cp::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};
use std::path::PathBuf;

pub struct SubprocessSessionStarter {
    pub exe: PathBuf,
}

/// argv after the exe: `session start <agent> [<target>] --detach [--mode m]
/// [--prompt p] [--into <clone_dir>]`. `--into` is added only when a repo
/// target AND a clone dir are present.
pub(crate) fn build_session_start_argv(req: &SessionStartRequest, clone_dir: Option<&str>) -> Vec<String> {
    let mut argv = vec!["session".to_string(), "start".to_string(), req.agent.clone()];
    if let Some(t) = &req.target { argv.push(t.clone()); }
    argv.push("--detach".to_string());
    if let Some(m) = &req.mode { argv.push("--mode".to_string()); argv.push(m.clone()); }
    if let Some(p) = &req.prompt { argv.push("--prompt".to_string()); argv.push(p.clone()); }
    if req.target.is_some() {
        if let Some(dir) = clone_dir { argv.push("--into".to_string()); argv.push(dir.to_string()); }
    }
    argv
}

/// Scan `session start --detach` stdout for the `session: <id>` line.
pub(crate) fn parse_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("session:") {
            let id = rest.trim();
            if !id.is_empty() { return Some(id.to_string()); }
        }
    }
    None
}

#[async_trait::async_trait]
impl SessionStarter for SubprocessSessionStarter {
    async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError> {
        // A repo target needs a persistent clone dir (the session lives on).
        let clone_dir = if req.target.is_some() && req.working_dir.is_none() {
            let base = crate::paths::global_dir()
                .map_err(|e| SessionStartError::Spawn(e.to_string()))?
                .join("clones")
                .join(ulid::Ulid::new().to_string());
            std::fs::create_dir_all(&base).map_err(|e| SessionStartError::Spawn(e.to_string()))?;
            Some(base.to_string_lossy().into_owned())
        } else {
            None
        };
        let argv = build_session_start_argv(&req, clone_dir.as_deref());

        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.args(&argv);
        if let Some(dir) = req.working_dir.as_deref() { cmd.current_dir(dir); }
        let out = cmd.output().await.map_err(|e| SessionStartError::Spawn(e.to_string()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(SessionStartError::Spawn(if err.is_empty() { "session start failed".into() } else { err }));
        }
        parse_session_id(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| SessionStartError::Spawn("could not determine session id from output".into()))
    }
}
```
(Confirm `crate::paths::global_dir()` is the accessor used elsewhere in rupu-cli; the session CLI uses `paths::global_dir()?`.)

- [ ] **Step 4:** `pub mod cp_session_starter;` in `lib.rs`. Install in `cmd/cp.rs` `serve` next to the agent launcher:
```rust
    let session_starter: Option<Arc<dyn rupu_cp::session_starter::SessionStarter>> =
        Some(Arc::new(crate::cp_session_starter::SubprocessSessionStarter { exe: exe.clone() }));
```
and add `.with_session_starter(session_starter)` to the `ServeOpts`/builder chain (match how `agent_launcher`/`session_sender` are passed).

- [ ] **Step 5:** `cargo test -p rupu-cli --lib cp_session_starter` → PASS; `cargo build -p rupu-cli` ok; clippy on the new file clean.
- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(cp): cp serve session-starter adapter"`

---

## Task 5: web — `api.startSession` + launch-kind toggle

**Files:** `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/components/AgentLauncherSheet.tsx` (+ its test).

- [ ] **Step 1: api.ts** — add (mirror `launchAgent`):
```ts
  startSession(
    agent: string,
    opts: { prompt?: string; mode?: LaunchMode; target?: string; working_dir?: string } = {},
  ): Promise<{ session_id: string }> {
    return request<{ session_id: string }>(`/api/agents/${encodeURIComponent(agent)}/session`, {
      method: 'POST',
      body: JSON.stringify({ prompt: opts.prompt, mode: opts.mode, target: opts.target, working_dir: opts.working_dir }),
    });
  },
```
`npx tsc --noEmit` clean. Commit `feat(cp/web): api.startSession`.

- [ ] **Step 2: AgentLauncherSheet** — read the file; add launch-kind state + toggle + branch.
  - `type LaunchKind = 'run' | 'session';` and `const [launchKind, setLaunchKind] = useState<LaunchKind>('run');`
  - Render a segmented toggle at the top of the modal body (above Prompt), two buttons "Single run" / "Session" (reuse the existing segmented button styling pattern from the old target-mode toggle / TriageRibbon — a `rounded-md px-2 py-1 text-[12px]` active=`bg-brand-600 text-white` else bordered).
  - In `onLaunch`, branch:
```tsx
    const opts = buildAgentLaunch(prompt, mode, target);
    if (launchKind === 'session') {
      const res = await api.startSession(agent, opts);
      navigate(`/sessions/${res.session_id}`);
    } else {
      const res = await api.launchAgent(agent, opts);
      navigate(`/runs/${res.run_id}`);
    }
    onClose();
```
  - Submit button label: `launchKind === 'session' ? (launching ? 'Starting…' : 'Start session') : (launching ? 'Launching…' : 'Run')`.
  - Add a tiny hint under the toggle when `session`: "Opens a multi-turn chat you can keep messaging."

- [ ] **Step 3: Test** (`AgentLauncherSheet.test.tsx`) — add a jsdom test: stub `api.launchAgent` + `api.startSession` + `useNavigate`; render, switch to Session, fill prompt, click Start session → assert `api.startSession` called (not `launchAgent`) and navigate to `/sessions/<id>`; and the default (run) path still calls `launchAgent` → `/runs/<id>`. (Stub `getRepos`/`getProjects`/`browseDir` for the TargetPicker as the existing tests do.)

- [ ] **Step 4:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): Single-run vs Session toggle in agent Run modal"`

---

## Task 6: Verify + PR
- [ ] `cargo test -p rupu-cp --lib` green; `cargo clippy -p rupu-cp --all-targets` clean; `cargo build -p rupu-cli` ok (rupu-cli full suite has the known pre-existing 1.95 failure — confirm it's the same).
- [ ] `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` green.
- [ ] Manual: `make cp-web && rupu cp serve`; open an agent → Run → switch to Session → Start → lands on `/sessions/:id`, send a message, see the turn run.
- [ ] `gh pr create --title "feat(cp): run an agent as a single run or a session" --body "…"`

## Self-review notes
- Spec coverage: CLI `--prompt` (T1), port (T2), endpoint (T3), adapter incl. persistent `--into` clone + `current_dir` + `session:` parse (T4), api + toggle (T5).
- `buildAgentLaunch` is reused unchanged for both kinds (same `{prompt,mode,target,working_dir}`).
- Repo session → persistent `~/.rupu/clones/<ulid>` via `--into`; dir → `current_dir`; workspace → cp-serve cwd. Matches the spec.
