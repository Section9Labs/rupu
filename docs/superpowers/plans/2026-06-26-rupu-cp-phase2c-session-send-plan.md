# rupu-cp Phase 2c — Send a message to a live session — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Send a message into a live session from the web (make the CP a chat surface). Mirrors the proven 2b launcher: rupu-cp defines a `SessionSender` **port**; rupu-cli's `cp serve` provides a **subprocess** adapter that shells `rupu session send <id> "<prompt>" --detach`. Session lifecycle (stop/archive/delete) is a follow-up (TODO).

**Design source:** the Phase-2c send-path analysis (this session). Part of CP Phase 2 (Control).

**Constraints:** no `any` (TS); static Tailwind; recharts out of main chunk; stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`. Toolchain: rupu-orchestrator/rupu-cp/web clean on 1.95; rupu-cli pre-existing red TEST baseline (verify it *compiles*; CI on 1.88 authoritative).

## Why subprocess (not reproduce)
`rupu session send <id> <prompt> --detach` (session.rs:163/1411) does the whole job atomically: ensures the session worker (`rupu session _worker --session-id <id>`, a detached process), mints `run_id = run_<ULID>`, updates the private `SessionRecord` (`runs` push, `active_run_id`, status→Running), writes `session.json`, and writes the queue file `<global>/sessions/<id>/queue/<request_id>.json` (tmp+rename). It prints `run: <run_id>` to stdout and exits promptly (turn execution is async in the `_worker`). Reproducing only the queue write would leave `session.json` inconsistent — so we shell the existing command (same pattern as `RunLauncher`/`SubprocessLauncher`).

---

### Task 1: rupu-cp — `SessionSender` port + `POST /api/sessions/:id/send`

**Files:** Create `crates/rupu-cp/src/session_sender.rs`; Modify `lib.rs` (module + ServeOpts), `state.rs` (AppState field), `api/sessions.rs` (endpoint + route).

**Context:** Mirror the existing `RunLauncher` wiring EXACTLY: `crates/rupu-cp/src/launcher.rs` (the port), `AppState.launcher: Option<Arc<dyn RunLauncher>>` (state.rs), `ServeOpts.launcher` (lib.rs) + `with_launcher` builder + `serve` passing it through. `api/sessions.rs` already reads sessions from `<global>/sessions/<id>/session.json` (active) / `sessions-archive` via `load_session_file`/`SessionDto` (with `status`).

- [ ] **Step 1:** `session_sender.rs`:
  ```rust
  #[derive(Debug, Clone)] pub struct SendMessageRequest { pub session_id: String, pub prompt: String }
  #[derive(Debug, thiserror::Error)] pub enum SendError {
      #[error("{0}")] Invalid(String), #[error("{0}")] Spawn(String),
  }
  #[async_trait::async_trait] pub trait SessionSender: Send + Sync {
      async fn send(&self, req: SendMessageRequest) -> Result<String, SendError>; // returns run_id
  }
  ```
  `pub mod session_sender;` in lib.rs.
- [ ] **Step 2:** `AppState` gains `pub session_sender: Option<Arc<dyn crate::session_sender::SessionSender>>` (default `None` in `new`; add a `with_session_sender(...)` builder, OR fold into the existing builder pattern). `ServeOpts` gains `pub session_sender: Option<Arc<dyn SessionSender>>`; `serve` passes it via the builder. Fix existing `AppState::new`/ServeOpts construction sites (default `None`).
- [ ] **Step 3: Write the failing endpoint test** with a **mock `SessionSender`** (captures the request, returns a fixed run_id): `POST /api/sessions/:id/send` body `{prompt}` → calls `send` with `{session_id, prompt}`, returns `{run_id}`; no sender installed → 501; an empty prompt → 400 (validate non-empty before calling). (Optional: a missing/stopped session → 400 — read the session via the existing reader and reject `status == "stopped"` or absent; if that's heavy, defer the stopped check to the subprocess and map its failure.)
- [ ] **Step 4: Implement** `async fn send_session(State(s), Path(id), Json(body): Json<SendBody>)` where `SendBody { prompt: String }`. Reject empty prompt → 400. If `s.session_sender` is `None` → 501 (`ApiError::not_available`). Optionally pre-check the session exists + isn't stopped (reuse `load_session_file`/the dir resolution in sessions.rs) → 404/409 if so. Else `sender.send(SendMessageRequest{ session_id: id, prompt }).await` → 200 `{run_id}`; map `SendError::Invalid` → 400, `Spawn` → 500. Register `.route("/api/sessions/:id/send", post(send_session))`.
- [ ] **Step 5:** `cargo test -p rupu-cp` + clippy clean. Commit. `git add crates/rupu-cp/src/session_sender.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/src/state.rs crates/rupu-cp/src/api/sessions.rs` → `feat(cp): SessionSender port + POST /api/sessions/:id/send`.

---

### Task 2: rupu-cli — subprocess `SessionSender` + wire into `cp serve`

**Files:** Create `crates/rupu-cli/src/cp_session_sender.rs`; Modify `crates/rupu-cli/src/cmd/cp.rs` (+ lib.rs module decl).

**Context:** Mirror `crates/rupu-cli/src/cp_launcher.rs` (`SubprocessLauncher`). `cmd/cp.rs` `Action::Serve` already builds `let launcher = Arc::new(SubprocessLauncher{ exe })` and passes `launcher: Some(launcher)` to `ServeOpts` — add the sender the same way.

- [ ] **Step 1:** `cp_session_sender.rs`: `pub struct SubprocessSessionSender { pub exe: PathBuf }` impl `rupu_cp::session_sender::SessionSender`:
  - `send`: validate `prompt` non-empty (else `SendError::Invalid`). Run `tokio::process::Command::new(&self.exe).args(["session","send",&req.session_id,&req.prompt,"--detach"]).output().await` (capture stdout/stderr; `session send --detach` enqueues + ensures the worker then exits promptly — `.output()` is fine).
  - On non-zero exit → `SendError::Spawn(<stderr, trimmed>)`.
  - Parse the run id from stdout: the CLI prints a line `run: <run_id>`. Factor a pure `fn parse_run_id(stdout: &str) -> Option<String>` (find the `run: ` line, take the id). If parsing fails but exit was success, fall back to `SendError::Spawn("could not determine run id")` (or return an empty/placeholder — prefer erroring so the UI knows). Return the run_id.
  - Unit-test `parse_run_id` (a sample stdout with `run: run_01ABC` → `Some("run_01ABC")`; no match → None).
- [ ] **Step 2:** Wire into `cp serve`: `let sender = Arc::new(SubprocessSessionSender { exe: current_exe.clone() });` and set `session_sender: Some(sender)` in the `ServeOpts` literal (reuse the `current_exe()` already resolved for the launcher).
- [ ] **Step 3:** `cargo build -p rupu-cli` compiles (ServeOpts new field satisfied); `cargo test -p rupu-cli cp_session_sender` (the parse test). Commit. `git add crates/rupu-cli/src/cp_session_sender.rs crates/rupu-cli/src/cmd/cp.rs crates/rupu-cli/src/lib.rs` → `feat(cli): subprocess SessionSender wired into cp serve`.

---

### Task 3: web — session message composer

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/SessionDetail.tsx`; Test.

