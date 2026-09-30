# Live usage — Plan 2: live CP + remote transport — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the CP show a running run's tokens and cost within ~2 s of each
LLM turn, for local runs and for runs on SSH/tunnel hosts (bucket: ≤15 s), and
fix the live readers that lose or duplicate lines.

**Architecture:**
- Remote hosts forward `usage.jsonl` through the existing artifact mirror
  (`ArtifactFile::Usage`).
- A light endpoint, `GET /api/runs/:id/usage`, serves the Plan-1 fold with an
  append-only `since`/`epoch` point cursor.
- The web polls it every 2 s while the run is live and the tab is visible.
  This adds no new SSE connection (the per-host pool cap is 6; see the SSE
  starvation arc).
- The CP transcript tailer and `FileTailRunSource` move onto Plan 1's
  `JsonlCursor`.
- The transcript footer sums usage instead of showing the first event.

**Tech Stack:** Rust (axum, tokio), React + TypeScript + vitest
(`crates/rupu-cp/web`).

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-live-usage-ledger-design.md`
§5.3, §5.4, §6, §7. **Depends on Plan 1 being merged or stacked**: it uses
`JsonlCursor`, `rupu_cp::usage::run_usage`, `RunUsage.{points, epoch, partial}`,
`summarize_run_usage` and `UsageSummary.partial`.

## Global Constraints

- The ledger never enters `events.jsonl`, and this plan must not add any event
  to the run/firehose SSE streams.
- Poll cadence 2000 ms while live, paused when `document.visibilityState !== 'visible'`,
  one final fetch when the run turns terminal.
- An older remote CP (404 on the new endpoint) must degrade to today's one-shot
  numbers, never an error banner.
- Node↔CP protocol: `usage` artifact lines are sent only to a CP that
  advertised the `usage_ledger` capability in `Welcome`.
- Web: vitest for every new pure function and hook. Run `npm test` (vitest) and
  `npm run build` in `crates/rupu-cp/web`; after web changes also run
  `make cp-web` before any release build (the binary embeds `web/dist`).
- Rust: never package-wide `cargo fmt` (format touched files only), no bare
  `git stash`, clippy `-D warnings` clean.
- GUI rule: a UI-affecting PR needs an in-browser check against a live run
  before merge (Task 6).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-cp/src/transcript_tail.rs` | Transcript SSE tailer on `JsonlCursor` (single owner task) |
| `crates/rupu-orchestrator/src/executor/file_tail.rs` | `FileTailRunSource` on `JsonlCursor` (single owner task) |
| `crates/rupu-cp/src/node/protocol.rs` | `ArtifactFile::Usage`; `Welcome { capabilities }` |
| `crates/rupu-cp/src/node/mirror.rs` | `Usage` → append `usage.jsonl` |
| `crates/rupu-cp/src/node/server.rs` | send `Welcome { capabilities: ["usage_ledger"] }` |
| `crates/rupu-cp/src/host/ssh.rs` | tail + route `usage.jsonl` |
| `crates/rupu-cp/src/host/bucket/poller.rs` | `usage*.jsonl` → `ArtifactFile::Usage` |
| `crates/rupu-cli/src/cmd/node.rs` | tunnel + bucket node forward `usage.jsonl` (tunnel: capability-gated) |
| `crates/rupu-cp/src/api/runs.rs` | `GET /api/runs/:id/usage` |
| `crates/rupu-cp/web/src/lib/api.ts` | `RunUsageResponse` type + `getRunUsage` |
| `crates/rupu-cp/web/src/lib/runUsage.ts` (new) | `mergeRunUsage` (pure) + `useRunUsage` hook |
| `crates/rupu-cp/web/src/pages/RunDetail.tsx` | header + chart from `useRunUsage` |
| `crates/rupu-cp/web/src/components/transcript/transcriptView.ts` | footer running sum |
| `crates/rupu-cp/web/src/components/TranscriptPanel.tsx` | snapshot/stream prefix merge (no duplicate backlog) |
| `crates/rupu-cp/web/src/pages/SessionDetail.tsx` | refresh session usage + timeline while a turn is in flight |
| `crates/rupu-cp/web/src/pages/Usage.tsx` | 30 s refresh when the window ends at now |

---

### Task 1: Live readers on `JsonlCursor` (no lost / duplicated lines, no full re-reads)

**Files:**
- Modify: `crates/rupu-cp/src/transcript_tail.rs` (whole tail implementation;
  `drain_and_emit_async` ~88-120)
- Modify: `crates/rupu-orchestrator/src/executor/file_tail.rs` (whole file)
- Test: inline tests in both files

