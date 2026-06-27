# Live session feel + error surfacing + e2e — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Surface session/turn errors in the web, make the session view feel live (instant turn appearance + completion, adaptive refresh, "working…" pill), and add a session read-path e2e test. Token streaming already works.

**Architecture:** Backend adds `last_error`/per-run `error` to the session DTOs (read from the existing on-disk fields). Frontend renders errors and tightens the existing poll into an adaptive refresh + completion callback on the already-streaming `TranscriptPanel`. A reqwest+axum e2e exercises the read path with on-disk fixtures.

**Tech Stack:** Rust + axum (rupu-cp), React 18 + TS + Vitest (web).

Spec: `docs/superpowers/specs/2026-06-27-rupu-cp-live-session-design.md`.

## Global Constraints
- `#![deny(clippy::all)]`; per-file `rustfmt` only.
- web vitest `globals: false`: component tests `// @vitest-environment jsdom` + `afterEach(cleanup)`; pure tests node env.
- No rupu-agent / provider changes (deferred): tool/thinking deltas are out of scope.
- Mirror `crates/rupu-cp/tests/transcript.rs` (boot axum + reqwest) and `sse.rs` (SSE read) for the e2e.

---

## Task 1: Surface errors in the session DTOs

**Files:** `crates/rupu-cp/src/api/sessions.rs`.

- [ ] **Step 1: Failing test** — extend the existing `session_runs_from_json_maps_turns_in_order` test (it already builds a `session.json` with runs and asserts row fields) to include a run with `"error":"provider: API error 401"` and assert the mapped row's `error` matches. Also add a small `get_session`-shape assertion isn't needed here — last_error is covered by the e2e (Task 2). Run:
  `cargo test -p rupu-cp --lib api::sessions` → FAILS (no `error` field on the row).

