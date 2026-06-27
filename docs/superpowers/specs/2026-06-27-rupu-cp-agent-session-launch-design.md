# rupu CP web — run an agent as Single run or Session

Date: 2026-06-27
Status: approved (design)

## Problem

The CP web agent Run modal (`AgentLauncherSheet`) only does a one-shot
`rupu run <agent>`. There's no way to start a multi-turn **session** from the
browser — the web can only `send` to sessions that already exist, so the
Sessions UI is effectively unusable from a fresh state. We add a **Single run /
Session** choice to the agent Run modal; Session mode runs `rupu session start`
and lands on the live SessionDetail chat.

This mirrors the existing `AgentLauncher` port end-to-end with a parallel
`SessionStarter` port (rupu-cp defines it; `rupu cp serve` installs the
subprocess adapter).

## Key differences vs the single-run path (from exploration)

- `rupu session start <agent> [target] [prompt]` takes the prompt **positionally**
  (no `--prompt` flag) and has **no `--working-dir`**; it clones a repo target
  with `--into <dir>` (persistent), not the single-run `--tmp` (auto-deleted) —
  sessions are long-lived, so their repo clone must persist.
- On `--detach`, `session start` prints `session: <id>` (then `run:`/`attach:`).

## CLI change (`rupu-cli`)

- Add a `--prompt` flag to `rupu session start` (`StartArgs`), mirroring the
  `rupu run --prompt` fix. Effective prompt = `--prompt` value, else the
  positional `prompt`, else default `"go"`. This lets the web always pass the
  prompt via the flag so it is never mis-parsed as a RunTarget when no target is
  given.

## Backend

### `rupu-cp` — `SessionStarter` port (`crates/rupu-cp/src/session_starter.rs`)
Mirror `agent_launcher.rs`:
```rust
pub struct SessionStartRequest {
    pub agent: String,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    pub working_dir: Option<String>,
}
pub enum SessionStartError { Invalid(String), Spawn(String) }
#[async_trait] pub trait SessionStarter: Send + Sync {
    async fn start(&self, req: SessionStartRequest) -> Result<String /*session_id*/, SessionStartError>;
}
```
`AppState.session_starter: Option<Arc<dyn SessionStarter>>` + `with_session_starter`
(mirror `agent_launcher`/`with_agent_launcher`; wire the lib `ServeOpts` field too).

### `rupu-cp` — endpoint (`crates/rupu-cp/src/api/agents.rs`)
`POST /api/agents/:name/session` (mirror `run_agent`):
- body `SessionStartBody { prompt?, mode?, target?, working_dir? }`.
- testable core `start_session_with(name, body, Arc<dyn SessionStarter>) -> Result<String, ApiError>` mapping `Invalid`→400, `Spawn`→500.
- handler: `s.session_starter.clone().ok_or_else(|| ApiError::not_available("starting sessions requires `rupu cp serve`"))?`; returns `{ "session_id": id }`.

### `rupu-cli` — adapter (`crates/rupu-cli/src/cp_session_starter.rs`)
Mirror `cp_session_sender.rs`/`cp_agent_launcher.rs`:
```
argv: session start <agent> [<target>] --detach [--mode m] [--prompt p] [--into <clone_dir>]
```
- `build_session_start_argv(req, clone_dir: Option<&str>)` (pure, tested): push `target` positionally when present; `--into <clone_dir>` only when both a repo target and a clone_dir are present; `--prompt` always when prompt present; `--mode` when present; always `--detach`.
- `start()`:
  - repo target (no `working_dir`) → generate a persistent clone dir
    `~/.rupu/clones/<ulid>` (create it), pass `--into <dir>`.
  - directory target (`working_dir`) → spawn with `current_dir(working_dir)`, no `--into`.
  - workspace (neither) → spawn in cp-serve cwd.
  - spawn detached (own process group + null stdio, like the agent adapter) BUT
    capture stdout to parse `session: <id>` — use `Command::output()` (the
    process exits promptly after enqueuing the first turn + detaching the
    worker, exactly like the existing `SubprocessSessionSender` which uses
    `output()`), then parse `session:` from stdout. Returns the session id.
  - (Note: unlike the run/agent launchers which fully detach and drop the
    handle, session start exits quickly and prints the id we need — so we
    `output()` and parse, matching `cp_session_sender.rs`.)
- Install in `crates/rupu-cli/src/cmd/cp.rs` `serve`: `.with_session_starter(Some(Arc::new(SubprocessSessionStarter { exe: exe.clone() })))`.
- Clone dir base: `paths::global_dir()?.join("clones")` (created on demand). A
  best-effort sweep is out of scope (clones persist with the session).

## Frontend (`rupu-cp/web`)

### `api.ts`
```ts
startSession(agent, opts: { prompt?; mode?: LaunchMode; target?; working_dir? }): Promise<{ session_id: string }>
  → POST /api/agents/:name/session
```

### `AgentLauncherSheet.tsx`
- Add a **launch-mode** toggle at the top: `Single run` | `Session` (segmented,
  default Single run). State `launchKind: 'run' | 'session'`.
- Prompt / Mode / `TargetPicker` are shared across both kinds.
- `onLaunch` branches on `launchKind`:
  - `run` → `api.launchAgent(agent, buildAgentLaunch(...))` → `navigate('/runs/'+run_id)`.
  - `session` → `api.startSession(agent, buildAgentLaunch(...))` → `navigate('/sessions/'+session_id)`.
  (`buildAgentLaunch` already produces `{prompt?,mode,target?,working_dir?}` — reuse it for both.)
- Submit button label flips: "Run" vs "Start session"; the title can read
  "Run agent" / hint that Session opens a live chat.

## Testing
- `rupu-cli`: `session start --prompt` parsed + used as the effective prompt
  (arg/handler test); `build_session_start_argv` cases (workspace/dir/repo;
  `--prompt` always; `--into` only for repo+clone_dir).
- `rupu-cp`: `POST /api/agents/:name/session` → 501 when no starter; with a mock
  `SessionStarter`, forwards `SessionStartRequest` (agent/prompt/mode/target/
  working_dir) and returns its `session_id`.
- web (vitest): the launch-kind branch calls the right api method and navigates
  to `/runs/:id` vs `/sessions/:id`; `buildAgentLaunch` reused unchanged.

## Out of scope
- Workflow-as-session (workflows stay single dispatch).
- A clones-dir GC/sweep (session clones persist).
- Surfacing session `last_error` in the UI (separate follow-up).