**Interfaces:**
- Consumes: `rupu_transcript::JsonlCursor` (Plan 1 Task 1).
- Produces: unchanged public APIs (`TranscriptTail::open`/stream,
  `FileTailRunSource::open`/stream).

- [ ] **Step 1: Write the failing tests** — in `file_tail.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::io::Write;

    fn ev_line(step: &str) -> String {
        serde_json::to_string(&Event::StepStarted {
            run_id: "r".into(), step_id: step.into(), kind: Default::default(), agent: None, host: None,
        }).unwrap() + "\n"
    }

    #[tokio::test]
    async fn partial_line_is_delivered_once_completed_never_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("events.jsonl");
        let full = ev_line("a");
        let (head, tail) = full.split_at(full.len() / 2);
        std::fs::write(&p, head).unwrap();
        let mut src = FileTailRunSource::open(&p).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(tail.as_bytes()).unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(3), src.next()).await.unwrap().unwrap();
        assert!(matches!(got, Event::StepStarted { ref step_id, .. } if step_id == "a"));
    }

    #[tokio::test]
    async fn large_backlog_is_emitted_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("events.jsonl");
        let body: String = (0..5000).map(|i| ev_line(&format!("s{i}"))).collect();
        std::fs::write(&p, body).unwrap();
        let mut src = FileTailRunSource::open(&p).await.unwrap();
        let mut n = 0;
        while let Ok(Some(_)) = tokio::time::timeout(std::time::Duration::from_millis(800), src.next()).await {
            n += 1;
        }
        assert_eq!(n, 5000);
    }
}
```

(Fill in `StepStarted`'s field list from `executor/event.rs`. If `StepKind`
has no `Default`, use its linear variant.)