- [ ] **Step 2: Implement.**
- `SessionDto` (struct at ~line 29): add a field
  ```rust
      #[serde(default)]
      last_error: Option<String>,
  ```
  (It's deserialized from `session.json` and re-serialized, so this both reads the on-disk `last_error` and includes it in the `/api/sessions/:id` + list responses.)
- `SessionRunChatRecord` (~line 328): add
  ```rust
      #[serde(default)]
      error: Option<String>,
  ```
- `SessionRunRow` (~line 354): add `error: Option<String>,`.
- In `impl From<SessionRunChatRecord> for SessionRunRow` (~line 367): add `error: r.error,`.

- [ ] **Step 3:** `cargo test -p rupu-cp --lib api::sessions` → PASS; `cargo clippy -p rupu-cp --all-targets` clean; per-file rustfmt.
- [ ] **Step 4: Commit** — `git add -A && git commit -m "feat(cp): surface session last_error + per-run error in DTOs"`

---

## Task 2: Session read-path e2e test

**Files:** Create `crates/rupu-cp/tests/sessions_live.rs`.

- [ ] **Step 1: Write the test** (mirror `tests/transcript.rs` boot + reqwest, `sse.rs` for the stream read):
```rust
//! e2e: a failed session surfaces last_error / per-run error, and its turn
//! transcript reads + streams. Read-path only (no live LLM / worker).
use futures_util::StreamExt as _;
use std::time::Duration;
use tokio::io::AsyncReadExt as _;
use tokio_util::io::StreamReader;

#[tokio::test]
async fn failed_session_surfaces_errors_and_transcript_streams() {
    use axum::http::StatusCode;

    let root = tempfile::tempdir().unwrap();
    let global = root.path().to_path_buf();

    // Turn transcript with an assistant message + a run_complete carrying an error.
    let tdir = global.join("transcripts");
    std::fs::create_dir_all(&tdir).unwrap();
    let tpath = tdir.join("run_a.jsonl");
    let e0 = r#"{"type":"assistant_message","content":"working on it"}"#;
    let e1 = r#"{"type":"run_complete","run_id":"run_a","status":"error","total_tokens":12,"duration_ms":5,"error":"provider: API error 401"}"#;
    std::fs::write(&tpath, format!("{e0}\n{e1}\n")).unwrap();

    // session.json: failed session, one run with an error + this transcript.
    let sid = "ses_TEST";
    let sdir = global.join("sessions").join(sid);
    std::fs::create_dir_all(&sdir).unwrap();
    let session_json = serde_json::json!({
        "session_id": sid,
        "agent_name": "triage",
        "status": "failed",
        "last_error": "provider: API error 401",
        "active_run_id": null,
        "runs": [{
            "run_id": "run_a",
            "prompt": "do it",
            "transcript_path": tpath.to_str().unwrap(),
            "status": "error",
            "error": "provider: API error 401"
        }]
    });
    std::fs::write(sdir.join("session.json"), serde_json::to_vec_pretty(&session_json).unwrap()).unwrap();

    let state = rupu_cp::state::AppState::new(global.clone(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let client = reqwest::Client::new();

    // session DTO surfaces last_error
    let resp = client.get(format!("http://{addr}/api/sessions/{sid}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["last_error"], "provider: API error 401");

    // runs row surfaces per-run error
    let resp = client.get(format!("http://{addr}/api/sessions/{sid}/runs")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rows: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(rows[0]["error"], "provider: API error 401");

    // transcript reads the turn (run_complete present)
    let canon = std::fs::canonicalize(&tpath).unwrap();
    let resp = client.get(format!("http://{addr}/api/transcript"))
        .query(&[("path", canon.to_str().unwrap())]).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let t: serde_json::Value = resp.json().await.unwrap();
    let types: Vec<&str> = t["events"].as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"run_complete"), "events: {types:?}");

    // SSE stream emits a data: line
    let resp = client.get(format!("http://{addr}/api/transcript/stream"))
        .query(&[("path", canon.to_str().unwrap())]).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let byte_stream = resp.bytes_stream().map(|r| r.map_err(std::io::Error::other));
    let mut reader = StreamReader::new(byte_stream);
    let line = tokio::time::timeout(Duration::from_secs(5), async {
        let mut acc = Vec::new(); let mut buf = [0u8; 256];
        loop {
            let n = reader.read(&mut buf).await.unwrap();
            if n == 0 { panic!("stream closed before data:"); }
            acc.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&acc);
            if let Some(l) = text.lines().find(|l| l.starts_with("data:")) { return l.to_string(); }
        }
    }).await.expect("timed out");
    let json: serde_json::Value = serde_json::from_str(line.strip_prefix("data:").unwrap().trim()).unwrap();
    assert!(json["type"].is_string());
}
```
(Verify the session.json field names match what `SessionDto` / `SessionRunChatRecord` deserialize — adjust keys if the structs use different names. The transcript event JSON must match `rupu_transcript::Event`'s serde tags: `assistant_message`, `run_complete` with `run_id,status,total_tokens,duration_ms,error` — confirm against `crates/rupu-transcript/src/event.rs`.)

- [ ] **Step 2:** `cargo test -p rupu-cp --test sessions_live` → PASS (depends on Task 1's fields).
- [ ] **Step 3: Commit** — `git add -A && git commit -m "test(cp): session e2e — errors surfaced + transcript reads/streams"`

---

## Task 3: Frontend api types + TranscriptPanel onComplete + poll helper

**Files:** `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/components/TranscriptPanel.tsx`, new `crates/rupu-cp/web/src/lib/sessionPoll.ts` (+test).

- [ ] **Step 1:** `api.ts` — add `last_error?: string | null;` to `SessionSummary` and `error?: string | null;` to `SessionRunRow`. `npx tsc --noEmit`. Commit later (group with this task).

- [ ] **Step 2: Failing test** for the poll helper — `crates/rupu-cp/web/src/lib/sessionPoll.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import { pollIntervalFor } from './sessionPoll';

describe('pollIntervalFor', () => {
  it('fast while active (running or has active_run_id)', () => {
    expect(pollIntervalFor({ status: 'running', active_run_id: null })).toBe(1500);
    expect(pollIntervalFor({ status: 'idle', active_run_id: 'run_x' })).toBe(1500);
  });
  it('slow when idle/terminal', () => {
    expect(pollIntervalFor({ status: 'idle', active_run_id: null })).toBe(5000);
    expect(pollIntervalFor({ status: 'failed', active_run_id: null })).toBe(5000);
    expect(pollIntervalFor(null)).toBe(5000);
  });
});
```

- [ ] **Step 3:** `npx vitest run src/lib/sessionPoll.test.ts` → FAILS. Implement `sessionPoll.ts`:
```ts
/** Refresh cadence for the session view: fast while a turn is in flight, slow otherwise. */
export function pollIntervalFor(
  session: { status?: string | null; active_run_id?: string | null } | null,
): number {
  if (!session) return 5000;
  const active = session.status === 'running' || !!session.active_run_id;
  return active ? 1500 : 5000;
}
```
`npx vitest run src/lib/sessionPoll.test.ts` → PASS.

- [ ] **Step 4: TranscriptPanel `onComplete`** — read `TranscriptPanel.tsx`; add an optional prop `onComplete?: () => void`. In the live SSE handler (where each event `e` is appended), when `e.type === 'run_complete' || e.type === 'run_failed'`, call `onComplete?.()` (guard so it fires once). Don't change existing rendering.

- [ ] **Step 5:** `npx tsc --noEmit` clean.
- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(cp/web): session error fields, pollIntervalFor, TranscriptPanel onComplete"`

---

## Task 4: SessionDetail live-feel + error rendering

**Files:** `crates/rupu-cp/web/src/pages/SessionDetail.tsx`, `crates/rupu-cp/web/src/components/session/SessionConversation.tsx` (+ a component test).

- [ ] **Step 1: SessionDetail** (read it first):
  - Replace the flat `setInterval(loadRuns, 5000)` with an adaptive interval driven by `pollIntervalFor(session)`: when the session (or its active state) changes, clear + re-arm the interval at the new cadence. (Simplest: a `useEffect` depending on `pollIntervalFor(session)` that sets the interval to that value.)
  - Expose a `reload()` that reloads both session + runs; call it immediately after a successful send (so the new turn appears at once). If the send handler lives in `SessionConversation`, pass `reload` down (or lift the runs state). Keep it minimal: pass an `onSent`/`onTurnComplete` callback that calls `loadRuns()`.
  - Render a **session error banner** when `session.status === 'failed'` or `session.last_error`: a red box near the header showing `session.last_error`.
  - Add a **"working…" pill** in the header when `pollIntervalFor(session) === 1500` (i.e. active) — pulsing dot + "working…", mirroring `TranscriptPanel`'s live-dot styling.

- [ ] **Step 2: SessionConversation** (read it first):
  - Pass `onComplete={onTurnComplete}` into the active turn's `<TranscriptPanel … live={isActive(...)} onComplete={...} />` so a streamed completion triggers the parent reload.
  - After `sendSessionMessage` resolves (wherever the send is invoked), call the reload immediately (don't wait for the poll).
  - Per-turn error: for a run row whose `error` is set, render a small red error line under that turn.

- [ ] **Step 3: Component test** (`SessionConversation.test.tsx` or `SessionDetail.test.tsx`, jsdom): render with a runs fixture where one run has `error` set → assert the error text appears; and a session with `last_error` → assert the banner renders. Stub `api.subscribeTranscript`/`getSessionRuns` as the existing session tests do (read the existing test for the mock style).

- [ ] **Step 4:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): live session feel + error banners in SessionDetail"`

---

## Task 5: Verify + PR
- [ ] `cargo test -p rupu-cp` (lib + tests incl. `sessions_live`) green; `cargo clippy -p rupu-cp --all-targets` clean.
- [ ] `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` green.
- [ ] Manual: `make cp-web && rupu cp serve`; open a failed session → see the error banner + per-turn error; start a session → the turn appears instantly, streams, shows "working…", and flips to done/failed the moment it ends.
- [ ] `gh pr create --title "feat(cp): live session feel + error surfacing + e2e" --body "…"`

## Self-review notes
- Spec coverage: DTO errors (T1), e2e (T2), api+onComplete+poll helper (T3), live-feel + render (T4).
- No rupu-agent/provider changes (deferred tool/thinking deltas).
- `pollIntervalFor` is pure + unit-tested; the e2e validates error surfacing + stream wiring without a live LLM.