**Context:** `SessionDetail.tsx` shows identity + a usage chart + a Turn Runs list (polled every 5s, so a new run appears within 5s). Transcript streaming exists via `subscribeTranscript(path)` / the `/transcript` route.

- [ ] **Step 1:** `api.ts`: `sendSessionMessage(id: string, prompt: string): Promise<{ run_id: string }>` → `POST /api/sessions/${id}/send` body `{ prompt }`. No `any`.
- [ ] **Step 2:** `SessionDetail.tsx`: add a **composer** at the bottom — a `<textarea>` + **Send** button. On Send: `sendSessionMessage(id, prompt)`; on success clear the input + (a) refetch the turn-runs list immediately so the new run shows, and (b) optionally navigate to / open the new run's transcript (`/transcript?path=<transcripts_dir>/<run_id>.jsonl&live=1` if the transcripts dir is known, else just rely on the polled list). Disable Send while in-flight or when `status === 'stopped'` (with a hint). Inline `role="alert"` error on failure. Cmd/Ctrl+Enter to send is a nice touch. Static Tailwind; no `any`.
- [ ] **Step 3: Test** (`SessionDetail.test.tsx`): typing a prompt + clicking Send calls `sendSessionMessage(id, prompt)` (mock api); the composer is disabled when `status==='stopped'`. `npm test -- --run` + `npm run build` green; recharts grep = 0.
- [ ] **Step 4: Commit.** `git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/SessionDetail.tsx <test>` → `feat(cp/web): session message composer (send to a live session)`.

---

### Final verification
- `cargo test -p rupu-orchestrator -p rupu-cp` green; clippy clean; `cargo build -p rupu-cli` compiles.
- `npm test -- --run` green; `npm run build` strict; recharts out of main chunk.
- Final review (the send chain endpoint→port→subprocess argv `session send … --detach`; run_id parse; 501/400 mapping; composer disabled when stopped), then matt visual-validates.
- TODO note: 2c ships **send only**; session **stop/archive/delete/start** lifecycle control + inline live-transcript embed are follow-ups.