In `transcript_tail.rs`, add the same two tests, adapted to
`rupu_transcript::Event` lines: serialize
`Event::Usage { provider, model, served_model: None, input_tokens: 1, output_tokens: 1, cached_tokens: 0, purpose: None }`.
Add a third test, `utf8_split_does_not_drop_chunk`: write a line containing
`"✓"` split mid-character across two writes, and assert it arrives.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-orchestrator file_tail && cargo test -p rupu-cp transcript_tail`
Expected: `partial_line…` fails (line lost) and/or `large_backlog…` fails
(count > 5000 from the drain/poll race). The UTF-8 test fails.

- [ ] **Step 3: Implement** — replace both files' two-task design with one task that owns a cursor:

```rust
impl FileTailRunSource {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(256);
        let path_buf: PathBuf = path.to_path_buf();
        tokio::spawn(async move {
            let mut cursor = rupu_transcript::JsonlCursor::new();
            loop {
                if tx.is_closed() {
                    return;
                }
                // Blocking read of only the appended bytes; bounded work per tick.
                let p = path_buf.clone();
                let mut c = std::mem::take(&mut cursor);
                let (c, lines) = tokio::task::spawn_blocking(move || {
                    let mut lines = Vec::new();
                    let _ = c.drain_with(&p, || {}, |l| lines.push(l.to_string()));
                    (c, lines)
                })
                .await
                .unwrap_or_default();
                cursor = c;
                for line in lines {
                    if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                        if tx.send(ev).await.is_err() {
                            return;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
        Ok(Self { rx })
    }
}
```

`unwrap_or_default()` needs `(JsonlCursor, Vec<String>): Default`, which holds
because both are `Default`. On a `JoinError` the cursor resets to 0. That is
acceptable because it only happens on panic; log a `warn!` there instead if you
prefer explicitness.

The first iteration drains the whole backlog (from 0), so there is exactly one
owner and no race. Update the module doc: remove the "initial-drain task +
shared offset" paragraph and describe the single cursor-owning task. Keep the
note about not using `notify`.

In `transcript_tail.rs`, apply the same shape: one spawned task, a
`JsonlCursor`, `spawn_blocking` per tick, parse
`rupu_transcript::Event`, and a 250 ms sleep. Delete `drain_and_emit_async`
and the `AtomicU64` offset.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-orchestrator && cargo test -p rupu-cp`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-orchestrator/src/executor/file_tail.rs crates/rupu-cp/src/transcript_tail.rs
git commit -m "fix(cp): tail events/transcripts with JsonlCursor — no lost partial lines, no duplicate backlog, no full re-reads"
```

---

### Task 2: Forward `usage.jsonl` from remote hosts (SSH, tunnel, bucket)

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs:73-83` (enum), and the `Welcome`
  variant (~line 13)
- Modify: `crates/rupu-cp/src/node/mirror.rs:163-215` (`append` match)
- Modify: `crates/rupu-cp/src/node/server.rs:169` (Welcome construction)
- Modify: `crates/rupu-cp/src/host/ssh.rs:1362-1368` (tail list), ~1437 (routing)
- Modify: `crates/rupu-cp/src/host/bucket/poller.rs:106-130` (`classify_key`)
- Modify: `crates/rupu-cli/src/cmd/node.rs`:
  - ~434 (Welcome match; capture capabilities);
  - ~471 (tunnel drain);
  - ~515-520 (`FileOffsets` init);
  - ~941 (bucket drain);
  - the `FileOffsets` / bucket `seq` structs.
- Test: `protocol.rs`, `mirror.rs`, `poller.rs`, `ssh.rs` inline tests; the
  `node.rs` drain test (~1134).

**Interfaces:**
- Produces:
  - `ArtifactFile::Usage` (serde `"usage"`).
  - `Frame::Welcome { #[serde(default)] capabilities: Vec<String> }`.
  - `pub const CAP_USAGE_LEDGER: &str = "usage_ledger";` in `protocol.rs`.
  - `NodeMirror::append(.., ArtifactFile::Usage, line)` appends to
    `run_store.usage_ledger_path(run_id)`.

- [ ] **Step 1: Write the failing tests**
  - `protocol.rs`:
    - `ArtifactFile::Usage` round-trips as `"usage"`.
    - `{"type":"welcome"}` (old CP) parses to `Welcome { capabilities: [] }`.
    - `Welcome { capabilities: vec!["usage_ledger".into()] }` round-trips.
  - `mirror.rs`: copy the existing `Events` append test and use
    `ArtifactFile::Usage`. Assert that the line lands in
    `<runs>/<id>/usage.jsonl`.
  - `poller.rs`: extend `classify_key_maps_known_suffixes` with
    `assert!(matches!(classify_key("usage.0001.jsonl"), Some(ArtifactFile::Usage)));`.
  - `ssh.rs`:
    - Extend the tail-command test (~5490) with
      `assert!(cmd.contains("$HOME/.rupu/runs/run_01ABC/usage.jsonl"));`.
    - Extend the pump-routing test (~5149-5210) with a
      `==> …/usage.jsonl <==` block whose line must land in the mirror's
      `usage.jsonl`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cp node:: host::bucket host::ssh`
Expected: compile errors / assertion failures.

- [ ] **Step 3: Implement**
  1. `protocol.rs`:
     - Add the `Usage` variant with a doc comment: "the run's usage ledger
       (`runs/<id>/usage.jsonl`, spec 2026-09-29 §3)".
     - Change `Welcome {}` to
       `Welcome { #[serde(default, skip_serializing_if = "Vec::is_empty")] capabilities: Vec<String> }`.
     - Add `CAP_USAGE_LEDGER`.
     - Fix every `Frame::Welcome {}` pattern and constructor the compiler
       flags. Use `Frame::Welcome { .. }` in `matches!`.
  2. `server.rs:169`: send
     `Frame::Welcome { capabilities: vec![CAP_USAGE_LEDGER.to_string()] }`.
  3. `mirror.rs`: add the arm

```rust
            ArtifactFile::Usage => {
                let path = self.run_store.usage_ledger_path(run_id);
                let mut f = OpenOptions::new().create(true).append(true).open(path)?;
                writeln!(f, "{line}")?;
            }
```

     and extend the doc list above `append`.
  4. `ssh.rs`: add `$HOME/.rupu/runs/{run_id}/usage.jsonl \` to `tail_cmd`
     (keep the transcript path last). Add the routing arm
     `} else if path.ends_with("usage.jsonl") { Some(ArtifactFile::Usage)`
     right after the `unit_checkpoints` arm. The pump never truncates the
     mirrored ledger. A replay after a reconnect re-appends lines, which the
     fold dedups by row `id` — state this in a comment.
  5. `poller.rs`: `if key.starts_with("usage") { return Some(ArtifactFile::Usage); }`
     inside the `.jsonl` branch, and update the doc list.
  6. `node.rs`:
     - Tunnel: after the Welcome check at ~434, capture
       `let usage_ok = matches!(&welcome_frame, Frame::Welcome { capabilities } if capabilities.iter().any(|c| c == CAP_USAGE_LEDGER));`.
     - Add `usage: u64` to `FileOffsets` (init 0).
     - In the drain loop (~471), when `usage_ok`:

```rust
            for line in drain_new_lines(&run_dir.join("usage.jsonl"), &mut state.offsets.usage) {
                send_artifact(&mut sink, rid, ArtifactFile::Usage, line).await;
            }
```

     - Bucket (~941): add a `usage_seq` counter and the same drain → put pattern
       with `result_key("usage", state.usage_seq)`. The bucket needs no
       capability gate, because older CPs skip unknown keys (`classify_key`
       → `None`).

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-cp && cargo test -p rupu-cli node`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cp/src/node crates/rupu-cp/src/host crates/rupu-cli/src/cmd/node.rs
git commit -m "feat(hosts): forward the usage ledger from SSH/tunnel/bucket hosts (tunnel gated on usage_ledger capability)"
```

---

### Task 3: `GET /api/runs/:id/usage` live endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs`:
  - add the route next to `/usage-timeline` in the router;
  - add a handler modeled on `get_run_usage_timeline` (~1103-1130) and
    `usage_timeline_from_host` (~1060-1085).
- Test: `crates/rupu-cp/tests/usage.rs` (or `run_observation.rs`, whichever
  already exercises `/usage-timeline`).

**Interfaces:**
- Consumes: `crate::usage::{run_usage, summarize_run_usage, summarize}`, `RunUsage`.
- Produces: `GET /api/runs/:id/usage?host=&since=&epoch=`, which returns
  `RunUsageResponse`:

```rust
#[derive(Debug, Serialize)]
pub struct RunUsageResponse {
    pub summary: crate::usage::UsageSummary,
    pub steps: std::collections::BTreeMap<String, crate::usage::UsageSummary>,
    pub turns: u64,
    pub partial: bool,
    /// Serialized as a STRING: `RunUsage.epoch` is a u64 around 1.8e18
    /// (UNIX-nanos base), beyond JS's 2^53 safe-integer range — a JSON number
    /// would round and two epochs would compare equal in the browser.
    #[serde(serialize_with = "ser_u64_as_string")]
    pub epoch: u64,
    /// Index of `points[0]` in the full series.
    pub points_from: usize,
    pub points: Vec<crate::usage::TurnPoint>,
}
```

  Contract: when `since` and `epoch` are both supplied, `epoch` matches the
  run's current epoch, and `since <= points.len()` → `points_from = since`.
  Otherwise `points_from = 0` and the full series is returned.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn run_usage_endpoint_serves_summary_steps_and_incremental_points() {
    // Harness: same as the existing usage-timeline test. Seed a Running run
    // whose usage.jsonl has 3 rows (steps "a","a","b"; input 10/20/30, output 1).
    let v = get_json(&app, "/api/runs/<id>/usage").await;
    assert_eq!(v["summary"]["total_tokens"], 63);
    assert_eq!(v["steps"]["a"]["input_tokens"], 30);
    assert_eq!(v["points_from"], 0);
    assert_eq!(v["points"].as_array().unwrap().len(), 3);
    let epoch = v["epoch"].as_str().expect("epoch is a string").to_string();

    // Append one more row → incremental fetch returns only the new point.
    append_ledger_row(/* step "b", input 40, output 1 */);
    let v2 = get_json(&app, &format!("/api/runs/<id>/usage?since=3&epoch={epoch}")).await;
    assert_eq!(v2["points_from"], 3);
    assert_eq!(v2["points"].as_array().unwrap().len(), 1);
    assert_eq!(v2["summary"]["total_tokens"], 104);

    // Wrong epoch → full series.
    let v3 = get_json(&app, "/api/runs/<id>/usage?since=3&epoch=1").await;
    assert!(v3["epoch"].is_string());
    assert_eq!(v3["points_from"], 0);
    assert_eq!(v3["points"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn run_usage_endpoint_404s_for_unknown_run() { /* GET /api/runs/run_nope/usage → 404 */ }
```

Use the existing test harness helpers in that file (`get_json` or its
equivalent). Seed the ledger with serialized `LedgerRow` values.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cp --test usage run_usage_endpoint`
Expected: 404 / route missing.

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, Deserialize, Default)]
pub struct RunUsageQuery {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub since: Option<usize>,
    /// Accepts the string form the server emits (see `RunUsageResponse.epoch`).
    #[serde(default, deserialize_with = "de_opt_u64_from_str")]
    pub epoch: Option<u64>,
}

fn ser_u64_as_string<S: serde::Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

/// `?epoch=` arrives as a query string; accept a decimal string (or absent).
fn de_opt_u64_from_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let o: Option<String> = Option::deserialize(d)?;
    o.map(|s| s.parse::<u64>().map_err(serde::de::Error::custom)).transpose()
}

fn build_run_usage_json(
    store: &RunStore,
    id: &str,
    pricing: &rupu_config::PricingConfig,
    since: Option<usize>,
    epoch: Option<u64>,
) -> ApiResult<serde_json::Value> {
    store.load(id).map_err(|e| run_not_found_or_internal(id, e))?;
    let u = crate::usage::run_usage(store, id);
    let from = match (since, epoch) {
        (Some(n), Some(e)) if e == u.epoch && n <= u.points.len() => n,
        _ => 0,
    };
    let steps = u
        .by_step
        .iter()
        .map(|(k, rows)| (k.clone(), crate::usage::summarize(rows, pricing)))
        .collect();
    let resp = RunUsageResponse {
        summary: crate::usage::summarize_run_usage(&u, pricing),
        steps,
        turns: u.turns,
        partial: u.partial,
        epoch: u.epoch,
        points_from: from,
        points: u.points[from..].to_vec(),
    };
    serde_json::to_value(resp).map_err(|e| ApiError::internal(e.to_string()))
}
```

Mirror `get_run_usage_timeline` exactly for routing:
- explicit `?host=` → `run_usage_from_host`, which builds locally when
  `conn.serves_runs_from_local_mirror()`, else proxies
  `/api/runs/{id}/usage?since=…&epoch=…` (forward the two params when present);
- `resolve_run_location` → Global / ProjectLocal / Host;
- `Unpersisted` → a 200 with a zero summary and empty points.

The fold does blocking IO, so run `build_run_usage_json` inside
`tokio::task::spawn_blocking`, cloning the `Arc<RunStore>` / pricing.
Register the route in the same router block as `/api/runs/:id/usage-timeline`.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-cp`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cp
git commit -m "feat(cp): GET /api/runs/:id/usage — live summary, per-step, append-only points"
```

---

### Task 4: Web — `useRunUsage` + RunDetail header/chart go live

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`:
  - `RunUsageResponse` interface near `UsageTimelinePoint` (~1097);
  - `getRunUsage` next to `getRunUsageTimeline` (~2014).
- Create: `crates/rupu-cp/web/src/lib/runUsage.ts`
- Create: `crates/rupu-cp/web/src/lib/runUsage.test.ts`
- Modify: `crates/rupu-cp/web/src/lib/usage.ts` — `UsageSummary.partial?: boolean`
- Modify: `crates/rupu-cp/web/src/pages/RunDetail.tsx`:
  - the header block (~750-761);
  - the chart (~1164);
  - `displayUsage` (~441).
- Test: `crates/rupu-cp/web/src/pages/RunDetail.test.tsx` (extend)

**Interfaces:**
- Consumes: `GET /api/runs/:id/usage` (Task 3).
- Produces:

```ts
export interface RunUsageResponse {
  summary: UsageSummary; steps: Record<string, UsageSummary>; turns: number;
  partial: boolean; /** decimal string — exceeds JS 2^53 */ epoch: string; points_from: number; points: UsageTimelinePoint[];
}
export interface RunUsageState {
  summary: UsageSummary; steps: Record<string, UsageSummary>; turns: number;
  partial: boolean; epoch: string; points: UsageTimelinePoint[];
}
export function mergeRunUsage(prev: RunUsageState | null, resp: RunUsageResponse): RunUsageState;
export function useRunUsage(id: string | undefined, host: string | undefined, live: boolean,
  opts?: { intervalMs?: number }): { usage: RunUsageState | null; unavailable: boolean };
```

- [ ] **Step 1: Write the failing tests** — `lib/runUsage.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { mergeRunUsage, useRunUsage } from './runUsage';
import { api } from './api';

const pt = (turn: number) => ({ turn, label: 's', tokens_in: turn, tokens_out: 1, tokens_cached: 0 });
const summary = (total: number) => ({ input_tokens: total, output_tokens: 0, cached_tokens: 0,
  total_tokens: total, cost_usd: null, priced: true, runs: 1 });
const resp = (over: Partial<Parameters<typeof mergeRunUsage>[1]>) => ({
  summary: summary(0), steps: {}, turns: 0, partial: false, epoch: '7', points_from: 0, points: [], ...over,
});

describe('mergeRunUsage', () => {
  it('appends points when epoch matches and points_from continues the series', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2)] }));
    const b = mergeRunUsage(a, resp({ points_from: 2, points: [pt(3)], summary: summary(9) }));
    expect(b.points.map((p) => p.turn)).toEqual([1, 2, 3]);
    expect(b.summary.total_tokens).toBe(9);
  });
  it('replaces the series on epoch change or points_from 0', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2)] }));
    const b = mergeRunUsage(a, resp({ epoch: '8', points: [pt(1)] }));
    expect(b.points).toHaveLength(1);
    expect(b.epoch).toBe('8');
  });
  it('truncates to points_from before appending (server resent a tail)', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2), pt(3)] }));
    const b = mergeRunUsage(a, resp({ points_from: 2, points: [pt(3), pt(4)] }));
    expect(b.points.map((p) => p.turn)).toEqual([1, 2, 3, 4]);
  });
});

describe('useRunUsage', () => {
  beforeEach(() => { vi.useFakeTimers({ shouldAdvanceTime: true }); });
  afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

  it('polls while live, sends since/epoch, stops when not live', async () => {
    const spy = vi.spyOn(api, 'getRunUsage')
      .mockResolvedValueOnce(resp({ points: [pt(1)] }))
      .mockResolvedValue(resp({ points_from: 1, points: [pt(2)] }));
    const { result, rerender } = renderHook(({ live }) => useRunUsage('run_1', undefined, live, { intervalMs: 50 }),
      { initialProps: { live: true } });
    await waitFor(() => expect(result.current.usage?.points).toHaveLength(1));
    await act(async () => { vi.advanceTimersByTime(60); });
    await waitFor(() => expect(spy).toHaveBeenCalledWith('run_1', { host: undefined, since: 1, epoch: '7' }));
    const calls = spy.mock.calls.length;
    rerender({ live: false });
    await act(async () => { vi.advanceTimersByTime(500); });
    // exactly one final fetch after going terminal, then silence
    expect(spy.mock.calls.length).toBeLessThanOrEqual(calls + 1);
  });

  it('reports unavailable (no throw) when the endpoint 404s', async () => {
    vi.spyOn(api, 'getRunUsage').mockRejectedValue(Object.assign(new Error('404'), { status: 404 }));
    const { result } = renderHook(() => useRunUsage('run_1', 'old-host', true, { intervalMs: 50 }));
    await waitFor(() => expect(result.current.unavailable).toBe(true));
    expect(result.current.usage).toBeNull();
  });
});
```

(Match how `api.ts` errors expose status: check `request<T>` at `api.ts:41`
and use its real error type or status field.)

- [ ] **Step 2: Run to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/runUsage.test.ts`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement**
  - `api.ts`: add the interface, plus

```ts
  getRunUsage(id: string, opts?: { host?: string; since?: number; epoch?: string }): Promise<RunUsageResponse> {
    const q = new URLSearchParams();
    if (opts?.host) q.set('host', opts.host);
    if (opts?.since != null && opts?.epoch != null) { q.set('since', String(opts.since)); q.set('epoch', opts.epoch); }
    const qs = q.toString() ? `?${q}` : '';
    return request<RunUsageResponse>(`/api/runs/${encodeURIComponent(id)}/usage${qs}`);
  },
```

  - `runUsage.ts`:

```ts
import { useEffect, useRef, useState } from 'react';
import { api, type RunUsageResponse, type UsageTimelinePoint } from './api';
import type { UsageSummary } from './usage';

export interface RunUsageState {
  summary: UsageSummary; steps: Record<string, UsageSummary>; turns: number;
  partial: boolean; epoch: string; points: UsageTimelinePoint[];
}

/** Fold one server response into the client series (append-only within an epoch). */
export function mergeRunUsage(prev: RunUsageState | null, resp: RunUsageResponse): RunUsageState {
  const continues = prev !== null && prev.epoch === resp.epoch && resp.points_from > 0
    && resp.points_from <= prev.points.length;
  const points = continues ? prev.points.slice(0, resp.points_from).concat(resp.points) : resp.points;
  return { summary: resp.summary, steps: resp.steps, turns: resp.turns, partial: resp.partial,
    epoch: resp.epoch, points };
}

/**
 * Live run usage (spec 2026-09-29 §7): fetch once, then poll every
 * `intervalMs` (default 2000) while `live` and the tab is visible; one final
 * fetch when `live` turns false. A failing endpoint (older remote CP) sets
 * `unavailable` so callers fall back to the one-shot graph usage.
 */
export function useRunUsage(id: string | undefined, host: string | undefined, live: boolean,
  opts?: { intervalMs?: number }) {
  const intervalMs = opts?.intervalMs ?? 2000;
  const [usage, setUsage] = useState<RunUsageState | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const stateRef = useRef<RunUsageState | null>(null);
  const inFlight = useRef(false);

  useEffect(() => { setUsage(null); stateRef.current = null; setUnavailable(false); }, [id, host]);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    const tick = async () => {
      if (inFlight.current || cancelled) return;
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      inFlight.current = true;
      try {
        const prev = stateRef.current;
        const resp = await api.getRunUsage(id, prev
          ? { host, since: prev.points.length, epoch: prev.epoch } : { host });
        if (cancelled) return;
        const next = mergeRunUsage(prev, resp);
        stateRef.current = next;
        setUsage(next);
      } catch {
        if (!cancelled) setUnavailable(true);
      } finally {
        inFlight.current = false;
      }
    };
    void tick();
    if (!live) return () => { cancelled = true; };
    const t = window.setInterval(() => void tick(), intervalMs);
    const onVis = () => { if (document.visibilityState === 'visible') void tick(); };
    document.addEventListener('visibilitychange', onVis);
    return () => { cancelled = true; window.clearInterval(t); document.removeEventListener('visibilitychange', onVis); };
  }, [id, host, live, intervalMs]);

  return { usage, unavailable };
}
```

    The hook calls `getRunUsage(id, { host, since, epoch })`. The test asserts
    `{ host: undefined, since: 1, epoch: 7 }`, so pass exactly those keys.
  - `RunDetail.tsx`:
    - Add
      `const { usage: liveUsage } = useRunUsage(id, host, isRunning);`
      after `isRunning` is computed (~516). Hooks must run unconditionally:
      place it with the other hooks, and compute `isRunning` earlier if
      needed.
    - `displayUsage = liveUsage?.summary ?? graph?.usage`.
    - The chart gets `series={liveUsage?.points ?? series}`.
    - When `displayUsage.partial` is set, prefix the total with `≥` and add
      `title="Some transcripts were not readable on this CP (remote host not yet mirrored)"`.
    - Keep the existing one-shot `getRunUsageTimeline` effect as the fallback
      for `unavailable`.
  - `usage.ts`: add `partial?: boolean;` to `UsageSummary`.
  - `RunDetail.test.tsx`: add a test that mocks `api.getRunUsage` to return a
    summary with `total_tokens: 1234567` and asserts the header renders
    `formatTokens(1234567)`. Follow the file's existing mocking pattern for
    `getRunGraph`.

- [ ] **Step 4: Run to verify**

Run: `cd crates/rupu-cp/web && npx vitest run && npm run build`
Expected: PASS; the build succeeds.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cp/web/src
git commit -m "feat(web): run detail tokens + cost update live (2s poll while running)"
```

---

### Task 5: Web — transcript footer sum, no duplicate backlog, live sessions, Usage page refresh

**Files:**
- Modify: `crates/rupu-cp/web/src/components/transcript/transcriptView.ts`
  (~49-50 doc, ~586-605)
- Modify: `crates/rupu-cp/web/src/components/TranscriptPanel.tsx` (~87-150)
- Modify: `crates/rupu-cp/web/src/pages/SessionDetail.tsx` (~54-127)
- Modify: `crates/rupu-cp/web/src/pages/Usage.tsx` (~153-180)
- Test: `transcriptView.test.ts`, `TranscriptPanel.test.tsx`, and new
  `SessionDetail.live.test.tsx` / `Usage.refresh.test.tsx` (follow the
  existing page-test patterns)

**Interfaces:**
- Produces: `export function mergeSnapshotAndStream<T>(snapshot: T[], stream: T[]): T[]`,
  exported from `TranscriptPanel.tsx` or a small helper module next to it.

- [ ] **Step 1: Write the failing tests**
  - `transcriptView.test.ts`:

```ts
it('footer totals are a running sum of usage events until run_complete', () => {
  const ev = (type: string, data: object) => ({ type, data } as never);
  const v = buildTranscriptView([
    ev('run_start', { run_id: 'r', agent: 'a', provider: 'p', model: 'm', started_at: '2026-01-01T00:00:00Z' }),
    ev('usage', { input_tokens: 100, output_tokens: 10 }),
    ev('usage', { input_tokens: 200, output_tokens: 20 }),
  ]);
  expect(v.footer?.totalTokens).toBe(330);
});
```

    (Match the `TranscriptEvent` shape used by the file's existing tests.)
  - `TranscriptPanel.test.tsx`: `mergeSnapshotAndStream([1,2,3],[1,2])`
    → `[1,2,3]`; `([1,2],[1,2,3,4])` → `[1,2,3,4]`; `([], [1])` → `[1]`.
  - `SessionDetail.live.test.tsx`:
    - Mock `getSession` to return `{ active_run_id: 'run_x', status: 'running', usage: { total_tokens: 1 } }`
      first and `{ …, usage: { total_tokens: 2 } }` after.
    - With fake timers, advance 1500 ms and assert the header shows the new
      total and `getSessionUsageTimeline` was called again.
  - `Usage.refresh.test.tsx`:
    - With a window whose `until` is undefined (ends now), advancing 30 s
      calls `getUsage` again.
    - With a fixed past `until`, it does not.

- [ ] **Step 2: Run to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript src/components/TranscriptPanel.test.tsx src/pages/SessionDetail.live.test.tsx src/pages/Usage.refresh.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement**
  - `transcriptView.ts`:
    - Add `let usageSum = 0;` before the event loop.
    - Replace the `usage` case with:

```ts
      case 'usage': {
        usageSum += (asNumber(data.input_tokens) ?? 0) + (asNumber(data.output_tokens) ?? 0);
        if (!sawRunComplete) {
          footer = { ...(footer ?? { status: null, durationMs: null, error: null }), totalTokens: usageSum };
        }
        break;
      }
```

    - Fix the doc comment at ~49-50: "a footer from `run_complete`, falling back
      to the running sum of `usage` events while the run is in flight."
  - `TranscriptPanel.tsx`:
    - Keep `snapshot` and `stream` in separate states; render
      `buildTranscriptView(mergeSnapshotAndStream(snapshot, stream))`.
    - Both start at byte 0 of the same file (the CP tailer replays from 0), so
      each is a prefix of the other:

```ts
export function mergeSnapshotAndStream<T>(snapshot: T[], stream: T[]): T[] {
  return stream.length >= snapshot.length ? stream : snapshot;
}
```

    - Reset both on a `path` change. The `onComplete` detection stays on the
      stream callback.
  - `SessionDetail.tsx`:
    - Hoist the `getSession` and `getSessionUsageTimeline` fetches into
      `loadSession()` / `loadTimeline()` callbacks.
    - Keep the mount effects calling them.
    - In the adaptive poll effect, alongside `loadRuns()`, call both when
      `isSessionActive(session)` (import from `lib/sessionPoll`). A fast poll
      (1.5 s) therefore refreshes the header usage and chart while a turn is in
      flight.
    - `reload()` also calls `loadTimeline()`.
  - `Usage.tsx`: in the `getUsage` effect, when `usageWindow.until == null`
    (or whatever marks "ends at now" in `UsageRangeControls`; check its
    window construction), start
    `window.setInterval(refetch, 30000)`, gated on
    `document.visibilityState === 'visible'`. Clear it in cleanup. Do the same
    for outliers.

- [ ] **Step 4: Run to verify**

Run: `cd crates/rupu-cp/web && npx vitest run && npm run build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cp/web/src
git commit -m "fix(web): transcript footer sums usage; no duplicated live backlog; live session + Usage page refresh"
```

---

### Task 6: In-browser verification + PR

- [ ] **Step 1: Full suites**

```bash
cargo test -p rupu-orchestrator -p rupu-cp -p rupu-cli 2>&1 | grep -E '^test result|FAILED|panicked'
cargo clippy -p rupu-orchestrator -p rupu-cp -p rupu-cli --all-targets -- -D warnings
cd crates/rupu-cp/web && npx vitest run && npm run build
```

- [ ] **Step 2: Live check with the in-app browser.**
  - Build the web bundle (`make cp-web`) and the binary (`cargo build -p rupu-cli`).
  - Start `target/debug/rupu cp serve --bind 127.0.0.1:7979` with
    `RUPU_HOME` pointed at a scratch dir, via a `.claude/launch.json` entry
    and `preview_start`.
  - Create a run in that scratch store (`RunStore::create` via a tiny
    `#[ignore]` helper, or by copying an invented fixture run dir) with status
    `running`.
  - Append invented ledger rows to its `usage.jsonl` every second from a
    shell loop.
  - Open `/runs/<id>` and verify that:
    - the header total and the "Token usage by turn" chart grow within ~2 s;
    - the tab-hidden pause works;
    - flipping `run.json` to `completed` stops polling after one final fetch.
  - Screenshot before and after for the PR.
  - Invent all fixture data. Never copy real `~/.rupu` data.
- [ ] **Step 3: Open the PR** (stacked on Plan 1's branch if it isn't merged
  yet; `--base <plan-1-branch>`). Push with an explicit refspec. Body:
  - what went live;
  - the transport changes;
  - the reader fixes;
  - the screenshots;
  - the note that matt should check against a real remote run before merge
    (GUI rule);
  - `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

---

## Self-review notes (plan author)

- **Spec coverage:**
  - §5.3 → Task 3.
  - §5.4 → Task 1.
  - §6 → Task 2.
  - §7 → Tasks 4, 5.
  - "Ledger never enters events.jsonl" is a Global Constraint.
  - The runs-list cadence is unchanged by design (spec §7).
- **Type consistency:**
  - `RunUsageResponse` (Rust) ↔ `RunUsageResponse` (TS): same field names.
  - `points_from` / `epoch` semantics are identical in `build_run_usage_json`
    and `mergeRunUsage`.
